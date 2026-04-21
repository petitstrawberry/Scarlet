# Windows AArch64 ABI 実装ステータス

> 最終更新: 2026-04-21  
> 対象アーキテクチャ: AArch64 (ARM64)  
> 対象 Windows バージョン: Windows 11 Build 26100

## 概要

Scarlet OS はカーネル内に Windows AArch64 ABI エミュレーション層を実装している。  
Windows PE バイナリ（`.exe` / `.dll`）をネイティブの Scarlet プロセスとして実行し、`ntdll.dll` が発行する NT システムコール（`SVC #0x2C`）をインターセプトして処理する。

**現状**: ntdll.dll の初期化プロセスがヒープ作成まで完了するが、exe のロード前に `STATUS_DLL_INIT_FAILED` で終了する。

---

## 1. アーキテクチャ

```
┌─────────────────────────────────────────────────┐
│              ユーザモード (EL0)                    │
│                                                   │
│  ┌─────────┐   ┌───────────┐   ┌──────────────┐  │
│  │ test.exe │   │ ntdll.dll │   │ (kernel32等) │  │
│  └────┬─────┘   └─────┬─────┘   └──────────────┘  │
│       │               │                            │
│       │    SVC #0x2C  │                            │
├───────┼───────────────┼────────────────────────────┤
│       ▼               ▼    カーネルモード (EL1)     │
│  ┌──────────────────────────────────────────────┐ │
│  │        Windows AArch64 ABI Module            │ │
│  │  ┌──────────────┐  ┌───────────────────────┐ │ │
│  │  │ Syscall Table │  │ syscall dispatch      │ │ │
│  │  │ (980 entries) │  │ (match by name)       │ │ │
│  │  └──────────────┘  └───────────┬───────────┘ │ │
│  │                                │              │ │
│  │  ┌──────────────┐  ┌──────────┴──────────┐   │ │
│  │  │ PEB/TEB setup│  │ NT Object Manager   │   │ │
│  │  │ Process Env  │  │ (Handle Table)       │   │ │
│  │  └──────────────┘  └─────────────────────┘   │ │
│  └──────────────────────────────────────────────┘ │
│  ┌──────────────────────────────────────────────┐ │
│  │        PE Loader (共通インフラ)                │ │
│  │  parse_pe_headers / map_sections / reloc     │ │
│  └──────────────────────────────────────────────┘ │
│  ┌──────────────────────────────────────────────┐ │
│  │        Scarlet Kernel 共通基盤                │ │
│  │  VMM / Task / VFS / Memory Manager           │ │
│  └──────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────┘
```

### 実行フロー

1. **バイナリ検出**: PE（MZ ヘッダ）+ ARM64 マジックを検出 → Windows ABI にルーティング
2. **PE ロード**: カーネルが exe をタスクのアドレス空間にマップ
3. **ntdll ロード**: `/System32/ntdll.dll` を固定アドレス `0x180000000` にマップ
4. **環境初期化**: PEB / TEB / ProcessParameters / SharedUserData / CONTEXT を設定
5. **LdrInitializeThunk**: ntdll のエントリポイントにジャンプ
6. **ntdll 初期化**: ヒープ作成、LDR データ構築、レジストリ参照、exe ロード...
7. **SVC #0x2C**: ntdll がシステムコールを発行 → カーネルがディスパッチ

---

## 2. コードベース構成

### ファイル一覧

| ファイル | 行数 | 役割 |
|---------|------|------|
| `kernel/src/abi/windows/aarch64/mod.rs` | 1,794 | ABI メイン実装。システムコールハンドラ、プロセス初期化、ntdll ロード |
| `kernel/src/abi/windows/aarch64/syscall_table.rs` | 3,971 | 自動生成システムコールテーブル（489エントリ×2＝番号順+名前順）。番号→名前のマッピング |
| `kernel/src/abi/windows/peb.rs` | 389 | PEB / TEB / ProcessParameters / CONTEXT / SharedUserData の初期化 |
| `kernel/src/abi/windows/object/mod.rs` | 171 | NT オブジェクトマネージャ（ハンドルテーブル、NtObject 型） |
| `kernel/src/abi/windows/ntdef.rs` | 56 | NT 構造体定義（`#[repr(C)]`） |
| `kernel/src/abi/windows/error.rs` | 117 | NTSTATUS コード定義 |
| `kernel/src/abi/windows/mod.rs` | 14 | モジュールエクスポート |
| `kernel/src/task/pe_loader/mod.rs` | 930 | PE ローダー（ヘッダ解析、セクションマッピング、リロケーション、エクスポート解決） |
| `kernel/src/task/pe_loader/headers.rs` | 548 | PE ヘッダ定数・構造体 |
| **合計** | **~8,000** | |

