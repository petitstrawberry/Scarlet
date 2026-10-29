# Retained mount namespaces

A native `MountNamespace` is an identified set of original-View to local-View
correspondences. It is separate from Environment, PID visibility, lifecycle
scopes, credentials and Shield subjects. Splitting copies mount nodes and keeps
configured propagation relationships. Ordinary fork/exec retain the selected
namespace.

`MountNamespace::current` (533) captures the current namespace in a new
close-on-exec capability. Capture materializes already available implicit View
correspondences, including the current Environment's roots, without changing
namespace identity. `identity` (535) returns an eight-byte little-endian ID that
is never reused. Capabilities can be duplicated or transferred and retain their
View mappings after the original task exits; they expose no stream/mapping or
View-administration capability.

`join` (534) requires both the retained capability and native `MANAGE_MOUNTS`.
All roots of the current Environment must have explicit correspondences in the
destination. The selected ABI chooses its existing Environment root, and only
that root's local View changes. An incompatible Environment is rejected rather
than silently retaining an unrelated root. The caller's cwd pathname must resolve
in the destination; failure leaves namespace, cwd and Environment unchanged.
Old directory descriptors retain their separately held authority.

A live task sharing the caller's filesystem context prevents joining. The
caller can first unshare that context; other threads with separate filesystem
contexts are unaffected. Join does not require unrelated VM or handle-table
resources to be exclusive. Validation includes registered tasks before they
become runnable.

Linux `/proc/self/ns/mnt` and `/proc/thread-self/ns/mnt` return this native
capability. `readlink`, stat/fstat/statx, and `/proc/self/fd` report the stable
namespace identity. `setns` (268) accepts a mount namespace or type zero and
checks native authority and View compatibility. Querying an FD, even at UID
zero, does not grant authority to join. Namespace type ioctl is supported;
other namespace kinds, pidfds, numeric PID namespace paths and general nsfs
operations are outside this initial adapter.
