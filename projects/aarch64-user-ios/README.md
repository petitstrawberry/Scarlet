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

通常起動では Rust 製の `/init` が PID 1 で `/dev/tty0` を初期化し、ext2 へのルート切り替え後に `/bin/sh` を起動します。UIKit の `TTY input` 欄に `ps -l` と入力して Return を押すと、ゲストのプロセス一覧が表示されます。端末画面は UTF-8 とカーソル制御を処理し、改行前のプロンプトも表示します。

`--login` を起動引数に加えると、既存の `/bin/login` が端末を取得し、認証後に `fork` と `exec` でシェルを起動します。初期のコンソールアカウントは `root`、パスワードは空です。シェルで `exit` すると次のログインを受け付けます。

`/bin` と `/lib` は現在のアプリに同梱した initramfs を参照します。初期ディスクには BSP がリンクと権限 0600 の `/etc/scarlet-users` を用意します。`--boot-check` では検証用の共有ライブラリを呼び出し、ext2 のファイルを初回起動で作成して同期し、再起動後に値を読み戻します。成功時には `SCARLET_ROOT_WRITTEN` または `SCARLET_ROOT_REOPENED` に続いて `SCARLET_BOOT_PASS` が表示されます。

## スケジューラとプロセス起動の回帰検証

```sh
env -u DEVELOPER_DIR -u SDKROOT /usr/bin/xcrun simctl launch \
  --terminate-running-process --console booted org.scarletos.ios --scheduler-check
```

アプリは 2 つのゲストで再配置された関数ポインタ、BSS、TLS の独立性を確認します。2 つのループはシステムコールを使わずに互いの進行を 4 回確認し、片方が Scarlet の `Sleep` で待機中にもう片方が進みます。結果はアプリ画面とコンソールに `SCARLET_AOT_PASS` または `SCARLET_AOT_FAIL` と表示されます。

Scarlet の exec は、VFS から読んだ ELF の SHA-256 で同梱イメージを選びます。既存の exec トランザクションで新しいアドレス空間と引数スタックを準備し、`AT_PAGESZ` に 16 KiB を設定します。検証では準備失敗後も元のゲストが実行でき、ゲスト所有のスタックへの書込みが通り、アプリ側アドレスへの書込みが拒否されることを確認します。

ゲストからの `EnvironmentSpawn` と `Waitpid` も検証します。子は同じ ELF の別インスタンスで起動し、エントリ直後にプロセスグループとセッションを照会します。従来レコードによる継承、新規グループ、同一セッション内の既存グループへの参加、新規セッションを検証し、親は各子の終了コードを受け取ります。

`brk` と `sbrk` はインスタンスごとに予約した最大 8 MiB の範囲で、必要な 16 KiB ページを割り当てます。検証では縮小後の範囲への usercopy が拒否され、再拡張したページがゼロで初期化されることを確認します。

`Clone` による `fork` は同じゲストアドレスで親子を再開します。最初の fork でカーネル用の別名マッピングを確保し、子には Darwin のコピーオンライトで非実行ページを複製します。同じ実行アドレスを使う親子が交替するときに、そのプロセスのページを割り当て直します。子のスタック、ヒープ、DATA、TLS への書き込みが親に見えず、親が `Waitpid` で子の終了コードを読めることを検証します。子がパイプを読み待つ間に親が再開し、データを渡すケースも含みます。

現行の `sh` では、パイプライン内の組み込みコマンドの出力が端末に出る場合があります。シェルはパイプ端に標準出力の役割を設定しますが、組み込みの `echo` は引き続きハンドル 1 に書き込みます。

## 動的 ELF のパッケージ化

`tools/ios-aot` は静的 PIE のほか、`--lib-dir DIRECTORY` を指定すると `PT_INTERP=/bin/scarlet-ld` の Scarlet AArch64 PIE を処理します。パッケージ時に Scarlet の ELF ローダーと同じ `LoaderContext` を使い、`DT_NEEDED` を再帰的に読み、シンボルと再配置を解決します。検証用の `boot-check` は PLT 経由で `libboot-check.so` を呼び、同ライブラリは GOT 経由でデータを参照します。

exec 時は元の実行ファイルと依存する各 `/lib` ファイルの SHA-256 を照合し、署名済み RX とインスタンスごとの可変ページを配置します。依存ライブラリの初期化関数を呼んでから元のエントリに入ります。対象は 16 KiB のページ境界に合う配置と x18 を予約したコードです。実行中の `dlopen`、共有ライブラリの TLS、任意の `PT_INTERP` は未対応です。

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
