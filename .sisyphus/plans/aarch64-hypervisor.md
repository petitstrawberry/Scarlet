# AArch64 Hypervisor Implementation Plan (SHV/USHV/KVM)

## Design Decisions

- **VHE優先**: ホストカーネルはEL2で動作（HCR_EL2.E2H=1, TGE=1）
- **EL1 graceful fallback**: EL1のみ環境ではHV機能を無効化
- **GICv3ハイブリッド**: カーネルはICH_LRで割り込み注入、GIC MMIOはユーザー空間
- **QEMU要件**: `-cpu max -machine virt,virtualization=on,gic-version=3`
- **最初のターゲット**: ベアメタルhello_worldゲスト

## Critical Bug: mmu.rs Stage-2 Descriptor Format

`kernel/src/arch/aarch64/hv/mmu.rs` の `map_stage2_page_new` が
**RISC-V PTEフォーマット**を使用している。AArch64 Stage-2記述子形式で
全面的に書き直しが必要。

- RISC-V: Valid(bit0), R(bit1), W(bit2), X(bit3), U(bit4), A(bit6), PPN<<10
- AArch64 Stage-2: Valid(bit0), Page(bit1), MemAttr[2:0](bits4-2),
  S2AP[1:0](bits7-6), SH[1:0](bits9-8), AF(bit10), OutputAddr[47:12]

`walk_stage2`の中間テーブル記述子も`.validate()`(bit0のみ)ではなく
bits[1:0]=0b11（テーブル記述子）が必要。

## Implementation Phases

### Phase 1: VHE Boot Contract
**Goal**: カーネルをEL2でブートし、VHE有効化、既存EL1コードがそのまま動くことを確認

**Files to modify**:
- `kernel/src/arch/aarch64/boot/limine.rs` — EL2検出、VHE有効化、EL1降下
- `bsp/aarch64-limine/tools/run_aarch64.sh` — QEMU設定変更

**Details**:
1. `limine_entry()`の先頭でCurrentEL確認
2. EL2の場合:
   - HCR_EL2 = E2H | TGE (VHE有効化)
   - SCTLR_EL1 = 0 (EL1クリーンアップ)
   - SPSR_EL2 = EL1h + SPSR_EL2.DAIFマスク
   - ELR_EL2 = 次の命令アドレス
   - ERET → EL1に降下（VHEによりEL1レジスタがEL2エイリアスにリダイレクト）
3. EL1の場合: そのまま続行（HV無効フラグ設定）
4. QEMU: `-cpu max` + `virtualization=on` 追加

**Acceptance**: `cargo make run-aarch64` で "Current EL: EL1" 表示後、
既存カーネルが正常動作。`-cpu max`でVHE有効動作確認。

### Phase 2: EL2 Vector and Minimal Hypervisor Init
**Goal**: EL2例外ベクタのインストール、HV初期化インフラ

**Files to modify**:
- `kernel/src/arch/aarch64/hv/mod.rs` — arch_init_hv()の実装
- `kernel/src/arch/aarch64/hv/sysreg.rs` — VHE対応システムレジスタsave/restore
- `kernel/src/arch/aarch64/trap/` — EL2例外ベクタ追加

**Details**:
1. VBAR_EL2設定（EL2例外ベクタテーブル）
2. VHEコンテキスト: CNTHCTL_EL2, CPTR_EL2, VTCR_EL2 の初期設定
3. init_hv_per_cpu() でper-CPU EL2ステート初期化
4. HCR_EL2トラップ設定（初期は最小: WFI trap等）

**Acceptance**: arch_init_hv()が例外を出さず完了。EL2ベクタが正しくインストール。

### Phase 3: Stage-2 MMU (Clean Rewrite)
**Goal**: AArch64 Stage-2ページテーブルの正しい実装

**Files to modify**:
- `kernel/src/arch/aarch64/hv/mmu.rs` — 全面書き直し

**Details**:
1. AArch64 Stage-2記述子型を定義（RISC-V PTE型を廃止）
   - S2_PAGE_VALID, S2_PAGE_DESCRIPTOR, S2_TABLE_DESCRIPTOR
   - S2_MEMATTR_SHIFT, S2_S2AP_SHIFT, S2_SH_SHIFT, S2_AF
   - S2_ADDR_SHIFT = 12
