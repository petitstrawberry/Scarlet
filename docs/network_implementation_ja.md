# ネットワーク機能の実装

## 概要

本ドキュメントは、Scarletカーネルのネットワーク機能の設計と実装について説明します。この設計は、Unixドメインソケット的な機能を提供し、双方向通信とプロトコルスタック(TCP/IP)をサポートしながら、Scarletの既存のアーキテクチャパターンに従っています。

## 目標

1. **パターンの一貫性**: VfsManagerとDeviceManagerで確立された既存パターンに従う
2. **抽象化**: 異なるOS互換レイヤーで利用しやすいように、明確に定義された抽象化を提供
3. **双方向通信**: エンドポイント間の全二重通信を可能にする
4. **プロトコルスタックのサポート**: 将来のTCP/IPおよび他のプロトコル実装のための設計
5. **Unixドメインソケット**: ファイルシステムライクなパスを通じたローカルIPCを提供

## アーキテクチャ

### 高レベル設計

ネットワーク機能は、以下の3つの主要コンポーネントで構成されています：

1. **SocketObject**: ネットワークエンドポイントを表すKernelObjectタイプ
2. **NetworkManager**: ソケットのライフサイクルと接続を管理するグローバルマネージャー
3. **ソケットタイプ**: 異なるソケット実装(UnixDomain、TCP、UDPなど)

これはVFS設計をミラーリングしています：
- `SocketObject` ≈ `FileObject` (エンドポイントを表す)
- `NetworkManager` ≈ `VfsManager` (リソースを管理)
- ソケットタイプ ≈ ファイルシステムタイプ (異なる実装)

### コンポーネント構造

```
kernel/src/network/
├── mod.rs                    # モジュール定義とNetworkManager
├── socket.rs                 # SocketObjectトレイトと共通タイプ
├── unix_domain.rs            # Unixドメインソケット実装
├── protocol_stack.rs         # プロトコルスタック抽象化
└── syscall.rs               # ソケット関連システムコール
```

## 主要な型とトレイト

### SocketObjectトレイト

SocketObjectは、StreamIpcOpsを拡張し、ソケット固有の操作を提供します：

- ソケットタイプ、ドメイン、プロトコルの取得
- bind、connect、listen、acceptなどの接続操作
- sendto、recvfromなどのデータ転送操作
- getsockname、getpeernameなどのアドレス取得
- setsockopt、getsockoptなどのオプション設定
- shutdown操作とステート管理

### ソケットタイプと列挙型

**SocketType (ソケットタイプ)**:
- Stream: ストリームソケット(接続指向、信頼性あり)
- Datagram: データグラムソケット(非接続、信頼性なし)
- Raw: Rawソケット(プロトコル直接アクセス)
- SeqPacket: シーケンスパケットソケット

**SocketDomain (ソケットドメイン/アドレスファミリー)**:
- Unix: Unixドメインソケット(ローカルIPC)
- Inet: IPv4インターネットプロトコル
- Inet6: IPv6インターネットプロトコル
- Netlink: Netlinkソケット(カーネル-ユーザー通信)
- Packet: パケットソケット(低レベルパケットインターフェース)

**SocketProtocol (ソケットプロトコル)**:
- Default: ソケットタイプ/ドメインのデフォルトプロトコル
- Tcp: TCPプロトコル
- Udp: UDPプロトコル
- Icmp: ICMPプロトコル
- Raw: 特定の番号を持つRawプロトコル

### NetworkManager

NetworkManagerは、DeviceManagerとVfsManagerのパターンに従います：

**主要な機能**:
- ソケットの作成と管理
- Unixドメインソケットの名前空間管理(パス → リスニングソケット)
- アクティブなソケット接続の管理
- プロトコルスタックのレジストリ管理

**主要なメソッド**:
- `create_socket()`: 新しいソケットを作成
- `register_unix_socket()`: Unixドメインソケットをパスに登録
- `lookup_unix_socket()`: パスからUnixドメインソケットを検索
- `register_protocol_stack()`: プロトコルスタックを登録
- `get_protocol_stack()`: ドメインのプロトコルスタックを取得

## 実装の詳細

### Unixドメインソケット

Unixドメインソケットは、ファイルシステムライクなパスを通じてローカルIPCを提供します。

