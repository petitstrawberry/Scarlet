# Scarlet / SGFX / ScarletUI v1.0 作業引き継ぎ

更新日: 2026-09-06。引き継ぎ時点のローカルチェックアウトを確認して記載した。
本文は引き継ぎ時点の記録。作業再開後の変更と確認結果は末尾の「再開後の進捗」と「SGFXの互換性境界を確定」を参照。後者の決定により、SGFXの全Rust公開APIを1.xで凍結する旧案と一律enum移行はRC条件から外れた。

最新の訂正: **ユーザーの指すSDKは別repoの`scarlet-sdk`（`cargo-scarlet`とimage plugin）。native APIの整理をもってSDK契約完了とした報告は撤回した。ScarletUIの現行契約は記録済み、`scarlet-sdk`本体の契約整理は未完。** A618は保留、Boxcraftはユーザー確認済みで完了。ローカルpatchのコミット混入を撤回し、SGFX smokeはユーザー指示で削除した。末尾の「誤認・ローカル設定混入の撤回」を現在の状態として優先する。

## まず結論

- 今回の **SGFX完了API・ネイティブ投入スケジューリング・ScarletUIの安全なフレーム拒否処理は、実装修正完了として区切る**。ユーザーもこの区切りを了承している。
- **v1.0全体の準備完了ではない。** 残りはScarlet側の未コミット接続変更の整理、未確定の公開契約、A618、最終リリースの依存・版・サポート範囲の確定。
- 巨大な投入を無制限に処理する実装はしていない。現在の64 MiB上限は維持し、超過を安全に通知してアプリの入力・状態更新を続けられる扱いにした。
- 新しい修正について、BoxcraftのQEMU動作成功はまだ報告されていない。これは「未実装」ではなく、ユーザー担当の動作確認が未報告という区別。勝手にQEMUを起動して再検証しない。
- 前担当は確認作業を広げすぎて、コミット・引き渡しを遅らせた。次担当は完了済みの作業や環境確認を繰り返すところから始めないこと。

## 1. ユーザーとの作業上の取り決め

1. **開発の優先順位・対象範囲の決定はユーザーが行う。** 未承認の契約案を決定済みにしない。変更が必要なら、何を問題とし、何を取り決めるのか具体的に説明する。
2. メインのチェックアウトを直接操作する。新しいworktree、ソースコピー、独自taskラッパー、追加の作業管理機構を作らない。
3. **QEMU・GUIの動作確認はユーザーが行う。** ユーザーのQEMUを起動・停止しない。Dockerを起動しない。`cargo make`も使わない。古いAGENTS/roadmapの一般的なコマンド例より、この会話での具体的な指示を優先する。
4. Nixの既存環境と `scarlet-rust-toolchain` を使う。環境の正当性を毎回調べ直さない。通常のScarletユーザー空間ターゲットはRust `std` 対応であり、Scarlet target = `no_std` ではない。
5. Git依存更新は通常の `cargo update`。不必要に `--precise` や手作業のハッシュ差し替えを持ち込まない。
6. 当面Scarletは `dev` 直で進めてよい。コミットは小さく行う。タグ作成・リリース公表・実験機能の範囲変更は別の判断。
7. ユーザー所有の差分を巻き戻したり、無断で混ぜてコミットしたりしない。以下に実際の残存差分を明記している。
8. QEMUが快適に動くというユーザーの確認済み前提を尊重する。過去のdebugビルド時の遅延を再び未解決課題にしない。showcaseのFPSにはSWS側の制限も関係していた。

## 2. リポジトリ・公開状態

共通の親ディレクトリは `/Users/petitstrawberry/Development/Rust`。

| リポジトリ | ブランチ / HEAD | 現在の状態 |
| --- | --- | --- |
| `Scarlet` | `dev` / `58cb461e` | 手元の `origin/dev` と一致。下記4ファイルに未コミット差分あり。 |
| `sgfx` | `main` / `456dde7` | push済み。手元の `origin/main` と一致。作業ツリーに差分なし。 |
| `scarlet-ui` | `main` / `d1632b87` | 手元の `origin/main` と一致。追跡ファイルに未コミット差分なし。未追跡 `.omo/` のみ。 |
| `scarlet-project-chromebook` | HEAD `0508d00` | この修正では変更していない。別作業の差分あり。 |

この表のremote確認は既存のremote-tracking refとの比較。引き継ぎ書作成のためのfetch、push、ビルドはしていない。

**直前の会話との差分:** 前担当が「ScarletUIは未コミット」と返した後、引き継ぎ確認時には以下のコミットが存在し、`origin/main` も更新されていた。古い返答ではなく、この表を基準にする。

- SGFX `f9b2a34`: 論理投入のスケジューラ、パケット分割、保持・完了処理。
- SGFX `c64604b`: 論理受付とネイティブ実行の責任分担の文書化。
- SGFX `456dde7`: `Error::is_recoverable_rejection()` と分類テスト・契約追記。
- ScarletUI `358225ee`: `Cargo.lock` のSGFX参照を `456dde7b8cb1c82d40b8aedb1feeb72b7810e776` へ更新。
- ScarletUI `d1632b87`: フレーム回収・エラー通知・イベントループ継続・契約文書をコミット。ユーザーが編集していた `examples/widget-factory/src/main.rs` もこのコミットに含まれている。前担当の変更だけのコミットと誤認して一括revertしない。

