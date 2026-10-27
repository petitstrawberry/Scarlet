# Network management regression fixture

Run `python3 tools/test-network-management.py` from the Scarlet development
shell. The standalone `guest/` crate is the native userspace probe; `guest/kernel.ld`
is the runner's Linux Image linker fixture. The runner packages init, netcfgd,
the probe and the current ELF interpreter, then uses QMP to change carrier on
two separate virtio NICs. Outputs are kept in `.build/network-management/`.
The probe also changes a live interface from DHCP to static IPv4, disables it,
and restores DHCP without a carrier change. It checks DNS withdrawal/replacement
and retention of the previous configuration when a fragment is invalid.

This probe restores the lifecycle coverage originally added in Scarlet commit
`d7e5605e9`. Its former `guest_tests/network_management` location was removed in
`ce96d1359`; the dedicated runner now owns it under `tools`, alongside the
network performance/regression fixtures. It does not redefine the repository's
other `guest_tests` or install/flashes any board image.

The library target imports the production netcfgd lifecycle gate for host tests:

```sh
cargo test --manifest-path tools/network-management/guest/Cargo.toml --target <host-triple> --lib
```