**主要な機能**:
- ストリーム指向(SOCK_STREAM)とデータグラム(SOCK_DGRAM)のサポート
- 双方向通信
- 接続指向ストリーム
- 非接続データグラム
- クレデンシャル渡し機能(将来)
- ファイルディスクリプタ渡し機能(将来)

**内部構造**:
- ソケット状態管理
- ローカルアドレス(パス)
- ピアアドレス
- リスニングソケット用の接続バックログ
- データバッファ
- ソケットオプション

### プロトコルスタックの抽象化

TCP/IPおよび他のプロトコルスタックをサポートするための抽象化：

**ProtocolStackトレイト**:
- プロトコルスタックドメインの取得
- ソケットの作成
- 受信パケットの処理
- 統計情報の取得

このトレイトにより、異なるプロトコルスタック(TCP/IP、UDPなど)を統一的に扱うことができます。

### KernelObjectとの統合

KernelObject enumにSocket variantを追加：

```rust
pub enum KernelObject {
    File(Arc<dyn FileObject>),
    Pipe(Arc<dyn PipeObject>),
    EventChannel(Arc<EventChannelObject>),
    EventSubscription(Arc<EventSubscriptionObject>),
    Socket(Arc<dyn SocketObject>),  // 新しいvariant
}
```

## システムコールインターフェース

### ソケット作成と管理

- `sys_socket()`: 新しいソケットを作成
- `sys_bind()`: ソケットをアドレスにバインド
- `sys_connect()`: リモートアドレスに接続
- `sys_listen()`: 接続を待ち受け
- `sys_accept()`: 接続を受け入れ

### データ転送

- `sys_sendto()`: ソケット経由でデータを送信
- `sys_recvfrom()`: ソケットからデータを受信

### ソケットオプションと制御

- `sys_getsockname()`: ローカルアドレスを取得
- `sys_getpeername()`: リモートアドレスを取得
- `sys_setsockopt()`: ソケットオプションを設定
- `sys_getsockopt()`: ソケットオプションを取得
- `sys_shutdown()`: ソケットをシャットダウン

## 既存システムとの統合

### VFS統合

Unixドメインソケットは、VFS経由でアクセスできます：

1. ファイルシステムにソケットノードを作成(例: `/tmp/socket.sock`)
2. NetworkManagerに登録
3. アプリケーションはファイルシステムパス経由で接続
4. VFSがパス解決を処理、NetworkManagerがソケット操作を処理

### IPCモジュール統合

SocketObjectはStreamIpcOpsを拡張し、以下を提供します：
- ストリームソケット用の`read()`/`write()`
- 既存のIPCインフラとの統合
- Selectableトレイト経由のselect/poll操作のサポート

### デバイス統合

ネットワークデバイスは物理/仮想レイヤーを提供します：
- NetworkDeviceトレイトは既に`kernel/src/device/network/`に存在
- プロトコルスタックがSocketObjectとNetworkDeviceをブリッジ
- パケットフロー: アプリケーション → ソケット → プロトコルスタック → ネットワークデバイス

## 使用例

### Unixドメインストリームソケット(サーバー)

```rust
// ソケットを作成
let socket = NetworkManager::get_manager()
    .create_socket(SocketDomain::Unix, SocketType::Stream, SocketProtocol::Default)?;

// パスにバインド
let addr = SocketAddress::Unix(UnixSocketAddress::from_path("/tmp/server.sock")?);
socket.as_socket().unwrap().bind(&addr)?;

// 接続を待ち受け
socket.as_socket().unwrap().listen(5)?;

// 接続を受け入れ
let client_socket = socket.as_socket().unwrap().accept()?;

// StreamOpsを使用してデータを読み書き
let mut buffer = vec![0u8; 1024];
let n = client_socket.as_stream().unwrap().read(&mut buffer)?;
```

### Unixドメインストリームソケット(クライアント)

```rust
// ソケットを作成
let socket = NetworkManager::get_manager()
    .create_socket(SocketDomain::Unix, SocketType::Stream, SocketProtocol::Default)?;

// サーバーに接続
let addr = SocketAddress::Unix(UnixSocketAddress::from_path("/tmp/server.sock")?);
socket.as_socket().unwrap().connect(&addr)?;

// StreamOpsを使用してデータを書き込み
let data = b"Hello, server!";
socket.as_stream().unwrap().write(data)?;
```

## テスト戦略

### ユニットテスト

