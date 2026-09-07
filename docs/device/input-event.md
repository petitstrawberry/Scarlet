# Scarlet Input Event Device - Usage Guide

## Overview

Scarletネイティブなイベントデバイス実装により、マウス、キーボード、タッチスクリーンなどの入力デバイスからのイベントを効率的に処理できます。

この実装は以下の特徴を持ちます:

- **Rustフレンドリー**: `u64`タイムスタンプ（ナノ秒）を使用し、Rustで扱いやすい設計
- **Linux互換概念**: Type/Code/Valueモデルを継承し、将来的なLinux互換レイヤの構築が容易
- **devfs統合**: `CharDevice`トレイトにより、自動的に`/dev/inputX`として公開

## Architecture

```
┌─────────────────┐
│  Input Driver   │ (e.g., Mouse, Keyboard)
│  (Interrupt)    │
└────────┬────────┘
         │ push_event()
         ▼
┌─────────────────┐
│  EventDevice    │ (Ring buffer + Waker)
│  /dev/input0    │
└────────┬────────┘
         │ read()
         ▼
┌─────────────────┐
│  User Space     │ (Window Server, Input Handler)
│  Application    │
└─────────────────┘
```

## Data Structure

```rust
#[repr(C)]
pub struct InputEvent {
    pub time: u64,      // Timestamp in nanoseconds since boot
    pub type_: u16,     // Event type (EV_KEY, EV_REL, etc.)
    pub code: u16,      // Event code (KEY_A, REL_X, etc.)
    pub value: i32,     // Event value
}
```

### Event Types

- `EV_SYN` (0x00): Synchronization events
- `EV_KEY` (0x01): Key/button press and release
- `EV_REL` (0x02): Relative axis movement (e.g., mouse)
- `EV_ABS` (0x03): Absolute axis position (e.g., touchscreen)
- `EV_SW` (0x05): Posture and lid switch state changes

### Multitouch and Switch ABI

Linux-compatible type-B multitouch devices report `ABS_MT_SLOT` (0x2f),
`ABS_MT_TRACKING_ID` (0x39), `ABS_MT_POSITION_X` (0x35),
`ABS_MT_POSITION_Y` (0x36), and optional `ABS_MT_TOUCH_MAJOR` (0x30) or
`ABS_MT_PRESSURE` (0x3a), followed by one `EV_SYN`/`SYN_REPORT` for the
complete frame. `SYN_MT_REPORT` (0x02) remains available for legacy contact
reports.

`EV_SW` uses Linux switch codes including `SW_LID` (0) and
`SW_TABLET_MODE` (1). A non-zero event value means the reported switch is
active; zero means inactive.

`EventDevice` exposes additive Scarlet controls:

| Control | Meaning |
| --- | --- |
| `0x5353_0104` | Multitouch slot count |
| `0x5353_0105` | Bit mask of supported switch codes |
| `0x5353_0106` | Current state (0 or 1) for the switch code passed as the argument |

## Kernel Usage

### 1. Device Registration

```rust
use crate::device::input::event_device::EventDevice;
use crate::device::manager::DeviceManager;
use alloc::sync::Arc;

// In device driver initialization
let event_dev = Arc::new(EventDevice::new("input0"));
DeviceManager::get_mut_manager().register_device(event_dev.clone());
```

### 2. Pushing Events from Interrupt Handler

```rust
use crate::device::input::event_types::*;
use crate::device::input::rel_codes::*;
use crate::device::input::syn_codes::*;

// In mouse driver interrupt handler
fn handle_mouse_interrupt(event_dev: &EventDevice, dx: i32, dy: i32) {
    // Push relative movement events
    event_dev.push_event(EV_REL, REL_X, dx);
    event_dev.push_event(EV_REL, REL_Y, dy);
    
    // Synchronization marker (end of event packet)
    event_dev.push_event(EV_SYN, SYN_REPORT, 0);
}
```

### 3. Keyboard Example

```rust
use crate::device::input::key_codes::*;
use crate::device::input::key_values::*;

fn handle_keyboard_interrupt(event_dev: &EventDevice, key: u16, pressed: bool) {
    let value = if pressed { KEY_PRESS } else { KEY_RELEASE };
    
    event_dev.push_event(EV_KEY, key, value);
    event_dev.push_event(EV_SYN, SYN_REPORT, 0);
}
```

## User Space Usage

### Reading Events

