# Scarlet OS TTY Subsystem Design (Revised)

## Overview

このドキュメントは、Scarlet OSにUnixライクなTTYサブシステムを導入するための実装可能な設計案を示します。既存のDeviceManager・CharDevice・VFS・割り込み処理システムを最大限活用し、段階的に実装できる構造で設計されています。

**注意**: この設計案は実際のコードベースに合わせて継続的に修正されます。シグナル処理などの未実装機能はplaceholderとして扱います。

## Architecture Overview

### Key Design Principles

1. **Generic Event System**: デバイス固有のコールバックではなく、汎用的な`DeviceEvent`システムを使用
2. **Minimal Dependencies**: 既存のDeviceManager・CharDevice・VFS・割り込み処理システムを最大限活用
3. **Stepwise Implementation**: 段階的な実装により、基本機能から高度な機能まで順次導入
4. **Memory Safety**: Weak参照による自動生存期間管理

### 4-Layer Architecture

```
[Application Layer]
      ↓
[VFS Layer] - /dev/tty0, /dev/tty1, etc.
      ↓
[TTY Layer] - Line discipline, Terminal I/O control
      ↓
[Character Device Layer] - CharDevice abstraction
      ↓
[Hardware/Driver Layer] - UART, Console drivers
```

### Core Components

1. **TTY Device Layer** (`TtyDevice`)
   - Terminal I/O制御
   - Line discipline (canonical/raw mode)
   - Signal handling
   - Job control

2. **Character Device Layer** (`CharDevice`)
   - 既存のCharDeviceトレイト
   - 統一されたキャラクタデバイスインターフェース

3. **Hardware Driver Layer** (`UartDevice`, `ConsoleDevice`)
   - 実際のハードウェアドライバ
   - 割り込み処理
   - イベント通知

4. **Event System** (`DeviceEvent`, `DeviceEventListener`)
   - デバイス間のイベント通知
   - 非同期通信サポート

## Implementation Strategy

### Phase 1: Minimal Working TTY (基本機能)
1. **基本入出力**: UART → TTY → Application の最小限のデータフロー
2. **デバイス登録**: `/dev/tty0` としてVFSに登録
3. **単純なコールバック**: イベントシステムなしで直接呼び出し

### Phase 2: Line Discipline (行編集機能)
1. **カノニカルモード**: 改行までバッファリング
2. **エコーバック**: 入力文字の表示
3. **基本的な行編集**: Backspace処理

### Phase 3: Advanced Features (高度な機能)
1. **RAWモード**: 即座の文字転送
2. **特殊文字処理**: Ctrl+C, Ctrl+D (シグナルはplaceholder)
3. **ioctl**: 端末設定の変更

### Phase 4: Future Extensions (将来拡張)
1. **シグナル処理**: プロセス管理システム実装後
2. **Job Control**: プロセスグループ管理
3. **Pseudo-terminals**: pty/ptmx

## Detailed Design

### 1. Simplified Event System (Phase 1実装)

実際のScarletコードベースに合わせたシンプルなコールバック方式:

