# diskhog

TUI that lists the largest files on disk and lets you delete the ones you
pick — either to the trash, or for real — with each file's size and its
share of total disk space shown right in the list.

<img src="https://raw.githubusercontent.com/wabuntu/diskhog/main/docs/list.png" alt="diskhog listing the largest files under a directory, ranked by size with each file's share of total disk space" width="480">
<img src="https://raw.githubusercontent.com/wabuntu/diskhog/main/docs/confirm.png" alt="diskhog's delete confirmation dialog, showing the exact path and size before moving a file to the trash" width="480">

```
$ diskhog /                       # scan the whole filesystem
$ diskhog ~/Downloads              # or any directory
$ diskhog --top 200 /var           # show more than the default 30
$ diskhog --max-cpu 60 /var        # allow a bigger/smaller worker pool than the default 30%
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

## Deleting always asks, and always asks which way

Selecting a file and pressing Enter/`d` shows a confirmation dialog with
the exact path and size, and two ways to actually remove it:

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
