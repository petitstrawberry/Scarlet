# Shield

Shield is Scarlet's protection mechanism. Its first primitives are immutable
label policies, capability-controlled subject assignment, and explicit
allow/deny queries. `kernel/src/shield/` has no Android or Linux terminology.
Environment still maps ABIs to filesystem views; it does not own policy or
process labels. A userspace environment manager composes these primitives.

## Native policy and authority

A policy contains up to 256 uniquely named labels and 4,096 grants. A grant is
`(source label, target label, class, permission bits)`. Classes and the 64-bit
permission mask belong to the enforcement point. Unknown labels, classes and
ungranted bits deny. Equal label numbers in different policy objects do not
identify the same subject, and cross-policy authorization denies.

`ShieldPolicyCreate` (1310) copies fixed-width label and rule records and
publishes a fully validated, immutable policy. It requires bootstrap authority
or an existing policy management capability. The returned handle is
close-on-exec. `ShieldPolicyCurrent` (1311) returns query authority only; handle
metadata changes cannot upgrade it. `ShieldApply` (1312) requires management
authority and an exclusive, single-threaded process. It assigns one of that
policy's labels to the current process. The `scarlet_os::shield::Policy` wrapper
owns these capabilities.

`ShieldPolicyCreateWithTransitions` (1313) additionally accepts at most 4,096
unique `(source label, target label, principal)` transitions. A transition requires
all base, active and retained principal IDs to equal the authorized principal.
`ShieldTransition` (1314) checks the live subject and credentials in an exclusive,
single-threaded process. Query authority is enough for an explicitly delegated
transition; it cannot create grants or perform an arbitrary `ShieldApply`.

Clone and Environment spawn inherit the subject. Exec, including an ABI or
Environment change, preserves it; failed exec leaves it unchanged. Assignment
is explicit: executable names and filesystem xattrs cannot grant a process
identity. An administrator can select a subject, spawn a child, then select
another subject before launching the next child. Management handles need not
be transferred to the children.

## Android service authorization

The Android ABI supplies an `android-selinux` filesystem. Creating it requires
`policy=<management handle>`; mounting it uses existing View capabilities.
It exposes libselinux's immutable status page, enforcing state, class and
permission discovery, context validation, and per-open access transactions.
Only tasks in the mounted policy can open its query files. Android
`statfs`/`fstatfs` translate its filesystem identity without adding SELinux
knowledge to the Linux ABI implementation.

For this adapter, native class **1** means `service_manager`; bits **1**, **2**
and **4** mean `add`, `find` and `list`. Labels use exact SELinux
context strings at `s0`, optionally followed by canonical, strictly increasing
`c0` through `c1023` categories. These suffixes are opaque labels: there is no
MLS dominance relation, category range expansion, or implicit grant.
The status page reports enforcing operation for these queries and cannot be
mapped writable or made writable with `mprotect`. Policy loading and permissive
mode are rejected. Writing `/proc/self/attr/current` translates a delegated
Shield transition. Each writable descriptor is bound to the original task, and
the live policy, source label and credentials are rechecked when writing.
Inherited descriptors cannot relabel their original owner or their new holder. Because policies are immutable,
libselinux may cache decisions without a policy-update notification channel.

`/proc/self/attr/current`, `/proc/thread-self/attr/current`, and PID-based reads
return the kernel-held subject, resolved through the caller's PID namespace.
Cross-policy reads deny. Binder retains the subject at open: relabeling cannot
reuse an old Binder descriptor, and calls or context-manager death watches
cannot cross Shield policies within an Environment. Existing unlabeled Binder
users keep their current behavior. This confinement does not yet implement
SELinux's Binder call/transfer permission classes.

The protocol follows AOSP Android 13
[access decisions](https://android.googlesource.com/platform/external/selinux/+/refs/tags/android-13.0.0_r2/libselinux/src/compute_av.c),
[class discovery](https://android.googlesource.com/platform/external/selinux/+/refs/tags/android-13.0.0_r2/libselinux/src/stringrep.c), and
[status-page handling](https://android.googlesource.com/platform/external/selinux/+/refs/tags/android-13.0.0_r2/libselinux/src/sestatus.c).

This is a service-authorization foundation, not a complete SELinux engine or
system-wide sandbox. It does not consume Android binary policy, enforce VFS
MAC or attach policy semantics to `security.selinux` xattrs. Native Shield
credentials separately hold principal/group identities and change authority;
Linux and Android translate these to UID/GID and supported capability operations.
Inherited Shield execution constraints enforce Linux seccomp filters before ABI
dispatch, including attempts to escape through native syscalls. The initial fixture tests unmodified libselinux. The subsequent
[service integration](../../tools/android-compat/README.md#stock-servicemanager-integration)
uses distinct process labels and the unmodified stock servicemanager to verify
allowed registration/lookup and denied add/find/list requests.


## Reserved exec transitions

`ShieldReserveExec` (1315) reserves a transition delegated by the immutable
policy for the current source label and all native principal slots. Both the
policy/source identity and the resulting image's principal are rechecked before
exec commit. A successful image replacement commits the new subject and consumes
the reservation; failed exec retains it. Fork/spawn inherit a reservation by
value, so a child's exec does not consume its parent's reservation.
`ShieldClearExec` (1316) cancels the current task's reservation without a grant.
An explicit delegated transition is distinct from file-derived privileges on a
no-privilege-gain mount.

Android maps `/proc/self/attr/exec` to this primitive, including stock
libselinux's zero-byte `write(fd, NULL, 0)` cancellation. Descriptors retain their
original owner; inherited/transferred descriptors cannot mutate another task's
reservation. Native and Linux stream writes dispatch explicit zero-byte control
operations without dereferencing a user pointer. Ordinary streams remain no-ops.

## Information constraints

`ShieldConstrainInformation` (1318) monotonically suppresses kernel-address
introspection; `ShieldInformation` (1319) queries it. Fork/spawn/exec retain the
constraint independently of numeric identity, subject labels or syscall filters.
It redacts privileged PC samples and opaque CPU breadcrumb context, blocks raw
kernel-log reads even through an already-open descriptor, and rejects profiler
dumps that could disclose addresses. User instruction addresses remain available.

The Android manager fixture installs this constraint before creating threads or
launching init. Linux `kptr_restrict` reports that native state and accepts `2`
only when already enforced. Requests to loosen it, or fabricate a global setting
without an installed constraint, are rejected. This adapter does not implement
host-global Linux sysctl mutation or the intermediate privilege-dependent level.
The bounded init diagnostic grants only init-to-vendor_init at principal zero;
it is not a full Android MAC policy. Its writable-kmsg diagnostic redirect writes
userspace messages to the inherited console; privileged kernel-log forwarding is
disabled under the constraint.

Android's query adapter also publishes `property_service:set` as class 2, bit 1.
These grants remain separate from `service_manager` grants. Unknown classes and
permission bits deny access; registering a property label grants no authority.
The native policy label bound is 512 so a manager can register the pinned image's
property contexts alongside process subjects without merging label identities.
