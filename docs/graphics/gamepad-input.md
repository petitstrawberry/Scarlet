# Native gamepad input

Gamepads remain Scarlet input devices rather than impersonating keyboards.
`InputDeviceKind::Gamepad` is the additive kind value **7**; the existing
values 0–6 are unchanged. Devices named `gamepad` receive independent
`/dev/gamepadN` names. Drivers declare raw absolute-axis ranges in
`InputDeviceMetadata`, use Linux-compatible `BTN_GAMEPAD` button codes and
`ABS_X/Y/Z/RX/RY/RZ` or `ABS_HAT0X/Y` axes, and end each coherent frame with
`SYN_REPORT`. Authoritative snapshots allow recovery after `SYN_DROPPED`.

SWS discovers `/dev/gamepad0` through `/dev/gamepad7`. Its input environment
advertises gamepad presence with capability bit `1 << 4`, independently of
keyboard presence. SWS owns optional menu navigation and key repeat.

## Pair a Bluetooth gamepad

On a system with a Classic Bluetooth adapter, log in as root at the serial
console and run `btctl`. The Bluetooth bundle installs a system service
that `stemd` starts with physical-seat management authority. The service
authenticates the calling account and accepts an unconfined root session.
The CLI also authenticates the service before sending a command.

The default adapter is `/dev/bluetooth0`. To select another numbered
adapter, put its path before the command, for example
`btctl --device /dev/bluetooth1 status`.

Put the controller in pairing mode and check that the adapter reports
`ready=true`. Scan waits for discovery to finish and lists each peer's MAC
and name. Use the MAC exactly as printed in place of the example below.
`btctl` converts it to HCI wire order.

```sh
btctl status
btctl scan
btctl pair 01:23:45:67:89:AB
```

Pairing establishes the link. For a first-generation `Joy-Con (L)` or
`Joy-Con (R)`, look for `connected=true encrypted=true profile-ready=true`.
`encrypted` reports link encryption. `profile-ready` means the HID profile
has registered the half as a native `/dev/gamepadN`. A generic peer can
have an encrypted link with `profile-ready=false`. The adapter handles
one active peer at a time.

When `pair` prints `bond saved`, the service has stored the link key in a
root-owned mode-`0600` file at `/etc/bluetooth-<ADAPTER>-<PEER>.bond`.
Both addresses use uppercase display order with the colons removed. The
final status shows `key-readable=false` after the service consumes the key.
Link-key bytes stay out of service replies and terminal output.

Keep `/etc` on persistent storage to reuse the bond after reboot. The
service rejects symlinks, permissive bond files, and storage whose ownership
or ACLs it cannot verify. If the link is established but key export or
storage fails, the command reports the failure and exits nonzero.

To end the current link and later reconnect with the saved bond on the
same adapter, run the following commands. `status` shows the active peer
and the controller's `last-error` code, with `0x0` indicating success.

```sh
btctl disconnect
btctl connect 01:23:45:67:89:AB
btctl status
```

## Menu policy

The left stick and first hat navigate with arrow keys. The default confirm
button is South and cancel is East. HOME forms the configured `Super+Space`
system chord. Nintendo A/B layout belongs in distribution configuration:

```toml
[gamepad]
navigation = true
confirm_button = "east"
cancel_button = "south"

[keybindings]
home = "Super+Space"
```

Button names are `south`, `east`, `north`, and `west`, referring to physical
positions. Sticks use a deadzone with hysteresis. Navigation key ownership is
scoped to the gamepad reader; releasing or disconnecting a gamepad does not
release the same key held by a physical keyboard. `navigation = false`
disables the global gamepad menu policy.

## Native client API and wire format

Optional SWS capability `GAMEPAD_INPUT` is `1 << 12`. Protocol version 9 remains
unchanged because clients negotiate this extension before using it.
`Connection::set_gamepad_input(surface_id, enabled, navigation)` controls an
owned window. Raw snapshots are opt-in and go to the focused subscribed
window. Set `navigation` to false for games handling their own controls;
the HOME system action remains available when global navigation is enabled.
The server rejects requests for another client's window.
ScarletUI surfaces default to navigation only (`enabled = false`,
`navigation = true`). A game using `PlatformWindow::set_gamepad_input(true,
false)` receives raw snapshots without a second synthetic confirm key.

Client message **55**, `SET_GAMEPAD_INPUT`, has exactly 12 little-endian bytes:
`window_id: u32`, `enabled: u32`, `navigation: u32`. Both flags must be 0 or 1.
It is an asynchronous setting, not an acknowledged state query.

Server event **39**, `GAMEPAD_INPUT`, has exactly 36 little-endian bytes:

| Offset | Field | Encoding |
| --- | --- | --- |
| 0 | Window ID | u32 |
| 4 | Device ID | u32, unique per reader instance, including reconnects |
| 8 | Buttons | u32 bit mask |
| 12, 14 | Left X/Y | i16, −32767…32767 |
| 16, 18 | Right X/Y | i16, −32767…32767 |
| 20, 22 | Left/right triggers | u16, 0…32767 |
| 24, 25 | Hat X/Y | i8, −1…1 |
| 26 | Flags | u16, `RESET = 1` |
| 28 | Timestamp | u64, monotonic nanoseconds |

Positive Y points down. Button bits 0–14 correspond to codes `0x130`–`0x13e`;
bits 16–20 correspond to `BTN_TRIGGER_HAPPY1`–`5` (`0x2c0`–`0x2c4`). Bit 15
and higher unused bits are reserved. Digital trigger buttons fill the trigger
value when no analog trigger axis is declared. Axes are normalized from driver
metadata; unavailable controls are zero.

`sws-client::Event::GamepadInput { surface_id, state }` carries an authoritative
snapshot. Focus loss, unsubscription, stream loss and disconnect issue a reset
to invalidate cached buttons and axes. A stalled server-side window backlog
is compacted to resets plus the latest state of each gamepad, preserving
unrelated window messages. Consumers should also clear state on window teardown
or transport failure, where further delivery cannot be guaranteed.

Adding this event extends the exhaustive `sws-client::Event` enum. Consumers
with exhaustive matches must add a gamepad arm; existing wire clients remain
compatible through capability negotiation. Rebuild coordinated native clients
and ScarletUI with the matching protocol sources.
