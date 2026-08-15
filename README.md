# diskhog

TUI that lists the largest files on disk and sends the ones you pick to the
trash — with each file's size and its share of total disk space shown right
in the list.

```
$ diskhog                # scans / by default
$ diskhog ~/Downloads     # or any directory
$ diskhog --top 200 /var  # show more than the default 100
```

Stays on the filesystem it started on — it won't wander into `/proc`,
`/sys`, other mounted disks, or network shares. Symlinks are skipped so they
can't be followed off the filesystem or double-count another file's size.

## Deleting is always a trash move, and always confirmed

`diskhog` never calls the equivalent of `rm`. Selecting a file and pressing
Enter/`d` shows a confirmation dialog with the exact path and size; only
pressing `y` actually moves it — to your desktop trash (via the
[`trash`](https://docs.rs/trash) crate, following the freedesktop.org trash
spec on Linux), where it can be restored like anything else you deleted by
hand. Any other key cancels.

## Install

- Cargo: `cargo install diskhog`
- Debian package: https://github.com/wabuntu/diskhog/tree/main/target/debian
- RPM package: https://github.com/wabuntu/diskhog/tree/main/target/release/rpmbuild/RPMS/x86_64
- Single binary: https://github.com/wabuntu/diskhog/tree/main/binaries

## Usage

Keys:

- `↑`/`↓`: move the selection
- `Enter` / `d`: ask to move the selected file to the trash
- `y`: confirm the pending delete (any other key cancels)
- `q` / `Esc`: quit