### Scarletに残る未コミット差分

| パス | 所有・内容 | 次の扱い |
| --- | --- | --- |
| `user/std-bin/src/sgfx_ir_support.rs` | 前担当の接続修正。`FrameExecutor::new(executor)` に追従。UI側の512 KiB分割・タイマー再試行を削除しSGFXに委譲。 | 今回の接続変更として保持・コミット対象。新しいScarletUIとの組み合わせに必要。 |
| `docs/graphics/gpu-async-submission.md` | 前担当の更新。スケジューラ、フレーム拒否契約、確認済み範囲を記録。 | 上の変更とともにコミット対象。 |
| `.cargo/Cargo.lock` | ユーザーが更新した差分。引き継ぎ時98行の差分。 | 無断で巻き戻し・上書き・前担当分として一括stageしない。 |
| `projects/aarch64-limine-full/scarlet.lock` | ユーザーが更新したプロジェクトロック。引き継ぎ時329行の差分。 | 同上。最終リリースの固定とは区別する。 |
| `test-kernel` | 既存の未追跡項目。 | 今回の成果物ではない。削除しない。 |

この引き継ぎ書自体も、新規の未コミット文書として追加する。引き継ぎの依頼を、残存コードのコミットやpushの許可とは解釈していない。

Chromebook側には `projects/aarch64-coachz-limine/scarlet.lock`、同 `tools/build-coachz-sgfx-userspace.sh` の変更と、未追跡 `coachz-handoff-cmd-db.txt`、`kernel`、`scripts/.evidence/` がある。この修正の差分ではなく、保持する。

## 3. 合意した契約

### 責任分担

| 層 | 担当すること |
| --- | --- |
| Boxcraft等のアプリ | シーンの負荷・描画距離・LODなどの方針。エラー通知を受けて状態を変える。 |
| ScarletUI | フレーム境界、前の表示の保持、拒否フレームの回収、キャッシュ無効化、全画面再描画、アプリへの通知。 |
| SGFX native backend / queue | ネイティブパケットへの分割、順序付き投入、転送領域の再利用待ち、キュー容量管理、全体の完了。 |
| Scarlet GPU kernel / driver | 受付済みネイティブ要求とリソースの保持、実行、権威ある完了通知。 |
| SWS | 提示・保持・buffer release。GPU完了とは別の条件。 |

### ScarletUIの失敗分類

正式な詳細は [ScarletUIのフレーム失敗契約](../../../scarlet-ui/docs/FRAME_FAILURES.md)。

- `Busy`: 一時的な容量不足。受付済みの先行処理を正常完了まで回収したうえで前の表示を保持し、次のイベント処理周期で新しい全画面フレームを組み立てて再試行する。
- `Rejected`: 入力・対応範囲・資源上限による拒否。先行処理の正常完了を確認したうえで前の表示を保持する。変更のない入力を自動再試行し続けず、新しいシーンの無効化通知を待つ。
- `RecoveryRequired`: 部分受付失敗、GPU実行失敗、完了確認失敗・不確実な回収。画像を再利用せず、アプリへの通知後にrunnerからエラーを返す。継続するならbackend/windowの再生成が必要。自動GPUリセットは実装していない。
- アプリには `Application::on_render_error(&WindowContext, &RenderFailure)` でkindと原因を通知する。`RenderFailure.reason` は診断用文字列であり、文字列解析で分岐する契約ではない。
- 失敗フレームで `on_frame_presented` を呼ばない。成功した提示と偽って扱わない。
- SGFXの `Rejected` は **その一回の投入の未受付** だけを保証する。同一フレームの先行投入を巻き戻すものではない。フレーム全体や受付済みprefixを無条件に再実行してはいけない。

「任意サイズのフレームを必ず処理できること」はv1.0必須とはしない。一方、拒否・失敗時の所有権、安全性、続行／復旧の方法はv1.0前に固定する。この項目は今回合意し実装した。

## 4. 実装の所在と重要な不変条件

### SGFX

`sgfx/crates/sgfx-backend-scarlet-virgl/src/`:

- `scheduler.rs`: syscallから独立したFIFO。16論理投入、合計64 MiB、16ネイティブ要求を上限とする。Busyで未投入位置を保持し、受付済みの要求を再送しない。後続が未投入prefixを追い越さない。
- `packets.rs`: 完全なパケットの境界で現在の2 MiBネイティブ上限へ分割。頂点arena/offsetを選ぶ前にチャンク切替を行う。
- `dispatch.rs`: contextごとのworkerが自律的に投入・回収する。receiptのpollやfrontend ownerの生存に進捗を依存させない。
- `completion.rs`: receiptは論理投入の全チャンクと順序付きprefixを覆う。Dropはキャンセル・完了ではない。
- `virgl.rs`: 4個×2 MiBのupload arena。前回のネイティブfenceの正常完了前には再利用しない。arenaから頂点ストレージへGPUコピーを順序付きで発行する。
- `ir_execute.rs`: 受付前に検証・lowering。Busy/RejectedではCPUの初期化・revision状態を復元する。論理受付後の実行失敗はreceiptとdispatcherを失敗状態にする。

