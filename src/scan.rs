use ignore::overrides::{Override, OverrideBuilder};
use ignore::{WalkBuilder, WalkState};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::SystemTime;

/// Default worker-thread cap, as a percentage of available cores, when the
/// caller doesn't ask for a different one.
pub const DEFAULT_MAX_CPU_PERCENT: usize = 30;

/// Cap the worker pool instead of using every core, so a background scan
/// doesn't compete too hard with whatever else is running. This bounds how
/// many stat() calls diskhog can have in flight at once, not a hard
/// OS-enforced CPU quota, but keeping the pool small is the practical lever
/// a plain CLI tool has over its own load. Never returns 0, even on a
/// single-core machine.
fn capped_threads(available: usize, max_cpu_percent: usize) -> usize {
    ((available * max_cpu_percent) / 100).max(1)
}

pub struct FileEntry {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: SystemTime,
}

/// Wraps a FileEntry so it can live in a BinaryHeap ordered purely by size.
struct BySize(FileEntry);

impl PartialEq for BySize {
    fn eq(&self, other: &Self) -> bool {
        self.0.size == other.0.size
    }
}
impl Eq for BySize {}
impl PartialOrd for BySize {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for BySize {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.size.cmp(&other.0.size)
    }
}

pub struct ScanResult {
    pub top_files: Vec<FileEntry>,
    pub files_scanned: u64,
    pub scan_errors: u64,
}

/// Builds the exclude list `WalkBuilder` understands from plain glob
/// strings like `node_modules` or `*.log` — each is treated as an
/// exclude (never an include-only whitelist), matching what `--exclude`
/// users expect regardless of whether they think to type a leading `!`.
fn build_excludes(root: &Path, patterns: &[String]) -> Result<Override, String> {
    let mut builder = OverrideBuilder::new(root);
    for pattern in patterns {
        builder
            .add(&format!("!{pattern}"))
            .map_err(|e| format!("invalid --exclude pattern '{pattern}': {e}"))?;
    }
    builder.build().map_err(|e| e.to_string())
}

/// Start a scan on a background thread and return a handle to join for the
/// final result, plus a live counter of files scanned so far that the
/// caller can poll from another thread (e.g. to print progress) without
/// blocking on the scan itself. Fails fast (before spawning anything) if
/// one of `excludes` isn't a valid glob.
pub fn scan_top_files_async(
    root: &Path,
    top_n: usize,
    max_cpu_percent: usize,
    excludes: &[String],
) -> Result<(JoinHandle<ScanResult>, Arc<AtomicU64>), String> {
    let overrides = build_excludes(root, excludes)?;
    let progress = Arc::new(AtomicU64::new(0));
    let progress_for_scan = Arc::clone(&progress);
    let root = root.to_path_buf();
    let handle = std::thread::spawn(move || {
        scan_top_files(&root, top_n, max_cpu_percent, overrides, &progress_for_scan)
    });
    Ok((handle, progress))
}

/// Walk `root` in parallel across all available cores, staying on its own
/// filesystem (never descending into other mounts: /proc, /sys, other
/// disks, network shares, ...), and keep the `top_n` largest regular files
/// seen. Symlinks are skipped entirely so they can't be followed off the
/// filesystem or double-count another file's size. Hidden files and
/// .gitignore rules are intentionally NOT respected here (unlike this
/// walker's usual ripgrep-style default) since a huge file being
/// gitignored doesn't make it any less real on disk.
///
/// Entries that can't be read (permission denied, etc.) are counted as scan
/// errors and skipped rather than aborting the whole scan. `progress` is
/// incremented as files are found, for a caller on another thread to poll.
fn scan_top_files(
    root: &Path,
    top_n: usize,
    max_cpu_percent: usize,
    overrides: Override,
    progress: &AtomicU64,
) -> ScanResult {
    if top_n == 0 {
        return ScanResult {
            top_files: Vec::new(),
            files_scanned: 0,
            scan_errors: 0,
        };
    }

    let heap: Mutex<BinaryHeap<Reverse<BySize>>> = Mutex::new(BinaryHeap::with_capacity(top_n + 1));
    let scan_errors = AtomicU64::new(0);
    let available = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let threads = capped_threads(available, max_cpu_percent);

    WalkBuilder::new(root)
        .standard_filters(false)
        .hidden(false)
        .same_file_system(true)
        .overrides(overrides)
        .threads(threads)
        .build_parallel()
        .run(|| {
            Box::new(|entry| {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => {
                        scan_errors.fetch_add(1, Ordering::Relaxed);
                        return WalkState::Continue;
                    }
                };

                // file_type() is only None for the special "read from stdin"
                // entry, which we never produce since we always walk a real path.
                let Some(file_type) = entry.file_type() else {
                    return WalkState::Continue;
                };
                if file_type.is_symlink() || !file_type.is_file() {
                    return WalkState::Continue;
                }

                let (size, mtime) = match entry.metadata() {
                    Ok(m) => (m.len(), m.modified().unwrap_or_else(|_| SystemTime::now())),
                    Err(_) => {
                        scan_errors.fetch_add(1, Ordering::Relaxed);
                        return WalkState::Continue;
                    }
                };

                progress.fetch_add(1, Ordering::Relaxed);

                let mut heap = heap.lock().unwrap();
                if heap.len() < top_n {
                    heap.push(Reverse(BySize(FileEntry {
                        path: entry.path().to_path_buf(),
                        size,
                        mtime,
                    })));
                } else if let Some(Reverse(smallest)) = heap.peek()
                    && size > smallest.0.size
                {
                    heap.pop();
                    heap.push(Reverse(BySize(FileEntry {
                        path: entry.path().to_path_buf(),
                        size,
                        mtime,
                    })));
                }

                WalkState::Continue
            })
        });

    let mut top_files: Vec<FileEntry> = heap
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|Reverse(BySize(f))| f)
        .collect();
    top_files.sort_by_key(|f| Reverse(f.size));

    ScanResult {
        top_files,
        files_scanned: progress.load(Ordering::Relaxed),
        scan_errors: scan_errors.load(Ordering::Relaxed),
    }
}

