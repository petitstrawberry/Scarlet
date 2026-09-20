# AArch64 outline atomics 引き継ぎ

更新日: 2026-09-20。**この作業は別セッションで実装する。現在のセッションはSwitchのMMC/SD実機確認を担当する。**

## ユーザーが依頼したこと

Linux式のoutline atomicsに対応する。カーネルにCPU機能のprobeとユーザーランドへの通知を設け、Rustの機能検出・起動初期化へ接続する。LSE対応CPUはLSE、SwitchのCortex-A57はLL/SCを使い、一つの汎用ユーザーランドと標準ライブラリで動かす。

この文書を最初に渡した時点では実装未着手。調査後に読んだファイルはあるが、その時点のCPU機能・auxv・Rust target/runtimeへの変更はゼロ。以前追加しかけたNixの非LSE版std差し替えは全撤回し、ビルドも停止済み。別セッション開始後の進捗は、この調査時点の記述とは区別すること。

## 責任分離と禁止事項

- Rust fork: ターゲットの最低ISA、outline helper、実行時機能検出、起動時初期化。
- Scarlet kernel: 各CPUの機能検出、ユーザーランドが安全に使える共通機能の確定、ABI経由の通知。
- cargo-scarlet: 必要なら明示的なユーザーランド用コンパイル設定を全パッケージへ伝える。カーネル用ABI・リンカ設定とは分離。
- Nix: Rust側の定義に沿った通常のツールチェーンのビルドと配布。
- 機種プロジェクト: 正式な設定を選択する。スクリプトによるCargo設定生成、全アプリへのstd再ビルド強制、別sysroot生成、AAC無効化、生成した特殊bundleで問題を隠す対応は禁止。

「LSEを必須とする既定値を維持する」はユーザーの要求ではなく、前の担当が勝手に置いた前提だった。これを再び前提にしない。Linux等の一次資料を確認し、GitHubアクセスにはghを使う。既存のMMC変更を巻き戻さない。専用force_linkや無関係なSMP修正を追加しない。

## 関連リポジトリ

共通親ディレクトリ: `/Users/petitstrawberry/Development/Rust`

| リポジトリ | 調査時点 |
|---|---|
| Scarlet | HEAD `3f3d8a8b391fb8eec96c0c0d6de76972a7b6e34e`、MMC関連の未コミット変更あり |
| rust-scarlet | HEAD `b8e2ec959f5348b18e02e7d5d8955ee634f37223` |
| scarlet-rust-nix | Rust revisionは上記b8e2ec。今回の変更なし |
| scarlet-sdk | ローカルHEAD `bdf8620544f8405124149ce073c6f786bc030474`。Switchの実使用pinは下記 |
| scarlet-project-switch | SDK pin `71c7d9e0510989c23ead340883b30f2b4191521e`。MMC関連変更あり |
| scarlet-project-chromebook | 既存の別作業のdirty filesあり。変更・復元しない |

使用コンパイラ: rustc / cargo 1.94.0-nightly、LLVM 21.1.8、host aarch64-apple-darwin。

Rust forkには既存の未追跡 `.github/workflows/notify-scarlet-rust-nix.yml` がある。Nix/SDKには既存の未追跡 `.DS_Store` がある。担当変更と誤認して消さない。

## Linux/Rustの方式と確認した根拠

Linux GNU/musl向けRustの既定featuresは `+v8a,+outline-atomics`。汎用bare-metalのupstreamは `+v8a,+strict-align,+neon`。どちらもLSE必須ではない。

LinuxはELF補助情報の `AT_HWCAP` に `HWCAP_ATOMICS` を載せる。ユーザー側はこれを読み、outline helperの切替フラグを初期化する。LSE非対応時はLL/SCへ分岐する。helper内にLSE命令が存在しても、CPU機能判定で保護されていれば問題ない。