### メモリレイアウト定数

| 定数 | 値 | 用途 |
|------|-----|------|
| `NTDLL_IMAGE_BASE` | `0x180000000` | ntdll.dll のロードアドレス |
| `WIN_TEB_BASE` | `0x200000000` | TEB（Thread Environment Block）のベースアドレス |
| `WIN_PEB_BASE` | `0x200010000` | PEB（Process Environment Block）のベースアドレス |
| `SHARED_USER_DATA_BASE` | `0x7FFE0000` | KUSER_SHARED_DATA のマップ先 |
| `API_SET_MAP_ADDR` | `0x20060000` | API Set Map のマップ先 |
| ProcessParameters | `0x20040000` | RTL_USER_PROCESS_PARAMETERS の配置先 |

---

## 3. プロセス環境初期化

### 3.1 PEB（Process Environment Block）

`PEB @ 0x200010000`、サイズ 0x300 バイト

| オフセット | フィールド | 設定値 | 備考 |
|-----------|-----------|--------|------|
| +0x08 | MutantHandle | `0xFFFFFFFFFFFFFFFF` | Loader Lock（非NULLである必要あり） |
| +0x10 | ImageBaseAddress | exe のベースアドレス | PE ローダーが決定 |
| +0x18 | Ldr | `0x0` | **意図的に NULL**。ntdll が自ら LDR_DATA を構築する |
| +0x20 | ProcessParameters | `0x20040000` | RTL_USER_PROCESS_PARAMETERS へのポインタ |
| +0x30 | ProcessHeap | 引数依存 | 初期値 0、ntdll が RtlCreateHeap で設定 |
| +0x68 | ApiSetMap | `0x20060000` | API Set Namespace のアドレス |
| +0xB8 | NumberOfProcessors | `1` | |
| +0xC8 | HeapSegmentReserve | `0x100000` (1MB) | ヒープセグメント予約サイズ |
| +0xD0 | HeapSegmentCommit | `0x2000` (8KB) | ヒープセグメントコミットサイズ |
| +0xD8 | HeapDeCommitTotalFreeThreshold | `0x10000` | |
| +0xE0 | HeapDeCommitFreeBlockThreshold | `0x1000` | |
| +0xE8 | NumberOfHeaps | `0` | |
| +0xEC | MaximumNumberOfHeaps | `0x100` | |
| +0xF0 | ProcessHeaps | PEB + 0x300 | ヒープポインタ配列の開始位置 |
| +0x118 | OSMajorVersion | `10` | Windows 10 |
| +0x11C | OSMinorVersion | `0` | |
| +0x120 | OSBuildNumber | `26100` | Windows 11 24H2 |

### 3.2 TEB（Thread Environment Block）

`TEB @ 0x200000000`、サイズ 0x800 バイト

| オフセット | フィールド | 設定値 |
|-----------|-----------|--------|
| +0x08 | StackBase | ユーザスタックの上限 |
| +0x10 | StackLimit | ユーザスタックの下限 |
| +0x30 | Self (TEB ptr) | `0x200000000` |
| +0x40 | ClientId.UniqueProcess | task.get_id() |
| +0x48 | ClientId.UniqueThread | task.get_id() |
| +0x60 | ProcessEnvironmentBlock | PEB アドレス |
| +0x68 | LastErrorValue | `0` |

### 3.3 RTL_USER_PROCESS_PARAMETERS

`ProcParams @ 0x20040000`、1 ページ

