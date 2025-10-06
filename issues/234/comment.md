---
## 詳細設計・計画 — 「ABIゾーン」管理機構の実装

### アーキテクチャ
TaskはBox<dyn AbiModule>としてABIインスタンスの所有権を持つ。ABIゾーンも同様に、AbiRegistryから生成されたBox<dyn AbiModule>インスタンスを所有する。カーネルはシステムコール時にsepcに基づき、Taskのゾーンマップから適用すべき&mut dyn AbiModuleへの参照を動的に解決する。

### Part 1: データ構造の変更 (既存実装への統合)
#### 1. Task構造体の拡張 (kernel/src/task/mod.rs)
abiフィールドをdefault_abiに改名し、ABIゾーンを管理するためのBTreeMapを追加します。

```rust
// in kernel/src/task/mod.rs
use crate::abi::AbiModule;
use alloc::{collections::BTreeMap, boxed::Box};
use core::ops::Range;

// ABIゾーンを表す構造体。所有権を持つBoxを保持する。
pub struct AbiZone {
    pub range: Range<usize>,
    pub abi: Box<dyn AbiModule + Send + Sync>,
}

pub struct Task {
    // ... 既存のフィールド ...

    // 'pub abi: Option<Box<dyn AbiModule>>' を以下のように変更
    /// このタスクのデフォルトABI。ELFのOSABIなどから決定される。
    pub default_abi: Box<dyn AbiModule + Send + Sync>,

    /// 特定のメモリ範囲に適用されるABIゾーンのマップ。
    /// キーは範囲の開始アドレス。
    pub abi_zones: BTreeMap<usize, AbiZone>,
}

impl Task {
    // ...

    /// 指定されたアドレスに適用されるABIへの可変参照を解決する。
    /// handle_syscall(&mut self)を呼び出すために可変参照が必要。
pub fn resolve_abi_mut(&mut self, addr: usize) -> &mut (dyn AbiModule + Send + Sync) {
        // addrを含むゾーンを効率的に検索
        if let Some((_start, zone)) = self.abi_zones.range_mut(..=addr).next_back() {
            if zone.range.contains(&addr) {
                return zone.abi.as_mut();
            }
        }
        // ゾーンが見つからなければデフォルトABIを返す
        self.default_abi.as_mut()
    }
}
```

#### 2. 新規システムコールの定義 (kernel/src/abi/scarlet.rs)
Scarlet Native ABIにABIゾーンを管理するシステムコールを追加します。ABIの指定には、あなたのAbiRegistryが使う文字列名（へのポインタ）を使います。
 * SYS_REGISTER_ABI_ZONE: `sys_register_abi_zone(start: usize, len: usize, abi_name_ptr: *const u8) -> Result<()>`
 * SYS_UNREGISTER_ABI_ZONE: `sys_unregister_abi_zone(start: usize) -> Result<()>`

### Part 2: 実装タスクと手順 (既存コードの活用)
#### ステップ 1: abi/mod.rsの微修正 (必要であれば)
 * AbiModuleトレイトに`clone_boxed`がすでにあるので、これを活用します。もし各ABI構造体（例: ScarletAbi）が`#[derive(Clone)]`を実装していない場合、`clone_boxed`を手動で実装する必要があります。

```rust
// 例: scarlet.rs
impl AbiModule for ScarletAbi {
    // ...
    fn clone_boxed(&self) -> Box<dyn AbiModule> {
        Box::new(self.clone()) // selfがClone可能である必要がある
    }
    // ...
}
```

#### ステップ 2: task/mod.rsの修正
 * 上記のAbiZone構造体を定義し、Task構造体を修正します（abi -> default_abi, abi_zones追加）。
 * `resolve_abi_mut`メソッドを実装します。`BTreeMap::range_mut`を使うことで、効率的かつ安全に可変参照を取得できます。
 * `new_user_task`などのタスク生成部分を修正します。`AbiRegistry::instantiate("scarlet")`などを呼び出して`default_abi`を初期化し、`abi_zones`を空の`BTreeMap`で初期化します。

#### ステップ 3: システムコールディスパッチャの改造 (kernel/src/abi/mod.rs or syscall/mod.rs)
あなたの`syscall_dispatcher`関数を、この新しいアーキテクチャに合わせて改造します。

```rust
// in kernel/src/abi/mod.rs
pub fn syscall_dispatcher(trapframe: &mut Trapframe) -> Result<usize, &'static str> {
    // 1. プログラムカウンタ(sepc)を取得
    let pc = trapframe.sepc; 

    // 2. 現在のタスクへの可変参照を取得
    let task = mytask().unwrap();
    
    // 3. 適用すべきABIへの可変参照を動的に解決
    let abi_module = task.resolve_abi_mut(pc);

    // 4. 解決したABIでシステムコールを処理
    abi_module.handle_syscall(trapframe)
}
```

#### ステップ 4: 新規システムコールの実装 (kernel/src/abi/scarlet.rs)
Scarlet Native ABIの`handle_syscall`内に、新しいシステムコールのロジックを追加します。

```rust
// in kernel/src/abi/scarlet.rs's handle_syscall
// (擬似コード)
match syscall_id {
    // ...
    SYS_REGISTER_ABI_ZONE => {
        let start = trapframe.get_arg(0);
        let len = trapframe.get_arg(1);
        let name_ptr = trapframe.get_arg(2) as *const u8;
        
        // ユーザー空間からABI名文字列を安全にコピー
        let abi_name = copy_string_from_user(name_ptr)?; 
        
        // あなたのAbiRegistryを使って新しいインスタンスを生成
        if let Some(new_abi) = AbiRegistry::instantiate(&abi_name) {
            let current_task = mytask().unwrap();
            let new_zone = AbiZone {
                range: start..(start + len),
                abi: new_abi,
            };
            current_task.abi_zones.insert(start, new_zone);
            Ok(0) // 成功
        } else {
            Err("ABI not found")
        }
    },
    // ...
}
```

### Part 3: 初期テスト計画
これは前回と同様ですが、システムコールの引数が`abi_id`ではなく`abi_name_ptr`になる点が異なります。テストプログラムは、登録したいABIの名前（例: "wasi"）を文字列としてカーネルに渡します。

この計画であれば、あなたの既存のAbiRegistryと`Box<dyn AbiModule>`による所有権モデルを完全に活用し、その上に新機能をアドオンする形で、クリーンに実装を進めることができます.
