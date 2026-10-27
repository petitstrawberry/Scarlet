# Protected EAPOL and GTK rekey integration

From the repository root in the development shell:

```sh
cargo test --locked --manifest-path tools/wireless-management/host/Cargo.toml --target <host-triple>
python3 tools/test-wpa-supplicant-host.py
python3 tools/wireless-ccmp/generate-vectors.py
python3 tools/test-wireless-rekey.py
```

The runner extends the [CCMP fixture](../wireless-ccmp/README.md) with a test-only
configuration marker. It builds and launches the actual Native supplicant, then
an independent AP sends protected RSN Group Key Handshake messages. Upstream
wpa_supplicant 2.12 validates EAPOL-Key MIC/replay and unwraps GTK KDEs. No upstream
vendor file is modified. The test AP uses separate HMAC/PTK/AES-wrap code and
libcrypto EVP CCM, including an RFC 3610 self-test.

## Native contract

`FEATURE_PROTECTED_EAPOL` adds `EAPOL_PROTECTED` in the existing bounded EAPOL
record. TX requests unicast PTK encryption; RX marks the PDU only after CCMP MIC,
PN, peer/source/destination and EAPOL length validation. Unknown flags and missing
backend capability are rejected. Protected EAPOL uses the **same PTK TX PN and RX
priority domain as ordinary data**, including while the controlled port is
closed. It bypasses the data hold queue, never the cryptographic checks, and
never goes to IP sockets. Unicast peer/PTK is required; GTK-authenticated group
EAPOL is rejected. A held ordinary ciphertext can be classified without committing
its PN and is revalidated when released. Loss of a deferred encrypted packet is
conservatively treated as possible control loss and closes authentication.

The Native adapter passes the verified protection indication into upstream and
requires protected group messages after authentication. Plaintext group-key
messages are ignored before altering replay or authorization state. Initial
four-way messages and upstream-validated pairwise retransmissions retain their
existing behavior. G2 after authentication is protected with the live PTK.

`FEATURE_GROUP_REKEY` adds `REPLACE_GROUP_KEY` with the existing `KeyInstall`
payload. It is controller-only, asynchronous, exact-scope/state checked, and
requires an already-authorized connection with confirmed PTK/GTK. Only one key
operation may be pending. New material/version checks and the existing five-second
operation deadline still apply. Ordinary `InstallKey`/`DeleteKey` retain their
blocking semantics.

Before confirmation, old PTK/GTK traffic continues. At the confirmed replacement,
the software port takes the ingress delivery barrier and state lock, installs the
new GTK and wipes every superseded GTK slot. The mock's slots and common key
ledger report the same cutover. PTK, its PN, link identity, packet incarnation,
DHCP lease, routes and DNS stay live. Already-decrypted old GTK delivery finishes
before cutover; held old GTK ciphertext is checked against current slots on
release. No post-cutover old GTK fallback is permitted. This conservative policy
can drop in-flight old multicast around cutover; it does not provide a dual-GTK
grace window.

The adapter does not revoke the controlled port merely for upstream
`WPA_GROUP_HANDSHAKE`. It waits for confirmed GTK commit before returning from the
key callback and before G2 is sent. Failed installation, cancellation, timeout,
MIC/key-data failure or retired-key rollback revokes the connection and retires
all keys through the normal barrier. Equal/older EAPOL replay counters are ignored;
a higher-counter retransmission of the current GTK is acknowledged without
reinstallation or RSC/PN reset. The kernel independently rejects reuse of retired
material within the connection. Existing bounded history and fresh-PTK rules are
unchanged; fresh GTK material gets its authenticated RSC floor rather than an
arbitrary zero reset. See upstream's
[key reinstallation advisory](https://w1.fi/security/2017-1/wpa-packet-number-reuse-with-replayed-messages.txt)
for the security reason to preserve counters on retransmission.

## Evidence

The QEMU test first waits for a plaintext group message to be delivered and
consumed, and requires unchanged key revision with no pending key work before
sending its protected counterpart. It makes GTK confirmation take one second in the explicit mock to
observe the pending window. It exchanges unicast UDP and old-key multicast while
pending, checks AUTHORIZED, connection/link generation and IP stability, then
checks the new GTK after G2. Two updates cover different-slot and same-slot
replacement. Old GTK, wrong-slot old material, equal/lower RSC/PN, exact protected
frame replay, same-inner-counter replay with a fresh outer PN, and a higher-counter
GTK retransmission are exercised. The latter sends a new protected G2 while key
revision remains unchanged. All outgoing data and G2 frames must have strictly
increasing PTK PN.

An authenticated attempt to restore retired GTK material closes the first
connection. A separate real-daemon association receives a CCMP-valid G1 with an
invalid inner MIC and must close without G2. Existing carrier reconnect, old-key
ciphertext, process teardown and scripted held TX/RX retirement cases also run.

The host tests additionally verify unsupported capability rejection, pending/
failed/canceled/timed-out core operations, independent protected EAPOL vectors,
atomic slot retirement, unchanged PTK counters, failed-candidate isolation and
preserved replay floors. The upstream-engine host peer tests encrypted GTK KDEs,
plaintext downgrade, MIC/wrap/driver failure and retired-material rollback.

Outputs are under `.build/wireless-rekey/`: `validation.json`, logs, encrypted
`wire.pcap` and AP-side decrypted `bridge.pcap`. Receipts hash sources and actual
images/binaries and record the independent crypto library, replay/ack counts and
GTK fingerprints; secret key bytes are not exported. `--no-build` repackages the
ephemeral UDP port and explicitly records reused artifacts. Hardware RF/DMA,
firmware offload, PMF/IGTK, AP mode and in-place PTK rekey remain unqualified.
