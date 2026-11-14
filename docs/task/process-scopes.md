# Process scopes

A process scope explicitly groups task lifetimes and selects an orphan reaper.
It is independent of PID namespaces, filesystem views, execution Environments,
Shield labels, and Linux process groups. A PID namespace supplies hierarchical
names and visibility; PID 1 does not automatically acquire a lifecycle role.

The creator receives a close-on-exec management capability. An exclusive process
may enter once, then close the capability before executing an application.
Membership survives fork, thread creation, Environment spawn, and exec. A member
cannot replace its scope by creating another one. This initial API supports one
scope per process; it does not implement nested lifecycle scopes.

The controller may select a live member as reaper, or explicitly nominate itself
without entering the scope. The chosen process must see every member through its
PID namespace. New members must remain visible to that reaper. An orphan goes to
the surviving process thread-group leader first, then the explicitly configured
scope reaper. An unavailable or no-longer-visible reaper is skipped and the
existing host-init fallback remains. Binding a reaper does not retroactively
move existing children. Closing a management handle or losing PID 1 does not
implicitly stop a scope: userspace chooses its policy and calls termination.

Termination irreversibly stops admission and delivers an uncatchable kill event
to each member. Task publication and scope closure share a lock, so a fork which
races closure is either included or rejected before publication. Event delivery
failure is reported and the controller can retry. Other scopes and the outside
controller retain their lifetimes. No numeric UID or PID grants control authority.

Native syscalls 1320–1324 provide create, enter, set-reaper, terminate and status.
`scarlet_os::process_scope::ProcessScope` and `scarlet_std::process_scope` expose
them. Status is a fixed 16-byte record: closing, live, members, reserved (four
little-endian u32 values). Members includes exited tasks awaiting reclamation;
zero live members does not prove every exit status has been collected. No current
scope query exposes a management capability to a contained application.