2. `walk_stage2` — 中間記述子にbits[1:0]=0b11を使用
3. `map_stage2_page_new` → `map_stage2_page` AArch64形式で書き直し
4. `set_guest_root_stage2` — VTTBR_EL2書き込み + TLBI
5. `verify_hgatp_stage2` — VTTBR_EL2読み取り検証
6. VTCR_EL2設定（IPA size = 40bit等）
7. Stage-2 TLBIヘルパー (tlbi vmalle1is相当)

**Acceptance**: Stage-2ページマップがARMマニュアル通りのビットレイアウト。
ユニットテストで記述子ビット位置を検証。

### Phase 4: Guest VCPU and VM Object
**Goal**: Aarch64VmObjectとGuestVcpuの完全実装

**Files to modify**:
- `kernel/src/arch/aarch64/hv/vm.rs` — VmObject実装
- `kernel/src/arch/aarch64/hv/guest_vcpu.rs` — GuestVcpuのレジスタ操作
- `kernel/src/arch/aarch64/hv/reg_index.rs` — 必要に応じてPSTATE等追加

**Details**:
1. Aarch64VmObject::new() — owner_mm保存、VMID割り当て、Stage-2初期化
2. create_vcpu() — Aarch64VcpuObject生成
3. set_memory_region() — MemorySlotManagerにスロット登録
4. owner_mm() — 保持しているVirtualMemoryManagerを返す
5. ControlOps実装 — SET_MEMORY_REGION, GET_VCPU_COUNT, SET_FAST_PATH
6. Aarch64VcpuObject — VcpuObject trait実装
   - get_reg/set_reg — GuestVcpuに委譲
   - inject_interrupt/clear_interrupt
   - run() — Phase 5で実装

**Acceptance**: VM作成→メモリリージョン設定→vCPU作成のフローが
例外なく完了。cargo testが通る。

### Phase 5: World Switch (VHE)
**Goal**: VHEモードでのゲストEntry/Exit

**Files to modify**:
- `kernel/src/arch/aarch64/hv/switch.rs` — arch_run_guest_loop, arch_guest_trap_exit

**Details**:
VHEのワールドスイッチはnon-VHEよりシンプル:
1. ホストは既にEL2にいる
2. ゲストEntry:
   - ホストEL1ステート保存（sysreg.save()）
   - HCR_EL2変更（E2H=0にクリア、VMビット設定）
   - VTTBR_EL2にゲストStage-2 root設定
   - ゲストEL1レジスタロード
   - ERET → EL1ゲストに突入
3. ゲストExit:
   - EL2例外ベクタで捕捉
   - ゲストステート保存
   - ホストEL1ステート復元
   - HCR_EL2復元（E2H=1, TGE=1でVHEに戻る）
   - VTTBR_EL2クリア
4. アセンブリ/インラインアセンブリで実装

**Acceptance**: ベアメタルゲストがERETで起動し、HVCでVmExitを返す。

### Phase 6: Trap Handling
**Goal**: ゲストトラップのESR_EL2解析→VmExit変換

**Files to modify**:
- `kernel/src/arch/aarch64/hv/trap.rs` — arch_guest_trap_handler

**Details**:
1. ESR_EL2読み取り → EC(Exception Class)デコード
2. トラップタイプ別処理:
   - ESR_EC_SMC64 → VmExit::FirmwareCall
   - ESR_EC_HVC64 → VmExit::FirmwareCall
   - ESR_EC_DABT_LOW → Stage-2アボート:
     - HPFAR_EL2 + FAR_EL2 でIPA復元
     - メモリスロット検索 → RAMマップ or MMIO exit
     - MMIO: VmExit::MmioRead/MmioWrite
   - ESR_EC_IABT_LOW → 命令アボート
   - WFI → VmExit::Hlt
   - ESR_EC_SYS64 → システムレジスタトラップ
3. is_from_guest() — VHEモードでは常にtrue（HVコンテキスト内）
4. clear_guest_mode()