- [Linux向けRust target](https://github.com/rust-lang/rust/blob/feaadeeaca7db0594da854e7c8c07495341c7439/compiler/rustc_target/src/spec/targets/aarch64_unknown_linux_gnu.rs)
- [upstream bare-metal target](https://github.com/rust-lang/rust/blob/feaadeeaca7db0594da854e7c8c07495341c7439/compiler/rustc_target/src/spec/targets/aarch64_unknown_none.rs)
- [Rust側のLSE初期化](https://github.com/rust-lang/rust/blob/feaadeeaca7db0594da854e7c8c07495341c7439/library/std/src/sys/configure_builtins.rs)
- [Linux ARM64 ELF HWCAP仕様](https://docs.kernel.org/arch/arm64/elf_hwcaps.html)

使用中forkの `library/compiler-builtins/compiler-builtins/src/aarch64_outline_atomics.rs` では `HAVE_LSE_ATOMICS` の初期値が0。`__rust_enable_lse()` が設定するまでLL/SCを使う。

使用中forkの `library/std/src/sys/configure_builtins.rs` は `RUST_LSE_INIT` を `.init_array.90` に置き、`is_aarch64_feature_detected!("lse")` の後にhelperを有効化する。compiler-builtins-cを選ぶ構成ではcompiler-rt側にも `getauxval(AT_HWCAP)` を読むconstructorがある。現在のScarlet用Nixビルドはoptimized-compiler-builtins=false。

## 問題の発生源

[fork commit ee10e251a412](https://github.com/petitstrawberry/rust/commit/ee10e251a412f11bc5e5faa236bb0b9c3840f538) が以下の二つへ独自に `+lse` を追加した。

- `compiler/rustc_target/src/spec/targets/aarch64_unknown_scarlet.rs`
- `compiler/rustc_target/src/spec/targets/aarch64_unknown_none.rs`

配布済みstdもLSE前提になる。`-Ctarget-cpu=cortex-a57` だけでは明示された+lseが消えない。アプリに `-Ctarget-feature=-lse` を付けても、配布済みstdは再ビルドされない。

AtomicU64のfetch_add、Arc、Mutex、printlnを使う小さなstdプログラムを生成・逆アセンブルして確認済み。

| ビルド指定 | 自作atomic関数 | ELF内LSE命令数 |
|---|---|---:|
| 既定 | ldaddal | 105 |
| cortex-a57 | ldaddal | 105 |
| cortex-a57 / -lse | ldaxr / stlxr | 98 |

最後の98命令はstdのTLS、allocator等に残る。LinuxのCPU判定で保護されたoutline helperと混同しない。実機での実行結果ではなく生成物の確認結果。

## 実装開始位置と未実装部分

### カーネル

- `kernel/src/arch/aarch64/mod.rs`: `init_arch(cpu_id)`、`init_ap_cpu(cpu_id)` が各CPUの初期化箇所。
- `kernel/src/arch/aarch64/boot/linux/smp.rs`: PSCIのAP起動、`secondary_cpu_entry`、`start_secondary_cpus`。
- `kernel/src/arch/aarch64/boot/limine.rs`: LimineのAP起動・release経路。Linux bootだけに対応して終わらせない。
- `kernel/src/task/elf_loader/mod.rs`: `build_auxiliary_vector`。AT_HWCAP定数はあるが、値の追加はTODOのまま。ネイティブとLinux ABIがこの処理を利用する。
- `kernel/src/arch/user_context.rs`: user-fpu/user-vectorの有効性。CPUに機能があっても、カーネルが保存・復元を支援しないものは通知しない。

probeではARMのIDレジスタを一次資料とLinuxの実装に照らして解釈する。LSEだけを判定する場当たり的な分岐ではなく、CPUごとの機能と公開可能な共通機能を扱う仕組みにする。ただし未対応のSVE等まで通知しない。

**SMPでの重要な順序問題:** `kernel/src/lib.rs` はinit ELF・タスクを作成してから、1082行付近の `start_secondary_cpus_hook` を呼ぶ。Linux bootのhookはCPU_ONを発行し、Limineのhookは待機APをreleaseする。BSPだけを見てHWCAPを公開すると、後から異なる機能のAPが参加した時に既存タスクの契約を破る。最初のユーザー実行・auxv作成と機能確定の順序を設計すること。単に共有マスクをfetch_andで減らすだけでは公開済みauxvは更新されない。

ユーザーランドへ公開する機能は、タスクが実行され得るCPUすべてで使える必要がある。probe未完了・遅れて起動するCPU・起動失敗も考慮し、公開後に弱いCPUを参加させない等の明確な契約が必要。SMPの別問題をついでに修正する依頼ではない。

### Rust fork / 起動処理

- `library/std_detect/src/detect/mod.rs`: Scarlet用OS backendがなく `os/other.rs` に入る。
- `library/std_detect/src/detect/os/other.rs`: 空の機能集合を返す。
- `library/std_detect/src/detect/os/linux/aarch64.rs`: Linux HWCAPから機能集合への変換の参考。
- `library/std/src/sys/pal/scarlet/common.rs`: `_start` はenv初期化からmainへ進む。調査した静的起動経路には `.init_array` を実行する処理がない。
- `library/std/src/sys/configure_builtins.rs`: outline helper初期化の登録。
- `library/compiler-builtins/compiler-builtins/src/aarch64_outline_atomics.rs`: Rust実装のhelperと安全な初期値。

Scarletのauxv取得経路とstd_detect backendを設け、feature cacheが初期化前の空集合で固定されない順序にする。envpの終端後にauxvがあるネイティブABIと、kernelの `setup_native_stack` を確認する。LSE用関数を場当たり的に一度呼ぶだけでなく、constructor実行の責務・通常main以外の起動形態も検討する。

`+lse` を `+outline-atomics` に置換するだけでは自動LSE選択は成立しない。初期化されないhelperはLL/SC側に残る。汎用bare-metalターゲットにはユーザーランドの機能通知・std初期化を前提とさせない。

Scarlet側の旧no_std用 `user/targets/aarch64-unknown-scarlet-elf.json` も `+lse,+neon,+fp-armv8` を定義している。配布stdを使う組み込みtargetとは別の設定なので、両者を混同せず、最低ISAの整合性を確認すること。

### cargo-scarlet

実使用pinの `cargo-scarlet/src/main.rs` をghで取得して調査済み。

- 1325行 `userspace_target_triple`: aarch64をaarch64-unknown-scarletへ固定変換。
- 1155行 `project_cargo_command`: CARGO_HOMEを `.scarlet/cache/cargo-home` にする。
- 3984行 `install_package`: 各sourceへcurrent_dirを変えてCargoを起動。プロジェクトのCargo設定を明示的に渡していない。
- manifestには独立したユーザーランドtarget/cpu/target-features設定がない。

[固定ターゲット選択](https://github.com/petitstrawberry/scarlet-sdk/blob/71c7d9e0510989c23ead340883b30f2b4191521e/cargo-scarlet/src/main.rs#L1325-L1333) と [Cargo起動](https://github.com/petitstrawberry/scarlet-sdk/blob/71c7d9e0510989c23ead340883b30f2b4191521e/cargo-scarlet/src/main.rs#L4023-L4047)を参照。

SDK自身は+lseを指定していない。Linux式の汎用target/runtimeが完成すれば、Switchだけ特別なstdを選ぶ必要はなくなる。機種別最適化の設定機能が必要かはこの修正と分けて判断する。kernel targetのsoftfloat、NEON禁止、リンカスクリプトをユーザーランドへコピーしてはいけない。

## 別問題: build-stdとstd別名依存

Switchの既存 `scripts/build-console.sh` は全ユーザーランドに `build-std=std,panic_abort` を強制し、`prepare-console.py` はAACを無効化して衝突を避けていた。この既存処理はまだ残っている。新しく追加したNix差し替えだけを撤回済みで、既存スクリプトを直したとは言わないこと。

Symphonia forkの `std = { package = "symphonia-std", ... }` と再ビルドしたstdの両方が.rmetaで渡される条件でE0464が出る。SymphoniaもSDKも含まない最小crateで確認した。

- 通常のreleaseビルド: 成功。
- `-Zbuild-std=std,panic_abort -Zbuild-std-features=compiler-builtins-mem` 付きrelease: 同じE0464。
- check + build-std: この条件では成功。実stdの入力.rlibだけを.rmetaに置き換えると再現。

全build-stdや全依存別名が必ず壊れるという結論ではない。最新upstreamでの全条件の確認も未実施。LSE問題を解決するためにAACを無効化する必要はない。通常のChromebook/QEMUユーザーランドは配布stdを使うため、同じfull bundleでもこのエラーを踏まない。

## 検証すべき項目

1. CPU IDレジスタの判定、未対応・予約値、複数CPUの共通機能算出、公開後の不変性。
2. 初期プロセスと後続execの両方のauxv。ネイティブABIとLinux ABI。RV32/RV64への副作用。
3. QEMU等の非LSE CPUとLSE CPUで**同じユーザーランドバイナリ**を起動し、機能検出結果とhelperの選択を確認。
4. メインより前のatomic、Arc/Mutex、複数スレッド、CPU間の移動で正しさを確認。起動・constructor順序を確認。
5. Switch A57で通常のstd利用バイナリが動くこと。LSE対応機でLSE経路に入ること。
6. 通常のcargo-scarlet full bundleが、スクリプトによるstd再ビルド/AAC除去なしでビルドできること。
7. RustとNixの通常配布手順、フォーマット、必要な回帰検証。ソース修正だけで配布済みstdまで直ったと報告しない。

## 調査証跡

同じMacの `/tmp/scarlet-lse-audit-20260920/` に詳細を保存済み。

- `REPORT.md`: 詳細な比較・判断と制約。
- `probe.rs`、`default.s`、`cpu-a57.s`、`no-lse.s`、各disassemblyとELF。
- `target-cfg.json`、`compiler-version.txt`、`cargo-version.txt`。
- `upstream-*.rs`、`fork-*.rs`、`sdk-pinned-main.rs`、`mandatory-lse-commit.patch`。
- `std-alias/`: E0464の独立した最小再現コードとビルドログ。
- `config-scope/`: Cargoのカレントディレクトリによる設定探索の確認。

一時ディレクトリが消えていても、この文書のpin・パス・結果を出発点にできる。

## 現在のMMCセッションとの境界

MMC/SDは別の進行中作業。共通MMC/SDHCI、SDカード初期化、MBRパーティション列挙、Tegra210 SDMMC1ドライバが未コミットで存在する。最初の実機起動でmmcblk0と123773911040 bytesのSDカード認識は成功。続く実機起動でmmcblk0p1〜p4の開始セクタ・容量一致とUARTシェル到達を確認した。ファイル読み書き、永続rootfsは未完了。実機記録はSwitchプロジェクトの `docs/mmc-sd-bringup.md` と `.cache/mmc-sd-20260920/` に保存する。

Switchのkernel targetはすでに-LSEで、動作確認済みの非LSE initramfsも保存されている。outline対応担当がMMC側のビルド設定や既存変更を勝手に差し替えない。共有作業ツリーであることを意識し、変更範囲を分ける。

## 実装セッションの結果（2026-09-20）

上記の「未着手」「未実装」は引き継ぎ時点の記録。今回の実装は以下のとおり。MMC/SDの未コミット変更は保持した。

- `kernel/src/arch/cpu_features.rs` にアーキテクチャ共通のCPU能力レジストリを追加。CPUごとの報告から共通部分を計算し、最初のELFより前に固定する。公開後に到着したCPUは公開済み全能力を満たす場合だけスケジューラへ進む。AArch64固有のIDレジスタ解釈は `kernel/src/arch/aarch64/cpu_features.rs` に分離した。
- Linux/PSCI bootではAPの能力probeを最初のELFより前に行い、従来のスケジューラ解放位置は維持した。Limine bootでもAPが解放前に報告する。FP/SIMDの通知はユーザーコンテキスト保存・復元のポリシーに従う。AArch64のネイティブ/Linux ABIのauxvへ `AT_HWCAP` と `AT_HWCAP2` を載せる。RISC-Vのauxvは変更しない。
- Rust forkの `aarch64-unknown-scarlet` は `+outline-atomics` を使い、`+lse` を必須にしない。Scarlet stdの入口はauxvをconstructorより前に公開し、`.init_array` をmainより前に実行する。std_detectはHWCAPからLSE等を検出する。汎用bare-metal Rust targetとScarletの旧no_stdユーザーtargetもLSE必須から外した。
- cargo-scarletに `[userspace].cargo-config` を追加し、すべてのユーザーパッケージビルドへ明示設定を渡す。Switchはチェックインした設定でローカル依存を選び、スクリプト生成のCargo設定・全std再ビルド・AAC無効化を外した。

検証済み: 共通レジストリとAArch64デコーダの3テスト、AArch64（Limine/Linux）とRV64のkernel check、cargo-scarletの63テスト、ローカルRust stage1のターゲットstdビルド、Switchの通常full bundleビルド。QEMUでは**同一**の `/tmp/scarlet-outline-probe`（SHA-256 `d7a1d09b51448c971b44233e3da5c741ef3d4e2aafc023fca0fc65875ae1919f`）を用い、A57・2 CPUで `HWCAP=0xfb`、LSE検出/outline helperフラグとも0、main前atomic=1、2スレッドのatomic合計=200を確認。`max`・1 CPUでは `HWCAP=0x2007fb`、LSE検出/フラグとも1、同じく1と200。逆アセンブルで見つかったLSE命令14個はすべてoutline helper内にある。QEMU `max` の `ID_AA64ISAR0_EL1.Atomic=3` はLSE128を含むので、2だけでなく3もLSEとして扱うよう修正した。結果はQMPでユーザー空間の固定結果領域を読んだもので、シリアルへの推測ではない。

この時点でRust fork・Nix・SDKの公開、toolchain/SDK pin更新、固定済み配布物での再ビルド、Switch実機での起動確認が残っていた。後続の公開結果は以下に記録する。

## 公開作業の結果（2026-09-20）

ユーザー承認後、Rust fork `scarlet-target` に [`39c689a4859b`](https://github.com/petitstrawberry/rust/commit/39c689a4859b9d8ee1828720135defd125c03d31) を公開した。SDKの設定伝播は [PR #6](https://github.com/petitstrawberry/scarlet-sdk/pull/6) でmainへマージ済み（`116882ab42b48a23613d3d574e2c987aca277786`）。

Nixの更新ワークフロー [run 35484779626](https://github.com/petitstrawberry/scarlet-rust-nix/actions/runs/35484779626) と [PR #19](https://github.com/petitstrawberry/scarlet-rust-nix/pull/19) は成功・マージ済み。mainは `7e579ad8de2b85ad9dff6748a30b0a836f8d322a` を指す。[3ホスト配布CI](https://github.com/petitstrawberry/scarlet-rust-nix/actions/runs/35494821781) はx86_64-linux、aarch64-linux、aarch64-darwinで成功し、新ツールチェーンはCachixから取得できた。

Scarletの最初のRV32 CIで、共通レジストリの `AtomicU64` が32-bitターゲットに合わないことが判明した。CPUごとの一度だけの報告をRelease/Acquireで公開する方式へ修正。次のCIでは新規テストを通常の `#[test]` としたためlibtestなし構成で失敗し、kernel固有の `#[test_case]` へ修正した。最終的にAArch64 1,261件、RV64 1,288件、RV32 1,228件のローカルテストが通過。Scarletの [PR #567](https://github.com/petitstrawberry/Scarlet/pull/567) はMMCのローカル先行コミットを含めず、[最終CI](https://github.com/petitstrawberry/Scarlet/actions/runs/35500034709) 全件成功後、`dev`へマージ済み（`4fe644fadafffa21c8e1eba651de570162fdc156`）。アーキテクチャ共通の設計説明は公開済みの `docs/development/cpu-capabilities.md` に記した。

Switchの [PR #3](https://github.com/petitstrawberry/scarlet-project-switch/pull/3) はMMC/GPUのローカル先行コミットを含めず、`main`へマージ済み（`8d2f0e7415b988c7a8039f391c97ff7f149234fc`）。Scarlet/Switchのtoolchain lockは上記Nix mainへ、SwitchのSDK pinは上記SDK mainへ固定した。固定済みNix開発シェルでSwitchの通常51レイヤーフルバンドルを再ビルドし、AAC依存を含めて成功。initramfsは86,905,344 bytes、SHA-256 `c7d434eed38125bba7cc7b2c08158d230443f853aac66f00214e2db9fd607577` で、L4Tパッケージも生成した。Switch実機でこの新しい汎用stdの起動・atomic経路は未確認。
