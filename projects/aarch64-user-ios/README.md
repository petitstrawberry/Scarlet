# Scarlet on iOS

`p0/` は iOS 実機で署名済み AArch64 コードの再配置と実行を確認する検証アプリです。位置独立のアセンブリを Mach-O フレームワークとしてビルドし、dyld で読み込んだファイルの RX セグメントを 2 つの仮想アドレスに配置します。それぞれに独立した DATA、BSS、関数ポインタ、TLS、ヒープ、ガード付きスタックを用意します。ELF からの AOT 変換は P2 の作業です。

検証は 1 本の carrier pthread で実行します。BRK とタイマーによる停止から復帰し、x0–x30、SP、PC、NZCV、Q0–Q31、FPCR、FPSR の保持を確認します。x18 はゲストの保存状態で仮想化し、物理レジスタは Darwin に明け渡します。2 つのゲストを交互に 32 回実行し、各タイマー区間でゲストのループが進み、切替中のゲスト VM 操作がゼロであることを確認します。その後、可変ページを解放して同じ RX 枠を使う起動を 64 回繰り返します。

## 実機での検証

Apple Silicon Mac、Xcode、ペアリング済みの iOS 端末、開発用署名 ID とプロビジョニングプロファイルが必要です。プロファイルには端末と使用するバンドル ID を含めます。ビルドスクリプトは既定で `org.scarletos.ios-p0` を使用します。端末のデベロッパモードを有効にし、インストールと起動の間はロックを解除します。[Apple の実機セットアップ](https://developer.apple.com/documentation/xcode/running-your-app-on-simulated-or-physical-devices)

リポジトリのルートで実行します。

```sh
app=$(PROVISIONING_PROFILE=/path/to/profile.mobileprovision \
      SIGNING_IDENTITY="Apple Development: Your Name (IDENTIFIER)" \
      projects/aarch64-user-ios/p0/build.sh iphoneos)

xcrun devicectl device install app --device "$device" "$app"
xcrun devicectl device process launch --device "$device" \
  --terminate-existing --console org.scarletos.ios-p0
```

`device` には `xcrun devicectl list devices` が表示する識別子を設定します。`BUNDLE_ID` を指定してビルドした場合は起動コマンドにも同じ ID を渡します。署名オプションを省略すると実機向けのビルドだけを確認できます。スクリプトは同じ開発用 ID でアプリとフレームワークを署名します。

起動時に検証が始まり、アプリ画面とコンソールに `SCARLET_P0_PASS` または `SCARLET_P0_FAIL` が表示されます。P0 の合格条件は、デバッガ未接続の実機でのコールド起動が成功し、`SCARLET_P0_PASS` が出ることです。失敗時は OS ビルド、コンソール出力、プロセスが終了した場合はクラッシュレポートを記録します。

## ローカルでの検証

```sh
out=$(projects/aarch64-user-ios/p0/build.sh macosx)
"$out/scarlet-p0" "$out/ScarletFixture.dylib"

app=$(projects/aarch64-user-ios/p0/build.sh iphonesimulator)
xcrun simctl install booted "$app"
xcrun simctl launch --terminate-running-process --console booted org.scarletos.ios-p0
```

macOS と iOS Simulator は同じ検証コードを使用します。iOS の署名済み RX マッピングの可否は実機の結果で判定します。
