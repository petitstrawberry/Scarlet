# Bluetooth input in QEMU

From the repository root, run this in the Scarlet development shell.

```sh
python3 tests/bluetooth/run.py
```

The runner builds this checkout's AArch64 kernel, init, loader, `btctl` and guest test. It boots them with a private ext2 root for bond storage. A successful run prints `BT_QA_INPUT_PASS` for each half followed by `BLUETOOTH_QEMU_PASS`. Build products and the test disk live in a temporary directory. To inspect a run, pass `--output /path/to/scratch` for a dedicated output directory containing `serial.log`. Pass the same output directory with `--no-build` to boot those binaries again with fresh bond storage.

The fixture emulates an H4 adapter and two first-generation peers. Real `btctl scan` resolves both names. The guest selects the second discovered address first, then pairs the other half after disconnect. The radio exchanges HCI pairing, authentication, encryption and packet-credit events, negotiates both L2CAP HID channels, and answers the Joy-Con device-info and report-mode subcommands. Its HID reports cross ACL fragment boundaries and short H4 reads.

For each half, the guest checks the encrypted and profile-ready status, bond save, and native gamepad registration. It reads real input records for a held shoulder button and two horizontal movements. The left half uses the direction buttons and the right half uses its stick. Those records run through SWS's production navigation code, which must emit Left press, Left release, Right press, then Right release on disconnect. The guest compares the final frame on its open handle with the initial neutral frame and checks that the removed device cannot be reopened.

The graphical viewer is outside the QEMU fixture. Scarlet's `pdfview` launches Zathura in presentation mode. In the pinned release, Right advances one page and Left goes back one page. [pdfview launcher](../../user/bin/src/pdfview.rs) · [Zathura 0.5.12 presentation bindings](https://github.com/pwmt/zathura/blob/0.5.12/zathura/config.c#L492-L501)