| オフセット | フィールド | 設定値 |
|-----------|-----------|--------|
| +0x00 | MaximumLength | PAGE_SIZE |
| +0x04 | Length | PAGE_SIZE |
| +0x08 | Flags | `0x6001` |
| +0x10 | ConsoleHandle | `NULL` |
| +0x20 | StandardInput | `0xFFFFFFFFFFFFFFFF` |
| +0x28 | StandardOutput | `0xFFFFFFFFFFFFFFFF` |
| +0x30 | StandardError | `0xFFFFFFFFFFFFFFFF` |
| +0x38 | CurrentDirectory | `C:\` (UTF-16LE) |
| +0x50 | DllPath | `C:\Windows\System32` (UTF-16LE) |
| +0x60 | ImagePathName | `C:\test_exit.exe` (UTF-16LE) |
| +0x70 | CommandLine | `test_exit.exe` (UTF-16LE) |
| +0x80 | Environment | 空の環境ブロック |

### 3.4 SharedUserData（KUSER_SHARED_DATA）

`SharedUserData @ 0x7FFE0000`、1 ページ

| オフセット | フィールド | 設定値 |
|-----------|-----------|--------|
| +0x0260 | NtBuildNumber | `26100` |
| +0x026C | NtMajorVersion | `10` |
| +0x0270 | NtMinorVersion | `0` |
| +0x0330 | Cookie | `0xCAFEBABEDEADBEEF`（ポインタエンコード用） |

### 3.5 CONTEXT レコード

AArch64 用の `NtdllContext` 構造体（`#[repr(C)]`）、サイズ 0x390 バイト:

```
+0x000: context_flags (u32) = CONTEXT_FULL
+0x004: cpsr (u32) = 0
+0x008: x[0..30] (u64×31)  // x[0] = exe entry point
+0x100: sp (u64) = initial stack pointer
+0x108: pc (u64) = exe entry point
```

### 3.6 初期レジスタ設定

LdrInitializeThunk にジャンプ時:

| レジスタ | 値 | 意味 |
|---------|-----|------|
| ELR (PC) | LdrInitializeThunk | ntdll の初期化ルーチン |
| SP | CONTEXT アドレス | コンテキストレコードのスタック上の位置 |
| X0 | CONTEXT アドレス | 第1引数 |
| X1 | ntdll.image_base | ntdll のベースアドレス |
| X18 (TPIDR_EL0) | TEB アドレス | Windows ARM64 では x18 = TEB |

---

## 4. PE ローダー

カーネル共通インフラとして実装（`kernel/src/task/pe_loader/`）。

### 対応機能

| 機能 | ステータス | 詳細 |
|------|----------|------|
| PE ヘッダ解析 | ✅ 完了 | DOS/PE シグネチャ検証、Optional Header 解析 |
| ARM64 対応 | ✅ 完了 | Machine type `0xAA64` を認識 |
| PE32+ (64-bit) | ✅ 完了 | `PE32PLUS_MAGIC` 対応 |
| セクションマッピング | ✅ 完了 | 各セクションの VA/RawSize/VirtualSize に基づくマッピング |
| ヘッダマッピング | ✅ 完了 | ヘッダページをイメージベースに Read-Only でマップ |
| ベースリロケーション | ✅ 完了 | ARM64 リロケーション型（ADDR64, ADDR32NB, PAGEBASE_REL21, PAGEOFFSET_12A/L） |
| エクスポート解決（名前） | ✅ 完了 | `find_export_by_name()` |
| エクスポート解決（序数） | ✅ 完了 | `find_ordinal_only_export()` |
| リソース | ❌ 未実装 | |
| デバッグ情報 | ❌ 未実装 | |
| TLS | ❌ 未実装 | |
| 動的リンク（インポート解決） | ❌ 未実装 | カーネル側では実装せず、ntdll が担当 |

---

## 5. NT オブジェクトマネージャ

### ハンドルテーブル

- 実装: `BTreeMap<u32, NtObject>`
- ハンドル開始: 4、4 ずつ増加（NT 慣例）
- 疑似ハンドル: STD_INPUT_HANDLE (`-4`), STD_OUTPUT_HANDLE (`-9`), STD_ERROR_HANDLE (`-11`)

### オブジェクト型

| 型 | ステータス | 用途 |
|----|----------|------|
| `NtFileObject` | ✅ 実装済み | ファイル I/O。Scarlet の FileObject をラップ |
| `NtSectionObject` | ✅ 実装済み | メモリマップトセクション。ファイルバックまたは匿名 |
| `NtProcessObject` | 🟡 スタブ | PID のみ保持 |
| `NtThreadObject` | 🟡 スタブ | TID のみ保持 |
| `NtEventObject` | 🟡 スタブ | 空の構造体 |
| `NtTimerObject` | 🟡 スタブ | 空の構造体 |
| `NtMutantObject` | 🟡 スタブ | ミューテックス |
| `NtSemaphoreObject` | 🟡 スタブ | セマフォ |
| `NtKeyedMutexObject` | 🟡 スタブ | |
| `NtIoCompletionObject` | 🟡 スタブ | I/O 完了ポート |
| `NtDebugObject` | 🟡 スタブ | デバッグオブジェクト |

