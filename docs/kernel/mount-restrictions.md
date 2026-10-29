# Mount restrictions

Mount restrictions are immutable properties of a VFS mount. They do not alter
Environment selection, Shield policy, or filesystem contents. A caller can still
read an ordinary file on a no-execute mount.

| Native restriction | Effect |
| --- | --- |
| `VFS_MOUNT_NO_EXECUTE` | Reject executable images and script entry points, ELF interpreters, executable file mappings, and later changes that would grant execute access to those mappings. |
| `VFS_MOUNT_NO_DEVICES` | Reject character/block device opens before invoking the filesystem/device open callback; metadata remains available. |
| `VFS_MOUNT_NO_PRIVILEGE_GAIN` | Preserve inherited native credentials and Shield subject through image preparation; reject file-derived changes before committing the new image. Current production loaders do not implement set-ID grants. |

Bind mounts, views rooted at a source directory, and mount namespace copies
retain source restrictions. Overlay file opens retain the resolved backing mount,
including a nested mount or a replaced inode retained across rename. Open
objects keep their authority after descriptor transfer or mount detachment.

File mappings carry a permanent maximum-permission mask. Fault materialization,
COW, fork, VMA splitting, relocation and anonymous growth retain that ceiling;
coalescing cannot combine different ceilings. Protection changes validate the
entire requested range before changing any VMA or page-table state. Anonymous
memory is independent: copying bytes into an anonymous mapping does not attach
the source mount's restrictions to those bytes.

Native builders use `VfsView::mount_restricted` (`VfsViewMountRestricted`, syscall
530) with the `VFS_MOUNT_*` constants from `scarlet_abi::fs`. The existing
four-argument `VfsViewMount` keeps its original unrestricted-mount contract.
Unknown restriction bits, including unimplemented read-only mounting, are
rejected. Stacking over an existing mount is rejected with `Busy`.

The Linux adapter translates `MS_NOSUID`, `MS_NODEV` and `MS_NOEXEC` explicitly.
Its initial filesystem-creation operation supports tmpfs, root mode metadata,
default owner/group zero, and a checked byte/K/M/G size limit. Nonzero owners,
unknown options, read-only/bind/remount operations, and unsupported filesystems
return errors. Linux mount authority comes from native `MANAGE_MOUNTS`, not
numeric UID zero. Existing private/slave propagation requests remain private.

This does not implement general Unix DAC, SELinux mount contexts, shared mount
propagation, set-ID execution, or read-only mount enforcement.
