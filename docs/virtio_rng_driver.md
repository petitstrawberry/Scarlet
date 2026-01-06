# VirtIO RNG Device Driver - Implementation Guide

## Overview

Scarlet用のVirtIO RNG (Random Number Generator) Deviceドライバを実装しました。このドライバは、VirtIOプロトコルを使用してホストのエントロピーソースから暗号学的に安全な乱数を提供します。

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
│ VirtioRngDevice  │ (Kernel Driver)
│   - requestq     │
│   - Buffer (256B)│
└────────┬─────────┘
         │ CharDevice
         ▼
┌──────────────────┐
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

### 1. VirtioRngDevice

VirtIO RNG仕様に基づいて実装された乱数生成デバイスドライバです。

**主な機能:**
- ホストのエントロピーソースから乱数を取得
- 内部バッファリングによる効率的な読み取り
- CharDeviceインターフェースの実装

**実装ファイル:** `kernel/src/drivers/virtio_rng.rs`

### 2. Internal Buffer

256バイトの内部バッファを使用して、VirtIO queueの操作回数を最小化します。バッファが空になると自動的にホストから新しい乱数データを要求します。

### 3. VirtQueue Management

RNG deviceは単一のvirtqueue (requestq) を使用します:
- Queue size: 8 descriptors (小規模で十分)
- Device-writable descriptorを使用してホストから乱数を受信

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

デバイスは自動的に `/dev/random` として登録されます（最初のRNGデバイスの場合）。

### Reading Random Data

ユーザースペースからは通常のCharDeviceとして読み取り可能です:

```c
// Open the random device
int fd = open("/dev/random", O_RDONLY);

// Read random bytes
unsigned char buffer[32];
read(fd, buffer, sizeof(buffer));

close(fd);
```

## Implementation Details

### Feature Negotiation

VirtIO RNG基本仕様にはデバイス固有の機能フラグがないため、標準VirtIO機能のみをサポートします。

### Buffer Fill Process

1. 内部バッファが空になったら `fill_buffer()` を呼び出し
2. 256バイトのバッファをアロケート
3. VirtQueue descriptorを設定（device-writable）
4. デバイスに通知（`notify(0)`）
5. ポーリングで完了を待機
6. 受信したデータを内部バッファにコピー

### Thread Safety

- `Mutex`を使用してvirtqueueと内部バッファへのアクセスを保護
- `RwLock`で機能フラグを管理

## Testing

### Manual Testing

1. QEMUでvirtio-rngデバイスを有効化
2. カーネルを起動し、デバイスが検出されることを確認:
   ```
   [Virtio] Detected Virtio RNG Device at 0x... registering as random
   [VirtIO RNG] Device initialized with features: 0x...
   ```
3. `/dev/random`が存在することを確認
4. デバイスから読み取りテスト

## Future Enhancements

- 非同期I/Oサポート
- 統計情報の提供 (読み取りバイト数など)
- エントロピープール管理の統合
- `/dev/urandom`のサポート

## References

- [VirtIO Specification - Entropy Device](https://docs.oasis-open.org/virtio/virtio/v1.1/cs01/virtio-v1.1-cs01.html#x1-2920004)
- Scarlet VirtIO Infrastructure: `kernel/src/drivers/virtio/`
- Character Device Interface: `kernel/src/device/char/`