---

## 6. システムコール実装

### システムコールテーブル

- **総エントリ数**: 489（ユニークな NT システムコール）
- **ハンドラ実装数**: 44 個のシステムコールに固有ハンドラあり（カバレッジ ≈ 9%）
- **未実装**: 残り約 445 個は `_ => STATUS_NOT_IMPLEMENTED` にフォールバック

### 実装済みシステムコール一覧

#### メモリ管理

| Syscall | 番号 | ステータス | 詳細 |
|---------|------|----------|------|
| `NtAllocateVirtualMemory` | 0x0018 | ✅ 実装 | MEM_COMMIT/MEM_RESERVE 対応。ページ割り当て、ベース/サイズ書き戻し |
| `NtAllocateVirtualMemoryEx` | 0x0078 | ✅ 実装 | 拡張パラメータ（MEM_EXTENDED_PARAMETER）のログ出力あり。MEM_REPLACE_PLACEHOLDER 処理。ページ保護は未実装 |
| `NtFreeVirtualMemory` | 0x001B | ✅ 実装 | ページ解放 |
| `NtProtectVirtualMemory` | 0x0050 | ✅ 実装 | ページ保護変更（ログ出力のみ、実際の保護変更は未実装） |
| `NtQueryVirtualMemory` | 0x0023 | ✅ 実装 | メモリ情報クエリ。Section/Base/Size 情報を返す |
| `NtMapViewOfSection` | 0x0028 | ✅ 実装 | セクションのマップ。ファイルバックおよび匿名セクション対応 |
| `NtUnmapViewOfSection` | 0x0029 | ✅ 実装 | セクションのアンマップ |
| `NtCreateSection` | 0x0030 | ✅ 実装 | セクションオブジェクト作成 |
| `NtOpenSection` | 0x0031 | ✅ 実装 | セクションオブジェクトを名前でオープン |
| `NtQuerySection` | 0x0032 | ✅ 実装 | セクション情報クエリ |
| `NtWriteVirtualMemory` | 0x003A | ✅ 実装 | 他プロセスのメモリに書き込み |

#### ファイル I/O

| Syscall | 番号 | ステータス | 詳細 |
|---------|------|----------|------|
| `NtCreateFile` | 0x0055 | ✅ 実装 | NT パス → Scarlet VFS パス変換。ファイル作成/オープン |
| `NtReadFile` | 0x0006 | ✅ 実装 | ファイル読み出し。非同期 I/O は未対応 |
| `NtWriteFile` | 0x0007 | ✅ 実装 | ファイル書き込み |
| `NtClose` | 0x000F | ✅ 実装 | ハンドルクローズ。オブジェクトテーブルから削除 |

#### プロセス・スレッド

| Syscall | 番号 | ステータス | 詳細 |
|---------|------|----------|------|
| `NtTerminateProcess` | 0x002C | ✅ 実装 | プロセス終了 |
| `NtQueryInformationProcess` | 0x0019 | 🟡 部分 | ProcessBasicInformation, ProcessCookie (class=36) のみ対応 |
| `NtContinue` | 0x004C | ✅ 実装 | コンテキストレコードからレジスタを復元して再開 |

#### システム情報

| Syscall | 番号 | ステータス | 詳細 |
|---------|------|----------|------|
| `NtQuerySystemInformation` | 0x0036 | 🟡 部分 | class 0 (Basic), 3 (TimeOfDay), 5 (空), 62 (CodeIntegrity) のみ対応 |
| `NtQuerySystemInformationEx` | 0x016E | 🟡 部分 | class 107 のみ対応 |
| `NtQuerySystemTime` | 0x0058 | ✅ 実装 | 現在時刻を FILETIME で返す |
| `NtQueryPerformanceCounter` | 0x0031 | ✅ 実装 | パフォーマンスカウンタ値を返す |

#### レジストリ

| Syscall | 番号 | ステータス | 詳細 |
|---------|------|----------|------|
| `NtOpenKey` | 0x0012 | 🟡 スタブ | ダミーハンドルを返す |
| `NtQueryValueKey` | 0x0017 | 🟡 スタブ | 常に `STATUS_OBJECT_NAME_NOT_FOUND` を返す |