`sgfx/crates/sgfx/src/lib.rs` の `Error::is_recoverable_rejection()` は、`SubmitError::Rejected` 内のエラーにだけ適用する。completion errorや `Failed` に適用して回復可能へ格下げしない。不明なtransport/deviceエラーは保守的に扱う。

kernel側の `Scarlet/kernel/src/device/gpu/execution.rs` ではqueue capabilityがcontextとattachment群を保持する。workerの所有するqueueが、まだネイティブ投入していないパケットの資源所有権も支える。明示的detach、readback、同期実行等は先行論理仕事をdrainする。所有権や同期をUIへ逆流させないこと。

### ScarletUI

- `crates/scarlet-ui-renderer-sgfx/src/submission.rs`: `FrameExecutor::discard()` を追加。受付済みのprefixだけを回収し、破棄したフレームを提示可能な状態には戻さない。`wait()` とは別の操作。
- `crates/scarlet-ui-renderer-sgfx/src/lowering.rs`: `SgfxPaintEncoder::discard_frame()` がcanvas内容のrevisionキャッシュを無効化する。正常完了済みのmesh/texture uploadまで無条件に捨てて再転送しない。
- `crates/scarlet-ui-platform-sws/src/backend.rs`: 回復可能な拒否とGPU故障を分ける。安全な回収後だけ継続し、提示slotを進めず前の画像を保持、次回を全画面再描画にする。
- `crates/scarlet-ui-platform-sws/src/lib.rs`: 分類を汎用 `RenderError` に潰さず上位へ渡す。
- `crates/scarlet-ui-core/src/error.rs`: `RenderFailure` / `RenderFailureKind`。
- `crates/scarlet-ui-core/src/application.rs`: `on_render_error`、Busyの遅延再試行、Rejected時のイベントループ継続、故障時の通知と終了。
- `crates/scarlet-ui-core/src/pipeline/rendering.rs`: 元からあるエラー時のfull-repaint要求を利用。今回このファイル自体に変更は加えていない。

## 5. Boxcraftの問題と、解決した範囲

1. 当初はネイティブ一要求の2 MiB上限を論理投入全体にも適用してしまい、60,000頂点・2.4 MBのmeshを `OutOfResources` にしていた。SGFX自身で分割・スケジュールする実装へ変更済み。
2. 次にユーザーは、しばらくプレイして描画チャンクを増やした後の `SubmissionTooLarge` を報告した。
3. 待機キューの合計容量不足なら `Busy`。一回分が64 MiBを超える場合は `SubmissionTooLarge` であり、待っても同じ入力は入らない。内部パケット／arena範囲超過も同じエラー分類になるため、**その報告の実バイト数・厳密な超過地点は計測していない**。ログだけで「過去フレームの蓄積」と確定しない。
4. 現在のUIは提示前にフレーム全体の完了を待つので、通常経路で過去フレームが無制限に滞留する構造ではない。
5. 今回はこの拒否でUI全体を永久失敗扱いにし、Boxcraftを終了させてしまう経路を修正した。**64 MiBを超えるフレームそのものが描けるようになった、とは言っていない。**

将来、さらに大きな入力を受け付けるなら、入力データの所有・保持予算と、GPUへ送る作業領域の予算を分けたstreaming設計が必要。単に上限を上げる、恒久的なTooLargeをBusyへ置換する、UIにネイティブの細切れ処理を戻す、という方向へ無断で変更しない。これは今回の完了範囲とは別の追加作業。

## 6. 検証結果 — 通過と未通過を区別

引き継ぎ作成中に新しいテストは実行していない。以下は実装中の実行結果と、停止時に既に走っていたプロセスの完了結果を回収したもの。

### 最新のフレーム拒否修正

| 確認 | 結果 |
| --- | --- |
| `scarlet-ui-core` host全libテスト、1 thread | **311 passed / 0 failed / 1 ignored**。ignoredをpassに含めない。 |
| `scarlet-ui-renderer-sgfx` host全libテスト | **41 passed / 0 failed**。prefix回収、Pending/失敗時の非再利用、canvas無効化を含む。 |
| Application関連テスト | 上のCoreに含む13件が通過。新規3件は通知、前表示維持、再試行方針、全画面再描画を確認。 |
| SGFX facadeの回復可能拒否分類hostテスト | **2 passed**。実GPUは起動しない分類テスト。 |
| SGFX facade AArch64テストハーネス | `--no-run` 成功。ネイティブ分類テストはコンパイル済み、実行はしていない。 |
| UI std + platform-sws、AArch64 / RISC-V | 両方 `cargo check --locked --offline` 成功。GitのSGFX `456dde7b` を使用し、SGFXのローカルpatchには依存しない確認。 |
| Scarlet本体のSWS / ui-sgfx-showcase、AArch64 | `.cargo/Cargo.toml` 経由のcheck成功。こちらは既存の兄弟ローカルpatchを使用。 |
| SGFX facade、AArch64 strict Clippy / Rustdoc | 通過。 |
| 変更ファイルのrustfmt / diff whitespace | 確認済み。ユーザーのexample等を勝手に整形しない。 |
| 新修正のQEMU / Boxcraft runtime | ユーザー担当。成功報告は未受領。前担当は起動していない。 |

最新のhost結果は合計 **354 passed**（311 + 41 + 2）、別途1 ignored。以前の65件などの集計と足して「最新の総数」にしない。

### strict Clippyで残っている具体的指摘

