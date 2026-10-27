# Wireless control regression fixtures

`host/` compiles the production kernel wireless state machine, authentication helpers and deterministic
backend directly, without a substitute implementation of the control logic.
`guest/` exercises actual native control handles and poll in a separate QEMU
image. The image is built with `wireless-mock` and booted with `wifi.mock=1`;
normal kernels do not create test radios.

```sh
cargo test --manifest-path tools/wireless-management/host/Cargo.toml --target <host-triple>
python3 tools/test-wireless-management.py
```

The default control-only mock modes differ in association placement and intermediate notifications;
they supply no Ethernet or WPA implementation. The registered Wi-Fi links stay
DOWN. Scripting hooks are available only on these test backends. Build and guest
logs are preserved in `.build/wireless-management/`.

The optional data-capable mock and real virtio/slirp UDP lifecycle checks are in
[`../wireless-data`](../wireless-data). Run `python3 tools/test-wireless-data.py`
for controlled-port enforcement, DHCP, packet/configuration retirement and
reconnect/carrier/reset coverage.

Authentication host tests cover confirmed PTK/GTK gating, malformed keys, stale
versions and connection/session completions, reused key material, deletion,
revocation, backend failures, cancel/deadline handling, teardown barriers,
EAPOL copyout commit and authentication/event queue loss. The dedicated
[`../wireless-auth`](../wireless-auth) fixture adds real EAPOL frame transport in
QEMU with simulated firmware key slots.
