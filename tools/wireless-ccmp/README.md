# Software CCMP-128 integration

Run from the repository development shell:

```sh
cargo test --locked --manifest-path tools/wireless-management/host/Cargo.toml --target <host-triple>
python3 tools/test-wireless-ccmp.py
```

The production [CCMP engine](../../kernel/src/device/wireless/ccmp.rs) uses pinned
RustCrypto `aes 0.9.3` (with key-schedule zeroization) and `ccm 0.6.1`. It implements
CCMP's AES-128, 13-byte nonce and 8-byte MIC over three-address infrastructure data
MPDUs. The CCM primitive follows [RFC 3610](https://www.rfc-editor.org/rfc/rfc3610).
802.11 AAD/nonce masking and replay-domain handling can be compared against
[mac80211 WPA](https://github.com/torvalds/linux/blob/master/net/mac80211/wpa.c)
and [RX QoS parsing](https://github.com/torvalds/linux/blob/master/net/mac80211/rx.c).
Those references describe the protocol; no Linux implementation is copied here.

## Supported policy

- STA TX uses the pairwise TK for ToDS traffic, including Ethernet multicast sent to the AP. It allocates a global 48-bit TX PN before encryption/submission, beginning at 1. Failed submissions burn the PN. Exhaustion refuses transmission and closes the facade for reauthentication; no wrap is allowed.
- STA RX accepts protected FromDS data with the associated AP as transmitter. Unicast selects PTK index 0; multicast/broadcast selects the indicated GTK slot. Reserved/ExtIV/header/address fields are checked before decryption. Raw ordinary Ethernet is rejected in CCMP mode; bounded peer EAPOL still uses the uncontrolled port.
- RX watermarks are per key and priority. Non-QoS and QoS TID 0 share priority 0, because they share the nonce domain. Other TIDs are independent. PN zero, equal and lower PN are rejected. RX state advances only after successful MIC and supported payload validation; failed decryption never reaches the IP stack.
- Key installation, PN state, packet admission and key retirement share the facade lock. RX delivery has the existing ingress retirement barrier. Key changes/teardown first block data and retire held TX/RX, deferred transport work, ARP and IP configuration. Confirmed mock key completion installs the software key; a failure is reported as failed installation, so authorization remains closed.
- Duplicate M3 is suppressed by upstream and the existing key-material/version checks; authorization changes alone never recreate the CCMP key or reset PN. Retired key bytes and AES schedules are wiped. Non-secret fingerprints and RX floors survive disconnect: an old pairwise TK cannot be reinstalled; a legitimately repeated GTK inherits the greater of its old RX floor and authenticated RSC.
- History is bounded to 256 distinct temporal-key fingerprints per facade lifetime. Exhaustion rejects new installation rather than forgetting replay history. Retained fingerprints/watermarks are not credential storage. A new physical radio/session implementation must retain this nonce-safety rule across its own resets and require fresh pairwise TKs after recreation.

The current codec supports Ethernet-II payloads up to 1514 bytes through RFC1042
LLC/SNAP, unfragmented data and QoS TIDs 0–15. Four-address/WDS, mesh, fragments,
A-MSDU/HT-control, protected management and aggregation
are not implemented. RX is strictly increasing per priority, with no reorder
window. A real driver supplies MLME, sequence numbers/retries, FCS/radio framing
and transport-specific DMA integration. Hardware crypto offload is not silently
inferred from this software capability.

## Independent checks

The host tests use RFC 3610 packet vector 1 and committed CCMP vectors generated
by [peer.py](peer.py) using libcrypto's independent EVP CCM implementation. The
peer self-checks the RFC vector on every run. It does not call the Rust engine.
Tests cover QoS/unicast/GTK, replay, tampered MIC/AAD/address/header, non-QoS/TID-0
sharing, RX floor preservation, key deletion/reinstallation, retired-scope rejection,
fresh-key isolation, TX exhaustion, burned PN and bounded history.

QEMU's `wifi.mock.ccmp=1` selects an explicitly enabled software crypto mock.
Real protected 802.11 MPDUs travel inside **test-only experimental EtherType
0x88b5** envelopes over virtio. Four directional filter-redirector streams let an
independent AP decrypt guest TX before forwarding Ethernet to slirp, and encrypt
slirp replies before guest RX. This is not RF or an 802.11 QEMU device.

The first three associations launch the actual Native `wpa-supplicant` executable.
The independent AP checks M2/M4 MICs, derives TK/GTK, and verifies strictly
increasing guest TX PN while encrypted DHCP/ARP/UDP work. It injects equal/lower/
zero PN, duplicate packets, bad MIC, modified TID, wrong GTK slot and plaintext
bypass attempts. A real M3 retransmission leaves both key revision and TX/RX PN
state unchanged; a subsequent valid packet advances TX. The next association
rejects ciphertext authenticated with the retired TK. Carrier loss retires keys
and IP, drops old traffic and triggers fresh authentication in the same daemon.

Two additional **scripted-key** associations use the same production crypto path
to hold TX plaintext and RX ciphertext in software queues. Disconnect clears the
queues and keys; attempted release of their retained stale test copies after a
fresh connection drops both without submission/decryption or UDP delivery. Fresh
data continues to work. These phases explicitly exercise queue lifetime, not WPA
authentication. The main WPA phases require no scripted key acknowledgements.

Artifacts are under `.build/wireless-ccmp/`: serial/build logs, encrypted
`wire.pcap`, decrypted AP-side `bridge.pcap`, and source/artifact-hashed
`validation.json`. Receipts identify the actual libcrypto path, Rust/QEMU/OpenSSL
versions, pinned upstream source and image/executable hashes. No key bytes are
exported. `--no-build` records reused binaries and does not validate newly changed
source. Physical radio/firmware and hardware PN/DMA behavior remain separate
acceptance work.

During regression validation, a pre-existing ARP reply/enqueue race could strand
UDP before it reached the crypto layer. Resolution now publishes the learned MAC
while holding the pending-queue lock; a late sender rechecks the cache and retains
its original packet incarnation when sending. Pending insertion no longer
replaces other senders' queued data. The general network regression runner
executes five deterministic production-method tests for both resolution orders,
concurrent admission, retirement and preserved packet tokens.

[Protected EAPOL and atomic GTK rotation](../wireless-rekey/README.md) now extend
this same codec and transport. The dedicated runner also verifies ordinary data
continues while GTK confirmation is pending, old GTK rejection and PN continuity.
