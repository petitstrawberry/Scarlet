# WPA2-Personal integration fixture

Run from the repository root in `nix develop --accept-flake-config`:

```sh
python3 tools/test-wpa-supplicant-host.py
python3 tools/test-wpa-supplicant.py
```

The host runner links the actual vendored upstream RSN engine. The QEMU runner
builds the local kernel, init, netcfgd, ELF interpreter, Native supplicant and
[guest probe](guest/src/main.rs), then starts the **actual executable** as a
separate userspace process. The probe observes state; it does not send mock key
completion or controlled-port authorization commands for the supplicant.
`wifi.mock.wpa2=1` makes the mock provide association RSN metadata and asynchronous
key completion, unlike the older manually scripted raw EAPOL fixture.

[authenticator.py](authenticator.py) is an independent AP peer, using Python
PBKDF2/HMAC-SHA1/PTK derivation and RFC3394 AES key wrapping through OpenSSL's
AES block primitive. It verifies M2/M4 MICs and checks the host engine's installed
TK/GTK fingerprints against independently derived values. It never calls the
supplicant's cryptographic implementation. Test credentials are synthetic;
receipts contain hashes, not plaintext PMK/PTK/GTK values.

Host coverage includes two fresh successful handshakes, duplicate M3 without
key reinstallation, MIC corruption, wrong PSK, ANonce mismatch, invalid wrapped
GTK, mismatched RSN IE, unavailable entropy, key-install failure, authentication
timeout and truncated input. A key-install failure may follow M4 transmission
in upstream's ordering; ordinary data remains closed and live keys are retired.

The QEMU AP exchanges real `0x888e` Ethernet frames over filter-mirror and
filter-redirector Unix streams with four-byte network-order lengths. The
controlled data path uses virtio/slirp and a host UDP echo socket. Assertions cover:

- Blocked ordinary traffic before authentication; DHCP and UDP only after confirmed keys and authorization.
- Corrupt MIC, wrong password, timeout and a previous connection's M3 close the association, retire keys/IP state and transmit no ordinary data.
- Duplicate M3 leaves the key revision unchanged.
- A new process connection and carrier loss/recovery in the same supplicant process use fresh SNonce/TK, new connection identities and new DHCP transactions.
- Killing the controlling process retires software and simulated hardware keys, including pending work.
- A second boot without virtio-rng fails closed before M2, with no keys, DHCP or UDP.

Artifacts are under `.build/wpa-supplicant/`: `host-validation.json`,
`validation.json`, build logs, serial logs and pcaps. Both runners verify all
vendor hashes and record source hashes before building, require unchanged inputs
at completion, and record executable/image hashes and toolchain versions.
`--no-build` explicitly records reused artifacts and does not prove current
source was compiled. QEMU logs can contain unrelated virtio/kernel diagnostics;
the receipt requires the guest assertions and independently observed frames.

The cryptographic handshake, MIC verification and GTK unwrap are real. RF,
802.11 MLME interoperability, firmware, CCMP data encryption and real-device PN
handling are **not** emulated or validated. The mock only stores simulated keys
and gates ordinary Ethernet traffic. Hardware acceptance must test those remaining
responsibilities with a real backend.

The optional [CCMP integration fixture](../wireless-ccmp/README.md) now adds real
software data encryption and PN enforcement using `wifi.mock.ccmp=1`. It uses
an independent AP bridge over encrypted test envelopes; this original WPA-only
runner intentionally keeps its existing Ethernet transport behavior.

The engine host runner also checks protected GTK rekey, higher-counter retries
without reinstallation, plaintext downgrade refusal and group MIC/wrap/driver/
retired-key failures. [Native protected EAPOL/GTK tests](../wireless-rekey/README.md)
exercise these through the actual daemon and encrypted QEMU data path.
