# Network management and the Wi-Fi boundary

Status: link lifecycle, IPv4 management, common Wi-Fi control and the controlled
Ethernet facade are implemented, including EAPOL transport, confirmed key
management and explicit port authorization. The pinned upstream
[Native WPA port](../../user/lib/scarlet-wpa/README.md) provides WPA2-Personal
four-way authentication and MIC verification. The optional
[software CCMP path](../../tools/wireless-ccmp/README.md) adds data encryption
and PN/replay enforcement, with protected EAPOL and confirmed GTK rotation
that preserves the active IP configuration. Real radio drivers, hardware crypto integration and
a wireless policy service remain subsequent milestones.

The [native Wi-Fi control/data API](wireless.md) defines the current common
contract, mock backends, controlled-port enforcement and implementation roadmap.

## Ownership

| Component | Owns |
| --- | --- |
| Device driver | Bus/firmware, frame TX/RX, carrier, MAC, MTU, retained link epoch |
| Kernel network manager | Interface identity, link snapshots, IPv4 addresses/routes/ARP, stale-update rejection |
| Wireless control service (next milestone) | Saved profiles, network selection, scan/connect/disconnect, authentication policy, reconnect backoff |
| `netcfgd` | IPv4 static/DHCP policy, lease renewal/expiry, preferred interface, resolver publication |
| UI/CLI | Display state and submit user intent; no independent DHCP or resolver writers |

Use a single `netcfgd` for a network namespace. The current stack is global;
these calls do not introduce namespace isolation or a new authorization model.
Legacy network setters remain available for compatibility but cannot provide
incarnation checking. Do not mix manual setters and `netcfgd` on a managed NIC.

The existing Ethernet frame path is also suitable for a FullMAC Wi-Fi chip.
`NetworkDevice::link_kind()` distinguishes Wi-Fi from wired links without
coupling IPv4/DHCP to a particular firmware protocol. A Wi-Fi device reports
carrier UP **only after association and required key installation complete**.
Scanning, authentication and association must not trigger DHCP prematurely.

## Link identity and ABI

`scarlet_abi::network` is the shared source of the fixed layouts:

| Native syscall | Operation |
| --- | --- |
| 920, `NetworkListLinksV1` | `(records_ptr, capacity)` returns required/actual record count |
| 921, `NetworkUpdateIpv4V1` | `(request_ptr)` applies or clears IPv4 for an exact link incarnation |

A link record is 64 bytes, aligned to eight bytes: boot-local nonzero `u64` ID,
`u64` generation, NUL-terminated 32-byte name, six-byte MAC, kind, state,
`u32` MTU, reserved zero. Names are 1–31 bytes. IDs are never reused during a
boot; removing and registering the same name produces a different ID.
States are UNKNOWN (0), DOWN (1), UP (2); kinds are Ethernet (1), Wi-Fi (2).
An unknown link is not considered ready.

A zero capacity probes the count. Insufficient capacity writes no records and
returns the required count, allowing callers to retry without mistaking a
truncated list for device removal. The userspace helper retries resizing.
Other failures return `usize::MAX`, consistent with the existing native network
ABI. Existing interface/IPv4 V1/V2 record layouts and syscall numbers are intact.

An IPv4 update is 40 bytes, aligned to eight bytes: ID, generation,
four-byte address/netmask/gateway, flags, metric, reserved zero. Flags are
HAS_GATEWAY (1), MAKE_DEFAULT (2), CLEAR (4). CLEAR is exclusive and requires
zero addresses and metric; it is allowed while down. Normal updates require
UP and a contiguous netmask. Unknown flags or nonzero reserved data are rejected.
No gateway flag requires a zero gateway field.

The lifecycle lock serializes registration/removal and generation validation
with configuration. A response from an old DHCP job therefore cannot configure
a removed/replaced NIC or a **known** obsolete generation. Hardware can still
change during an operation; this is not a transaction with the radio.

