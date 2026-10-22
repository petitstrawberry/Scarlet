# Native overlay save regression

Build `cargo build --release --manifest-path tests/native-overlay-save-smoke/Cargo.toml
--target aarch64-unknown-scarlet`. Boot the binary through the real bootstrap init
in an isolated initramfs/overlay guest; never run it on a user's filesystem.

Package `/root/native-overlay-save-smoke/` in the lower initramfs with these exact
contents before booting:

| File | Contents (including final newline) |
| --- | --- |
| `fixture-marker` | `isolated-native-overlay-save-v1` |
| `lower-target.txt` | `packaged lower target` |
| `lower-source.txt` | `packaged lower source` |
| `whiteout-target.txt` | `packaged whiteout target` |

The marker is checked before any mutation. The fixture creates only named files
in that disposable directory and does not recursively remove it. It checks a
pure-upper staging file replacing a lower target, retained source and old
destination handles, inode identity, repeated actual Notepad saves/reopens,
upper-to-absent no-replace rename, collision preservation, and rejection of
lower-backed sources, directories and whiteouts. Success prints
`SCARLET_OVERLAY_SAVE_SMOKE_OK`; errors/panics print
`SCARLET_OVERLAY_SAVE_SMOKE_FAIL`. The pre-fix kernel is expected to fail at the
first replacing rename. A successful run does not claim general overlay rename
support or persistence across reboot.

The portable runner in `tests/native-desktop-smoke/run.py` also accepts
`--program /bin/overlay-save-smoke --expect-marker SCARLET_OVERLAY_SAVE_SMOKE_OK
--duration 60`, in addition to its kernel/staging/QEMU/firmware arguments. For
this console fixture the staging tree needs only the ordinary bootstrap `init`,
`bin/overlay-save-smoke`, the standard empty mountpoint directories and the four
packaged fixture files above. The runner reuses the loader-smoke marker monitor,
records the exact kernel hash and exits with failure if no success marker arrives.
