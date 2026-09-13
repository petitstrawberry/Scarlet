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

Client message **55**, `SET_GAMEPAD_INPUT`, has exactly 12 little-endian bytes:
`window_id: u32`, `enabled: u32`, `navigation: u32`. Both flags must be 0 or 1.
It is an asynchronous setting, not an acknowledged state query.

Server event **39**, `GAMEPAD_INPUT`, has exactly 36 little-endian bytes:

| Offset | Field | Encoding |
| --- | --- | --- |
| 0 | Window ID | u32 |
| 4 | Device ID | u32, stable for the reader's lifetime |
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