停止前に開始していた次のコマンドは、結果回収時 **exit 101** だった。成功扱いにしない。

```text
cargo-clippy clippy --locked --offline -p scarlet-ui-renderer-sgfx --no-deps -- -D warnings
```

- `crates/scarlet-ui-renderer-sgfx/src/lowering.rs:620`: `encode_frame` の8引数に対する既存の `clippy::too_many_arguments`。
- `crates/scarlet-ui-renderer-sgfx/src/submission.rs:66-68`: `FrameExecutor::new` の引数説明直後のdoc段落に対する `clippy::doc_lazy_continuation` 3件。前担当の変更で残した文書lint。段落区切り／インデントの整理が必要。
- 依存するUI Coreにも既存のunused/dead-code警告がある。今回すべてを解消したとは主張しない。包括的なlint無効化で隠さない。

これは引き継ぎ用の既知残件であり、この文書作成中にソース修正や再実行はしていない。

一度 `cargo clippy` がupstreamのrustup proxyを選んでScarlet targetを認識しなかったが、Nixの `cargo-clippy clippy` でSGFXのstrict確認は成功した。環境全体を壊れたものと扱って調査をやり直す必要はない。

### 以前の確認を再開しないためのメモ

- schedulerのhost 6テスト、packetのhost 4テストは投入・回収実装時に通過済み。
- その時点のSGFX native std両arch、AArch64 legacy std、更新したnative smokeのrelease buildも通過済み。これは最新UI変更のlegacy確認とは区別する。
- 以前のgear/swarm/UIの頂点破損は、upload arenaと順序付きGPUコピーへの変更後に、ユーザーが「確認した。正常」と報告済み。
- ユーザーのnative smoke連続ALL PASSは以前の版の実測。後から追加した大きな入力や最新UI拒否処理の実測と混同しない。
- kernel両archのテスト・過去のfull-image結果、ringのCコンパイラ選択修正、CI修復の記録は既存baselineを参照。引き継ぎのために再実行しない。

## 7. 残件と次の担当者の着手順

### A. 今回の修正の引き渡しを閉じる

1. Scarletの前担当所有2ファイルを、ユーザー所有のlock差分と区別してコミットする。SGFXとUIは既に公開側へ反映されているので、同じ変更を作り直さない。
2. 上記の具体的なClippy残件を通常のlint整理として扱う。入力・完了契約を再設計する理由にはしない。
3. 最新の動作結果はユーザーから受け取る。ユーザーが新しい不具合を報告しない限り、完了済みの修正を再び全環境で検証し始めない。

### B. v1.0全体に残る本体作業

既存の [release scope](1.0-scope.md) と [roadmap](1.0-roadmap.md) に基づく。下記の順番やスコープ変更はユーザーと決める。

- **未確定の公開API契約を確定する。** SGFXのIR・direct backend・公開enum/struct/trait・feature・imported-image解放・描画意味、Scarlet SDK/GPU/SWS、ScarletUIのView/State/Scene/拡張境界を、現在の実装と照合する。今回の完了・拒否契約は決定済みだが、既存SGFX `1.0-contract.md` 全体はまだdraftであり、全部ユーザー承認済みとは扱わない。
- **A618の非同期投入・完了・保持処理。** 現在はasync capacity 0、legacy同期経路。既存scopeでは非同期実装と実機根拠がRC gateになっている。終わったことにしない。今回のVirGL修正に便乗して無断で着手／対象外にもしない。
- **失敗・reset・owner teardown・imported-image解放の残る適合性確認。** 正常描画やコンパイルのみでデバイス故障時の安全性や全機種対応を保証しない。今回のUIは自動device-loss復旧を追加していない。
- **公開APIの拡張方針の実装・必要な契約テスト。** 開放／閉鎖enum、required trait項目、公開フィールド、外部型依存など。数を増やす検証ではなく、決まった保証との差を埋める。
- **既存warnings / lint / ignoredテストの整理・扱いの記録。** 正しさの失敗と、最適化目標・手動テストを分ける。

### C. RC・v1.0を出すための最終整理

- SGFX・ScarletUI・Scarlet・ChromebookのGit参照方式とロックの組を揃える。共有型を持つ `sgfx-core` が同一バイナリに複数sourceで入らないことが重要。version文字列一致だけでは不十分。
- 現在のローカルpatchはユーザー指定の開発状態。最終候補では公開済みsourceだけで依存を解決できる組を選び、必要な段階でpatchを整理する。今すぐ無断で外さない。
- repository単位ではSGFX→Adreno→SGFX core、SGFX→Scarlet runtimeの参照があるため、タグ作成順・候補コミットを事前に決める。公開済みタグを動かして修復しない。
- 配布対象のfull image、実験bundleを含む外部アプリ群が、その候補lockでbuild可能なことを最終候補段階で確認する。smoke専用imageで代用したり、失敗するアプリを無断でbundleから外したりしない。
- パッケージ一覧をstaging時に更新し、project-owned packageを `1.0.0-rc.1` → `1.0.0` に揃える。既存inventoryは2026-09-05時点の98 package（97 first-party + vendored sunset）で、現在の確定件数ではない。vendor版・ABI版・wire版はpackage版と連動して変更しない。
- Gitだけの配布でよく、crates.io公開は必須ではない。immutableな新規RCタグ、release notes、対応構成・制限・lock/toolchain記録、最終承認が残る。今回の作業では版上げ・RCタグ・v1.0公開をしていない。
- 依存追従PRの自動化は残件。bare Git依存の追従をRenovateが自動で済ませるとは仮定せず、通常のlock更新workflowと組み合わせる。自動mergeは前提にしない。

