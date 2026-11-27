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
| `BT_QA_SDP_PASS` | The keyboard's report descriptor and PnP identity reach hidraw. The non-HID peer pairs and stays connected without opening HID channels. |
| `BT_QA_REPORT_PASS` | The descriptor-driven input driver publishes a keyboard press and release. Two raw readers get the same unmodified reports, and outbound reports preserve their boundaries. |
| `BT_QA_RECONNECT_PASS` | Unplug detaches the old descriptor. A saved bond reconnects and publishes a replacement at the same path while the other peer stays connected. |
| `BLUETOOTH_QEMU_PASS` | All guest checks passed. |
