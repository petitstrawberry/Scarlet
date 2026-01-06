# VirtIO RNG Device Driver - Implementation Guide

## Overview

Scarlet用のVirtIO RNG (Random Number Generator) Deviceドライバを実装しました。このドライバは、カーネルレベルの乱数生成サブシステムのエントロピーソースとして機能し、ホストのエントロピーソースから暗号学的に安全な乱数を提供します。

## Architecture

```
┌──────────────────┐
│   QEMU/KVM       │
│   virtio-rng     │
│  (Host Entropy)  │
└────────┬─────────┘
         │ VirtIO Protocol
         ▼
┌──────────────────┐
│ VirtioRngDevice  │ (Entropy Source)
│   - requestq     │
│   - Buffer (256B)│
└────────┬─────────┘
         │ EntropySource trait
         ▼
┌──────────────────┐
│ RandomManager    │ (Kernel RNG Subsystem)
│   - Pool (4096B) │
│   - Sources[]    │
└────────┬─────────┘
         │ CharDevice
         ▼
┌──────────────────┐
│RandomCharDevice  │
│   /dev/random    │ (DevFS)
└────────┬─────────┘
         │ read()
         ▼
┌──────────────────┐
│   User Space     │
│  (Applications)  │
└──────────────────┘
```

## Implementation Components

### 1. Kernel RNG Subsystem (`kernel/src/random.rs`)

カーネル全体で使用できる乱数生成機能を提供する中央サブシステムです。

**主な機能:**
- エントロピーソースの抽象化と管理
- 4096バイトの内部プール
- スレッドセーフなランダムバイト生成API
- `/dev/random` CharDeviceの提供

**API:**
```rust
// カーネル内から乱数を取得
use crate::random::RandomManager;
let mut buffer = [0u8; 32];
RandomManager::get_random_bytes(&mut buffer);
```

### 2. VirtioRngDevice (`kernel/src/drivers/virtio_rng.rs`)

VirtIO RNG仕様に基づいて実装されたエントロピーソースです。

**主な機能:**
- `EntropySource` traitの実装
- 内部バッファリングによる効率的な読み取り
- VirtIO queueの管理

**実装ファイル:** `kernel/src/drivers/virtio_rng.rs`

### 3. EntropySource Trait

複数のエントロピーソース（将来的にはハードウェアRNG、タイマージッタなど）をサポートするための抽象化レイヤーです。

```rust
pub trait EntropySource: Send + Sync {
    fn name(&self) -> &'static str;
    fn read_entropy(&self, buffer: &mut [u8]) -> usize;
    fn is_available(&self) -> bool;
}
```

## Usage

### QEMU Configuration

VirtIO RNGデバイスをQEMUで使用するには、以下のオプションを追加します:

```bash
qemu-system-riscv64 \
    -machine virt \
    ... (other options) \
    -device virtio-rng-device,bus=virtio-mmio-bus.5
```

### Device Registration

デバイスは自動的に検出され、カーネルのRandomManagerに登録されます。最初のRNGデバイスで `/dev/random` が作成されます。

### Reading Random Data from Userspace

ユーザースペースからは通常のCharDeviceとして読み取り可能です:

```c
// Open the random device
int fd = open("/dev/random", O_RDONLY);

// Read random bytes
unsigned char buffer[32];
read(fd, buffer, sizeof(buffer));

close(fd);
```

### Reading Random Data from Kernel

カーネル内部から直接乱数を取得できます:

```rust
use crate::random::RandomManager;

let mut buffer = [0u8; 32];
let bytes_read = RandomManager::get_random_bytes(&mut buffer);

// Or get a single byte
if let Some(byte) = RandomManager::get_random_byte() {
    // Use the random byte
}
```

## Implementation Details

### Entropy Source Registration

VirtIO RNGデバイスが検出されると、自動的にエントロピーソースとして登録されます:

1. VirtIO デバイスプローブでRNGデバイスを検出
2. `VirtioRngDevice`を作成・初期化
3. `RandomManager::register_entropy_source()` で登録
4. 最初のデバイスの場合、`/dev/random` CharDeviceも作成

### Random Pool Management

- 内部プールサイズ: 4096バイト
- プールが空になると自動的にエントロピーソースから補充
- 複数のエントロピーソースがある場合、利用可能な順に試行

### Thread Safety

- `Mutex`を使用してエントロピーソースリストとプールへのアクセスを保護
- スレッドセーフなAPI設計

## Future Enhancements

- 追加のエントロピーソース（ハードウェアRNG、タイマージッタ、割り込みタイミング）
- エントロピープールの品質評価
- 非ブロッキングモードのサポート
- `/dev/urandom`の実装
- エントロピー統計情報の提供

## References

- [VirtIO Specification - Entropy Device](https://docs.oasis-open.org/virtio/virtio/v1.1/cs01/virtio-v1.1-cs01.html#x1-2920004)
- Scarlet VirtIO Infrastructure: `kernel/src/drivers/virtio/`
- Kernel RNG Subsystem: `kernel/src/random.rs`
- Character Device Interface: `kernel/src/device/char/`