#### 同期・その他

| Syscall | 番号 | ステータス | 詳細 |
|---------|------|----------|------|
| `NtCreateEvent` | 0x0048 | 🟡 スタブ | ダミーオブジェクトを作成してハンドル返す |
| `NtTraceEvent` | 0x005E | 🟡 スタブ | 常に `STATUS_SUCCESS` |
| `NtRaiseHardError` | 0x0175 | ✅ 実装 | エラーダイアログ要求を処理（ログ出力＋応答設定） |
| `NtManageHotPatch` | 0x011A | 🟡 スタブ | 常に `STATUS_NOT_IMPLEMENTED` |

### 「成功を返すだけ」のスタブ群

以下のシステムコールは個別ハンドラを持たず、マッチリストで一括して `STATUS_SUCCESS` を返す:

```
NtWorkerFactoryWorkerReady, NtAcceptConnectPort, NtRemoveIoCompletion,
NtQueryObject, NtQueryInformationFile, NtReleaseMutant, NtOpenProcess,
NtAccessCheckAndAuditAlarm, NtQueryDirectoryFile, NtQueryAttributesFile,
NtClearEvent, NtReadVirtualMemory, NtOpenEvent, NtQueryEvent,
NtDelayExecution, NtWaitForMultipleObjects
```

### NtQuerySystemInformation 対応クラス

| Class | 名前 | ステータス |
|-------|------|----------|
| 0x00 | SystemBasicInformation | ✅ `#[repr(C)]` 構造体で実装 |
| 0x03 | SystemTimeOfDayInformation | ✅ `#[repr(C)]` 構造体で実装 |
| 0x05 | （不明） | ✅ 空のハンドラ（成功を返すだけ） |
| 0x07 | SystemProcessorFeaturesInformation | ✅ `#[repr(C)]` 構造体定義済み |
| 0x3E (62) | SystemCodeIntegrityInformation | ✅ `#[repr(C)]` 構造体で実装 |
| 0xC5 (197) | （SystemSecureSpeculationControl 疑い） | ❌ `STATUS_NOT_IMPLEMENTED` |
| 0x37 (55) | （不明） | ❌ `STATUS_NOT_IMPLEMENTED` |
| 0x6B (107) | NtQuerySystemInformationEx のみ対応 | ✅ 成功を返す |

### NTSTATUS コード定義

`error.rs` に 100+ の NTSTATUS コードが定義済み。ヘルパー関数:

```rust
pub fn failed(status: u32) -> bool  // bit 31 set → failure
pub fn success(status: u32) -> bool // bit 31 clear → success
```

主要コード:

| コード | 名前 | 用途 |
|--------|------|------|
| 0x00000000 | STATUS_SUCCESS | 成功 |
| 0xC0000002 | STATUS_NOT_IMPLEMENTED | 未実装のシステムコール |
| 0xC000000D | STATUS_INVALID_PARAMETER | 不正パラメータ |
| 0xC0000034 | STATUS_OBJECT_NAME_NOT_FOUND | レジストリキーなし |
| 0xC0000017 | STATUS_NO_MEMORY | メモリ不足 |
| 0xC0000145 | STATUS_DLL_INIT_FAILED | DLL 初期化失敗 |

---

## 7. 実行実績

### テストバイナリ

`test_exit.exe` — 最小限の Windows ARM64 PE バイナリ。`NtTerminateProcess` のみを呼び出す。

### 最新の実行トレース（105 システムコール）

