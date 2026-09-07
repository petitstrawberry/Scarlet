# VirtIO Input Device Driver - Implementation Guide

## Overview

Scarlet用のVirtIO Input Deviceドライバを実装しました。このドライバは、VirtIOプロトコルを使用してキーボード、マウス、タッチスクリーンなどの入力デバイスをサポートします。

## Architecture

```
┌──────────────────┐
│   QEMU/KVM       │
│  virtio-input    │
└────────┬─────────┘
         │ VirtIO Protocol
         ▼
┌──────────────────┐
│ VirtioInputDevice│ (Kernel Driver)
│   - Event Queue  │
│   - Status Queue │
└────────┬─────────┘
         │ push_event()
         ▼
┌──────────────────┐
│   EventDevice    │ (Scarlet Native)
│   /dev/input0    │
└────────┬─────────┘
         │ read()
         ▼
┌──────────────────┐
│   User Space     │
│  (GUI, etc.)     │
└──────────────────┘
```

## Implementation Components

### 1. VirtIO Input Event Structure

```rust
#[repr(C)]
struct VirtioInputEvent {
    type_: u16,    // Event type (EV_KEY, EV_REL, etc.)
    code: u16,     // Event code (KEY_A, REL_X, etc.)
    value: i32,    // Event value
}
```

このフォーマットはLinuxのvirtio_input_eventと互換性があります（タイムスタンプは含まれず、8バイト）。

### 2. VirtioInputDevice

- **Event Queue**: デバイス→ドライバの入力イベント配信
- **Status Queue**: ドライバ→デバイスのステータス更新（オプション、LEDなど）
- **EventDevice統合**: Scarletネイティブなイベントデバイスに変換

### 3. Device Registration

ドライバは自動的に検出され、以下のように登録されます:

```rust
VirtioDeviceType::Input => {
    let _dev = Arc::new(VirtioInputDevice::new(base_addr));
    // EventDevice is automatically registered as /dev/input0, /dev/input1, etc.
}
```

## Key Features

### Initialization

1. **VirtIO Device Setup**
   - デバイスリセット
   - 機能ネゴシエーション
   - Virtqueue初期化（Event + Status）

2. **Event Queue Prefill**
   - 8個の受信バッファを事前割り当て
   - デバイスがイベントを書き込むための準備

3. **EventDevice Creation**
   - `/dev/inputX`として自動登録
   - VFSからアクセス可能

### Event Processing

```rust
pub fn handle_interrupt(&self) {
    // Read ISR status
    let isr_status = self.read32_register(Register::InterruptStatus);
    self.write32_register(Register::InterruptAck, isr_status);
    
    // Process events
    self.process_events();
}

fn process_events(&self) {
    while let Some(desc_idx) = eventq.pop() {
        // Read VirtIO event
        let virtio_event = /* ... */;
        
        // Convert to Scarlet event
        self.event_device.push_event(
            virtio_event.type_,
            virtio_event.code,
            virtio_event.value
        );
        
        // Reuse buffer
        eventq.push(desc_idx)?;
    }
}
```

## QEMU Configuration

### Basic Mouse/Keyboard

```bash
qemu-system-riscv64 \
    -machine virt \
    -device virtio-keyboard-device \
    -device virtio-mouse-device \
    ...
```

### Tablet (Absolute Positioning)

```bash
qemu-system-riscv64 \
    -machine virt \
    -device virtio-tablet-device \
    ...
```

### Multiple Devices

```bash
qemu-system-riscv64 \
    -machine virt \
    -device virtio-keyboard-device,id=kbd0 \
    -device virtio-mouse-device,id=mouse0 \
    -device virtio-tablet-device,id=tablet0 \
    ...
```

それぞれが`/dev/input0`, `/dev/input1`, `/dev/input2`として登録されます。

## Testing

### In-Kernel Tests

```rust
#[test_case]
fn test_virtio_input_event_size() {
    assert_eq!(VirtioInputEvent::size(), 8);
}

#[test_case]
fn test_event_conversion() {
    let virtio_event = VirtioInputEvent {
        type_: EV_KEY,
        code: KEY_A,
        value: 1,
    };
    assert_eq!(virtio_event.type_, EV_KEY);
}
```

### User Space Testing

```rust
// Read from input device
let fd = sys_open("/dev/input0", O_RDONLY);
let mut buffer = [0u8; size_of::<InputEvent>()];

loop {
    sys_read(fd, &mut buffer);
    let event: InputEvent = unsafe { 
        core::ptr::read(buffer.as_ptr() as *const InputEvent) 
    };
    
    match event.type_ {
        EV_KEY => println!("Key: code={}, pressed={}", 
            event.code, event.value),
        EV_REL => println!("Mouse: code={}, delta={}", 
            event.code, event.value),
        _ => {}
    }
}
```

## Implementation Notes

### Memory Management

- **Buffer Allocation**: `Box<[u8]>`を使用してDMA可能なメモリを確保
- **Physical Addressing**: `translate_vaddr()`でカーネル仮想→物理アドレス変換
- **Buffer Reuse**: イベント処理後、バッファを再利用してパフォーマンス向上

### Virtqueue Management

Event Queue (Queue 0):
- Size: 8 descriptors
- Direction: Device → Driver
- Purpose: Input events delivery

Status Queue (Queue 1):
- Size: 8 descriptors  
- Direction: Driver → Device
- Purpose: LED status, etc. (currently unused)

### Interrupt Handling

1. ISRを読み取り、interrupt statusを確認
2. Acknowledgement (ISR write)
3. Used ringからイベントを取得
4. EventDeviceにpush
5. バッファを再利用

## Future Enhancements

1. **Status Queue Support**
   - キーボードLED制御（Caps Lock, Num Lock, etc.）
   - フォースフィードバック（ゲームコントローラー）

2. **Device Configuration**
   - デバイス名の取得（`VIRTIO_INPUT_CFG_ID_NAME`）
   - デバイスIDの取得（Vendor/Product ID）
   - サポートされているイベントタイプの照会

3. **Advanced Features**
   - マルチタッチサポート
   - 加速度センサー
   - ジャイロスコープ

4. **Performance Optimization**
   - イベントバッチ処理
   - より大きなキューサイズ
   - VIRTIO_RING_F_EVENT_IDX対応

## Related Files

- [kernel/src/drivers/virtio_input.rs](../../kernel/src/drivers/virtio_input.rs) - ドライバ実装
- [kernel/src/device/input/mod.rs](../../kernel/src/device/input/mod.rs) - イベント定義
- [kernel/src/device/input/event_device.rs](../../kernel/src/device/input/event_device.rs) - EventDevice実装
- [Input Event Device](./input-event.md) - EventDevice利用ガイド

## References

- [VirtIO Specification v1.2 - Input Device](https://docs.oasis-open.org/virtio/virtio/v1.2/csd01/virtio-v1.2-csd01.html#x1-3390008)
- [Linux virtio_input driver](https://github.com/torvalds/linux/blob/master/drivers/virtio/virtio_input.c)
- [QEMU virtio-input devices](https://www.qemu.org/docs/master/system/devices/virtio-input.html)
