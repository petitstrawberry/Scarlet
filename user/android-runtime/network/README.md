# Scarlet Android network backends

This component supplies the three-function `libnetd_updatable.so` interface that stock
Android 13 `netd` loads from the tethering APEX. It is a userland adapter over Scarlet's
native packet-meter and interface-budget backend. It does not install eBPF programs, set
`bpf.progs_loaded`, attach packet filters, or claim BPF support.

## Public ABI pin

The public header and symbol version are copied verbatim from AOSP Connectivity tag
`android-13.0.0_r16`, whose peeled commit is
`a4d6851e1910a9bde2a64d73373259e1a986e797`.

| Input | AOSP path | Git object |
| --- | --- | --- |
| Header | `netd/include/NetdUpdatablePublic.h` | `1ca5ea2aeb6525131428d8d8846de8cf671d6104` |
| Version script | `netd/libnetd_updatable.map.txt` | `dcb11a1019164cd77b948e651ad8ae8c37970345` |
| Behavior source | `netd/BpfHandler.cpp` | `6ae26c38f159bf627784672258fb131adcfdd2d9` |

The version script exports exactly `libnetd_updatable_init`,
`libnetd_updatable_tagSocket`, and `libnetd_updatable_untagSocket` under
`LIBNETD_UPDATABLE`.

## Initialization and supported operations

`libnetd_updatable_init()` validates a non-empty absolute cgroup path, opens it with
the public contract's directory/open-directory/close-on-exec flags, and confirms that
the opened filesystem is cgroup v2. It then issues Scarlet native syscall
`0x53430000 + 922` with the exact 80-byte v1 query record: version `1`, operation
`0`, and all payload and name bytes zero.

Initialization succeeds only when the kernel returns the exact supported mask
`0xfe`. Bits 1 through 7 cover meter set/remove, interface link set/clear, global
set/clear, and clearing all links for one meter. The kernel requires the existing
`MANAGE_TRAFFIC` authority. The provider stores the validated cgroup descriptor and
mask in its own initialized state; it does not configure or expose BPF state.

The selected accounting provider supports quota-2 and interface budgets through the
existing native interface and iptables adapter. Per-UID and per-tag socket accounting
is not implemented yet. After successful initialization, `tagSocket()` and
`untagSocket()` validate the supplied descriptor, then return `-EOPNOTSUPP`. They
never report invented success. A missing descriptor or non-socket descriptor retains
the corresponding `EBADF` or `ENOTSOCK` error. Before initialization, both calls return
`-EPERM`, matching the stock provider's pre-init behavior.

## Interface statistics

`libnetworkstats.so` implements the pinned AOSP C++ entry points used by
`libservice-connectivity.so`. Interface and device snapshots enumerate native links
with syscall 920 and read their existing link-layer byte/packet counters with syscall
923. Each read checks the link ID and generation so hotplug cannot silently attribute
a replacement device's counters to an old observation. Failed reads leave the caller's
output unchanged. Per-UID/tag detail remains `-EOPNOTSUPP`; the adapter does not create
BPF maps or fabricate zero counters.

The C++ library links against the selected tethering APEX's `libc++.so` using its
`__1` namespace. The build records that runtime's hash, and staging verifies it before
replacing either library. The unchanged public headers are pinned in `SOURCE_PINS.json`.

## Build and test

The Android build uses NDK `25.1.8937393` for arm64 Android API 33:

```sh
make PLATFORM_LIBCXX=/path/to/selected/tethering/apex/lib64/libc++.so
make test
```

Override `NDK_HOME`, `ANDROID_SDK`, `ABI`, or `API` in the make environment when
needed. Both builds are bounded to two compiler processes by default; override
`JOBS` deliberately for a different limit. The normal non-CFI DSO is compiled with
the NDK's default Clang settings. No
global CFI disable switch or synthetic `__cfi_check` bypass is used. Internal backend
symbols stay hidden, and the Android linker script controls the ABI.

The host test substitutes a scripted native-query backend. It checks the exact feature
mask, negative syscall failure propagation, missing and unexpected bits, cgroup-open
failure propagation, and that valid socket tag/untag calls remain explicit feature
failures while invalid descriptors retain their public argument errors.

Immutable AOSP provenance and the header/version-script SHA-256 values live in
`SOURCE_PINS.json`; CMake rejects a configure when either pinned local file changes.
Each successful Android link writes `build/<abi>/build.json` with source and artifact
hashes, actual CMake compiler metadata, the `0xfe` provider contract, and the expected
export set. The build directory is ignored. Parent staging performs the authoritative
`readelf` export check.

