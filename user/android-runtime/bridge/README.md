# Android absolute mouse bridge

This is an `app_process` receiver for Scarlet's inherited fd 3 mouse stream. It
uses only the stock Android 13 `InputManager`; it does not carry a system
service, input device, boot shim, or locked-screen helper.

Each frame is exactly 40 bytes, little endian:

| Offset | Type | Meaning |
| --- | --- | --- |
| 0 | `u32` | `SAM1` (`0x314d4153`) |
| 4 | `u16` | version 1 |
| 6 | `u16` | 1 position, 2 button, 3 scroll, 4 cancel |
| 8 | `i32` | absolute x |
| 12 | `i32` | absolute y |
| 16 | `u32` | Linux button 272/273/274 or wheel 6/8 |
| 20 | `i32` | button state 0/1 or wheel delta |
| 24 | `u64` | CLOCK_MONOTONIC nanoseconds; zero uses Android uptime |
| 32 | `u32` | modifier bits: shift, ctrl, alt, meta, caps, num, scroll |
| 36 | `u32` | reserved, zero |

Coordinates are bounded to one million pixels in either direction, wheel
deltas to 1,000 steps, and the timestamp to the positive `i64` range. Invalid
or partial frames stop the bridge. EOF, cancellation, malformed input, and an
injection failure clear held buttons; EOF and malformed input also issue
`ACTION_UP`, and an active hover is ended with `ACTION_HOVER_EXIT`.

The event produced for a mouse down is `ACTION_DOWN` followed by
`ACTION_BUTTON_PRESS` with the changed `actionButton`. Release produces
`ACTION_BUTTON_RELEASE` followed by the final `ACTION_UP`. Mouse events carry
`SOURCE_MOUSE`, `TOOL_TYPE_MOUSE`, and display id 0. Wheel events use
`ACTION_SCROLL` with `AXIS_HSCROLL` or `AXIS_VSCROLL` at the current position.

Build and host tests:

```sh
make -C user/android-runtime/bridge test
make -C user/android-runtime/bridge dex
```

The DEX is written to `user/android-runtime/bridge/build/classes.dex` and packed
as `user/android-runtime/bridge/build/input-bridge.jar`. Override `ANDROID_JAR`,
`D8_JAR`, or `BUILD_DIR` when the local SDK is elsewhere.

The manager must launch it as the existing privileged process (no credential
change here), using the Android `BOOTCLASSPATH` it parses from
`/init.environ.rc`, with fd 3 inherited as the request pipe:

```sh
CLASSPATH=/scarlet/runtime/input-bridge.jar \
  /system/bin/app_process64 /system/bin \
  org.scarlet.android.input.InputBridge
```

`InputManager.getInstance()` and `injectInputEvent(InputEvent, 1)` use platform
reflection and retry after `ServiceManager.waitForService("input")`. A false
return from injection is reported as a failure. Mode 1 verifies permission and
the injection result without waiting for application processing. The bridge
logs `SCARLET_ANDROID_INPUT_BRIDGE_READY` after obtaining the stock manager.
It wraps inherited fd 3 with `FileDescriptor.setInt$(3)` and explicitly closes
that descriptor once when the stream ends.
If the system has no dex build
hook, the manager's Android runtime packaging hook should copy the produced
`input-bridge.jar` into the image and use the command above; this bridge deliberately
does not add a script under `tools/`.

