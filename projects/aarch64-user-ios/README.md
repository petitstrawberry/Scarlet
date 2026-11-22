# Scarlet on iOS

`bsp/` に iOS のホスト起動コード、`app/` に UIKit のエントリポイント、`tests/` に実行検証用のコードを置きます。

## Simulator で Scarlet を起動

Apple Silicon Mac の Scarlet 開発シェルで、リポジトリのルートから実行します。Simulator を起動しておきます。

```sh
app=$(projects/aarch64-user-ios/build.sh iphonesimulator)
env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun simctl install booted "$app"
env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun simctl launch \
  --terminate-running-process --console booted org.scarletos.ios
```

初回起動時にアプリは同梱の ext2 イメージを Application Support の `scarlet-root.ext2` にコピーします。以降の起動では同じファイルを `/dev/hostroot` として使います。

通常起動では Scarlet の共通ブート処理が同梱 initramfs をマウントします。Rust 製の `/init` が PID 1 で起動し、`/dev/tty0` を初期化してルートを ext2 に切り替えます。その後、`EnvironmentExec` で現在のアプリと一緒に署名された `/old_root/bin/boot-check` に切り替わります。検証プログラムはルート内のファイルを初回起動で作成して同期し、再起動後に値を読み戻します。結果は `SCARLET_ROOT_WRITTEN` または `SCARLET_ROOT_REOPENED` と `SCARLET_BOOT_PASS` で、UIKit の画面とホストのコンソールに表示されます。

ビルドでは `/init` と検証用 AArch64 PIE の ELF を `tools/ios-aot` で Mach-O フレームワークに変換して署名します。Scarlet は署名済みの RX を異なるアドレスへファイルマップし、インスタンスごとに DATA、BSS、GOT、静的 TLS、スタックを初期化します。同じ枠を再利用するときは RX を保持し、可変ページを作り直します。

## スケジューラとプロセス起動の回帰検証

```sh
env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun simctl launch \
  --terminate-running-process --console booted org.scarletos.ios --scheduler-check
```

アプリは 2 つのゲストで再配置された関数ポインタ、BSS、TLS の独立性を確認します。2 つのループはシステムコールを使わずに互いの進行を 4 回確認し、片方が Scarlet の `Sleep` で待機中にもう片方が進みます。結果はアプリ画面とコンソールに `SCARLET_AOT_PASS` または `SCARLET_AOT_FAIL` と表示されます。

Scarlet の exec は、VFS から読んだ ELF の SHA-256 で同梱イメージを選びます。既存の exec トランザクションで新しいアドレス空間と引数スタックを準備し、`AT_PAGESZ` に 16 KiB を設定します。検証では準備失敗後も元のゲストが実行でき、ゲスト所有のスタックへの書込みが通り、アプリ側アドレスへの書込みが拒否されることを確認します。

ゲストからの `EnvironmentSpawn` と `Waitpid` も検証します。子は同じ ELF の別インスタンスで起動し、親は子の終了コードを受け取ります。iOS でのプロセス生成には `EnvironmentSpawn` を使います。

`brk` と `sbrk` はインスタンスごとに予約した最大 8 MiB の範囲で、必要な 16 KiB ページを割り当てます。検証では縮小後の範囲への usercopy が拒否され、再拡張したページがゼロで初期化されることを確認します。

現在の AOT 入力は 16 KiB ページで分離された 1 個の RX セグメントを持つ自己完結の PIE です。再配置は `R_AARCH64_RELATIVE` に対応します。動的インタープリタ、依存ライブラリ、初期化関数テーブルには追加のローダー処理が必要です。

## ネイティブの init をパッケージ化

アプリのビルドは、予約レジスタを守るように `core` と `alloc` も含めて `user/bin` の `init` をビルドします。この工程だけを実行する場合は Scarlet 開発シェルで次を実行します。

```sh
RUSTFLAGS='-C relocation-model=pic -Zfixed-x18 --cfg getrandom_backend="custom"' \
  cargo build --locked --release --target aarch64-unknown-scarlet \
  -Zbuild-std=core,alloc,compiler_builtins -Zbuild-std-features=compiler-builtins-mem \
  --manifest-path user/bin/Cargo.toml --target-dir .build/scarlet-ios/native-init \
  --no-default-features --features ios-init-pie --bin init
cargo run --locked --manifest-path tools/ios-aot/Cargo.toml \
  --target-dir .build/scarlet-ios/rust -- \
  .build/scarlet-ios/native-init/aarch64-unknown-scarlet/release/init \
  .build/scarlet-ios/native-init/payload
```

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
