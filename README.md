# diskhog

[![Crates.io](https://img.shields.io/crates/v/diskhog.svg)](https://crates.io/crates/diskhog)
[![CI](https://github.com/wabuntu/diskhog/actions/workflows/rust.yml/badge.svg)](https://github.com/wabuntu/diskhog/actions/workflows/rust.yml)

TUI that lists the largest files on disk and lets you delete the ones you
pick — either to the trash, or for real — with each file's size and its
share of total disk space shown right in the list.

<img src="https://raw.githubusercontent.com/wabuntu/diskhog/main/docs/list.png" alt="diskhog listing the largest files under a directory, ranked by size with each file's share of total disk space" width="480">
<img src="https://raw.githubusercontent.com/wabuntu/diskhog/main/docs/confirm.png" alt="diskhog's delete confirmation dialog, showing the exact path and size before moving a file to the trash" width="480">

```
$ diskhog /                              # scan the whole filesystem
$ diskhog ~/Downloads                    # or any directory
$ diskhog --top 200 /var                 # show more than the default 30
$ diskhog --max-cpu 60 /var              # allow a bigger/smaller worker pool than the default 30%
$ diskhog --exclude node_modules ~/code  # skip anything matching a glob (repeatable)
```

The directory is required — `diskhog` never picks a scan target for you.

Stays on the filesystem it started on — it won't wander into `/proc`,
`/sys`, other mounted disks, or network shares. Symlinks are skipped so they
can't be followed off the filesystem or double-count another file's size.
The walk itself is parallel (via the [`ignore`](https://docs.rs/ignore)
crate — the same walker ripgrep uses), capped by default to ~30% of
available cores so a scan doesn't try to claim the whole machine —
adjustable with `--max-cpu <1-100>` — with a live "N files scanned so far"
counter printed while it runs.

`--exclude <PATTERN>` (repeatable, e.g. `--exclude node_modules --exclude
'*.log'`) skips anything matching the glob — a plain name like
`node_modules` matches that directory anywhere under the scan root, same
as a `.gitignore` entry would. Handy for the things you already know are
big and already know you don't want to touch (build output, `.git`,
package caches).

Each row also shows how long ago the file was last modified — `3h`,
`12d`, `2mo`, `1y` — so you're not guessing whether something huge is
live data or forgotten cruft before you decide to delete it.

## Performance

A sample run on an Ubuntu desktop, finding the 30 largest files under `/`:

```
$ time sudo find / -type f -printf '%s %p\n' | sort -rn | head -30
...
real    0m12.125s
user    0m2.417s
sys     0m0.290s

$ time sudo diskhog /
...
real    0m0.994s
user    0m0.005s
sys     0m0.020s
```

diskhog finished about 12x faster than the `find | sort | head` approach —
the parallel walk and bounded top-N heap avoid both sorting every file in
the tree and holding the whole listing in memory at once.

`du` is the other common way people hunt for large files, so here's a
second sample — no root needed this time, 187,233 files / 8.8GB under
`/usr`, page cache already warm from repeated runs:

```
$ time du -ah /usr | sort -rh | head -30
...
real    0m1.473s
user    0m1.074s
sys     0m1.828s

$ time find /usr -type f -printf '%s %p\n' | sort -rn | head -30
...
real    0m0.338s
user    0m0.733s
sys     0m1.322s

$ time diskhog /usr
...
real    0m0.367s
user    0m0.142s
sys     0m0.951s
```

`du -ah` is the slowest of the three here — it computes a running total
for every directory *and* lists every file, which is more work than
either `find` or diskhog do. With the page cache already warm, `find`
and diskhog end up close to each other — the 12x gap above shows up on
a cold, uncached, whole-filesystem walk, where diskhog's parallel I/O
matters most; on a warm cache over a smaller tree, single-threaded `find`
can keep pace. `du`'s extra per-directory accounting is the one place it
consistently loses no matter the cache state.

## Deleting always asks, and always asks which way

Selecting a file and pressing Enter/`d` shows a confirmation dialog with
the exact path, size, and how long ago it was last modified, and two
ways to actually remove it:

- `t` — move to the trash (via the [`trash`](https://docs.rs/trash) crate,
  following the freedesktop.org trash spec on Linux), recoverable like
  anything else you deleted by hand. **This does not free disk space** — on
  the file's own filesystem it's a same-filesystem rename, so the data
  blocks stay allocated until the trash itself is emptied.
- `p` — permanently delete it (an actual unlink). Frees the space
  immediately; no way back.

Any other key cancels.

## Install

- Cargo: `cargo install diskhog`
- Debian package: https://github.com/wabuntu/diskhog/tree/main/target/debian
- RPM package: https://github.com/wabuntu/diskhog/tree/main/target/release/rpmbuild/RPMS/x86_64
- Single binary: https://github.com/wabuntu/diskhog/tree/main/binaries

## Usage

Keys:

- `↑`/`↓`: move the selection
- `Enter` / `d`: open the delete confirmation for the selected file
- `t`: move it to the trash (recoverable, space not freed)
- `p`: permanently delete it (frees space, no undo)
- any other key: cancel the pending delete
- `q` / `Esc`: quit

Flags:

- `--top <N>`: how many of the largest files to show (default 30)
- `--max-cpu <1-100>`: cap the scan's worker threads to this percentage of
  available cores (default 30)
- `--exclude <PATTERN>` / `-x <PATTERN>`: skip paths matching this glob
  (repeatable — pass it more than once to exclude several patterns)
