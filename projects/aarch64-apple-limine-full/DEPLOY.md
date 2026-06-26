# Apple Silicon デプロイ手順

## 前提

- m1n1 がターゲット Mac にインストール済み（`kmutil configure-boot`）
- `csrutil disable` + `nvram boot-args=-v` でバックドア proxy mode 有効
- USB-C ケーブルでホスト接続
- nix develop 環境（pyserial, construct 含む）

## 1. U-Boot ビルド（Docker）

blkmap bootcmd 入りの U-Boot をビルド：

```bash
cd projects/aarch64-apple-limine-full
./build-uboot.sh j293
```

これは Docker 内で `make apple_m1_defconfig && make` を実行し、`u-boot-nodtb.bin` を `m1n1/payloads/` に配置後、`make-boot.py` で `boot-j293.bin` を再生成する。

## 2. Scarlet イメージビルド

```bash
cargo scarlet image --project projects/aarch64-apple-limine-full
```

出力：`.scarlet/images/limine-aarch64-apple-full.img`（208MB）

## 3. デプロイ（m1n1 HV モード）

ターゲット Mac を再起動 → m1n1 のバックドア proxy mode（5秒間）に入る。

デバイス名を確認：
```bash
ls /dev/cu.usbmodem*
# /dev/cu.usbmodemC02DN1XV0KPF1 = proxy
# /dev/cu.usbmodemC02DN1XV0KPF3 = vUART (UART capture)
```

デプロイ実行：
```bash
python3 projects/aarch64-apple-limine-full/tools/deploy_m1n1_usb.py \
  --no-build \
  --proxy-device /dev/cu.usbmodemC02DN1XV0KPF1 \
  --secondary-device /dev/cu.usbmodemC02DN1XV0KPF3 \
  --connect-timeout 60
```

`--no-build` を外せばイメージビルドから自動実行。

## フロー

```
deploy_m1n1_usb.py
  ├─ chainload.py -r m1n1.bin     (subprocess, 公式ツール)
  ├─ UartInterface() 新規接続      (run_guest.py と同じ)
  ├─ ProxyUtils(heap_size=128MB)
  ├─ hv.init()                    (HV 初期化, vUART マップ)
  ├─ hv.load_raw(boot-j293.bin)   (m1n1+DTB+U-Boot を HV ゲストとしてロード)
  ├─ writemem(0x900000000, image) (Limine FAT イメージを RAM に push)
  └─ hv.start()                   (ゲスト起動)
      → 内側 m1n1 (EL1) が U-Boot を起動
        → CONFIG_BOOTCOMMAND:
          blkmap create s; blkmap map s 0 0x200000 mem 0x900000000
          load blkmap 0:1 ... /EFI/BOOT/BOOTAA64.EFI
          bootefi
        → Limine → Scarlet kernel
```

## UART 操作

対話端末から実行した場合、`deploy_m1n1_usb.py` は tmux を起動し、HV/m1n1 操作用ペインと secondary UART の UART console ペインを左右に分ける。UART console ペインは picocom を `--omap crlf --imap lfcrlf -b 500000` 相当で起動する。

UART デバイスは chainload/reboot の途中で一度消えることがあるため、右ペインでは `deploy_m1n1_usb.py --uart-console-only` が親プロセスとして残り、picocom が終了しても再接続し続ける。止める場合は UART ペインで Ctrl-C を押す。

secondary UART が既に別プロセスで開かれている場合は、新しい picocom ペインを起動しない。従来通り stdout に UART を混ぜたい場合は `--no-tmux` を付ける。

`--uart-log file.log` で UART 出力をファイルにも保存可能。

## トラブルシューティング

### U-Boot が blkmap を実行しない

`boot-j293.bin` が古い可能性がある。U-Boot ビルド後に `make-boot.py` を再実行：

```bash
cd projects/aarch64-apple-limine-full/m1n1
python3 make-boot.py j293
```

### Mac が proxy mode に入らない

`nvram boot-args=-v` と `csrutil disable` が必要。1TR から実行。

### デバイス名が見つからない

macOS ではシリアル番号が含まれる：`/dev/cu.usbmodem<serial>1` (proxy), `/dev/cu.usbmodem<serial>3` (vUART)。`ls /dev/cu.usbmodem*` で確認。
