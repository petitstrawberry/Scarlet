# ネットワーク機能の実装

## 概要

本ドキュメントは、Scarletカーネルのネットワーク機能の設計と実装について説明します。Scarletのgive agnostic思想（TTYデバイスと同様）に従い、コアは抽象的なソケットインフラを提供し、ABIモジュールが具体的な実装を提供します。

## 目標

1. **OS非依存設計**: コアは抽象化のみを提供（TTYデバイスと同様）
2. **パターンの一貫性**: 既存パターン（VfsManager、DeviceManager、TTY）に従う
3. **ABI柔軟性**: ファクトリパターンによる異なるOS互換レイヤーのサポート
4. **双方向通信**: エンドポイント間の全二重通信を可能にする
5. **プロトコルスタックのサポート**: TCP/IPおよび他のネットワークプロトコルの拡張可能な設計
6. **ローカルIPC**: ローカルプロセス間通信のサポート（Unixドメインソケット相当）

## 設計思想

**ScarletはUnixではありません。** TTYデバイスと同様に、ネットワーク実装は以下の原則に従います：

- **コア = 抽象化**: ScarletコアはOS中立のソケットインフラを提供
- **ABI = 実装**: ABIモジュール（Linux、xv6など）が具体的なソケット実装を提供
- **中立的な用語**: "Unix"ではなく"Local"を使用、ioctlの番号ではなくSCTL_SOCKET_*を使用
- **拡張性**: ファクトリパターン + プロトコルスタックサポートによる多様なユースケース

## アーキテクチャ

### 高レベル設計

ネットワーク機能は、以下の3つの主要コンポーネントで構成されています：

1. **SocketObject**: ネットワークエンドポイントの抽象トレイト（コアで定義）
2. **NetworkManager**: ソケットライフサイクルのグローバルマネージャー（登録、検索、接続追跡）
3. **ソケット実装**: ABIモジュールまたはプロトコルスタックが提供

これはVFSとTTYの両方の設計をミラーリングしています：
- VFSと同様: `SocketObject` ≈ `FileObject`、`NetworkManager` ≈ `VfsManager`
- TTYと同様: コアが`SocketObject`トレイトを定義、ABIが実装（`CharDevice` + `TtyControl`と同様）

### コンポーネント構造

```
kernel/src/network/
├── mod.rs                    # NetworkManagerとファクトリ登録
├── socket.rs                 # SocketObject/SocketControlトレイト、SCTL_SOCKET_*オペコード
└── protocol_stack.rs         # TCP/IP、UDPなどのプロトコルスタック抽象化
```

**注意**: コアには具体的なソケット実装はありません。ABIモジュールが提供します。

## 主要な型とトレイト

### SocketControlトレイト

TTYデバイスの`TtyControl`と同様に、`SocketControl`はOS中立のソケット操作を提供します：

- bind, connect, listen, acceptなどの接続管理操作
- getpeername, getsocknameなどのアドレス取得
- shutdownとステート管理

### SocketObjectトレイト

`StreamIpcOps`（データ転送）+ `SocketControl`（接続管理）+ `CloneOps`を組み合わせたもの：

完全なソケットインターフェース（`TtyDeviceEndpoint`に類似）

### Scarlet専用制御オペコード

TTYデバイスが`SCTL_TTY_*`を使用するのと同様に、ソケットは`SCTL_SOCKET_*`（マジック'SS' = 0x5353）を使用：

```rust
pub mod socket_ctl {
    pub const SCTL_SOCKET_BIND: u32 = 0x5353_0001;
    pub const SCTL_SOCKET_CONNECT: u32 = 0x5353_0002;
    pub const SCTL_SOCKET_LISTEN: u32 = 0x5353_0003;
    // ...
}
```

**ABIモジュールはOS固有のsyscalls/ioctlsをこれらのオペコードに変換します。**

### ソケットタイプと列挙型

**SocketDomain（ソケットドメイン）- OS非依存の用語**:
- Local: ローカルプロセス間通信（Unixに限定されない）
- Inet: IPv4インターネットプロトコル
- Inet6: IPv6インターネットプロトコル
- Packet: パケットレベル通信

注意: `LocalSocketAddress`はUnixドメインソケットアドレスのOS非依存バージョンです。

### NetworkManager

NetworkManagerは、DeviceManagerとVfsManagerのパターンに従い、ファクトリベースの設計を採用：

**主要な機能**:
- ドメインごとのソケットファクトリ登録（ABIモジュールによる）
- ネットワークプロトコル用のプロトコルスタック（TCP/IP、UDPなど）
- 名前付きソケットの名前空間（LocalIPC用）
- アクティブなソケット接続の管理

