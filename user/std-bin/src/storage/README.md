# Storage

Read-only, ranked logical file-size browser. The executable has its own
`main.rs` and is registered as the `storage` bin in `scarlet-std-bin`.

Choose a folder with **File → Choose Folder…** (Command+O on macOS, Ctrl+O
elsewhere), or pass an absolute folder path as the first command-line argument.
Click a directory to open its cached children; arrow keys and Enter also work.
**Go → Enclosing Folder**, Alt+Left or Backspace goes up within the snapshot.
**View → Refresh Scan** remeasures the original selected root. **Cancel Scan**
or Escape requests cooperative cancellation and retains measured entries.

The list ranks immediate children by recursive logical bytes and shows their
share of the current folder's measured total. Each regular file path contributes
its reported length, so hard links are counted per name and sparse-file holes
count toward logical length. This is **not allocated disk space or free space**.
Symbolic links (including cycles, broken links and links outside the selected
folder) are listed but not followed; symlink roots/ancestors are rejected.
Special files are excluded. No files are opened for content, modified or deleted.

One worker handles scans. New requests cancel and replace earlier requests;
generation checks prevent stale results from replacing the current folder.
The UI can continue browsing an older snapshot during Refresh and labels it as
such. A snapshot remains browseable even if files subsequently change or vanish.

Limits: 50,000 directory entries, 64 recursive levels, 16 MiB of retained path
bytes, and 15 seconds between filesystem operations. Node/diagnostic storage is
also bounded by the entry limit; each error message retains at most 240 chars.
Cancellation and the deadline are checked between filesystem calls; a blocked
kernel I/O call cannot be interrupted by this application. Directory enumeration
errors, unreadable/disappeared entries, depth/size/time limits and cancellation
mark affected totals as partial. This is a best-effort live scan, not an atomic
filesystem snapshot. Proportions on partial scans describe only measured bytes.

## Native package

`src/storage/app.toml` owns Storage's metadata beside its main and resources.
It uses the SDK's supported script build (as Cadence does). The sibling `install`
script builds only std-bin's `storage` binary from the existing repository and
stages it for SDK ELF validation. No source tree is copied or SDK checks changed.
The Cadence-style link-only seed preserves SGFX's resident-loader imports; the
script audits the final and staged ELF to allow only the expected `dl*` imports,
native relocations and `/bin/scarlet-ld`, with no startup shared-library dependency.
Script recipes always produce release builds; the SDK does not pass a profile
argument to scripts. `resources` contains the dedicated icon.
The current SDK rejects both cargo `source = "../.."` and recipe names other than
`app.toml`, so a package-root `storage.app.toml` cannot be used.
The desktop bundle installs it as `/applications/storage.app`.
The recipe generates the launcher entry and embeds the dedicated icon;
there is no duplicate legacy `/etc/stemd.d/apps` entry or `/bin/storage` layer.
No dependency pins or local sibling overrides are added.

From Scarlet:

```sh
nix develop . --no-update-lock-file --command sh -c 'cd projects/aarch64-limine-full && cargo scarlet app build --source /Users/petitstrawberry/Development/Rust/Scarlet/user/std-bin/src/storage/app.toml --target aarch64-unknown-scarlet --release --output /Users/petitstrawberry/Development/Rust/Scarlet/artifacts/storage-20261023/script-recipe/storage.app'
```

## Validation

Model tests compile without the unrelated media/networking bins or GUI runtime:

```sh
nix develop . --no-update-lock-file --command sh -c 'rustc --edition 2024 --test user/std-bin/src/storage/model_tests.rs -o /tmp/scarlet-storage-tests && /tmp/scarlet-storage-tests --test-threads=1'
```

Coverage includes recursive ranking, empty/missing/non-directory roots, symlink
cycles/external links, logical sparse-file and hard-link totals, unreadable
folders, cancellation, scan bounds, cached navigation after removal, and stale
request rejection. The AArch64 `.app` build is checked separately. Native menu,
folder-dialog, resizing, scrolling, launcher icon and bar appearance require
the parent task's QEMU operation/visual checks; build success does not claim those.
