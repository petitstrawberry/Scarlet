# Scarlet Native WPA2 supplicant engine

## System integration

The `bundles/wireless` bundle installs `wirelessd`, `wifictl`, the standalone
`wpa-supplicant`, and a stemd service. `wirelessd` owns the Native controller and
the authentication engine; Settings → Network and `wifictl` use its bounded
local control socket. DHCP and resolver configuration remain owned by `netcfgd`.

```text
wifictl status
wifictl scan # Disconnect first; background scans are not supported yet.
wifictl connect MyNetwork --password-file /root/wifi-password
wifictl disconnect
wifictl connect MyNetwork
wifictl forget
settings --network
```

Disconnect before scanning. A successful password-based connection saves one
SSID and derived PMK to `/etc/wireless.conf`; an empty password reuses that
profile. Disconnect stops retries for the current session; Forget also removes
the saved profile. Passwords are not placed in command arguments or logs.
The daemon reopens the radio and creates a fresh authentication engine after a
lost connection. Scan/connect work runs outside the Settings UI thread.

This crate embeds the **unmodified upstream wpa_supplicant 2.12 RSN state
machine and internal cryptographic implementation**. Scarlet supplies the OS
callbacks and the Native wireless driver adapter. It does not reimplement the
four-way handshake in Rust or translate Linux nl80211 messages.

The upstream release is pinned in [UPSTREAM.json](UPSTREAM.json), including the
release archive SHA-256 and every selected file's hash. [sources.txt](sources.txt)
is the compiled source list. All upstream headers are retained to make conditional
includes auditable; this does not enable their features. The independent test
runners verify the complete vendor tree against the manifest before building.
Upstream attribution and the BSD license are retained in
[vendor/wpa_supplicant-2.12/README](vendor/wpa_supplicant-2.12/README) and
[COPYING](vendor/wpa_supplicant-2.12/COPYING). Release source:
[official archive](https://w1.fi/releases/wpa_supplicant-2.12.tar.gz).

## Supported boundary

The first supported policy is one STA association using RSN/WPA2-Personal,
PSK, CCMP-128, and descriptor version 2 (HMAC-SHA1-128). Upstream handles PMK/PTK
derivation, M1–M4 processing, MIC verification, replay counters, RSN IE checks,
AES-unwrapping the GTK KDE and duplicate key installation suppression. The
actual AP RSN IE comes from the backend through `GetAssociationRsn`, separately
from the station's requested association IE. No passphrase or PMK enters the kernel.

The owning Rust `Engine` has one state machine per association. All C entry
points are serialized, and driver callbacks must not reenter an engine. Its
adapter contract requires a successful key callback to mean **confirmed device
installation**, rather than queue submission. Authorization follows upstream
`WPA_COMPLETED` after both pairwise and group installation. Upstream can send M4
before these driver callbacks complete; M4 transmission alone never opens data.
The optional upstream RX-only next-key preinstall is declined because Native V1
has no such activation mode. The actual key is installed once, avoiding PN resets.

Native callbacks capture link/device/connection identity and revalidate it around
queries and waits. Key operations wait for the request ledger's confirmed result;
authorization supplies the exact current key revision. Event loss, MIC/IE/decrypt
failure, scope change, failed key work, or the five-second authentication deadline
abort the association. Teardown closes the controlled port and retires keys via
the kernel's existing barrier. A subsequent connection creates a fresh engine
and SNonce. PTK0 in-place rekey is disabled; it requires reconnect. PMKSA caching, roaming, PMF, WPA3, Enterprise EAP and arbitrary wpa_supplicant
configuration/control interfaces are not qualified by this port.

Nonce randomness requires an exact-length Native `GetRandom` result with
`GET_RANDOM_FLAG_REQUIRE_ENTROPY`. Short/error results are wiped and abort before
M2; the default noncryptographic RNG fallback is never used. C allocations use
the caller's Rust allocator, retain exact layout metadata and are wiped on free;
Rust profile/PMK/key scratch uses `zeroize`. Diagnostic/key dumps are disabled.
The C formatter is a bounded diagnostic helper, not a general libc implementation.

## Build and use

From the repository root inside `nix develop --accept-flake-config`:

```sh
cargo build --locked --manifest-path user/wpa-supplicant/Cargo.toml \
  --release --target aarch64-unknown-scarlet
```

The development shell supplies clang and `SCARLET_NATIVE_RUNTIME` headers. The
port links to the single owning Rust runtime, not a second copy of `libscarlet_c`.
The executable is packaged by the dedicated QEMU runner. Production board images
must opt into installing/starting it when they supply an RSN-capable backend;
it is not added to every default boot image.

On Scarlet, run `wpa-supplicant /dev/wifiN /etc/wpa_supplicant.conf`. Start `netcfgd`
as the independent IP configuration owner. The executable retries closed/failed
associations with bounded exponential backoff (1–30 seconds). A larger wireless
policy service with discovery, multiple profiles and IPC remains future work.

The file accepts one `network={...}` block, at most 4096 UTF-8 bytes:

```conf
network={
    ssid="Example"
    psk="example passphrase"
    key_mgmt=WPA-PSK
    proto=RSN
    pairwise=CCMP
    group=CCMP
}
```

Use a protected configuration file (0600). SSIDs may also be raw hex, and PSK may
be a 64-digit hex PMK. Quoted strings have no escape syntax; unknown options,
duplicate fields, multiple networks and weaker ciphers are rejected. Omitted
policy fields retain the same strict WPA2/CCMP policy. Plaintext file buffers are
wiped after parsing; secrets are not passed in argv or printed in errors.

[Validation and limitations](../../../tools/wpa-supplicant/README.md) distinguish
real cryptographic EAPOL processing from the mock data path. The optional
[software CCMP backend](../../../tools/wireless-ccmp/README.md) now protects
data MPDUs and enforces PN/replay rules in the kernel. Its QEMU transport uses
explicit test envelopes; physical radios and hardware offload still need
transport-specific integration. The original WPA-only mock continues to use
plaintext ordinary Ethernet; select `wifi.mock.ccmp=1` for encrypted data tests.

The [protected EAPOL/GTK extension](../../../tools/wireless-rekey/README.md) is
qualified for the software CCMP backend. The adapter preserves authorization
through group handshake, confirms atomic GTK replacement before G2, and sends G2
under PTK protection. It ignores plaintext group-key downgrade attempts after
authentication. Current-GTK retransmissions do not reinstall keys or rewind PN;
failed/rollback exchanges revoke data and retire keys. The kernel capability bits
explicitly distinguish this from ordinary blocking key installation.
