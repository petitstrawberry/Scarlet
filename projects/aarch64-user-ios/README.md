# Scarlet on iOS

`bsp/` に iOS のホスト起動コード、`app/` に UIKit のエントリポイント、`tests/` に実行検証用のコードを置きます。アプリは Scarlet を CPU0 で起動し、2 つの署名済み AArch64 ゲストを同じ carrier pthread で実行します。

## Simulator で Scarlet を起動

Apple Silicon Mac の Scarlet 開発シェルで、リポジトリのルートから実行します。Simulator を起動しておきます。

```sh
app=$(projects/aarch64-user-ios/build.sh iphonesimulator)
env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun simctl install booted "$app"
env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun simctl launch \
  --terminate-running-process --console booted org.scarletos.ios
```

ビルドはテスト用の AArch64 PIE を ELF としてリンクし、`tools/ios-aot` で Mach-O フレームワークに変換して署名します。Scarlet は署名済みの RX を異なるアドレスへファイルマップし、インスタンスごとに DATA、BSS、GOT、静的 TLS、スタックを初期化します。同じ枠を再利用するときは RX を保持し、可変ページを作り直します。

アプリは 2 つのゲストで再配置された関数ポインタ、BSS、TLS の独立性を確認します。2 つのループはシステムコールを使わずに互いの進行を 4 回確認し、片方が Scarlet の `Sleep` で待機中にもう片方が進みます。結果はアプリ画面とコンソールに `SCARLET_AOT_PASS` または `SCARLET_AOT_FAIL` と表示されます。

現在の AOT 入力は 16 KiB ページで分離された 1 個の RX セグメントを持つ自己完結の PIE です。再配置は `R_AARCH64_RELATIVE` に対応します。動的インタープリタ、依存ライブラリ、初期化関数テーブルには追加のローダー処理が必要です。

## 署名済みコードの再配置を検証

`tests/native-execution/` は iOS 実機で署名済み AArch64 コードの再配置と実行を確認する独立した検証です。位置独立のアセンブリを Mach-O フレームワークとしてビルドし、dyld で読み込んだファイルの RX セグメントを 2 つの仮想アドレスに配置します。それぞれに独立した DATA、BSS、関数ポインタ、TLS、ヒープ、ガード付きスタックを用意します。

検証は 1 本の carrier pthread で実行します。BRK とタイマーによる停止から復帰し、x0–x30、SP、PC、NZCV、Q0–Q31、FPCR、FPSR の保持を確認します。x18 はゲストの保存状態で仮想化し、物理レジスタは Darwin に明け渡します。2 つのゲストを交互に 32 回実行し、各タイマー区間でゲストのループが進み、切替中のゲスト VM 操作がゼロであることを確認します。その後、可変ページを解放して同じ RX 枠を使う起動を 64 回繰り返します。

### 実機での検証

Apple Silicon Mac、Xcode、ペアリング済みの iOS 端末、開発用署名 ID とプロビジョニングプロファイルが必要です。プロファイルには端末と使用するバンドル ID を含めます。検証アプリは既定で `org.scarletos.ios-p0` を使用します。端末のデベロッパモードを有効にし、インストールと起動の間はロックを解除します。[Apple の実機セットアップ](https://developer.apple.com/documentation/xcode/running-your-app-on-simulated-or-physical-devices)

リポジトリのルートで実行します。

```sh
app=$(PROVISIONING_PROFILE=/path/to/profile.mobileprovision \
      SIGNING_IDENTITY="Apple Development: Your Name (IDENTIFIER)" \
      projects/aarch64-user-ios/build.sh iphoneos --probe)

env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun devicectl device install app --device "$device" "$app"
env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun devicectl device process launch --device "$device" \
  --terminate-existing --console org.scarletos.ios-p0
```

`device` には `xcrun devicectl list devices` が表示する識別子を設定します。`BUNDLE_ID` を指定してビルドした場合は起動コマンドにも同じ ID を渡します。署名オプションを省略すると実機向けのビルドだけを確認できます。スクリプトは同じ開発用 ID でアプリとフレームワークを署名します。

起動時に検証が始まり、アプリ画面とコンソールに `SCARLET_P0_PASS` または `SCARLET_P0_FAIL` が表示されます。P0 の合格条件は、デバッガ未接続の実機でのコールド起動が成功し、`SCARLET_P0_PASS` が出ることです。失敗時は OS ビルド、コンソール出力、プロセスが終了した場合はクラッシュレポートを記録します。

### ローカルでの検証

```sh
out=$(projects/aarch64-user-ios/build.sh macosx)
"$out/scarlet-execution-probe" "$out/ScarletFixture.dylib"

app=$(projects/aarch64-user-ios/build.sh iphonesimulator --probe)
env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun simctl install booted "$app"
env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun simctl launch \
  --terminate-running-process --console booted org.scarletos.ios-p0
```

macOS と iOS Simulator は同じ検証コードを使用します。iOS の署名済み RX マッピングの可否は実機の結果で判定します。