The generation advances for observed carrier, MAC, MTU or kind changes and for
changes in `NetworkDevice::link_epoch()`. The epoch retains notifications between
snapshots. Virtio increments it on configuration interrupts. A Wi-Fi driver
must increment its epoch on loss, reassociation and firmware reset, even when
carrier is already UP again when sampled. The default epoch of zero supports
older drivers but only detects changes visible at a snapshot. Hardware events
coalesced before the driver observes them are outside this guarantee.

V1 uses complete snapshots at one-second intervals. There is no subscription
handle yet. A future selectable event stream should carry ID, generation and
sequence; overflow must request a fresh snapshot. Snapshot recovery remains
mandatory, so this extension does not change `netcfgd`'s reconciliation model.

## Reconciliation and DHCP

`netcfgd` loads the TOML fragments and checks for changes each reconciliation
interval. Invalid edits leave the last valid configuration active. Changed
interface policies clear their address, route and DNS before reconfiguration;
pending DHCP results from the old policy are detached even when the link
generation is unchanged. Unchanged interfaces retain their configuration.
Settings → Network writes a validated per-interface override atomically to
`/etc/netcfgd.d/zz-settings-<interface>.toml` for DHCP, static IPv4 or disabled
configuration, with DNS overrides and route metrics. The daemon remains resident even if
no NIC is present, all NICs are down, or all configurations are static.
Exact name rules take precedence over `*`; the last rule for a name wins.
Disabled or unmatched interfaces are not managed.

Each poll performs these steps:

1. Read a complete link snapshot. On query failure, preserve state and retry.
2. Retire lost or changed generations. Clear IPv4, connected/default routes and
   ARP state; withdraw the generation's resolver source. Clear stale settings
   from managed down links after daemon restart as well.
3. Discover ready links, discard previous-owner settings, and start static
   configuration or DHCP. Reconnecting uses a fresh DHCP acquisition.
4. Consume completed DHCP work only when ID/generation still matches and carrier
   is UP. The kernel checks again at installation. Expired results are discarded.
5. Renew at T1, rebind after T2, and remove expired or NAK-rejected leases.
6. Select the preferred configured interface and publish the combined resolver.

DHCP workers exchange packets only. The coordinator owns installed configuration.
Workers on different NICs run concurrently; a bounded stale worker is allowed to
finish before a new worker on the same name begins. Completion cannot reinstall
an obsolete lease. Retry delay is 30 seconds; discovery interval is one second.
Configuration changes are applied by the resident coordinator without restarting
the daemon or changing the kernel's carrier generation.

UDP port reservations now include the explicitly bound interface. Two NICs can
both use port 68; same-interface reservations and wildcard overlaps still
conflict. Receive demultiplexing uses the ingress interface. Interface binding
must precede port binding; changing scope after binding is rejected. This is not
SO_REUSEPORT or general multiple-address binding support.

Preference follows existing policy: explicit default first, then lower metric,
then deterministic discovery order. Withdrawal chooses a remaining configured
interface, or clears the default when none remains. DNS sources follow the same
preference and are deduplicated. A lease owns its DNS data, so loss/expiry removes
that contribution. Static DNS follows carrier in the same way.

The resolver is assembled in memory and written to a sibling temporary file.
Filesystems supporting rename publish it atomically. Scarlet's current initramfs
overlay lacks rename; there the daemon falls back to replacing the destination
contents. Readers can observe a partial file in that fallback. Publication
failures are logged and retried; route and resolver updates are not one atomic
transaction. A future overlay rename implementation can remove this limitation.

Required entries delay initial service readiness until their selected interfaces
are present, UP and configured, and resolver publication succeeds. A required
wildcard includes matched down links. Readiness is an initial stemd notification,
not a claim that connectivity cannot subsequently be lost. Internet reachability,
captive portals and DNS query success are separate from carrier and address state.

## Wireless integration

