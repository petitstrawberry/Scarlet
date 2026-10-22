# Overlay replacement boundary

## Initial diagnosis

The native fixture executed the same write, `File::sync_all`, and `std::fs::rename`
sequence on the current kernel:

| Location | Write | Sync | Replacement |
| --- | --- | --- | --- |
| `/tmp` (tmpfs) | success | success | success |
| `/root` (initramfs overlay) | success | success | `ErrorKind::Other` |

Notepad's real GUI also saved successfully to `/tmp/fixture.txt`. On the overlay
path it reported the failure, kept the original and named the retained staging
file. This is not a durability test for persistent storage.

At the initial revision, `OverlayFS` did not implement `FileSystemOperations::rename`; the
trait's default returns `NotSupported`. The older replacing-rename syscall path
reduces this to the generic error observed by native std. The new operation 419
has richer errno reporting, but it cannot provide a missing driver operation.

Delegating directly to the upper filesystem's atomic rename would not be a safe
fix. `OverlayNode` contains an immutable path and delegates `metadata`, `open`,
lookup and other operations through it. VFS rename relocates the retained
`VfsEntry` while keeping its original node, so that node would continue referring
to the old pathname. The overlay's separate `inode_ids.by_path` registry must
also move the source identity and retire the replaced destination identity.

A narrowly scoped implementation for regular files created only in the upper
layer is feasible as a separate driver change, but it needs all of these pieces:

- A shared mutable identity/location record for every retained node of one
  overlay inode, or an equivalent stable backing-node design. Update all
  path-dependent operations, not only `open`; keep already-open file objects
  attached to their original content when the destination is replaced.
- One upper-filesystem rename as the publication point, with namespace and
  overlay-identity updates coordinated under the existing mutation discipline.
  Do not replace rename with copying or destination truncation.
- Reject source paths that also exist in lower layers until atomic whiteout
  semantics are defined. Otherwise moving the upper entry resurrects the lower
  source. Reject destination whiteouts as well: current lookup tests a whiteout
  before looking in the upper layer, and removing it separately is not an atomic
  publication.
- Handle parents that exist only below without exposing a partial operation.
  A minimal first version can require both parents already in the upper layer;
  staging an editor save normally creates that upper parent beforehand.
- Explicitly reject directory moves, lower-only sources and unsupported backing
  filesystems in that first version. General directory rename additionally needs
  subtree location updates, opacity and lower-directory semantics.

Expected scope is the overlay driver plus its kernel tests and an extension of
the native fixture: approximately a few hundred lines, not a one-call adapter.
Required tests include upper-to-absent and upper replacement, upper staging over
a lower destination, retained source/destination handles, repeated open/stat
after relocation, inode identity, same-path behavior, collisions, rejected
whiteouts/lower sources/directories, and injected upper-rename failure preserving
both original entries. The lower-only source case cannot be safely added through
the existing separate `copy_up`, `create_whiteout` and `remove` operations: it
requires a transactional publication design.


## Implemented boundary

The subsequent driver change implements same-parent rename of a regular source
created solely in the tmpfs upper layer, with link count one. It supports an
absent destination or replacement of an upper/lower regular destination. Shared
backing records preserve retained node identity and open file objects; the
existing VFS mutation lock coordinates publication. No copying or truncation is
used as a rename fallback.

Lower-backed sources, lower-only parents, whiteouts, directories, symlinks,
cross-parent moves and upper backing mount boundaries are rejected. General
overlay directory moves and transactional whiteouts remain outside this change.

The native fixture failed on the original kernel at its first replacement
(8.656 seconds), then passed on the final guarded implementation (11.807
seconds). It invokes production Notepad saves twice, verifies reopened content
and inode identities, checks retained source and old destination handles, and
checks collision/rejection preservation. The separate kernel suite passed 1544
tests, including six new overlay regressions. Exact native input hashes and
artifact locations are recorded in [VALIDATION.md](VALIDATION.md).
