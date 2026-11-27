# Classic Bluetooth HID in QEMU

From the repository root, run this in the Scarlet development shell.

```sh
python3 tests/bluetooth/run.py
```

The runner builds the kernel, init, loader, `btctl` and a guest test. It boots
with a private ext2 root for bond storage. Build products and the test disk
live in a temporary directory. Pass `--output /path/to/scratch` to retain them
with `serial.log`. Use the same directory with `--no-build` to rerun the boot
with fresh bond storage.

A synthetic H4 adapter presents two independently encrypted peers. The first
advertises a keyboard with numbered HID reports. The second returns an empty
SDP result for the HID service. The fixture fragments ACL data and splits SDP
attribute responses with continuation tokens, sharing one transmit credit
between links.

| Marker | Checks |
| --- | --- |
| `BT_QA_REGISTER_PASS` | A legacy bond enables page scan before the first broker request. The broker skips registration on an idle poll and restores the bond after an adapter reset. Inquiry learns its name, and forget unregisters it. |
| `BT_QA_SDP_PASS` | The keyboard's report descriptor and PnP identity reach hidraw. The non-HID peer pairs and stays connected without opening HID channels. |
| `BT_QA_REPORT_PASS` | The descriptor-driven input driver publishes a keyboard press and release. Two raw readers get the same unmodified reports, and outbound reports preserve their boundaries. |
| `BT_QA_RECONNECT_PASS` | Unplug detaches the old descriptor. Waking the bonded keyboard reconnects it and publishes a replacement at the same path while the other peer stays connected. |
| `BT_QA_OVERLAPPING_WAKE_PASS` | Both bonded peers page before either handshake completes. One fails authentication while the keyboard stays pending, then encrypts and delivers a HID report. The broker retains the last user command's generation and result throughout. |
| `BT_QA_HOST_RETRY_PASS` | The keyboard advertises HID host/any reconnection. An unexpected loss starts a delayed outgoing page, and a manual disconnect leaves the peer offline. |
| `BT_QA_BROKER_PASS` | An enabled non-root account scans, pairs, enumerates an offline bond, and reconnects without a new inquiry. Locked and Shield-confined clients are denied. |
| `BT_QA_FORGET_PASS` | Saved names survive disconnect. A legacy bond gains a name without replacing the key file. Forget detaches only its target, reports a failed disconnect, retries after physical loss, and prevents a keyless controller reconnect. |
| `BLUETOOTH_QEMU_PASS` | All guest checks passed. |