/// "1.2 GB"-style formatting, base-1000 (matches `du -h` / most disk tools
/// and lines up with `fs4`'s total_space, which is also base-1000 bytes).
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1000.0 && unit < UNITS.len() - 1 {
        size /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}

/// "3d"/"2mo"/"1y"-style relative age, compact enough to sit in a list
/// column next to the size. A clock-skewed mtime in the future (or one
/// exactly now) reads as "just now" rather than underflowing.
pub fn human_age(mtime: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(mtime)
        .unwrap_or_default()
        .as_secs();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 60 * 60 {
        format!("{}m", secs / 60)
    } else if secs < 60 * 60 * 24 {
        format!("{}h", secs / (60 * 60))
    } else if secs < 60 * 60 * 24 * 30 {
        format!("{}d", secs / (60 * 60 * 24))
    } else if secs < 60 * 60 * 24 * 365 {
        format!("{}mo", secs / (60 * 60 * 24 * 30))
    } else {
        format!("{}y", secs / (60 * 60 * 24 * 365))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    /// A directory under the OS temp dir that removes itself on drop, so
    /// scan tests don't leak files into /tmp on failure.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("diskhog-test-{}-{}", name, std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn scan(dir: &Path, top_n: usize) -> ScanResult {
        scan_with_excludes(dir, top_n, &[])
    }

    fn scan_with_excludes(dir: &Path, top_n: usize, excludes: &[&str]) -> ScanResult {
        let excludes: Vec<String> = excludes.iter().map(|s| s.to_string()).collect();
        let (handle, _progress) =
            scan_top_files_async(dir, top_n, DEFAULT_MAX_CPU_PERCENT, &excludes).unwrap();
        handle.join().unwrap()
    }

    #[test]
    fn capped_threads_stays_near_requested_percent_and_never_zero() {
        assert_eq!(capped_threads(16, 30), 4);
        assert_eq!(capped_threads(8, 30), 2);
        assert_eq!(
            capped_threads(4, 30),
            1,
            "30% of 4 rounds down, but must not be 0"
        );
        assert_eq!(
            capped_threads(1, 30),
            1,
            "a single-core machine still gets one thread"
        );
        assert_eq!(
            capped_threads(0, 30),
            1,
            "a bogus 0 reading still gets one thread"
        );
        assert_eq!(capped_threads(16, 100), 16, "100% uses every core");
        assert_eq!(
            capped_threads(16, 1),
            1,
            "a tiny percent still rounds up to 1"
        );
    }

    #[test]
    fn human_size_formats_common_ranges() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1_500), "1.5 KB");
        assert_eq!(human_size(2_000_000), "2.0 MB");
        assert_eq!(human_size(3_500_000_000), "3.5 GB");
    }

    #[test]
    fn scan_finds_and_ranks_files_by_size_descending() {
        let dir = TempDir::new("rank");
        fs::write(dir.path().join("small.bin"), vec![0u8; 10]).unwrap();
        fs::write(dir.path().join("big.bin"), vec![0u8; 1000]).unwrap();
        fs::write(dir.path().join("medium.bin"), vec![0u8; 100]).unwrap();

        let result = scan(dir.path(), 10);
        assert_eq!(result.files_scanned, 3);
        let sizes: Vec<u64> = result.top_files.iter().map(|f| f.size).collect();
        assert_eq!(sizes, vec![1000, 100, 10]);
    }

    #[test]
    fn scan_respects_top_n_limit() {
        let dir = TempDir::new("limit");
        for i in 0..5u64 {
            fs::write(
                dir.path().join(format!("f{}.bin", i)),
                vec![0u8; (i * 10 + 1) as usize],
            )
            .unwrap();
        }
        let result = scan(dir.path(), 2);
        assert_eq!(result.files_scanned, 5);
        assert_eq!(result.top_files.len(), 2);
        assert_eq!(result.top_files[0].size, 41);
        assert_eq!(result.top_files[1].size, 31);
    }

    #[test]
    fn scan_top_n_zero_returns_nothing_without_walking() {
        let dir = TempDir::new("zero");
        fs::write(dir.path().join("f.bin"), vec![0u8; 10]).unwrap();
        let result = scan(dir.path(), 0);
        assert!(result.top_files.is_empty());
        assert_eq!(result.files_scanned, 0);
    }

    #[test]
    fn scan_recurses_into_subdirectories() {
        let dir = TempDir::new("nested");
        fs::create_dir_all(dir.path().join("a/b")).unwrap();
        fs::write(dir.path().join("a/b/deep.bin"), vec![0u8; 42]).unwrap();

        let result = scan(dir.path(), 10);
        assert_eq!(result.files_scanned, 1);
        assert_eq!(result.top_files[0].size, 42);
    }

    #[test]
    fn scan_includes_hidden_and_gitignored_files() {
        let dir = TempDir::new("hidden");
        fs::write(dir.path().join(".hidden.bin"), vec![0u8; 7]).unwrap();
        fs::write(dir.path().join(".gitignore"), "*.bin\n").unwrap();
        fs::write(dir.path().join("ignored.bin"), vec![0u8; 9]).unwrap();

        let result = scan(dir.path(), 10);
        // .gitignore itself + the two .bin files
        assert_eq!(result.files_scanned, 3);
    }

    #[cfg(unix)]
    #[test]
    fn scan_skips_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new("symlink");
        let target = dir.path().join("real.bin");
        fs::write(&target, vec![0u8; 500]).unwrap();
        symlink(&target, dir.path().join("link.bin")).unwrap();

        let result = scan(dir.path(), 10);
        assert_eq!(result.files_scanned, 1, "the symlink must not be counted");
        assert_eq!(result.top_files.len(), 1);
        assert_eq!(result.top_files[0].path, target);
    }

    #[test]
    fn scan_skips_paths_matching_an_exclude_pattern() {
        let dir = TempDir::new("exclude");
        fs::create_dir_all(dir.path().join("node_modules")).unwrap();
        fs::write(dir.path().join("node_modules/big.bin"), vec![0u8; 1000]).unwrap();
        fs::write(dir.path().join("keep.bin"), vec![0u8; 10]).unwrap();

        let result = scan_with_excludes(dir.path(), 10, &["node_modules"]);
        assert_eq!(result.files_scanned, 1, "only keep.bin should be walked");
        assert_eq!(result.top_files[0].path, dir.path().join("keep.bin"));
    }

    #[test]
    fn scan_skips_paths_matching_a_glob_exclude_pattern() {
        let dir = TempDir::new("exclude-glob");
        fs::write(dir.path().join("app.log"), vec![0u8; 1000]).unwrap();
        fs::write(dir.path().join("keep.bin"), vec![0u8; 10]).unwrap();

        let result = scan_with_excludes(dir.path(), 10, &["*.log"]);
        assert_eq!(result.files_scanned, 1);
        assert_eq!(result.top_files[0].path, dir.path().join("keep.bin"));
    }

    #[test]
    fn scan_rejects_an_invalid_exclude_pattern_before_spawning_anything() {
        let dir = TempDir::new("bad-pattern");
        let err = scan_top_files_async(dir.path(), 10, DEFAULT_MAX_CPU_PERCENT, &["[".to_string()])
            .unwrap_err();
        assert!(err.contains("invalid --exclude pattern"));
    }

    #[test]
    fn human_age_formats_common_ranges() {
        let now = SystemTime::now();
        assert_eq!(human_age(now), "just now");
        assert_eq!(
            human_age(now - Duration::from_secs(60 * 5)),
            "5m",
            "5 minutes ago"
        );
        assert_eq!(
            human_age(now - Duration::from_secs(60 * 60 * 3)),
            "3h",
            "3 hours ago"
        );
        assert_eq!(
            human_age(now - Duration::from_secs(60 * 60 * 24 * 4)),
            "4d",
            "4 days ago"
        );
        assert_eq!(
            human_age(now - Duration::from_secs(60 * 60 * 24 * 60)),
            "2mo",
            "2 months ago"
        );
        assert_eq!(
            human_age(now - Duration::from_secs(60 * 60 * 24 * 400)),
            "1y",
            "just over a year ago"
        );
    }

    #[test]
    fn human_age_treats_a_future_mtime_as_just_now() {
        let future = SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(human_age(future), "just now");
    }
}