**主要なメソッド**:
- `register_socket_factory()`: ドメイン用のソケットファクトリを登録（ABIモジュールが呼び出す）
- `register_protocol_stack()`: プロトコルスタックを登録（ネットワークドライバまたはABIモジュールが呼び出す）
- `create_socket()`: 新しいソケットを作成（優先順位: ファクトリ → プロトコルスタック）
- `register_named_socket()`: 名前付きソケットを登録（LocalIPC用）
- `lookup_named_socket()`: 名前からソケットを検索
- `process_packet()`: 受信ネットワークパケットを処理

## 実装の詳細

### ABIモジュールの責務

ABIモジュール（Linux、xv6など）は以下を行う必要があります：

1. **SocketObjectの実装**: 特定のソケットタイプ用に
2. **ソケットファクトリの登録**: サポートするドメイン用に
3. **syscallの変換**: SCTL_SOCKET_*オペコードへ
4. **OS固有のセマンティクスの処理**: 例：Unixドメインソケットのパーミッション、クレデンシャル

### プロトコルスタックの抽象化

TCP/IP、UDPおよび他のネットワークプロトコル用：

**ProtocolStackトレイト**:
- プロトコルスタックドメインの取得
- このプロトコルスタック用のソケットの作成
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

## 実装状況

### フェーズ1: コアインフラ（完了 ✓）
- ✓ 設計ドキュメント（英語 + 日本語）
- ✓ SocketObjectとSocketControlトレイト
- ✓ ファクトリパターンを持つNetworkManager
- ✓ ProtocolStackトレイトとProtocolStackManager
- ✓ KernelObject::Socket variant
- ✓ SCTL_SOCKET_*制御オペコード
- ✓ SocketDomain、SocketType、SocketAddressタイプ
- ✓ ビルド統合とテスト

### フェーズ2: ABIモジュール実装（ABIメンテナー向け）
- [ ] Linux ABI: Localソケット（Unixドメインソケット相当）
- [ ] Linux ABI: システムコール変換（socket、bindなど）
- [ ] Linux ABI: アドレス構造変換
- [ ] xv6 ABI: ソケットサポート（必要に応じて）
- [ ] ソケット実装用のユニットテスト

### フェーズ3: プロトコルスタック実装（将来）
- [ ] TCP/IPプロトコルスタック
- [ ] UDPソケットサポート
- [ ] IPv4/IPv6アドレス処理
- [ ] パケットルーティングと処理
- [ ] ネットワークデバイス統合

### フェーズ4: 高度な機能（将来）
- [ ] 名前付きソケット用のVFS統合
- [ ] クレデンシャル渡し（SCM_CREDENTIALS）
- [ ] ファイルディスクリプタ渡し（SCM_RIGHTS）
- [ ] 補助データサポート
- [ ] Rawソケットサポート

## セキュリティの考慮事項

**ABIの実装者向け**:

1. **アドレス検証**: ユーザー空間からのすべてのソケットアドレスと長さを検証
2. **バッファ境界**: データコピー時のバッファオーバーフローを防止
3. **パーミッションチェック**: OS固有のパーミッションチェックを実装（例：Localソケットのファイルシステムパーミッション）
4. **リソース制限**: ソケット数、バッファサイズ、接続バックログの制限を強制
5. **クレデンシャル検証**: 特権操作のユーザークレデンシャルを検証

**コアインフラ**:

- NetworkManagerは適切なロックを使用した内部可変性を使用
- ソケットファクトリとプロトコルスタックは初期化時のみ登録
- ユーザー制御の関数ポインタなし

## パフォーマンスの考慮事項

**コア設計**:

1. **ロックフリーなルックアップ**: 読み取り重視の操作にRwLockを使用
2. **最小限のアロケーション**: ソケットIDの再利用、名前付きソケット用のweak参照
3. **ゼロコピーの可能性**: StreamOpsインターフェースによるバッファ共有が可能
4. **2段階作成**: まずファクトリを試行（高速パス）、次にプロトコルスタック

**ABIの実装者向け**:

1. **効率的なバッファ**: ソケットデータ用にリングバッファなどを使用
2. **非同期サポート**: SCTL_SOCKET_SET_NONBLOCKによる非ブロッキングモードを実装
3. **コネクションプーリング**: 可能な場合はソケット構造を再利用
4. **Select/Poll**: 効率的なI/O多重化のためのSelectableトレイトを実装
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