**直近で最も大きい未完項目は、未確定契約とA618。** 64 MiB超のstreaming、Vulkan frontend、新backend、全実験アプリの完成は、自動的に今回の追加必須項目へしない。

## 8. 参照・実行入口

主要文書:

- [今回のnative実装と過去の確認](../graphics/gpu-async-submission.md)
- [ScarletUIの新しい拒否・回復契約](../../../scarlet-ui/docs/FRAME_FAILURES.md)
- [ScarletUIのretained runtime契約](../../../scarlet-ui/docs/ARCHITECTURE.md)
- [SGFX完了契約](../../../sgfx/docs/completion-contract.md)
- [SGFXの未確定1.0契約案](../../../sgfx/docs/1.0-contract.md)
- [SGFX公開API棚卸し](../../../sgfx/docs/1.0-api-scope.md)
- [全体のrelease scope](1.0-scope.md)、[roadmap](1.0-roadmap.md)、[過去baseline](1.0-baseline.md)、[package一覧](1.0-packages.md)

**古いroadmap/API文書には「native SGFX/UI未接続」など、現在より前の時点の記述が残る。** それを理由に実装をやり直さない。今回の実装状態はこの引き継ぎ書と最新の完了／フレーム拒否契約を優先し、文書の時系列整合を通常の整理対象とする。

ローカル接続（当時の記録。以下のコミット混入・smoke組込みは末尾で撤回した）:

- `Scarlet/.cargo/Cargo.toml` は兄弟 `scarlet-ui` と `sgfx` への明示的patchを保持している。これはユーザーがローカル修正を使うため指定したもの。
- 当時はexperimental bundleから兄弟SGFXの診断を組み込んでいた。この取り決めは後の削除指示で廃止した。診断を復活させたり、新しい専用taskを作ったりしない。

必要な変更を加えた場合の最小確認入口。**この一覧を次担当が無条件で全部走らせるためのチェックリストにはしない。**

```sh
# cwd: /Users/petitstrawberry/Development/Rust/scarlet-ui
cargo test --locked --offline -p scarlet-ui-renderer-sgfx --lib
cargo test --locked --offline -p scarlet-ui-core --lib -- --test-threads=1
cargo check --locked --offline -p scarlet-ui --no-default-features --features std,platform-sws --target aarch64-unknown-scarlet

# cwd: /Users/petitstrawberry/Development/Rust/Scarlet
cargo check --locked --offline --manifest-path .cargo/Cargo.toml -p scarlet-std-bin --bin sws --bin ui-sgfx-showcase --target aarch64-unknown-scarlet
```

ユーザーが指定したQEMU環境は、vhost-user-video有効、audio有効、USB NCM無効、HVF、8 CPU、16 GiB、`virtio-gpu-gl-pci`、`cocoa,gl=on,retina=on,full-grab=on`、audio driver `coreaudio,out.fixed-settings=off,out.mixing-engine=on`。これは既存前提の記録であり、次担当への起動指示ではない。

## 引き継ぎ時の停止点

実装修正をクローズし、ここからは次担当とユーザーが残件の順番を決める。引き継ぎ書の作成のみを行い、コードの追加修正、ビルドの再実行、QEMU起動、残存差分のコミット・pushは行っていない。

## 再開後の進捗 — 2026-09-06