```rust
// kernel/src/device/char/tty.rs

use alloc::sync::{Arc, Weak};
use alloc::collections::VecDeque;
use spin::Mutex;
use crate::device::char::CharDevice;
use crate::device::manager::DeviceManager;
use crate::device::{Device, DeviceType};

/// TTY入力コールバック (シンプル版)
pub trait TtyInputCallback: Send + Sync {
    fn on_input_received(&self, byte: u8);
}

/// TTY device implementation (Phase 1)
pub struct TtyDevice {
    id: usize,
    name: &'static str,
    uart_device_id: usize,
    
    // 基本的な入出力バッファ
    input_buffer: Mutex<VecDeque<u8>>,
    
    // Line discipline フラグ (Phase 2で拡張)
    canonical_mode: bool,
    echo_enabled: bool,
}

impl TtyDevice {
    pub fn new(id: usize, name: &'static str, uart_device_id: usize) -> Self {
        Self {
            id,
            name,
            uart_device_id,
            input_buffer: Mutex::new(VecDeque::new()),
            canonical_mode: true,
            echo_enabled: true,
        }
    }
    
    /// UART割り込みハンドラから呼ばれる入力処理
    pub fn handle_input_byte(&self, byte: u8) {
        let mut input_buffer = self.input_buffer.lock();
        
        if self.canonical_mode {
            // Phase 2: カノニカルモード処理
            match byte {
                b'\r' | b'\n' => {
                    input_buffer.push_back(b'\n');
                    // TODO: wakeup waiting readers
                }
                0x08 | 0x7F => {
                    // Backspace処理
                    input_buffer.pop_back();
                    if self.echo_enabled {
                        self.echo_backspace();
                    }
                }
                0x03 => {
                    // Ctrl+C - TODO: シグナル処理 (placeholder)
                    // self.send_signal(SIGINT);
                }
                _ => {
                    input_buffer.push_back(byte);
                    if self.echo_enabled {
                        self.echo_char(byte);
                    }
                }
            }
        } else {
            // RAWモード: そのまま追加
            input_buffer.push_back(byte);
        }
    }
    
    fn echo_char(&self, byte: u8) {
        // 実際のUART deviceを取得して出力
        let device_manager = DeviceManager::get_manager();
        if let Some(borrowed_device) = device_manager.borrow_device(self.uart_device_id).ok() {
            let device = borrowed_device.device();
            let mut device_guard = device.write();
            if let Some(char_device) = device_guard.as_char_device() {
                let _ = char_device.write_byte(byte);
            }
        }
    }
    
    fn echo_backspace(&self) {
        // バックスペースのエコー: BS + space + BS
        self.echo_char(0x08);
        self.echo_char(b' ');
        self.echo_char(0x08);
    }
}

impl DeviceEventListener for TtyDevice {
    fn on_device_event(&self, event: &dyn DeviceEvent) {
        if let Some(input_event) = event.as_any().downcast_ref::<InputEvent>() {
            self.handle_input_byte(input_event.data);
        }
    }
    
    fn interested_in(&self, event_type: &str) -> bool {
        event_type == "input"
    }
}

impl Device for TtyDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    
    fn name(&self) -> &'static str {
        self.name
    }
    
    fn id(&self) -> usize {
        self.id
    }
    
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
    
    fn as_char_device(&mut self) -> Option<&mut dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for TtyDevice {
    fn read_byte(&mut self) -> Option<u8> {
        let mut input_buffer = self.input_buffer.lock();
        input_buffer.pop_front()
    }
    
    fn write_byte(&mut self, byte: u8) -> Result<(), &'static str> {
        // UARTデバイスに直接転送
        let device_manager = DeviceManager::get_manager();
        if let Some(borrowed_device) = device_manager.borrow_device(self.uart_device_id).ok() {
            let device = borrowed_device.device();
            let mut device_guard = device.write();
            if let Some(char_device) = device_guard.as_char_device() {
                return char_device.write_byte(byte);
            }
        }
        Err("UART device not available")
    }
    
    fn can_read(&self) -> bool {
        !self.input_buffer.lock().is_empty()
    }
    
    fn can_write(&self) -> bool {
        // UARTデバイスの状態を確認
        let device_manager = DeviceManager::get_manager();
        if let Some(borrowed_device) = device_manager.borrow_device(self.uart_device_id).ok() {
            let device = borrowed_device.device();
            let device_guard = device.read();
            if let Some(char_device) = device_guard.as_any().downcast_ref::<crate::drivers::uart::virt::Uart>() {
                return char_device.can_write();
            }
        }
        false
    }
}

impl TtyInputCallback for TtyDevice {
    fn on_input_received(&self, byte: u8) {
        self.handle_input_byte(byte);
    }
}
```

### 2. UART Device Integration (汎用的なイベント機構を活用)

デバイス固有のAPIを避け、汎用的なDeviceEventシステムを活用:

```rust
// kernel/src/device/events.rs (シンプル版)

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use spin::Mutex;

/// シンプルなデバイスイベント
pub trait DeviceEvent {
    fn event_type(&self) -> &'static str;
    fn as_any(&self) -> &dyn core::any::Any;
}

/// デバイスイベントリスナー
pub trait DeviceEventListener: Send + Sync {
    fn on_device_event(&self, event: &dyn DeviceEvent);
    fn interested_in(&self, event_type: &str) -> bool;
}

/// イベント対応デバイス
pub trait EventCapableDevice {
    fn register_event_listener(&mut self, listener: Weak<dyn DeviceEventListener>);
    fn unregister_event_listener(&mut self, listener_id: &str);
    fn emit_event(&self, event: &dyn DeviceEvent);
}

/// デバイスイベントエミッター (汎用実装)
pub struct DeviceEventEmitter {
    listeners: Mutex<Vec<Weak<dyn DeviceEventListener>>>,
}

impl DeviceEventEmitter {
    pub fn new() -> Self {
        Self {
            listeners: Mutex::new(Vec::new()),
        }
    }
    
    pub fn register_listener(&mut self, listener: Weak<dyn DeviceEventListener>) {
        let mut listeners = self.listeners.lock();
        listeners.push(listener);
    }
    
    pub fn emit(&self, event: &dyn DeviceEvent) {
        let mut listeners = self.listeners.lock();
        
        // 生きているリスナーのみに通知し、死んだ参照を削除
        listeners.retain(|weak_listener| {
            if let Some(listener) = weak_listener.upgrade() {
                if listener.interested_in(event.event_type()) {
                    listener.on_device_event(event);
                }
                true // Keep alive
            } else {
                false // Remove dead reference
            }
        });
    }
}

/// 入力イベント
#[derive(Debug)]
pub struct InputEvent {
    pub data: u8,
}

impl DeviceEvent for InputEvent {
    fn event_type(&self) -> &'static str {
        "input"
    }
    
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

impl DeviceEvent for InputEvent {
    fn event_type(&self) -> &'static str {
        "input"
    }
    
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}
```

```rust
// kernel/src/drivers/uart/virt.rs への修正

use crate::device::events::{DeviceEventEmitter, EventCapableDevice, InputEvent};
use alloc::sync::Weak;

pub struct Uart {
    base: usize,
    interrupt_id: Option<InterruptId>,
    rx_buffer: Option<alloc::sync::Arc<Mutex<VecDeque<u8>>>>,
    // 汎用的なイベントエミッター
    event_emitter: DeviceEventEmitter,
}

impl Uart {
    pub fn new(base: usize) -> Self {
        Uart {
            base,
            interrupt_id: None,
            rx_buffer: None,
            event_emitter: DeviceEventEmitter::new(),
        }
    }
    
    // ...existing methods...
}

impl EventCapableDevice for Uart {
    fn register_event_listener(&mut self, listener: Weak<dyn crate::device::events::DeviceEventListener>) {
        self.event_emitter.register_listener(listener);
    }
    
    fn unregister_event_listener(&mut self, _listener_id: &str) {
        // 実装は後回し - 通常はWeakRefが自動的に削除される
    }
    
    fn emit_event(&self, event: &dyn crate::device::events::DeviceEvent) {
        self.event_emitter.emit(event);
    }
}

/// 修正された UART interrupt handler
fn uart_interrupt_handler(handle: &mut crate::interrupt::InterruptHandle) -> crate::interrupt::InterruptResult<()> {
    let device_manager = crate::device::manager::DeviceManager::get_manager();
    
    if let Some(borrowed_device) = device_manager.borrow_first_device_by_type(crate::device::DeviceType::Char) {
        let device = borrowed_device.device();
        let device_guard = device.read();
        
        if let Some(uart) = device_guard.as_any().downcast_ref::<Uart>() {
            let iir = uart.reg_read(IIR_OFFSET);
            
            if iir & IIR_PENDING == 0 {
                match iir & 0x0E {
                    IIR_RDA => {
                        while uart.can_read() {
                            let byte = uart.read_byte_internal();
                            
                            // 汎用的なイベント発火
                            let input_event = InputEvent { data: byte };
                            uart.emit_event(&input_event);
                            
                            // 従来のバッファにも保存（後方互換性）
                            if let Some(buffer) = uart.get_rx_buffer() {
                                buffer.lock().push_back(byte);
                            }
                        }
                    }
                    IIR_THRE => {
                        // 送信完了割り込み - 将来的に対応
                    }
                    _ => {}
                }
            }
        }
    }
    
    handle.complete()
}
```
                            }
                        }
                    }
                    IIR_THRE => {
                        // 送信完了割り込み
                        // TODO: 将来的に送信完了通知
                    }
                    _ => {}
                }
            }
        }
    }
    
    handle.complete()
}
```

```rust
// kernel/src/device/char/tty.rs