各コンポーネントには包括的なユニットテストが必要です：

1. **ソケット作成**: 様々なパラメータでのソケット作成をテスト
2. **Unixドメインソケット**: bind、connect、listen、accept操作をテスト
3. **データ転送**: 双方向データ転送をテスト
4. **エラーハンドリング**: エラー条件(接続拒否など)をテスト
5. **ステート管理**: ソケットステート遷移をテスト

### 統合テスト

コンポーネント間の相互作用をテスト：

1. **VFS統合**: ファイルシステムパス経由のソケットアクセスをテスト
2. **IPC統合**: StreamOps実装をテスト
3. **マルチソケット**: 複数の同時接続をテスト
4. **並行アクセス**: スレッドセーフ操作をテスト

## 実装フェーズ

### フェーズ1: 基礎(現在)
- ✓ 設計ドキュメント
- [ ] SocketObjectトレイト定義
- [ ] NetworkManagerスケルトン
- [ ] KernelObject::Socket variant
- [ ] 基本エラー型

### フェーズ2: Unixドメインソケット
- [ ] UnixStreamSocket実装
- [ ] UnixDatagramSocket実装
- [ ] Unixソケットアドレス処理
- [ ] Bind/connect/listen/accept操作
- [ ] データ転送(read/write)
- [ ] ユニットテスト

### フェーズ3: システムコールインターフェース
- [ ] ソケットシステムコール実装
- [ ] Bind/connect/listen/accept syscalls
- [ ] Send/receive syscalls
- [ ] ソケットオプションsyscalls
- [ ] ハンドルテーブルとの統合

### フェーズ4: VFS統合
- [ ] ソケットファイルシステムノード
- [ ] パスベースのソケットアクセス
- [ ] パーミッションチェック
- [ ] クローズ時のソケットクリーンアップ

### フェーズ5: プロトコルスタック(将来)
- [ ] ProtocolStackトレイト実装
- [ ] TCP/IPスタック統合
- [ ] UDPソケット実装
- [ ] Rawソケットサポート

## セキュリティの考慮事項

1. **アドレス検証**: すべてのソケットアドレスと長さを検証
2. **バッファ境界**: データ転送におけるバッファオーバーフローを防止
3. **パーミッションチェック**: Unixソケットファイルのパーミッションを強制
4. **リソース制限**: ソケット数とバッファサイズに制限を実装
5. **接続制限**: バックログサイズと同時接続数を制限

## パフォーマンスの考慮事項

1. **ゼロコピー**: 可能な場合、大きなデータ転送に共有メモリを使用
2. **バッファ管理**: 効率的なリングバッファを実装
3. **ロック競合**: ホットパスでのロックスコープを最小化
4. **非同期操作**: ノンブロッキングと非同期操作をサポート
5. **コネクションプーリング**: ソケット構造を効率的に再利用

## 互換性に関する注意

### Linux ABI互換性
- ソケットシステムコールはLinuxセマンティクスに一致
- ソケットオプション値はLinuxと互換
- アドレス構造レイアウトはLinux sockaddrと一致

### xv6 ABI互換性
- xv6用の簡略化されたソケットインターフェース
- すべてのソケットオプションをサポートしない可能性
- コア機能に焦点

## 将来の拡張

1. **高度な機能**
   - クレデンシャル渡し(SCM_CREDENTIALS)
   - ファイルディスクリプタ渡し(SCM_RIGHTS)
   - 補助データサポート
   
2. **ネットワークプロトコル**
   - TCP/IPスタック実装
   - UDPソケットサポート
   - Rawソケットサポート
   - IPv6サポート
   
3. **パフォーマンス**
   - sendfileシステムコール
   - splice/tee操作
   - ゼロコピーネットワーキング
   
4. **高度なIPC**
   - UNIXソケットペア(socketpair)
   - 抽象名前空間ソケット
   - マルチキャストサポート

## 参考文献

- POSIX Socket API仕様
- Linux socket(7)およびunix(7)マニュアルページ
- Stevens, W. Richard. "Unix Network Programming"
- Scarlet VFS v2設計 (`kernel/src/fs/vfs_v2/`)
- Scarlet IPC設計 (`kernel/src/ipc/`)
- Scarlet Device Manager (`kernel/src/device/manager.rs`)

## 改訂履歴

- 2026-01-03: 初期設計ドキュメント
