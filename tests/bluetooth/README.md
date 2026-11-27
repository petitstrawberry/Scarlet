# Bluetooth input in QEMU

Run the fixture from the repository root in the Scarlet development shell.

```sh
python3 tests/bluetooth/run.py
```

The runner builds the AArch64 kernel, init, loader, Bluetooth binaries and guest from the current checkout. Init starts the guest in the default privileged environment. The guest launches the production `btctl --serve` broker and `joycond`, waits for a broker status exchange, then runs the shipped `btctl` client. Each boot gets a fresh private ext2 root for its bond files.

The QA kernel models a single H4 adapter with one shared host ACL packet credit. Its left and right peers have different ACL handles, link keys and remote HID channel IDs. The radio delivers H4 packets in short reads, interleaves fragmented HID reports across handles and sends both peers’ L2CAP echo requests with the same signaling identifier. It checks the echoed payload and handle, and fails if the host exceeds the advertised credit or sends plaintext ACL data. The guest uses a test-only radio device to supply button and stick reports, read the lamps and simulate link loss. All observations of pairing and gamepad input go through the production broker and input devices.

| Guest marker | Required observations |
| --- | --- |
| `BT_QA_RIGHT_PASS` | Scan resolves both names. Pairing the right half publishes `/dev/joycon-right0` and one logical gamepad. Both expose RX/RY. Repeated reports produce a single Left press, Left release, Right press and Right release through the SWS menu policy. |
| `BT_QA_MULTILINK_PASS` | Both bonds are saved, both peers remain encrypted and both upright pads deliver interleaved reports. The separate players show one and two steady lamps. The left gamepad delivers D-pad input during a lost player-two LED acknowledgement and again in the retry reply. |
| `BT_QA_COMBINED_PASS` | L+R creates one logical gamepad with independent sticks, hat and face input, and one lamp on each half. |
| `BT_QA_MINIS_PASS` | SL+SR on each half produces two sideways gamepads. The guest checks opposite stick rotations, rotated face buttons, suppressed grip buttons and the player lamps on both physical halves. |
| `BT_QA_RECONNECT_PASS` | Losing the left ACL releases its inputs. The right mini stays usable. Each half reconnects with its own saved key on a new handle while the other remains usable. Addressed disconnects and link loss release held navigation keys, remove the corresponding device nodes and leave zero connected peers. |

A successful boot ends with `BLUETOOTH_QEMU_PASS`. To keep the binaries and `serial.log`, pass `--output /path/to/scratch`. Use the same directory with `--no-build` to repeat the boot with fresh bond storage.