```
 #  Syscall                                    戻り値
 ──────────────────────────────────────────────────────
 1  NtQueryPerformanceCounter                  OK
 2  NtProtectVirtualMemory                     OK
 3  NtProtectVirtualMemory                     OK
 4  NtCreateEvent                              OK
 5  NtManageHotPatch                           ERR (NOT_IMPLEMENTED)
 6  NtCreateEvent                              OK
 7  NtQuerySystemInformation class=0           OK
 8  NtQueryInformationProcess                  OK
 9  NtOpenKey → NtQueryValueKey → NtClose      ERR (NOT_FOUND) ×3
10  NtQueryVirtualMemory                       OK
11  NtQueryVirtualMemory                       OK
12  NtProtectVirtualMemory                     OK
13  NtOpenKey → NtQueryValueKey → NtClose      ERR (NOT_FOUND)
14  NtOpenKey → NtQueryValueKey → NtClose      ERR (NOT_FOUND)
15  NtOpenKey → NtOpenKey → NtQueryValueKey×2  ERR×2
16  NtClose                                    OK
17  NtQueryInformationProcess                  OK
18  NtQuerySystemInformation class=0           OK
19  NtQuerySystemInformation class=62          OK
20-23  NtAllocateVirtualMemoryEx ×4            OK    ← ヒープ領域確保
24  NtQuerySystemInformationEx class=107       OK
25  NtOpenKey → NtQueryValueKey → NtClose      ERR (NOT_FOUND)
26-30  NtAllocateVirtualMemoryEx ×5            OK    ← ヒープ拡張
31  NtOpenKey → NtQueryValueKey → NtClose      ERR (NOT_FOUND)
32-35  NtAllocateVirtualMemoryEx ×4            OK    ← ヒープ拡張
36  NtQuerySystemInformation class=197         ERR (NOT_IMPLEMENTED) ★
37  NtQuerySystemInformation class=55          ERR (NOT_IMPLEMENTED) ★
38  NtTraceEvent                               OK
39  NtRaiseHardError (0xC0000145)              OK    ← STATUS_DLL_INIT_FAILED
40  NtTerminateProcess (0xC0000002)            終了
```

### 統計

| メトリック | 値 |
|-----------|-----|
| 総システムコール数 | 105 |
| 成功 (STATUS_SUCCESS) | 47 |
| レジストリ NotFound | 9 |
| 未実装 (NOT_IMPLEMENTED) | 1 (NtManageHotPatch) |
| 未対応 Info Class | 2 (class 197, 55) |
| NtAllocateVM 呼び出し | 13 回（ヒープ作成 + 拡張） |
| ProcessHeap 設定 | ✅ `0x40000000` に設定された |
| SEGMENT_HEAP 署名 | ✅ `0xDDEEDDEE` 確認 |
| Ldr (PEB_LDR_DATA) | ✅ ntdll が自ら構築 (`0x18038f160`) |

### ntdll が実行した初期化ステップ

| ステップ | ステータス | 備考 |
|---------|----------|------|
| 初期コンテキスト復元 | ✅ | LdrInitializeThunk から開始 |
| LDR データ構築 | ✅ | PEB->Ldr = NULL → ntdll が `0x18038f160` に構築 |
| ヒープ作成 | ✅ | RtlCreateHeap 完了、`0x40000000` に SEGMENT_HEAP |
| レジストリ参照 | ✅ | NtOpenKey/NtQueryValueKey で設定確認（すべて NOT_FOUND） |
| ETW トレース初期化 | ✅ | NtTraceEvent に成功 |
| **exe ロード** | ❌ | NtCreateFile/NtOpenSection が一度も呼ばれない |
| **DLL ロード** | ❌ | kernel32.dll 等のロード試行なし |
| **プロセス実行** | ❌ | NtRaiseHardError(STATUS_DLL_INIT_FAILED) で終了 |

---

## 8. 現在の問題

### BLOCKER: STATUS_DLL_INIT_FAILED（0xC0000145）

ntdll はヒープ初期化を完了した後、exe も DLL もロードしようとせずに `NtRaiseHardError(STATUS_DLL_INIT_FAILED)` を呼ぶ。

**観察される症状**:
- NtCreateFile / NtOpenSection / NtCreateSection が一度も呼ばれない
- ntdll はレジストリ参照 → ヒープ作成 → ETW → NtRaiseHardError の順で進行
- `NtQuerySystemInformation` class 197 と class 55 が `STATUS_NOT_IMPLEMENTED` を返している

**調査中の仮説**:

1. **NtQuerySystemInformation class=197/55 の NOT_IMPLEMENTED**: ntdll がこれらのクエリの失敗を致命的エラーと判断している可能性
2. **PEB の設定不備**: ImageBaseAddress や ProcessParameters の内容に問題がある可能性
3. **LdrSystemDllInitBlock の未設定フィールド**: CFG 関数ポインタや緩和策フラグがゼロであることが原因の可能性
4. **実行可能メモリ保護**: NtProtectVirtualMemory が実際にページ保護を変更していない

### 既知の制限事項

