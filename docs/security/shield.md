# Shield

Shield is Scarlet's native protection mechanism: capability-controlled policy
management, subject assignment, credentials, file labels, delegated transitions
and inherited execution constraints. Policies have stable identity and publish
validated generations through bounded additive updates. `kernel/src/shield/` has no Android or Linux terminology.
Environment still maps ABIs to filesystem views; it does not own policy or
process labels. A userspace environment manager composes these primitives.

## Native policy and authority

A policy contains up to 2,048 uniquely named labels and 16,384 grants.
Names are bounded to 255 bytes; the authoritative limits are in
[scarlet-abi/shield.rs](../../user/lib/scarlet-abi/src/shield.rs). A grant is
`(source label, target label, class, permission bits)`. Classes and the 64-bit
permission mask belong to the enforcement point. Unknown labels, classes and
ungranted bits deny. Equal label numbers in different policy objects do not
identify the same subject, and cross-policy authorization denies.

`ShieldPolicyCreate` (1310) copies fixed-width label and rule records and
publishes a fully validated initial policy generation. It requires bootstrap authority
or an existing policy management capability. A delegated `PolicyFactory` also
allows construction. The returned handle is
close-on-exec. `ShieldPolicyCurrent` (1311) returns query authority only; handle
metadata changes cannot upgrade it. `ShieldApply` (1312) requires management
authority and an exclusive, single-threaded process. It assigns one of that
policy's labels to the current process. The `scarlet_os::shield::Policy` wrapper
owns these capabilities.

`ShieldPolicyFactory` (1347) retains existing construction authority as a separate
close-on-exec handle. It references no policy and cannot query, apply or modify
an existing policy. The ordinary init explicitly passes it into PID 1's initial
program; stemd delegates it only through administrator-owned application grants.

`ShieldPolicyCreateWithTransitions` (1313) additionally accepts bounded
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

## Bounded policy updates

[Policy updates](../../kernel/src/shield/update.rs) require management authority
and an expected generation. Validation builds a private snapshot before atomic
publication; stale generations and invalid or over-capacity additions fail
without publishing a partial policy. Updates preserve label IDs/names, existing
grants and exec/default selection. They add authority rather than revoking it.
Current-policy/query handles cannot publish updates.

Updates can add labels, permission grants, exact-principal or inclusive
principal-range transitions, file rules and explicit label sets. A transition
checks all base, active and retained principal slots; a range is not an
unconditional relabel capability. Label sets are explicit native memberships,
not implicit SELinux category dominance. See
[sets](../../kernel/src/shield/sets.rs) and
[the userspace Policy wrapper](../../user/lib/scarlet-os/src/shield.rs).

## Credentials and file labels

[Native credentials](../../kernel/src/shield/identity.rs) hold principal/group
identities, supplementary groups and bounded change authority. Linux IDs and
capability operations translate to that state. A numeric UID alone does not
create a policy-management, mount-management or resource-management capability.

[File policy](../../kernel/src/shield/file.rs) selects and authorizes a new
inode's label before the inode is published. Relabel operations check the live
subject, old label, node kind and delegated target. This differs from retained
resource bindings used to associate a stable opened inode with an exec label.

A policy can opt into a stable 16-byte persistence domain. Supporting filesystems
store a versioned symbolic label under the reserved
`trusted.scarlet.shield.<domain-hex>` metadata key; policy addresses and numeric
label IDs are not persisted. Ext2 implements persistent metadata, while TmpFS
retains it for the lifetime of the in-memory inode. Public xattr APIs reject the
reserved namespace. Malformed or unknown label metadata must not silently
become an unlabeled file or grant new authority. Tests are in
[file.rs](../../kernel/src/shield/file.rs) and
[Ext2 xattr tests](../../kernel/src/fs/vfs_v2/drivers/ext2/xattr_tests.rs).

## Android service authorization

The Android ABI supplies an `android-selinux` filesystem. Creating it requires
`policy=<management handle>`; mounting it uses existing View capabilities.
It exposes libselinux's read-only status page, enforcing state, class and
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
Inherited descriptors cannot relabel their original owner or their new holder.
Native policy publication notifies the Android adapter, which updates its
[seqlock status page](../../kernel/src/abi/android/policy_status.rs) to expose the
new generation for access-decision cache invalidation.

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

Shield is not a complete SELinux binary-policy engine. Native file creation,
relabeling and exec selection use explicit Shield rules; Android policy/context
translation belongs to its adapter and userspace producer. This does not imply
complete SELinux filesystem permission coverage. Native Shield credentials
separately hold principal/group identities and change authority;
Linux and Android translate these to UID/GID and supported capability operations.
Inherited Shield execution constraints enforce Linux seccomp filters before ABI
dispatch, including attempts to escape through native syscalls.

## Reserved exec transitions

`ShieldReserveExec` (1315) reserves a transition delegated by the
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

A userspace manager can install this constraint before launching children.
Linux `kptr_restrict` reports that native state and accepts `2`
only when already enforced. Requests to loosen it, or fabricate a global setting
without an installed constraint, are rejected. This adapter does not implement
host-global Linux sysctl mutation or the intermediate privilege-dependent level.

Android's query adapter also publishes `property_service:set` as class 2, bit 1.
These grants remain separate from `service_manager` grants. Unknown classes and
permission bits deny access; registering a property label grants no authority.
Registering property and process labels uses the same bounded native policy
without merging their identities.

File-dependent exec rules match the source label, registered inode label and all
three native principal slots. The Android `create` query previews the target;
common exec validates the actual opened inode and credentials before committing
any process state. A failed exec retains its reservation. File-derived label
changes respect no-privilege-gain mounts; direct delegated transitions remain
separate. Registering an inode requires policy management authority. Bindings
retain the inode and filesystem, survive rename and hard links, and cannot move
to a replacement file at the same path. They are bounded and fixed once set.
Retained resource bindings provide exec selection. Inode-owned labels also
support the creation, relabeling and persistence rules described above; these
are not a claim of general filesystem MAC or file-integrity measurement.

Local sockets snapshot the native subject alongside the originating credentials.
Android translates the connected peer's captured subject through `SO_PEERSEC`;
an exited or relabeled peer does not change the snapshot, and foreign policy
identities fail authorization instead of returning a manufactured context.