この引き継ぎ書と [SGFX issue #1](https://github.com/petitstrawberry/sgfx/issues/1) を確認して作業を進める依頼に基づく追記。

- Scarlet `091a2b15`: 前担当所有の `sgfx_ir_support.rs` と `gpu-async-submission.md` をコミット。ユーザー所有の2つのlock差分は含めていない。
- ScarletUI `8cb8ca8b`: `FrameExecutor::new` のdoc段落を修正。`encode_frame` の既存公開シグネチャを維持する理由を関数単位の `#[expect(clippy::too_many_arguments)]` に記載。成功をGPU完了と混同していたRustdocも修正し、提示前に `FrameExecutor::wait` の `Complete` を要求することを明記。
- SGFX `9009e9d`: [実行境界の設計文書](../../../sgfx/docs/architecture.md) を追加。issue #1 の責任分担を既存crate・実装へ対応づけ、facadeと描画フロントエンドの役割を明確化。実行契約とAPI棚卸しにnative VirGL receiptを反映し、Adrenoのtracked submit未対応、同期互換経路、未承認契約案を区別した。
- Scarletのrelease scope / roadmapを更新。実装済みのnative SGFX/SWS/UI接続を未完としていた記述を修正し、過去の検証件数を当時のbaselineとして明示。

今回の追加確認は以下の範囲に限定した。過去の検証数へ加算しない。

| 確認 | 結果 |
| --- | --- |
| `cargo-clippy clippy --locked --offline -p scarlet-ui-renderer-sgfx --no-deps -- -D warnings` | 通過。依存UI Coreの既存25 warningsとpreview-demoの重複target警告は残る。 |
| `cargo test --locked --offline -p scarlet-ui-renderer-sgfx --lib` | **41 passed / 0 failed / 0 ignored**。 |
| UI renderer / SGFX facadeのhost Rustdoc | 両方 `--locked --offline --no-deps`、`RUSTDOCFLAGS='-D warnings'` で通過。 |
| 変更Rustファイルのrustfmt・diff whitespace | 通過。 |

既存のNix / Scarlet Rustを使用した。QEMU・GUI・Dockerは起動せず、完了済みの全環境検証は再実行していない。2つのユーザー所有lockファイルの内容は開始時と同一で、ユーザーのstage状態も保持した。

この再開分はローカルコミットまで。push、issueの投稿・close、タグ・リリース作成は行っていない。この時点では公開Rust APIの凍結範囲と拡張方針も未決定だったが、下記の方針確定で更新した。その他未承認契約、A618非同期化、最新Boxcraft修正のユーザー動作確認、最終候補の依存固定・リリース作業は引き続き残る。

## SGFXの互換性境界を確定 — 2026-09-06

issue #1 の構想と、SGFX段階で動的ロードは予定せず最終的にアプリ側はVulkanへ移行するというユーザーの説明・承認に基づく。今回確定したのは構成とRust更新方針であり、描画・import・presentation等の残る意味契約やv1.0公開の一括承認ではない。

- SGFXはドライバ内部の実行IR。`sgfx-core`、facade、各backend、codegenとそれらを直接使うfrontend/rendererは、互換なsource・lockの組を選んで同時更新・再ビルドする。
- アプリとの将来の安定境界はVulkan C ABI。`vulkan-sgfx + sgfx-core + 選択したbackend + 必要なcodegen` を一つのICD/libraryへリンクできる。SGFXのRust部品を独立に動的ロードするABIは設けない。Vulkan frontend実装済みという意味ではない。
- 全Rust `pub`の1.x固定、全enumの一律`#[non_exhaustive]`化、恒久的なclosed enum一覧はRC条件にしない。enum、trait、公開フィールド、feature、外部依存型を変える場合は、影響する実装・利用側・説明・必要なテストを一緒に更新する。現在のexportや注釈を一括で除去する作業でもない。
- IRの意味、資源保持、投入順序、有界な受付、完了、安全な拒否・失敗の契約は維持する。Rust型が内部実装だからといって、backendが異なる意味で受理したり未対応処理を黙って成功にしたりしてよいわけではない。
- ScarletUIの直接SGFX rendererは引き続き使える。将来のVulkan rendererへの移行も妨げない。ScarletUIのアプリ向けAPI、Scarletのsyscall/GPU ABI、SWS wire契約は、それぞれ別の保証範囲を保つ。

反映内容:

- SGFX `a1b9168`: `1.0-contract.md`のRust方針を置換し、architecture・API棚卸し・README・実行／完了契約への導線を統一。構成図はMermaidではなく通常のテキスト図にした。
- ScarletUI `b0cd4e71`: READMEの依存説明に、rendererとSGFXの協調更新、アプリ向けAPIとの境界を追記。
- Scarletの本改訂: release scope / roadmapからSGFXの一律Rust API凍結・enum移行ゲートを除き、実行意味と対応する依存セットの整合を確認する方針へ更新。この引き継ぎにも新しい決定を記録した。

今回の変更はMarkdownのみ。3 repositoryで`git diff --check`が通過し、SGFXの`1.0-contract.md`第3〜8節（資源・描画・実行・失敗・import・presentation）は変更前と同一であることを確認した。コード・wire形式・package版・lockは変更せず、build・runtimeテストを再実行していない。上の再開時テスト結果とは区別する。

この方針反映もローカルコミットまでで、push・issue投稿・タグ作成は行っていない。SGFXのRust凍結方針を未決定として再開したり、一律enum移行を残件に戻したりしない。残る本体作業は、未承認の描画・lifecycle契約と対応する適合性、A618の非同期化、最終候補の依存固定・リリース準備である。

## 作業対象の更新 — 2026-09-06

- ユーザーはA618を一旦保留と指示した。実装・実機検証・submit-wire整理に着手しない。完了扱いやサポート範囲の削除ではなく、保留として管理する。
- Boxcraftについてユーザーから「とっくに終わってる」と確認を受けた。修正とユーザー担当の動作確認は完了として扱い、過去の「未報告」を残件へ戻さない。エージェントが新しく実行したテスト、A618や故障時の根拠としては数えない。
- 当面の本体作業はScarletUIと`scarlet-sdk`の契約整理だった。候補lock・release notes・版上げ・RC公開はその後の手続きとして残る。
- ScarletUIの契約全体が未着手だったわけではない。`docs/ARCHITECTURE.md`はView/Elementのidentity、State共有、再構築時の保持規則を既に規定し、`docs/FRAME_FAILURES.md`もv1.0の失敗・回復契約を定義している。これらを再設計せず、Scene/Windowの宣言と実体の寿命、起動・open/close、公開export/feature、拡張traitの互換性を具体的に整理する。
- この段階でエージェントが`scarlet-sdk`をnativeユーザーライブラリと取り違え、Handle所有権・mapping・GPU/SWS等の監査を進めてしまった。後に承認を得たunsafe修正とnative API文書化は別作業の実績として扱い、SDK本体の契約整理を済ませたとは扱わない。

release scope / roadmapをこの作業対象と確認済み状態に更新した。この更新は文書のみで、APIの保証範囲を新たに確定したり、コード変更やruntime再検証を行ったりしていない。

## native API / ScarletUIの現行契約とunsafe境界修正 — 2026-09-06

ユーザーは「現行をそのまま規格化できる部分は進め、判断が必要なら知らせる」
方針を承認した。監査で、生syscall・任意mapping解除・生control等がsafeな関数
として公開されている点を報告し、unsafe境界へ移す修正について追加承認を受けた。

### 完了した変更

- Scarlet `326b1d04`（メッセージ訂正前`10e4db66`）: [native API契約](1.0-native-api-contract.md)を追加。
  `scarlet-abi` / `scarlet-sys` / `scarlet-os` / runtime / legacy / clientの役割、
  feature構成、Handleの成功・失敗時の所有権、mapping寿命、部分成功・readiness、
  GPU完了とSWS leaseの独立性を現行実装に沿って規定した。
- 同コミットで`syscall0..6`、map/unmap、生Handle/VM/vCPU control、break/TLS設定、
  生clone、guest-memory登録、event登録/returnを明示的なunsafe境界にした。
  公開入口にSafety要件を追加し、型付きnative API、通常std/legacyアプリ、GPU/SWS/
  audio/videoの呼び出し側を追従させた。syscall番号、引数ABI、GPU/SWSのrecordや
  wire、kernel実装は変更していない。
- `SharedMemory::from_handle` / `Socket::from_handle`の「失敗時は消費しない」
  説明を訂正。実装は元から所有Handleを消費し、失敗時にもDrop/closeしていた。
  event-demoのハンドラではatomicフラグだけを更新し、表示と終了を通常ループへ
  移した。native API Rustdocの既存リンク切れ3件も修正した。
- ScarletUI `50eea74d`: [アプリ・拡張契約](../../../scarlet-ui/docs/1.0-contract.md)
  を追加。公開export/feature、Scene宣言とWindow実体、起動時の最初の対象1窓、
  openの重複抑止、newの別identity、dismissの全同一キー対象とveto回避、
  WindowGroupのkey/launch規則、platform/paint拡張責任を現行のまま明文化。
  既存ARCHITECTURE / FRAME_FAILURESは維持し、対応Rustdocと4件の回帰テストを追加。
- SGFX `ec744ae`: legacy VirGLの時計取得1箇所を新しい生syscall署名へ追従。
- Chromebook `fcff35b`: Adrenoのmap/unmap 4箇所とlegacy時計取得1箇所のみ追従。
  **A618の非同期化・submit-wire変更・実機作業ではない。保留方針は維持した。**

### 今回の確認と限界

| 確認 | 結果 |
| --- | --- |
| `scarlet-std-bin --bins` | AArch64 / RISC-Vとも成功 |
| `userprogram --bins`（legacy native API消費側） | 両arch成功。今回は組込みScarlet targetでの確認であり、legacy JSON target全体の再検証ではない |
| `video_player --bins` / `scarlet-websocket-demo --bins` | それぞれ独立したfeature構成で両arch成功 |
| `scarlet-sys` host Rustdoc | unsafeなしの呼び出しを拒否するcompile-fail **7件成功**。syscallは実行しない |
| `scarlet-os` AArch64 Rustdocの`memory_mapping` / `control`フィルタ | compile-fail **5件成功**（mapping 4 + control 1）、別途既存例1件のcompile-only成功 |
| ScarletUI `application::tests`、1 thread | **17 passed / 0 failed / 0 ignored / 299 filtered**。新規4件を含む。全CoreテストやGPU実機テストの再実行ではない |
| `scarlet-sys` AArch64 strict Clippy | 成功 |
| `scarlet-os` AArch64 Rustdoc、`RUSTDOCFLAGS='-D warnings'` | 成功 |
| 変更Rustファイルのrustfmt / 各repoのdiff whitespace | 成功 |

上のconsumer確認は、既存のScarlet→兄弟SGFX/UI patchに加え、**Adrenoの兄弟
checkoutをCLI patchで選び、一時Cargo.lockを使用したローカル整合確認**である。
公開Gitだけの元lockで通ったとは報告しない。元lockの旧Adrenoはsafe呼び出しの
ままなので、新native APIとの通常ビルドには依存追従が必要。ユーザーの既存lock差分と
stageを保持するため、通常設定へのAdreno patch追加と該当lock追従は別途確認中。

確認コマンドの形（各package / targetを独立して指定）:

```sh
cargo check --manifest-path .cargo/Cargo.toml --offline \
  --lockfile-path <temporary-directory>/Cargo.lock -Z unstable-options \
  --config 'patch."https://github.com/petitstrawberry/scarlet-project-chromebook".sgfx-backend-scarlet-adreno.path="<absolute-adreno-crate-path>"' \
  -p <package> --bins --target <target> --keep-going
```

packageは`scarlet-std-bin` / `userprogram` / `video_player` /
`scarlet-websocket-demo`、targetは`aarch64-unknown-scarlet` /
`riscv64gc-unknown-scarlet`。一時lockは元の`.cargo/Cargo.lock`を基にCLI patchの
source変更を解決したもの。ソースコピー・worktree・新しい専用taskは作っていない。

失敗した試行も成功扱いにしない:

- stdとlegacyを同じCargo呼び出しへ混ぜた試行は`panic_impl`重複等で失敗。
  依存feature構成を分けて上表の消費側確認を完了した。混在をサポートしたわけではない。
- `scarlet-os`のhost doctestビルドは既存event trampolineのELF `.type` directiveが
  Darwin assemblerに拒否された。環境やtrampolineを改造せず、AArch64 Scarlet
  targetでcompile-fail / compile-only例を確認した。
- `scarlet-os` strict Clippyは**既存の`result_unit_err` 19件と
  `PollOptions::new`の`new_without_default` 1件**で失敗。エラー型の置換や一括lint
  無効化はしていない。今回の契約は現行`Result<_, ()>`も明記して維持したため、
  release時のlint dispositionとして残す。UI testにも既存23 warningsと
  preview-demoの重複target警告が残る。

native APIの全機能安全性、故障/reset、実機の資源解放を今回のコンパイルだけで証明した
とは扱わない。Boxcraftはユーザー確認済みの完了状態を維持し、QEMU/GUI/Dockerや
kernel全体の検証は再開していない。ABI/wire fixtureの既存gateも今回再実行していない。

現行native API/ScarletUIの契約文書化と承認されたunsafe修正はローカルコミット済み。
これは別repoの`scarlet-sdk`の契約整理ではない。
通常開発graphの依存追従、公開sourceによる候補lock固定、残る適合性・lint整理、
release notes・版上げ・RC公開は別段階。push、issue投稿、タグ作成は行っていない。

## ローカル依存追従の確認（配布構成の完了ではない） — 2026-09-06

上記の依存追従についてユーザーの追加承認を受け、`.cargo/Cargo.toml`へ兄弟
Chromebook checkoutの`sgfx-backend-scarlet-adreno` patchを追加した。既存の
Scarlet / SGFX / ScarletUI patchは維持し、一時lock・CLI patchを不要にした。

`.cargo/Cargo.lock`の今回の変更は、Adreno backend / codegenと
`adreno-a6xx-{layout,pm4,shader-pack,submit-wire}`の計6 packageについて、旧Git
source行を取り除いてpath参照にすることだけ。package版・依存一覧は変えていない。
既存stageのblobは`9d61059159d7e7af85a8c25d41f99b6e18376548`のままで、今回の
6行削除だけを未stage差分として保持した。ユーザーのproject lockも変更していない。

通常の設定・lockを使い、各package / targetを独立したCargo呼び出しで確認した:

```sh
cargo check --manifest-path .cargo/Cargo.toml --locked --offline \
  -p <package> --bins --target <target>
```

- `scarlet-std-bin`、`userprogram`、`video_player`、`scarlet-websocket-demo`の
  全4 packageを`aarch64-unknown-scarlet` / `riscv64gc-unknown-scarlet`で確認し、
  **8構成すべて成功**。一時lockやCLI source overrideは使用していない。
- std / legacyそれぞれの両targetで`cargo tree --locked --offline --invert sgfx-core`
  を確認し、全4構成で同じ兄弟checkoutのcore一つに解決。Adrenoも兄弟checkoutを参照。
- 既存stageの同一性、今回のlock差分が6 source行だけであること、project lockの
  内容保持、変更ファイルのdiff whitespaceを確認した。

これは通常の**ローカル開発graphでのcompile check**の完了であり、公開Gitだけの
候補lock検証やリンク・実機実行の代用ではない。上記の既存warnings / Clippy残件は
維持する。A618機能・実機作業、Boxcraft再検証、push・タグ・版上げは行っていない。

## 誤認・ローカル設定混入の撤回 — 2026-09-06

ユーザーからSDKの誤認とローカルpatchのコミット混入を指摘され、撤回と
不要なSGFX smokeの削除を指示された。

- `1.0-sdk-contract.md`を`1.0-native-api-contract.md`へ改名し、scope / roadmap /
  本書からSDK本体の契約が完了したという扱いを撤回した。`scarlet-sdk`のCLI、
  manifest / bundle、source / lock、image / plugin契約は未完として区別する。
  承認済みのnative API unsafe修正、ScarletUIの契約・回帰テストは取り消していない。
- Scarletの追跡設定からScarletUI 7件、SGFX 5件、Adreno 1件の兄弟path patchを
  撤去した。手元の開発用設定だけを未stage差分として保持し、既存stageを維持した。
  同じrepo内で完結するScarlet自身のpatchと、公開Gitのcrates.io置換は維持した。
- `sgfx-native-completion-smoke`のソースとbin宣言、experimental組込み、
  `projects/aarch64-sgfx-smoke`と`bundles/sgfx-smoke`の追跡ファイル、専用の
  image/run taskを削除した。ユーザーの既存full-project lockからも該当layerだけ
  除去し、残りの差分は保持した。生成済みのローカル画像・ログは削除していない。
- 過去の検証結果は歴史的な根拠として残し、削除したfixtureへの現役リンク・
  インストール指示・再実行要求は除去した。別のsmoke環境を作って置き換えない。

この撤回だけで公開依存構成が完成したとはしない。必要な外部修正の公開、
Cargo / project lockの整合、兄弟checkoutなしのNix / `cargo-scarlet`正規経路での
構築確認はまだ残る。既存のローカル8構成checkをその代用にしない。