```rust
use core::mem::size_of;

#[repr(C)]
struct InputEvent {
    time: u64,
    type_: u16,
    code: u16,
    value: i32,
}

fn main() {
    // Open the event device
    let fd = sys_open("/dev/input0", O_RDONLY);
    
    let mut buffer = [0u8; size_of::<InputEvent>()];
    
    loop {
        // Read blocks until an event is available
        let bytes_read = sys_read(fd, &mut buffer);
        
        if bytes_read == size_of::<InputEvent>() {
            let event: InputEvent = unsafe {
                core::ptr::read(buffer.as_ptr() as *const InputEvent)
            };
            
            handle_event(&event);
        }
    }
}

fn handle_event(event: &InputEvent) {
    match event.type_ {
        EV_KEY => {
            println!("Key event: code={}, pressed={}", 
                event.code, event.value);
        }
        EV_REL => {
            println!("Relative movement: code={}, delta={}", 
                event.code, event.value);
        }
        _ => {}
    }
}
```

### Non-blocking I/O

```rust
// Set non-blocking mode via fcntl
sys_fcntl(fd, F_SETFL, O_NONBLOCK);

// Read returns immediately if no data
let bytes_read = sys_read(fd, &mut buffer);
if bytes_read == 0 {
    // No data available, do other work
}
```

### Select/Poll Support

```rust
let mut fds = [PollFd {
    fd: input_fd,
    events: POLLIN,
    revents: 0,
}];

// Wait for input events
sys_poll(&mut fds, 1, timeout_ms);

if fds[0].revents & POLLIN != 0 {
    // Data available, read it
    sys_read(input_fd, &mut buffer);
}
```

## Linux Compatibility Layer

ユーザランドでLinux互換の`struct input_event`が必要な場合、変換ラッパーを提供できます:

```rust
// Linux compatibility wrapper
#[repr(C)]
struct LinuxInputEvent {
    time: libc::timeval,  // {tv_sec: i64, tv_usec: i64}
    type_: u16,
    code: u16,
    value: i32,
}

fn scarlet_to_linux(scarlet_event: &InputEvent) -> LinuxInputEvent {
    LinuxInputEvent {
        time: libc::timeval {
            tv_sec: (scarlet_event.time / 1_000_000_000) as i64,
            tv_usec: ((scarlet_event.time % 1_000_000_000) / 1000) as i64,
        },
        type_: scarlet_event.type_,
        code: scarlet_event.code,
        value: scarlet_event.value,
    }
}
```

## Implementation Details

### Ring Buffer

- デフォルト容量: 256イベント
- オーバーフロー時: 部分フレームを公開しない。キューを破棄し、`EV_SYN`/`SYN_DROPPED` (3)、続いて空フレーム境界の`EV_SYN`/`SYN_REPORT`を送る。中断されたフレームの残りは次の`SYN_REPORT`まで破棄する
- `SYN_DROPPED`を受け取った読取側は、保持しているキー、ボタン、またはマルチタッチslot状態を破棄し、直後の`SYN_REPORT`を空フレーム境界として扱う
- スレッドセーフ: `Mutex`による保護

### Blocking Behavior

- デフォルト: ブロッキングモード
- データがない場合、`Waker`により待機
- 割り込みハンドラからの`push_event()`で起床

### Memory Layout

`InputEvent`は`#[repr(C)]`により、メモリレイアウトが保証されています。これにより:
- ファイルから直接`read()`してキャスト可能
- 構造体サイズは`16バイト`
- エンディアンは実行環境に依存

## Testing

```bash
# Run tests
cd /workspaces/Scarlet/kernel
cargo test --target targets/riscv64gc-unknown-none-elf.json
```

テストケース:
- `test_event_device_creation`: デバイス作成のテスト
- `test_push_and_read_event`: イベント送受信のテスト
- `test_queue_overflow`: キューオーバーフローのテスト

## Future Enhancements

1. **タイムアウト付きブロッキング**: `select/poll`でのタイムアウトサポート
2. **イベントフィルタリング**: 特定のイベントタイプのみ受信
3. **複数デバイス対応**: `/dev/input0`, `/dev/input1`, etc.
4. **デバイス情報**: `EVIOCGNAME`などのioctl対応
5. **LED制御**: キーボードLED状態の設定

## References

- Linux Input Subsystem: https://www.kernel.org/doc/html/latest/input/
- Event Codes: https://github.com/torvalds/linux/blob/master/include/uapi/linux/input-event-codes.h