### 3. VFS Integration (実際のAPIに合わせた実装)

VFS v2との統合は既存のFileObjectインターフェースを活用:

```rust
// kernel/src/device/char/tty.rs への追加

use crate::fs::{FileObject, StreamOps, StreamError};
use alloc::sync::Arc;

/// TTY device を FileObject として実装
impl FileObject for TtyDevice {
    // TTYはseekをサポートしない
    fn seek(&self, _whence: crate::fs::SeekFrom) -> Result<u64, StreamError> {
        Err(StreamError::NotSupported)
    }
    
    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError> {
        Ok(crate::fs::FileMetadata {
            size: 0, // TTYは固定サイズなし
            is_dir: false,
            is_file: false,
            is_char_device: true,
            is_block_device: false,
        })
    }
}

impl StreamOps for TtyDevice {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut input_buffer = self.input_buffer.lock();
        let mut bytes_read = 0;
        
        if self.canonical_mode {
            // カノニカルモード: 改行まで待機
            // TODO: 実際にはwakeup機能が必要
            while bytes_read < buffer.len() {
                if let Some(byte) = input_buffer.pop_front() {
                    buffer[bytes_read] = byte;
                    bytes_read += 1;
                    
                    if byte == b'\n' {
                        break;
                    }
                } else {
                    break;
                }
            }
        } else {
            // RAWモード: 利用可能なバイトを返す
            while bytes_read < buffer.len() {
                if let Some(byte) = input_buffer.pop_front() {
                    buffer[bytes_read] = byte;
                    bytes_read += 1;
                } else {
                    break;
                }
            }
        }
        
        Ok(bytes_read)
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        let device_manager = DeviceManager::get_manager();
        if let Some(borrowed_device) = device_manager.borrow_device(self.uart_device_id).ok() {
            let device = borrowed_device.device();
            let mut device_guard = device.write();
            if let Some(char_device) = device_guard.as_char_device() {
                // 改行コード変換: \n -> \r\n
                let mut bytes_written = 0;
                for &byte in buffer {
                    if byte == b'\n' {
                        char_device.write_byte(b'\r').map_err(|_| StreamError::IoError)?;
                        char_device.write_byte(b'\n').map_err(|_| StreamError::IoError)?;
                    } else {
                        char_device.write_byte(byte).map_err(|_| StreamError::IoError)?;
                    }
                    bytes_written += 1;
                }
                return Ok(bytes_written);
            }
        }
        Err(StreamError::IoError)
    }
}
```

### 4. Initialization and Registration

既存のinitcall機構を使った初期化:

```rust
// kernel/src/device/char/tty.rs への追加

use crate::initcall::driver_initcall;
use alloc::boxed::Box;

/// TTY subsystem の初期化
fn init_tty_subsystem() -> Result<(), &'static str> {
    let device_manager = DeviceManager::get_manager();
    
    // 最初のUARTデバイスを見つける
    if let Some(borrowed_device) = device_manager.borrow_first_device_by_type(crate::device::DeviceType::Char) {
        let device = borrowed_device.device();
        let uart_device_id = {
            let device_guard = device.read();
            device_guard.id()
        };
        drop(borrowed_device);
        
        // TTYデバイスを作成
        let tty_device = Box::new(TtyDevice::new(0, "tty0", uart_device_id));
        let tty_device_arc = Arc::new(*tty_device);
        
        // DeviceManagerに登録
        let tty_id = device_manager.register_device(Box::new(TtyDevice::new(0, "tty0", uart_device_id)));
        
        // UARTデバイスにDeviceEventListenerを登録
        if let Some(borrowed_uart) = device_manager.borrow_device(uart_device_id).ok() {
            let uart_device = borrowed_uart.device();
            let mut uart_guard = uart_device.write();
            if let Some(uart) = uart_guard.as_any_mut().downcast_mut::<crate::drivers::uart::virt::Uart>() {
                let weak_tty = Arc::downgrade(&tty_device_arc);
                uart.register_event_listener(weak_tty);
            }
        }
        
        crate::early_println!("TTY device 'tty0' registered with ID: {}", tty_id);
        
        // TODO: VFS v2に /dev/tty0 として登録
        // これは実際のVFS APIが確定してから実装
        
        Ok(())
    } else {
        Err("No UART device found for TTY")
    }
}

// ドライバ初期化として登録
driver_initcall!(init_tty_subsystem);
```

