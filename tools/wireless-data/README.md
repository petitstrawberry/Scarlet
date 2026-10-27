# Wireless data-path regression fixture

Run from the repository with the Scarlet development toolchain:

```sh
nix develop --accept-flake-config -c python3 tools/test-wireless-data.py
```

The runner builds the current kernel/init/netcfgd/probe/ELF interpreter into a
separate initramfs, enables `wireless-mock`, and boots AArch64 QEMU with
`wifi.mock=1 wifi.mock.data=veth0`. The first radio wraps a real virtio/slirp
Ethernet transport with the common controlled port; the second stays a
control-only mock. `veth1` provides a separate fallback DHCP network.

The guest pauses at ASSOCIATED and verifies that normal traffic and DHCP remain
blocked. It attempts a static-address bypass, then scripts authorization and
waits for DHCP, preferred routing and both resolver sources. A host UDP echo
server confirms bidirectional traffic on each authorized connection.

The mock-only `MockDataTest` command holds bounded TX/RX queues while an actual
unresolved-neighbour packet waits in ARP. Disconnect must empty all three and
clear the address/default route immediately, followed by DNS reconciliation.
Old-generation test clones are then replayed after reconnect and must be rejected.
The runner also injects a UDP frame while the port is closed, verifies it in the
capture, and checks that the guest socket never receives it.

Further checks cover stale IPv4 updates, a short disconnect/reconnect interval,
QMP carrier loss/recovery, backend reset and controller close. The host requires
five successful UDP echoes and exactly five distinct DHCP Discover transaction
IDs, and rejects any captured old/blocked TX payload.

Artifacts are preserved under `.build/wireless-data/`: `qemu.log`, `veth0.pcap`,
`validation.json` (source/artifact hashes, compiler, QEMU command and results),
and build logs. `--no-build` repackages the host port in an existing image;
its receipt explicitly records that it did not rebuild current sources.

This tests the common Wi-Fi control/data/IP boundary. The authorization is
scripted open-network state; it does not implement WPA, RF or vendor firmware.
Retirement discards software queues while retaining submitted DMA ownership;
it cannot retract bytes already delivered to hardware or sockets.

For EAPOL transport, confirmed key operations and explicit authorization, run
[`../wireless-auth`](../wireless-auth)'s separate fixture. This data-only fixture
keeps the open-network mock configuration for regression coverage.
