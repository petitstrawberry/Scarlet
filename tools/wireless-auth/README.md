# Wireless authentication API regression fixture

```sh
nix develop --accept-flake-config -c python3 tools/test-wireless-auth.py
```

The runner builds the current kernel/userspace and boots AArch64 QEMU with
`wireless-mock` and `wifi.mock=1 wifi.mock.data=veth0 wifi.mock.auth=1`.
`veth0` combines the common wireless facade with a real virtio/slirp Ethernet
transport; `veth1` is a fallback DHCP network. No board-specific code is required.

A host peer exchanges raw Ethernet `0x888e` frames through QEMU filter character
channels (four-byte big-endian length, then frame). Each EAPOL TX is answered
with a wrong-source frame followed by the correct peer's frame. The guest must
receive only the latter while DHCP/IP traffic is still blocked. These are test
PDUs, not a four-way handshake or cryptographic authentication.

The mock retains owning key objects in simulated firmware slots and completes
key operations only on scripted backend confirmation. The guest verifies:

- Backend CONNECT success alone cannot authorize external authentication.
- A pending install or missing PTK/GTK prevents authorization.
- Confirmed keys plus the current revision allow DHCP and real UDP traffic.
- Active-key deletion blocks data before confirmation; replacement needs new
  key material and a fresh grant. Delete/reinstall of old material is rejected.
- Disconnect/reconnect retires backend keys, queued EAPOL and data/ARP queues.
- Old-session grants/key installs and delayed old completions cannot restore keys.
- Physical carrier loss, reset, explicit port revocation/reopening and controller
  close retire the relevant data/configuration/key state.
- Closing a controller cancels an unconfirmed key install; a late completion
  cannot recreate it after another controller claims the radio.

Success requires six EAPOL round trips, seven live UDP echoes, seven distinct
DHCP Discover IDs, captured preauthorization RX with no socket delivery, and
absence of blocked/old TX payloads from the capture. Logs, packet capture, images
and a source/artifact-hashed `validation.json` are kept in `.build/wireless-auth`.
`--no-build` repackages the host UDP port and records that sources were not rebuilt.

Production core/mock host tests separately exercise failure, timeout, cancel,
retirement refusal/recovery, malformed inputs, key version/reinstallation checks,
short/failed copyout and queue/event overflow. Run:

```sh
nix develop --accept-flake-config -c cargo test --manifest-path tools/wireless-management/host/Cargo.toml --target <host-triple>
```

This implements and tests the common API and lifetime contract. The mock does
not encrypt packets or verify EAPOL MICs. The separate
[WPA integration fixture](../wpa-supplicant/README.md) now tests the upstream
supplicant and Native adapter with real MICs and encrypted GTK KDEs. The separate
[software CCMP fixture](../wireless-ccmp/README.md) tests data encryption and
PN/replay enforcement. Real firmware and hardware crypto/PN offload still require
driver implementation and hardware acceptance tests.