## Simplified Data Flow (Phase 1)

### Input Flow (入力フロー)
1. **UART Hardware** → UART割り込み発生
2. **uart_interrupt_handler** → バイト読み取り
3. **TTY Callback** → `tty.on_input_received(byte)`
4. **Line Discipline** → カノニカルモード処理
5. **Input Buffer** → バッファに蓄積
6. **Application** → `read()` システムコールで読み取り

### Output Flow (出力フロー)
1. **Application** → `write()` システムコール
2. **TTY Device** → 改行コード変換 (`\n` → `\r\n`)
3. **UART Device** → ハードウェアへ直接送信

### Placeholder Features (未実装機能)

以下の機能はプレースホルダーとして設計に含めますが、実装は将来的に行います：

1. **Signal Processing**: Ctrl+C, Ctrl+Z処理
   - 現在: コメントアウトされたプレースホルダー
   - 将来: プロセス管理システム実装後に追加

2. **Job Control**: プロセスグループ管理
   - 現在: フィールドのみ定義
   - 将来: セッション・プロセスグループ機能実装後

3. **Advanced ioctl**: 端末設定の詳細制御
   - 現在: 基本的なstub実装
   - 将来: 完全なtermios互換

4. **Blocking I/O**: 入力待ちでのブロック
   - 現在: ポーリング方式
   - 将来: Wakerシステム統合後

## Testing and Validation

### Phase 1 テスト項目
1. **基本入出力**: 文字の入力・出力が正常に動作
2. **エコーバック**: 入力文字が画面に表示される
3. **改行処理**: Enter キーで改行が正しく処理される
4. **VFS統合**: `/dev/tty0` としてアクセス可能

### デバッグ用の確認コマンド
```rust
// TTY device が正しく登録されているか確認
let device_manager = DeviceManager::get_manager();
let devices = device_manager.devices.lock();
for (i, device_handle) in devices.iter().enumerate() {
    let device = device_handle.device.read();
    println!("Device {}: {} ({})", i, device.name(), device.device_type());
}
```

## Device Event System Migration

### Current Implementation (Phase 1)
現在の設計では、汎用的な`DeviceEvent`システムを使用しています：

1. **Event Abstraction**: すべてのデバイスイベントは`DeviceEvent`トレイトを実装
2. **Generic Registration**: デバイス固有のコールバックではなく、`DeviceEventListener`を使用
3. **Weak Reference Management**: 自動的な生存期間管理でメモリリークを防止

### Benefits of Generic Event System
- **Extensibility**: 新しいイベントタイプを簡単に追加可能
- **Decoupling**: デバイス間の依存関係を削減
- **Consistency**: 統一されたイベント処理パターン
- **Memory Safety**: Weak参照による自動クリーンアップ

### Future Event Types
```rust
// 将来的に追加予定のイベント
pub struct OutputCompleteEvent;
pub struct ErrorEvent { pub error_code: u32; }
pub struct StateChangeEvent { pub new_state: DeviceState; }
```

This design provides a practical, step-by-step approach to implementing a TTY subsystem in Scarlet OS. The implementation focuses on getting basic functionality working first, with clear placeholders for future enhancements that depend on other kernel subsystems.