| 項目 | 詳細 |
|------|------|
| ページ保護の未実装 | NtProtectVirtualMemory はログ出力のみ。実際のページ権限変更なし |
| NtAllocateVM の AllocationType 無視 | 非 Ex 版が `MEM_COMMIT|MEM_RESERVE` にハードコード |
| NtAllocateVM の PageProtection 無視 | 保護フラグ（args[4]/args[5]）を無視して常に RW+User で割り当て |
| レジストリの完全スタブ | NtQueryValueKey が常に NOT_FOUND。一部のレジストリ値は初期化に必要かも |
| NtCreateFile のパス解決 | NT パス（`\??\C:\...`）→ Scarlet VFS パスの変換が不完全 |
| 同期プリミティブ未実装 | Event / Mutex / Semaphore はスタブ。待機操作なし |
| NtContinue のコンテキスト復元 | CONTEXT_ARM64 レジスタのみ復元。浮動小数点レジスタなし |

---

## 9. 解決済みの問題

### ✅ SystemBasicInformation 構造体レイアウト（解決）

**問題**: `NtQuerySystemInformation class=0` が生バイトオフセットで値を書き込み、フィールド配置が完全に間違っていた。ntdll が7個のシステムコールでクラッシュしていた。

**解決**: `#[repr(C)]` 構造体 `SystemBasicInformation` を定義し、コンパイル時サイズアサーションを追加。システムコール数が 7 → 213（旧計測）/ 105（現行）に増加。

### ✅ PEB->Ldr = NULL の方針（解決）

**問題**: カーネルが LDR データを事前構築すべきか？

**解決**: `PEB->Ldr = 0` で開始し、ntdll に自ら構築させる。これは Windows の実際の動作に合致する。ntdll は `LdrInitializeThunk` 内で `PEB_LDR_DATA` を作成・リンクする。

### ✅ ヒープ初期化失敗（解決）

**問題**: ヒープメモリ `0x40000000` が全ゼロのまま。SEGMENT_HEAP 署名 `0xDDEEDDEE` が書き込まれない。`PEB->ProcessHeap = 0` で `ldr w8, [x25, #0x10]` が FAR=0x10 でクラッシュ。

**解決**: 実はヒープ初期化は正常に完了していた。以前のトレースは情報不足で誤診断していた。最新の実行で `[+0x10]=0xddeeddee` と `ProcessHeap=0x40000000` を確認。

### ✅ PE ローダーのコード破壊疑い（誤診断・解決済み）

**問題**: ntdll の `.text` セクションの命令が実行時とファイルで異なると判断。

**解決**: RVA をファイルオフセットとして使うという逆アセンブルのバグ。`.text` セクションの `VirtAddr=0x1000, RawPtr=0x600` なので正しいファイルオフセットは `RVA - 0x1000 + 0x600 = RVA - 0xA00`。修正後、実行時命令とファイルは完全に一致。

---

## 10. 今後のロードマップ

### Phase 1: test_exit.exe の実行（現在のブロッカー）

1. **STATUS_DLL_INIT_FAILED の原因特定**
   - NtQuerySystemInformation class 197/55 の実装
   - ntdll の RVA 0x15C4A0（NtRaiseHardError 呼び出し元）の逆アセンブル
   - PEB->ImageBaseAddress と exe のマッピング確認
2. **NtAllocateVM の PageProtection 実装**
3. **NtProtectVirtualMemory の実際の保護変更**

### Phase 2: 基本的な Win32 実行

1. **kernel32.dll のロード** — ntdll がロードできるようになったら
2. **インポート解決** — IAT の修补
3. **レジストリの最小実装** — 必要なキー/値の提供
4. **同期プリミティブ** — Event / Mutex の実装

### Phase 3: 実用的なアプリケーション

1. **コンソール I/O** — stdin/stdout の適切なパイプ接続
2. **ファイルシステムマッピング** — Windows パス ↔ Scarlet VFS の完全な変換
3. **ネットワーク** — Winsock のスタブ
4. **マルチスレッド** — NtCreateThread / TLS の実装

---

## 11. 開発方針

- **ユーザランドの役割はユーザランドに**: カーネルは ntdll がやるべきことを代行しない（ヒープ作成、LDR 構築など）
- **正しい実装を優先**: ワークアラウンドより正しい実装。スタブは OK だが TODO コメントを残す
- **構造体は `#[repr(C)]`**: 生バイトオフセットでの書き込みは禁止
- **ReactOS 属性の排除**: 逆コンパイル結果から構造体を作る際、ReactOS の著作権表示は含めない
- **デバッグログの維持**: デバッグが完了するまでログを削除しない