Wireless-specific operations use a versioned control handle, separate from the
IPv4 ABI. The [common API](wireless.md) implements capability discovery,
scan/connect/disconnect, per-session request/event identity and mock lifecycle
control. EAPOL TX/RX bypasses normal data admission on an associated connection;
key operations require backend confirmation and exact connection identity.
Explicit authorization requires confirmed pairwise/group keys and the current
key revision. Credentials and cryptographic authentication policy remain
subsequent milestones.

The controlled Ethernet facade keeps the link DOWN and rejects normal TX/RX
until authorized. Disconnect/reset/reconnect retires software queues and clears
IPv4, routes and ARP synchronously under the lifecycle lock. The next authorized
generation causes `netcfgd` to acquire a fresh lease. DNS withdrawal follows the
daemon's normal reconciliation interval. Delayed packets retain their old link
token and delayed DHCP completions fail generation checks.

Real backend milestones include bus/power/reset and firmware/calibration
loading, followed by frame I/O and connection events. Record those dependencies
in the relevant board project. Determine actual firmware security/offload
capabilities before choosing authentication placement. Validate one security
mode and reconnect path before adding roaming and power saving.

## Verification and QEMU scope

The runner's network-management fixture is maintained under
`tools/network-management/guest`, together with its kernel linker script. The
runner also packages the Scarlet ELF interpreter required by the current
userspace toolchain.

Run from the Scarlet repository using its development toolchain:

```sh
cargo test --manifest-path user/lib/scarlet-abi/Cargo.toml --target <host-triple>
cargo test --manifest-path tools/network-management/guest/Cargo.toml --target <host-triple> --lib
python3 tools/test-network-management.py
```

The runner builds the current local kernel, `init`, `netcfgd` and a guest probe,
then starts AArch64 QEMU with two virtio-mmio NICs on separate user networks.
Artifacts and logs go to `.build/network-management/`; `--no-build` reuses them.
It does not use distribution pins or write an SD card. The guest checks parallel
DHCP, preferred-NIC loss, fallback routes/DNS, reconnection, stale-update rejection,
all-links-down cleanup and recovery. Host tests exercise obsolete completion and
name-reuse rejection. Kernel UDP tests cover scoped reservations and wildcard
conflicts. Kernel test execution is separate from the guest runner.

QEMU's standard network devices provide Ethernet. A host Wi-Fi uplink does not
turn virtio-net into a guest Wi-Fi radio. The runner validates the carrier/IP
management boundary, not vendor radio registers, firmware, RF, scanning or WPA.
Linux `mac80211_hwsim` creates radios inside a Linux kernel; it is not directly a
Scarlet device. The dedicated
`tools/test-wireless-data.py` runner combines a scripted Wi-Fi backend and the
common controlled Ethernet facade over virtio to exercise admission, DHCP,
retirement and reconnection with real IP packets. Real radio/firmware acceptance
still requires hardware.

References: [QEMU network devices](https://www.qemu.org/docs/master/system/devices/net.html),
[Linux mac80211_hwsim](https://wireless.docs.kernel.org/en/latest/en/users/drivers/mac80211_hwsim.html).

## Transport admission backpressure

A network transport may return `network::TX_QUEUE_FULL` when it has not admitted
or submitted a packet. Ethernet preserves this distinction as the internal
`SocketError::TransmitBusy`; ARP deferral remains an already-queued operation.
TCP advances sequence numbers only for admitted data, returns a partial write
when appropriate, and retains no ownership of the unsent application suffix.
The next blocking write sleeps until the transport signals capacity through
`tcp::notify_transmit_available()`. Write readiness is suppressed while blocked;
rejection/registration races are covered by a publication sequence and wait
readiness is rechecked after registering the sleeper. Queue retirement must also
signal capacity so old writers can observe the changed link. Native/Linux ABI
boundaries report temporary admission failure as EAGAIN without an ABI revision.