**Acceptance**: ゲストのHVC#0がVmExit::FirmwareCall、
MMIOアクセスがVmExit::MmioRead/Writeを返す。

### Phase 7: VcpuObject::run() Integration
**Goal**: VcpuObject::run()の完全なランループ実装

**Files to modify**:
- `kernel/src/arch/aarch64/hv/vm.rs` — run()メソッド

**Details**:
1. RISC-VのRiscv64VcpuObject::run()と同じ構造:
   - sync_interrupts() + inject_pending_interrupts()
   - setup_for_guest() — VTTBR_EL2設定、HCR_EL2設定
   - arch_run_guest_loop() — ゲスト突入
   - ループ: save guest → trap handler → exit or re-enter
   - prepare_normal_task_and_save_guest() — ホストコンテキスト復元
2. VHE固有の最適化（後で追加可能）

**Acceptance**: sys_shv_vcpu_run syscallがユーザー空間に
VcpuExit構造体を正しく書き込む。

### Phase 8: Virtual Timer
**Goal**: ゲスト仮想タイマーサポート

**Files to modify**:
- `kernel/src/arch/aarch64/hv/trap.rs` — タイマートラップ処理
- `kernel/src/arch/aarch64/hv/sysreg.rs` — タイマーレジスタ

**Details**:
1. CNTHCTL_EL2設定（ゲストのEL0/EL1タイマーアクセス許可）
2. ゲストタイマー書き込みのトラップ/エミュレート
3. タイマー割り込み注入

### Phase 9: VGICv3 Hybrid (Interrupt Injection)
**Goal**: ICH_LRレジスタ経由の仮想割り込み注入

**Files to modify**:
- `kernel/src/arch/aarch64/hv/vm.rs` — inject/clear_interrupt
- 新規: `kernel/src/arch/aarch64/hv/vgic.rs` — VGICヘルパー

**Details**:
1. ICH_HCR_EL2, ICH_VMCR_EL2初期設定
2. ICH_LRn_EL2で割り込み注入（List Register管理）
3. メンテナンス割り込み処理
4. GIC MMIOアクセスはVmExit::MmioRead/Writeでユーザー空間へ

### Phase 10: KVM ABI AArch64
**Goal**: Linux KVM互換レイヤーのAArch64対応

**Files to modify**:
- `kernel/src/abi/linux/device/kvm/aarch64.rs` — レジスタ変換

### Phase 11: U-SHV AArch64 Port
**Goal**: ユーザー空間VMMのAArch64対応

**Files to create**:
- `user/bin/src/ushv/aarch64/mod.rs`
- `user/bin/src/ushv/aarch64/firmware/psci.rs`
- `user/bin/src/ushv/aarch64/timer.rs`
- `user/bin/src/ushv/devices/gic.rs`

**Files to modify**:
- `user/bin/src/ushv/main.rs` — cfg gate追加
- `user/bin/src/ushv/machine/dtb.rs` — ARM対応DTB生成

## Dependency Graph

```
Phase 1 (VHE Boot)
  ↓
Phase 2 (EL2 Vector/Init) ← Phase 1必須
  ↓
Phase 3 (Stage-2 MMU) ← 独立可能だがPhase 2と協調
  ↓
Phase 4 (VM/VCPU Objects) ← Phase 3必須
  ↓
Phase 5 (World Switch) ← Phase 2+4必須
  ↓
Phase 6 (Trap Handling) ← Phase 5必須
  ↓
Phase 7 (run() Integration) ← Phase 5+6必須
  ↓
Phase 8 (Timer) ← Phase 7必須
  ↓
Phase 9 (VGIC) ← Phase 7必須、Phase 8と並行可
  ↓
Phase 10 (KVM ABI) ← Phase 7必須
Phase 11 (U-SHV) ← Phase 9+10必須
```

## Parallelizable Work

- Phase 3 (Stage-2 MMU) と Phase 2 (EL2 Vector) は一部並行可能
- Phase 8 (Timer) と Phase 9 (VGIC) は並行可能
- Phase 10 (KVM ABI) と Phase 11 (U-SHV) はPhase 7以降並行可能
