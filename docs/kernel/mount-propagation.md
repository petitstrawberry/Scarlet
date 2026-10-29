# Native mount-event propagation

Propagation is a VFS property. Environment selection and Binder domains do not
change when a task copies or rejoins a mount namespace.

`VfsManager::set_mount_propagation` accepts `Private`, `Shared`, or `Downstream`.
A shared mount receives and sends events within its peer group. A downstream
mount receives events from an upstream group without sending events back; it
can additionally have its own shared group. Converting the last shared peer to
downstream retains an existing upstream relationship, or becomes private when
there is no upstream. Recursive changes apply to each selected submount.

Fresh independent View roots are private. Namespace copies retain configured
peer and upstream relationships. An explicit bind of a shared source connects
that source to the new attachment; sources bound to a subdirectory only receive
events inside that subtree. Recursive binds copy submount objects and repair
parent links. Copies sent downstream retain one-way relationships for their
nested shared mounts as well.

One sleepable topology transaction covers target resolution, peer changes,
namespace copying and mount publication/detachment. All receiving attachments
are checked before publication. A conflicting mount rejects the whole event.
Detachment follows event identity, so an upstream unmount cannot erase a later,
independently replaced downstream mount. Detached trees stop participating while
open objects retain their backing filesystems. Registry references are weak;
saved namespace capabilities, rather than propagation metadata, keep a View alive.

Native View builders use `set_propagation` (syscall 531) and `bind_recursive`
(532). Linux translates `MS_PRIVATE`, `MS_SHARED`, `MS_SLAVE`, and `MS_REC` to
these operations and supports directory `MS_BIND` with optional recursion.
Unsupported flags and stacking over an already mounted target remain errors.
