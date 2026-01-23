# ScarletUI 2.0 Architecture Design Document

## 1. Overview (概要)

ScarletUI 2.0は、Rustの厳格な型システムと所有権モデルに最適化された、宣言的UIフレームワークのアーキテクチャです。
FlutterやSwiftUIの宣言的な書き心地を維持しつつ、Rust特有の「動的ディスパッチ (`dyn Trait`)」と「ジェネリクス」の摩擦を解消するために、**Factory Methodパターン**と**Component/Render分離モデル**を採用しています。

### Core Philosophy

1. **View is a Factory**: Viewは単なるデータではなく、「自分のElementを製造する責任」を持つ。
2. **Explicit State Wiring**: Stateの依存関係はマクロによって静的に収集され、実行時に安全に配線される。
3. **Split Responsibilities**: 「構成管理 (Component)」と「描画管理 (Render)」を明確に分離する。

---

## 2. High-Level Architecture (全体構造)

データフローとオブジェクトの所有関係を示す全体図です。

```ascii
+----------------------+        +-------------------------+
|      User Code       |        |    Framework Core       |
+----------------------+        +-------------------------+
|                      |        |                         |
|  struct CounterView  | build  |   +-----------------+   |
|  (View / Factory)    |------->|   | ComponentElement|   |
|    - count: State    |        |   | (Manager)       |   |
|          |           |        |   +--------+--------+   |
|          |           |        |            | owns       |
+----------|-----------+        |            v            |
           | owns               |   +-----------------+   |
           v                    |   |  RenderElement  |   |
+----------------------+        |   | (Leaf Node)     |   |
|      State<T>        |        |   +--------+--------+   |
|    (Listenable)      |        |            | owns       |
|    - value: T        |<-------|            v            |
|    - subscribers     | notify |   +-----------------+   |
+----------------------+        |   |  RenderObject   |   |
                                |   | (Layout/Paint)  |   |
                                |   +-----------------+   |
                                |                         |
                                +-------------------------+

```

### Key Relationships

* **View → Element**: 1対1の関係。ViewはElementの「設計図」であり、`create_element()` メソッドを通じてElementを生成します。
* **Element → Element**: 親子関係（Tree構造）。`ComponentElement` は論理的な親となり、`RenderElement` は物理的な描画ノードとなります。
* **State → Element**: N対Nの購読関係。Stateが更新されると、それを購読しているElementがダーティとマークされ、再ビルドがトリガーされます。

---

### 2.1 Core Data Structures Overview (主要データ構造の概要)

| レイヤー | データ型 | 役割 | 所有するもの | ライフサイクル |
|---------|---------|------|------------|--------------|
| **View** | `struct CounterView` | 不変の設計図、Factory | `State<T>` | 作成後は不変 |
| **State** | `State<T>` | 状態の保持と通知 | `Arc<StateInner>` | 複数の所有者で共有 |
| **Element** | `ComponentElement<V>` | Viewの構成を管理 | 子Element、購読ID | State変化で再ビルド |
| **Element** | `RenderElement<V,R>` | RenderObjectのラッパー | RenderObject、子Element | View更新で再構成 |
| **RenderObject** | `TextRenderObject` | レイアウトと描画を担当 | `Buffer`（Leafのみ） | ダーティフラグで更新 |
| **Buffer** | `Buffer` | ピクセルデータを保持 | `Vec<u32>` (BGRA) | 描画時に再生成 |

### 2.2 Type Relationship Diagram (型関係図)

```ascii
┌─────────────────────────────────────────────────────────────────────┐
│                        TYPE HIERARCHY                                │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌────────────────┐                                                 │
│  │    View        │  ◄─── Trait (Factory)                          │
│  │  - create_el() │                                                 │
│  └───────┬────────┘                                                 │
│          │ implements                                                │
│          ▼                                                          │
│  ┌────────────────────────────────────────────────┐                 │
│  │  User Views                 Primitive Views  │                 │
│  │  ┌──────────────┐        ┌──────────────────┐│                 │
│  │  │CounterView   │        │    Text          ││                 │
│  │  │  : View      │        │  : View          ││                 │
│  │  └──────┬───────┘        └────────┬─────────┘│                 │
│  └─────────┼─────────────────────────┼───────────┘                 │
│            │ create_element()         │                             │
│            ▼                         ▼                             │
│  ┌──────────────────┐    ┌───────────────────────┐                 │
│  │ComponentElement  │    │   RenderElement<V,R>  │                 │
│  │  : Element       │    │     : Element         │                 │
│  │                  │    │                       │                 │
│  │  owns:           │    │  owns:                │                 │
│  │  - View          │    │  - View               │                 │
│  │  - child Element │    │  - RenderObject       │                 │
│  │  - subscriptions │    │  - children Vec<>     │                 │
│  └──────────────────┘    └───────────┬───────────┘                 │
│                                      │                             │
│                                      │ owns                        │
│                                      ▼                             │
│                          ┌───────────────────────┐                 │
│                          │    RenderObject       │                 │
│                          │  ◄─── Trait           │                 │
│                          │                       │                 │
│                          │  - layout()           │                 │
│                          │  - render()           │                 │
│                          │  - hit_test()         │                 │
│                          │  - get_buffer()       │                 │
│                          └───────────────────────┘                 │
│                                  │                                   │
│                ┌─────────────────┴─────────────────┐                │
│                ▼                                   ▼                │
│        ┌───────────────┐                ┌────────────────┐         │
│        │Leaf Objects   │                │Container Obj.  │         │
│        │               │                │                │         │
│        │- TextRO       │                │- VStackRO      │         │
│        │- ImageRO      │                │- HStackRO      │         │
│        │- ButtonRO     │                │- ZStackRO      │         │
│        │               │                │                │         │
│        │owns: Buffer   │                │owns: children  │         │
│        └───────────────┘                └────────────────┘         │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

### 2.3 Ownership & Borrowing Pattern (所有権と借用のパターン)

```rust
// === VIEW LAYER (Immutable) ===
struct CounterView {
    count: State<i32>,  // Arcでクローン可能、参照として借用
}

// === ELEMENT LAYER (Mutable Owner) ===
struct ComponentElement<V: View> {
    view: V,                    // Viewを所有（不変設計図として保持）
    child: Option<Box<dyn Element>>,  // 子Elementを所有
    subscriptions: Vec<SubscriptionId>,  // State購読を管理
}

struct RenderElement<V: View, R: RenderObject> {
    view: V,                    // Viewを所有
    render_object: R,           // RenderObjectを所有
    children: Vec<Box<dyn Element>>,  // 子Elementを所有
}

// === RENDER OBJECT LAYER (Mutable State) ===
struct TextRenderObject {
    view: Text,                 // 最新のViewを保持
    buffer: Option<Buffer>,     // 描画バッファを所有（Leafのみ）
    frame: Rect,                // レイアウト結果を保持
    dirty_flags: DirtyFlags,    // ダーティ状態を管理
}

// === STATE (Shared Ownership) ===
struct State<T> {
    inner: Arc<StateInner<T>>,  // 複数の所有者で共有
}
```

### 2.4 Data Flow: State to Screen (データフロー)

```ascii
┌────────────────────────────────────────────────────────────────────┐
│                    STATE UPDATE → SCREEN RENDER                    │
├────────────────────────────────────────────────────────────────────┤
│                                                                    │
│  1. USER MUTATES STATE                                            │
│     ┌─────────────────┐                                           │
│     │ counter.update  │ ──► State<T>                               │
│     │   (|c| *c += 1) │     │                                     │
│     └─────────────────┘     │ notify subscribers                  │
│                              ▼                                     │
│  2. STATE NOTIFIES           ┌─────────────────┐                   │
│     ┌─────────────────┐      │ComponentElement │                   │
│     │  Callbacks      │─────►│  .subscriptions │                   │
│     │  fire           │      └────────┬────────┘                   │
│     └─────────────────┘               │                             │
│                                      │ mark_dirty(BUILD)           │
│                                       ▼                            │
│  3. ELEMENT MARKS DIRTY    ┌─────────────────┐                    │
│     ┌─────────────────┐    │ PipelineOwner   │                    │
│     │ DirtyFlags::    │◄───│  .dirty_build   │                    │
│     │ BUILD           │    └────────┬────────┘                    │
│     └─────────────────┘             │ schedule frame              │
│                                     ▼                              │
│  4. BUILD PHASE           ┌─────────────────┐                     │
│     ┌─────────────────┐   │ view.body()     │                     │
│     │ ComponentElement│──►│   called        │──► New View          │
│     │  .rebuild()     │   └─────────────────┘                     │
│     └────────┬────────┘                                              │
│              │ create new View if needed                           │
│              ▼                                                      │
│  5. RECONCILIATION         ┌─────────────────┐                     │
│     ┌─────────────────┐   │ Element         │                     │
│     │ old_element     │──►│  .update(new)   │                     │
│     │ vs              │   └────────┬────────┘                     │
│     │ new_view        │            │                               │
│     └─────────────────┘            │ type check                    │
│                                   ▼                               │
│  6. UPDATE PROPERTIES     ┌─────────────────┐                      │
│     ┌─────────────────┐   │ RenderObject    │                      │
│     │ RenderObject    │──►│  .update(view)  │                      │
│     │  properties     │   └────────┬────────┘                      │
│     └─────────────────┘            │ mark_dirty(LAYOUT/PAINT)      │
│                                   ▼                                │
│  7. LAYOUT PHASE           ┌─────────────────┐                     │
│     ┌─────────────────┐   │ RenderObject    │                     │
│     │ constraints     │──►│  .layout()      │──► Size              │
│     └─────────────────┘   └────────┬────────┘                     │
│                                   │ set frame                     │
│                                    ▼                              │
│  8. RENDER PHASE           ┌─────────────────┐                     │
│     ┌─────────────────┐   │ RenderObject    │                     │
│     │ RenderObject    │──►│  .render()      │──► Buffer            │
│     │  .is_dirty()    │   └────────┬────────┘                     │
│     └─────────────────┘            │                               │
│                                   ▼                                │
│  9. COMPOSITE PHASE         ┌─────────────────┐                     │
│     ┌─────────────────┐   │ Compositor       │                     │
│     │ Buffers         │──►│  .composite()    │──► Window Buffer     │
│     └─────────────────┘   └─────────────────┘                     │
│                                                                    │
└────────────────────────────────────────────────────────────────────┘
```

### 2.5 Trait Summary (トレイト一覧)

| トレイト | 実装する型 | 役割 | 主要メソッド |
|--------|-----------|------|-------------|
| `View` | 全てのView型 | FactoryとしてElementを生成 | `create_element()`, `listenables()` |
| `Element` | Component/RenderElement | Elementツリーの共通インターフェース | `id()`, `update()`, `children()`, `mount()` |
| `RenderObject` | レイアウト/描画担当型 | レイアウト計算と描画の実行 | `layout()`, `render()`, `hit_test()` |
| `Listenable` | `State<T>` | 型消去された購読インターフェース | `subscribe_any()` |
| `ViewTuple` | タプル `(A,B,...)` | 複数Viewを一括でElement化 | `create_elements()`, `collect_listenables()` |

---

## 3. Layer Detail: The View Layer

Viewは**不変（Immutable）な設計図**であり、同時に**Factory**でもあります。
ユーザーは `struct` を定義し、`#[derive(View)]` を付けるだけで、フレームワークが必要とする全機能が自動実装されます。

### The `View` Trait

```rust
use std::any::Any;

pub trait View: 'static {
    // === Factory Method ===
    /// 自分の型を知っているのは自分だけ
    /// 戻り値を Box<dyn Element> にすることで、呼び出し元は型を知らなくて済む
    fn create_element(&self) -> Box<dyn Element>;

    // === State Dependencies ===
    /// 自分が持っているStateを申告する
    /// マクロが自動生成するため、手書き不要（デフォルト実装は空ベクトル）
    fn listenables(&self) -> Vec<&dyn Listenable> {
        Vec::new()
    }

    // === Type Information ===
    /// 自身のTypeIdを返す（ダウンキャスト用）
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    /// 型名を返す（デバッグ用）
    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    // === Any Downcasting ===
    /// Anyトレイト経由でダウンキャスト可能にする
    fn as_any(&self) -> &dyn Any;
}
```

### Viewトレイトの設計ポイント

| メソッド | 役割 | 呼び出し元 |
|---------|------|-----------|
| `create_element()` | Factoryパターン：自分専用のElementを生成 | フレームワーク |
| `listenables()` | 依存するStateを宣言（自動生成） | ComponentElement |
| `type_id()` | 型情報の取得（Reconciliation用） | Element |
| `as_any()` | ダウンキャスト用 | ユーザーコード/フレームワーク |

---

## 4. Layer Detail: The Element Layer

ScarletUI 2.0の最大の特徴は、Elementを役割に応じて2種類に明確に分割した点です。これにより、単一の `GenericElement` で発生していた複雑な条件分岐 を排除します。

### A. ComponentElement (The Manager)

ユーザー定義のView（`MyView` など）に対応します。描画実体（RenderObject）を持たず、**「他のViewを組み合わせて構成すること（Composition）」** が仕事です。

* **責務**:
* Stateの購読管理 (`mount` 時に `listenables()` を登録)。
* `rebuild` のトリガー時に `view.body()` を呼び出し、子Elementを展開・更新する。


* **構造**:
```rust
pub struct ComponentElement<V: View> {
    view: V,
    child: Option<Box<dyn Element>>, // 展開された子（1つ）
    subscriptions: Vec<SubscriptionId>,
    // ...
}

```



### B. RenderElement (The Painter)

プリミティブなView（`Text`, `Rectangle`, `VStack` など）に対応します。**「実際のレイアウトと描画」** が仕事です。

* **責務**:
* `RenderObject` の生成と保持。
* Viewのプロパティ変更を `RenderObject` に反映（`update`）。


* **構造**:
```rust
pub struct RenderElement<V, R> {
    view: V,
    render_object: R,
    children: Vec<Box<dyn Element>>, // RenderObjectが要求する子供たち
    // ...
}

```



### Element Tree Structure

この2つのElementが混在することで、効率的なツリーが形成されます。

```ascii
[Root]
  |
[ComponentElement (MyApp)]      <-- User Logic, State Subscriber
  |
  +-- [ComponentElement (Counter)] <-- Local State Scope
        |
        +-- [RenderElement (VStack)] <-- Layout Scope
              |
              +-- [RenderElement (Text)] --> Draw "Count: 1"
              |
              +-- [RenderElement (Button)] --> Draw Rect + Interaction

```

---

## 5. Layer Detail: The State Layer

Stateは「型」を隠蔽し、Elementが統一的に扱えるように抽象化されます。

### The `Listenable` Trait

`State<i32>` も `State<String>` も、Elementから見れば「変わったら教えてくれる何か」でしかありません。

```rust
pub trait Listenable {
    // 型引数 T を隠蔽してコールバックだけ登録する
    fn subscribe_any(&self, cb: Arc<dyn Fn() + Send + Sync>) -> SubscriptionId;
}

// State<T> はこれを実装する
impl<T: Clone + 'static> Listenable for State<T> { ... }

```

これによって、`ComponentElement` は以下のようにジェネリック非依存で購読が可能になります。

```rust
// ComponentElement::mount 内
for state in self.view.listenables() {
    state.subscribe_any(self.create_rebuild_trigger());
}

```

---

## 6. Reconciliation Process (更新フロー)

Stateが変更されたとき、どのように画面が更新されるかのフローです。

1. **Update**: ユーザーコードが `state.update(|v| *v += 1)` を実行。
2. **Notify**: `State` が登録されたコールバック（`ComponentElement` のリビルドトリガー）を発火。
3. **Mark Dirty**: `ComponentElement` が自分自身に `DirtyFlags::BUILD` を立て、スケジューラに登録。
4. **Rebuild (Next Frame)**:
* `ComponentElement` が `view.body()` を再実行。
* 新しいView（例: `Text("2")`）が生成される。


5. **Diff & Patch**:
* 子Elementに対して `child.update(new_view)` を呼び出す。
* 型が一致すればプロパティ更新（高速）。不一致ならElementごと作り直し。


6. **Render Update**:
* 末端の `RenderElement` が `RenderObject` の値を更新。
* `RenderObject` が `DirtyFlags::PAINT` を立て、再描画される。

---

## 7. State Ownership & Lifecycle (Stateの所有権とライフサイクル)

### 問題: View/ElementのライフサイクルとStateの衝突

StateがViewやElementによって所有されると、以下の問題が発生します：

```rust
struct CounterView {
    count: State<i32>,  // ViewがStateを所有
}

impl CounterView {
    fn body(&self) -> impl View {
        match self.mode.get() {
            Mode::Counter => CounterView { count: self.count.clone() },
            Mode::Text => TextView { ... },  // 型が違う！
        }
    }
}
```

**問題のフロー**:
1. `mode` が変わると `match` の分岐が変わる
2. **型が違う** → Reconciliationで古いElementが破棄される
3. 古いElementがdrop → Elementが持っていたStateもdrop
4. **Stateの実体が消滅** → コールバックが無効化される

### 解決策: StateRegistryによる一元管理

Stateの実体を `PipelineOwner` の配下にある `StateRegistry` で管理し、View/Elementは `Arc` での参照だけを持つ設計にします。

```ascii
┌─────────────────────────────────────────────────────────────────┐
│                    STATE OWNERSHIP MODEL                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌──────────────────────┐                                       │
│  │  PipelineOwner       │                                       │
│  │  ┌────────────────┐  │                                       │
│  │  │ StateRegistry  │  │ ← Stateの実体（マスター）を管理        │
│  │  │ - HashMap<..>  │  │                                       │
│  │  └────────┬───────┘  │                                       │
│  └───────────┼──────────┘                                       │
│              │                                                   │
│              │ Arc<StateInner>                                   │
│              │ (参照を渡す)                                       │
│              ▼                                                   │
│  ┌──────────────────────┐                                       │
│  │  View                │                                       │
│  │  count: State<i32>    │ ← Arcクローン（参照のみ）              │
│  └───────────┼──────────┘                                       │
│              │                                                   │
│              │ create_element()                                  │
│              ▼                                                   │
│  ┌──────────────────────┐                                       │
│  │  ComponentElement    │                                       │
│  │  - view: View        │ ← Viewを所有（StateはArc参照）          │
│  │  - subscriptions     │                                       │
│  └──────────────────────┘                                       │
│                                                                 │
│  【重要】Elementが破棄されても、Stateの実体はRegistryに残る     │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### StateRegistryの実装

```rust
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Stateを一意に識別するID
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StateId(pub u64);

impl StateId {
    /// コンパイル時に一意のIDを生成（マクロが使用）
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Stateの実体を一元管理するレジストリ
pub struct StateRegistry {
    states: HashMap<StateId, Box<dyn Any + Send + Sync>>,
}

impl StateRegistry {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
        }
    }

    /// Stateを登録（初回のみ呼ばれる）
    pub fn register<T: 'static + Send + Sync>(&mut self, id: StateId, state: State<T>) {
        self.states.insert(id, Box::new(state));
    }

    /// Stateを取得（既存のStateを返す）
    pub fn get<T: 'static + Clone>(&self, id: StateId) -> Option<State<T>> {
        self.states
            .get(&id)
            .and_then(|any| any.downcast_ref::<State<T>>())
            .cloned()
    }
}
```

### State構造の更新

```rust
/// StateはIDとArcの実体を持つ
#[derive(Clone)]
pub struct State<T> {
    id: StateId,
    inner: Arc<StateInner<T>>,
}

struct StateInner<T> {
    value: RwLock<T>,
    subscribers: RwLock<Vec<SubscriptionId>>,
}

impl<T> State<T> {
    /// 初期値からStateを作成（Registryに登録用）
    pub fn new(id: StateId, value: T) -> Self {
        Self {
            id,
            inner: Arc::new(StateInner {
                value: RwLock::new(value),
                subscribers: RwLock::new(Vec::new()),
            }),
        }
    }

    /// 値を取得
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.inner.value.read().unwrap().clone()
    }

    /// 値を設定
    pub fn set(&self, value: T) {
        *self.inner.value.write().unwrap() = value;
        self.notify();
    }

    fn notify(&self) {
        //購読者に通知...
    }
}

/// 初期化ヘルパー（マクロが使用）
impl<T: Default> State<T> {
    pub fn initial(id: StateId) -> Self {
        Self::new(id, T::default())
    }
}
```

### `#[state]` マクロの生成コード

```rust
// ユーザーが書くコード
#[derive(View)]
struct CounterView {
    #[state]
    count: i32,
}

// マクロが生成するコード
impl CounterView {
    fn new() -> Self {
        Self {
            count: State::initial(StateId::new(0), 0), // 一意のID、初期値0
        }
    }
}

// Viewトレイトの実装
impl View for CounterView {
    fn create_element(&self) -> Box<dyn Element> {
        // Stateは既にRegistryに登録されている
        Box::new(ComponentElement::new(self.clone()))
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        vec![&self.count as &dyn Listenable]
    }
}
```

### PipelineOwnerとの統合

```rust
pub struct PipelineOwner {
    element_tree: ElementTree,
    state_registry: StateRegistry,  // StateRegistryを持つ
    dirty_build: HashSet<ElementId>,
    dirty_layout: HashSet<ElementId>,
    dirty_paint: HashSet<ElementId>,
}

impl PipelineOwner {
    pub fn new() -> Self {
        Self {
            element_tree: ElementTree::new(),
            state_registry: StateRegistry::new(),
            dirty_build: HashSet::new(),
            dirty_layout: HashSet::new(),
            dirty_paint: HashSet::new(),
        }
    }

    /// StateRegistryへのアクセス
    pub fn state_registry(&self) -> &StateRegistry {
        &self.state_registry
    }

    pub fn state_registry_mut(&mut self) -> &mut StateRegistry {
        &mut self.state_registry
    }
}
```

### ライフサイクルの全体像

```ascii
1. アプリケーション起動
   |
   v
2. PipelineOwner::new() → StateRegistry作成
   |
   v
3. ルートViewの作成
   - #[state] フィールド → State::initial(id) が呼ばれる
   - StateRegistryに登録
   |
   v
4. State変更
   - State::set() → Registry内のState更新
   - コールバック発火 → ComponentElementがdirty
   |
   v
5. Build Phase
   - view.body() → 新しいView作成
   - 新しいViewのStateフィールド = 同じIDでRegistryから取得
   - → 同じArcを共有！
   |
   v
6. Reconciliation
   - 型が違う → 古いElement破棄
   - → StateはRegistryにあるので消えない！
   - 型が同じ → Element再利用
   |
   v
7. 次のState更新へ...
```

### まとめ

| 関心事 | 所有者 | 役割 |
|-------|--------|------|
| **Stateの実体** | `StateRegistry` | マスーデータを保持、アプリ全体で共有 |
| **Stateへの参照** | `View` / `Element` | `Arc` で軽量に参照、破棄してもOK |
| **Stateの識別** | `StateId` | コンパイル時に一意のIDを生成 |

この設計により：
- ✅ Elementが破棄されてもStateは残る
- ✅ 複数のViewで同じStateを共有できる
- ✅ ユーザーはAppStateなどを手動で作る必要がない
- ✅ `#[state]` マクロで自動化できる

---

## 8. コンテナVeiwについて

新アーキテクチャ（Component/Render分離 + Factory Method）においても、`VStack` などのコンテナがタプルを受け取る設計は非常にうまく機能します。むしろ、`Element` 生成の責務を `View` 側に移したことで、タプルの展開処理をきれいに隠蔽できるようになります。

具体的な実装イメージは以下のようになります。

### 1. タプル対応の仕組み (`ViewTuple` トレイト)

`VStack` は「1つのView」ではなく「Viewのリスト」を受け取りたいので、タプルに対して「一括でElementを作る能力」を与えます。

```rust
// 複数のViewを束ねるためのヘルパートレイト
pub trait ViewTuple {
    // 自分の要素をすべてElement化して返す
    fn create_elements(&self) -> Vec<Box<dyn Element>>;
    
    // 変更検知用のState収集も一括で行う
    fn collect_listenables<'a>(&'a self, collector: &mut Vec<&'a dyn Listenable>);
}

// マクロで (V1, V2), (V1, V2, V3)... に実装する
impl<V1: View, V2: View> ViewTuple for (V1, V2) {
    fn create_elements(&self) -> Vec<Box<dyn Element>> {
        vec![
            self.0.create_element(),
            self.1.create_element(),
        ]
    }

    fn collect_listenables<'a>(&'a self, collector: &mut Vec<&'a dyn Listenable>) {
        // 再帰的に収集
        collector.extend(self.0.listenables());
        collector.extend(self.1.listenables());
    }
}

```

### 2. VStackの実装

`VStack` はこの `ViewTuple` をジェネリクス `C` (Content) として持ちます。

```rust
#[derive(Clone)] // マクロを使わず手書き実装する例（プリミティブなので）
pub struct VStack<C> {
    content: C, // ここにタプル (Text, Button, ...) が入る
    spacing: f32,
}

impl<C: ViewTuple + 'static> View for VStack<C> {
    fn create_element(&self) -> Box<dyn Element> {
        // 子供たちを一括生成
        let children = self.content.create_elements();

        // RenderElementを作って返す
        // (RenderElementは children: Vec<Box<dyn Element>> を持つ)
        Box::new(RenderElement::new(
            self.clone(),
            RenderVStack::new(), // RenderObjectの実体
            children
        ))
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        let mut list = Vec::new();
        // 子供たちが持っているStateも親(VStack)の購読対象に含めるか？
        // -> 通常は含めない。子供の変更は子供のElementで閉じるべき。
        // ただし、VStack自体のプロパティ（spacingなど）がState依存なら必要。
        self.content.collect_listenables(&mut list);
        list
    }
    
    // ...
}

```

### 3. メリット

1. **静的型付け:** `vstack! { Text::new(), Button::new() }` が `VStack<(Text, Button)>` という型になり、コンパイル時に構造が確定します。
2. **ゼロコスト抽象化:** `Vec<Box<dyn View>>` を使うのと違い、ヒープアロケーションや動的ディスパッチを最小限に抑えられます。
3. **ComponentElementとの整合性:** ユーザー定義View (`impl View`) も、内部で `vstack!` を使うなら、その戻り値は `VStack<(...)>` という具体的な型になり、`create_element` のチェーンが途切れません。

### 補足：動的リスト (`ForEach`)

タプルは固定長なので、配列データを表示したい場合（`for item in items`）のために、別途 `ForEach` というViewが必要になります。

* **VStack (Tuple):** 構造が静的に決まっている画面レイアウト用。
* **ForEach (Vec):** データの数によって増減するリスト用。

この「静的（Tuple）」と「動的（ForEach）」の使い分けも、SwiftUIと同じメンタルモデルでいけます。

---

## 9. View Modifiers (SwiftUI-styleの修飾子)

SwiftUIの `.padding()`, `.background()` のようなメソッドチェーンでViewを修飾する仕組みです。

### 基本概念

```rust
// SwiftUI風の書き味
Text::new("Hello")
    .padding(10.0)
    .background(Color::RED)
    .cornerRadius(5.0)
```

### 実装方式: ViewWrapper パターン

修飾子を適用するたびに、元のViewをラップする新しいView型を生成します。

```rust
// 修飾子はViewをラップする新しい型を返す
pub struct Padding<V: View> {
    inner: V,
    insets: EdgeInsets,
}

impl<V: View> View for Padding<V> {
    fn create_element(&self) -> Box<dyn Element> {
        // innerのElementを作成し、修飾を適用
        Box::new(RenderElement::new(
            self.clone(),
            RenderPadding::new(self.insets),
            vec![self.inner.create_element()],
        ))
    }
}

// View traitに拡張メソッドを提供
pub trait ViewExt: View {
    fn padding(self, insets: EdgeInsets) -> Padding<Self>
    where
        Self: Sized,
    {
        Padding {
            inner: self,
            insets,
        }
    }

    fn background(self, color: Color) -> Background<Self>
    where
        Self: Sized,
    {
        Background {
            inner: self,
            color,
        }
    }
}

// すべてのView: Viewで自動的に利用可能
impl<V: View> ViewExt for V {}
```

### メリット

1. **型安全性**: 修飾のチェーンがコンパイル時に型チェックされる
2. **ゼロコスト**: ラップ構造は最適化で消える可能性が高い
3. **直感的**: SwiftUI/Flutterユーザーに馴染みのあるAPI

### 修飾子の例

| 修飾子 | 説明 | RenderObject |
|--------|------|--------------|
| `.padding()` | 余白を追加 | `RenderPadding` |
| `.background()` | 背景色/背景View | `RenderBackground` |
| `.frame()` | サイズ指定 | `RenderFrame` |
| `.opacity()` | 透明度 | `RenderOpacity` |
| `.offset()` | 位置オフセット | `RenderOffset` |
| `.clip_shape()` | クリッピング | `RenderClip` |
| `.gesture()` | ジェスチャー追加 | `RenderGesture` |

---

## 10. Environment Values (環境値の伝播)

SwiftUIの `@Environment` のように、ツリー全体で共有される値を伝播する仕組みです。

### Environmentの基本構造

```rust
/// 環境値を保持するKey-Valueストア
pub struct Environment {
    values: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl Environment {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    pub fn insert<T: 'static + Send + Sync>(&mut self, value: T) {
        self.values.insert(TypeId::of::<T>(), Box::new(value));
    }

    pub fn get<T: 'static + Clone>(&self) -> Option<T> {
        self.values
            .get(&TypeId::of::<T>())
            .and_then(|v| v.downcast_ref::<T>())
            .cloned()
    }
}

/// ViewがEnvironmentを要求するためのトレイト
pub trait EnvironmentAware {
    fn read_environment(&self, env: &Environment);
}
```

### 定義済み環境値

```rust
// 色テーマ
pub struct ColorTheme {
    pub primary: Color,
    pub secondary: Color,
    pub background: Color,
    pub text: Color,
}

// フォントサイズ
pub struct FontSize(pub f32);

// 色アクセシビリティ設定
pub struct ColorScheme {
    pub mode: ColorMode,
}

pub enum ColorMode {
    Light,
    Dark,
}
```

### 使用例

```rust
impl MyView {
    fn body(&self) -> impl View {
        // Environmentを設定して子Viewに伝播
        Text::new("Themed Text")
            .environment(ColorTheme {
                primary: Color::BLUE,
                background: Color::WHITE,
                // ...
            })
    }
}

// 子ViewからEnvironmentを読み取る
impl Text {
    fn render(&self, env: &Environment) {
        let theme = env.get::<ColorTheme>()
            .unwrap_or_else(|| ColorTheme::default());

        // themeを使って描画
        // ...
    }
}
```

---

## 11. Event Handling (イベント処理)

Flutterのイベント処理モデルをベースに、Hit Testとイベントディスパッチの仕組みを設計します。

### イベントフロー

```ascii
[Input Event]
      |
      v
+-------------------------+
|   Hit Test Phase        |  ← どのRenderObjectがイベント対象かを特定
+-------------------------+
      |
      v
+-------------------------+
|   Event Dispatch        |
|  - Capture Phase        |  ← ルートからターゲットへ（下降）
|  - Target Phase         |  ← ターゲットでの処理
|  - Bubble Phase         |  ← ターゲットからルートへ（上昇）
+-------------------------+
```

### Eventの定義

```rust
pub enum Event {
    MouseEvent(MouseEvent),
    KeyEvent(KeyEvent),
    FocusEvent(FocusEvent),
    LifecycleEvent(LifecycleEvent),
}

pub enum MouseEvent {
    Moved(Point),
    ButtonPressed(MouseButton, Point),
    ButtonReleased(MouseButton, Point),
    Scroll(f32, f32), // dx, dy
}

pub struct HitResult {
    pub target: NodeId,
    pub local_point: Point,
}
```

### Hit Test

```rust
pub trait RenderObject {
    /// 指定したポイントにある子Nodeを探す
    fn hit_test(&self, point: Point) -> HitResult {
        // デフォルト実装: 自分のフレームをチェック
        if self.frame().contains(point) {
            HitResult::Handled(self.id())
        } else {
            HitResult::Passthrough
        }
    }
}
```

### イベントディスパッチャー

```rust
pub struct EventDispatcher {
    root: ElementId,
}

impl EventDispatcher {
    /// イベントを適切なNodeにディスパッチ
    pub fn dispatch(&self, element_tree: &mut ElementTree, event: Event) {
        match event {
            Event::MouseEvent(e) => self.dispatch_mouse(element_tree, e),
            Event::KeyEvent(e) => self.dispatch_key(element_tree, e),
            // ...
        }
    }

    fn dispatch_mouse(&self, element_tree: &mut ElementTree, event: MouseEvent) {
        // 1. Hit Testでターゲットを特定
        let target = self.hit_test(element_tree, event.point());

        // 2. Capture Phase: ルート→ターゲット
        let path = self.build_path_to_target(element_tree, target);
        for node in &path {
            node.handle_event(&event, Phase::Capture);
        }

        // 3. Target Phase
        if let Some(target_node) = element_tree.find_mut(target) {
            target_node.handle_event(&event, Phase::Target);
        }

        // 4. Bubble Phase: ターゲット→ルート
        for node in path.into_iter().rev() {
            node.handle_event(&event, Phase::Bubble);
        }
    }
}
```

---

## 12. Gesture Recognizer (ジェスチャー認識)

SwiftUIの `.onTapGesture()` のような宣言的なジェスチャー処理です。

### 基本的なジェスチャー

```rust
pub enum Gesture {
    Tap(TapGesture),
    Drag(DragGesture),
    LongPress(LongPressGesture),
    Pinch(PinchGesture),
    Rotation(RotationGesture),
}

pub struct TapGesture {
    pub count: usize, // ダブルタップなど
}

pub struct DragGesture {
    pub minimum_distance: f32,
}
```

### 使用例

```rust
Text::new("Tap me")
    .gesture(TapGesture::new(1).on_action(|| {
        println!("Tapped!");
    }))
    .gesture(
        DragGesture::new()
            .on_changed(|translation| {
                println!("Dragging: {:?}", translation);
            })
    )
```

---

## 13. Focus Management (フォーカス管理)

キーボードフォーカスの管理システムです。

### Focus Node

```rust
pub trait Focusable {
    fn request_focus(&self);
    fn has_focus(&self) -> bool;
    fn focus_node(&self) -> Option<FocusNode>;
}

pub struct FocusNode {
    pub id: NodeId,
    pub focusable: bool,
    pub skip: bool, // タブ移動でスキップするか
}
```

### Focus Manager

```rust
pub struct FocusManager {
    focus_chain: Vec<NodeId>,
    current_focus: Option<NodeId>,
}

impl FocusManager {
    /// 次のフォーカスに移動
    pub fn focus_next(&mut self) {
        // ...
    }

    /// 前のフォーカスに移動
    pub fn focus_previous(&mut self) {
        // ...
    }

    /// 特定のノードにフォーカスを設定
    pub fn request_focus(&mut self, id: NodeId) {
        self.current_focus = Some(id);
    }
}
```

---

## 14. Animation System (アニメーション)

SwiftUIの `.animation()` のようなアニメーションシステムです。

### Animation Value

```rust
pub struct Animated<T: Clone> {
    current: T,
    target: T,
    animation: Animation,
}

pub enum Animation {
    Linear(Duration),
    EaseIn(Duration),
    EaseOut(Duration),
    Spring { stiffness: f32, damping: f32 },
}
```

### アニメート可能なプロパティ

```rust
pub trait Animatable: Clone {
    fn interpolate(&self, other: &Self, t: f32) -> Self;
}

impl Animatable for f32 {
    fn interpolate(&self, other: &Self, t: f32) -> Self {
        self + (other - self) * t
    }
}

impl Animatable for Color {
    fn interpolate(&self, other: &Self, t: f32) -> Self {
        // 色空間で補間
        // ...
    }
}
```

### 使用例

```rust
struct ToggleButton {
    is_on: State<bool>,
}

impl ToggleButton {
    fn body(&self) -> impl View {
        Rectangle::new()
            .fill(if self.is_on.get() {
                Color::GREEN
            } else {
                Color::GRAY
            })
            .animation(Animation::spring())  // SwiftUI風
            .frame(width: 50.0, height: 30.0)
    }
}
```

---

## 15. Lifecycle Hooks (ライフサイクル)

Viewの表示/非表示タイミングで処理を実行する仕組みです。

### Lifecycle Events

```rust
pub enum LifecycleEvent {
    Mount,
    Appear,
    Disappear,
    Unmount,
}
```

### 使用例

```rust
struct TimerView {
    timer: State<Timer>,
}

impl TimerView {
    fn body(&self) -> impl View {
        Text::new(format!("Time: {}", self.timer.elapsed()))
            .on_appear(|| {
                // Viewが表示されたときにタイマー開始
                self.timer.start();
            })
            .on_disappear(|| {
                // Viewが非表示になったときにタイマー停止
                self.timer.stop();
            })
    }
}
```

---

## 16. Keys & Identity (キーと識別)

ForEachなどの動的リストで、要素を一意に識別するための仕組みです。

### Keyの定義

```rust
pub trait ViewKey: PartialEq + Eq + Hash + Clone {
    fn as_any(&self) -> &dyn Any;
}

impl<T: PartialEq + Eq + Hash + Clone + 'static> ViewKey for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
```

### ForEachの実装

```rust
pub struct ForEach<Data, Item, Key> {
    data: Data,
    key_selector: fn(&Item) -> Key,
    builder: fn(&Item) -> impl View,
}

impl<Data, Item, Key> View for ForEach<Data, Item, Key>
where
    Data: IntoIterator<Item = Item>,
    Key: ViewKey,
{
    fn create_element(&self) -> Box<dyn Element> {
        // ReconciliationのためにKeyで追跡
        // ...
    }
}
```

### 使用例

```rust
struct ListView {
    items: State<Vec<Item>>,
}

impl ListView {
    fn body(&self) -> impl View {
        VStack::new((
            // IDをキーにしてレンダリング
            ForEach::new(
                self.items.get(),
                |item| item.id,    // Key selector
                |item| Text::new(&item.name),
            ),
        ))
    }
}
```

---

## 17. Macros (マクロ詳細)

コンパイル時のボイラープレートを自動生成するマクロ群です。

### `#[derive(View)]`

ユーザー定義構造体に `View` を自動実装します。

```rust
#[derive(View, Clone)]
struct CounterView {
    count: State<i32>,
    title: String,
}

// マクロが以下を自動生成:
impl View for CounterView {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(ComponentElement::new(self.clone()))
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        // Stateフィールドを自動検出
        vec![
            &self.count as &dyn Listenable,
        ]
    }
}
```

### `vstack!`, `hstack!`, `zstack!`

SwiftUI風の構文でタプルを受け取るコンテナを簡潔に記述するマクロです。

#### 基本的な使用例

```rust
// カンマ区切りのリストを記述
let view = vstack! {
    Text::new("Hello"),
    Text::new("World"),
};

// トレイリングカンマもOK
let view = vstack! {
    Text::new("Hello"),
    Text::new("World"),
};

// メソッドチェーンも可能
let view = vstack! {
    Text::new("Hello"),
    Text::new("World"),
}
.spacing(10.0)
.alignment(Alignment::Center);
```

#### マクロの展開

マクロは要素数に応じて適切なタプル型に展開されます。

```rust
// 入力:
vstack! {
    Text::new("A"),
    Text::new("B"),
    Text::new("C"),
}

// 出力:
VStack::new((
    Text::new("A"),
    Text::new("B"),
    Text::new("C"),
))
```

#### マクロの実装

```rust
macro_rules! vstack {
    // 0要素（空のVStack）
    () => {
        VStack::new(())
    };

    // 1要素以上
    ($($view:expr),* $(,)?) => {
        VStack::new(($($view),*))
    };
}

// hstack!, zstack! も同様
macro_rules! hstack {
    ($($view:expr),* $(,)?) => {
        HStack::new(($($view),*))
    };
}

macro_rules! zstack {
    ($($view:expr),* $(,)?) => {
        ZStack::new(($($view),*))
    };
}
```

#### 動的リストとの組み合わせ

静的な `vstack!` と動的な `ForEach` を組み合わせることができます。

```rust
struct ListView {
    items: State<Vec<Item>>,
    header: String,
}

impl ListView {
    fn body(&self) -> impl View {
        vstack! {
            // 静的なヘッダー
            Text::new(&self.header)
                .font_size(24.0),

            // 動的なリスト
            ForEach::new(
                self.items.get(),
                |item| item.id,
                |item| Text::new(&item.name),
            ),

            // 静的なフッター
            Text::new("End")
                .font_size(12.0),
        }
        .spacing(10.0)
    }
}
```

#### ネストしたコンテナ

```rust
vstack! {
    Text::new("Row 1"),

    hstack! {
        Text::new("Col 1"),
        Text::new("Col 2"),
    },

    Text::new("Row 2"),
}
```

### `@State` プロパティマクロ

SwiftUIの `@State` のようなプロパティレベルのマクロ（Rustでは実装困難だが、代替案として）:

```rust
struct CounterView {
    #[state]  // このフィールドを自動的に listenables に追加
    count: i32,
}
```

---

## 18. Dirty Tracking & Layout Pass

FlutterのDirtyフラグによる差分更新の仕組みです。

### Dirty Flags

```rust
bitflags! {
    pub struct DirtyFlags: u32 {
        const BUILD      = 1 << 0;  // Viewの再ビルドが必要
        const LAYOUT     = 1 << 1;  // レイアウト再計算が必要
        const PAINT      = 1 << 2;  // 再描画が必要
        const CHILDREN   = 1 << 3;  // 子の構造が変わった
    }
}
```

### Pipeline Owner

```rust
pub struct PipelineOwner {
    dirty_layout: HashSet<ElementId>,
    dirty_paint: HashSet<ElementId>,
    dirty_build: HashSet<ElementId>,
}

impl PipelineOwner {
    /// ダーティなノードを処理
    pub fn flush(&mut self, element_tree: &mut ElementTree) {
        // 1. Build Phase
        for id in &self.dirty_build {
            self.rebuild(element_tree, *id);
        }

        // 2. Layout Phase
        for id in &self.dirty_layout {
            self.layout(element_tree, *id);
        }

        // 3. Paint Phase
        for id in &self.dirty_paint {
            self.paint(element_tree, *id);
        }
    }
}
```

---

## 19. Complete Example: Counter App

全ての要素を組み合わせた完全な例です。

```rust
#[derive(View, Clone)]
struct CounterApp {
    #[state]
    count: i32,
}

impl CounterApp {
    fn body(&self) -> impl View {
        Window::new("Counter Demo",
            vstack! {
                Text::new("Counter Demo")
                    .font_size(24.0)
                    .padding(EdgeInsets::all(10.0)),

                Text::new(format!("Count: {}", self.count))
                    .font_size(48.0)
                    .padding(EdgeInsets::all(20.0)),

                hstack! {
                    Button::new("-")
                        .on_click(|| {
                            self.count.update(|c| *c -= 1);
                        }),

                    Spacer::new(),

                    Button::new("+")
                        .on_click(|| {
                            self.count.update(|c| *c += 1);
                        }),
                }
                .spacing(10.0)
                .padding(EdgeInsets::horizontal(20.0)),
            }
            .spacing(20.0)
            .padding(EdgeInsets::all(20.0))
            .background(Color::WHITE)
        )
        .size(Size::new(400.0, 500.0))
    }
}

impl Application for CounterApp {
    // body()メソッドはCounterAppで既に定義済み
    // init()はデフォルト実装を使用
}

fn main() {
    let mut app = CounterApp {
        count: State::initial(StateId::new(1), 0),
    };
    app.run();
}
```

---

## 20. RenderObject Detailed Design

RenderObjectは、UIの描画とレイアウトを担当する最下位のコンポーネントです。

### RenderObjectのライフサイクル

```ascii
[Create]
    |
    v
[Layout]  ← サイズ計算、子の配置
    |
    v
[Paint]   ← 自身のバッファに描画
    |
    v
[Composite] ← Compositorがウィンドウバッファに転送
```

### RenderObjectトレイトの完全版

```rust
pub trait RenderObject: Any {
    // === Identity ===
    fn id(&self) -> NodeId;
    fn type_id(&self) -> TypeId;
    fn type_name(&self) -> &'static str;

    // === Tree Structure ===
    fn parent(&self) -> Option<NodeId>;
    fn set_parent(&mut self, parent: NodeId);
    fn children(&self) -> &[Box<dyn RenderObject>];
    fn children_mut(&mut self) -> &mut [Box<dyn RenderObject>];

    // === Lifecycle ===
    /// 自身と子のレイアウトを計算
    fn layout(&mut self, constraints: LayoutConstraints) -> Size;

    /// 自身のバッファに描画
    fn render(&mut self);

    // === Frame & Geometry ===
    fn frame(&self) -> Rect;
    fn set_frame(&mut self, frame: Rect);

    // === Buffer ===
    /// 自身の描画バッファを返す（コンテナはNoneを返す）
    fn get_buffer(&self) -> Option<&Buffer>;
    fn get_buffer_mut(&mut self) -> Option<&mut Buffer>;

    // === Update ===
    /// 新しいViewで更新（Reconciliation時に呼ばれる）
    fn update(&mut self, new_view: &dyn View) -> UpdateResult;
    fn try_update(&mut self, new_view: &dyn View) -> Option<UpdateResult>;

    // === Event Handling ===
    fn hit_test(&self, point: Point) -> HitResult;
    fn handle_event(&mut self, event: &Event, ctx: &mut EventContext);

    // === Focus ===
    fn is_focusable(&self) -> bool;
    fn request_focus(&mut self) -> bool;
    fn lose_focus(&mut self);

    // === Dirty Tracking ===
    fn mark_dirty(&mut self, flags: DirtyFlags);
    fn is_dirty(&self) -> bool;
    fn clear_dirty(&mut self);
}
```

### レイアウトアルゴリズム

RenderObjectは制約ベースのレイアウトを採用します（Flutterと同様）。

```rust
pub struct LayoutConstraints {
    pub min: Size,
    pub max: Size,
}

impl LayoutConstraints {
    /// 自由なサイズ
    pub fn loose(max: Size) -> Self {
        Self {
            min: Size::ZERO,
            max,
        }
    }

    /// 固定サイズ
    pub fn tight(size: Size) -> Self {
        Self {
            min: size,
            max: size,
        }
    }

    /// 制約を満たすようにサイズをクランプ
    pub fn constrain(&self, size: Size) -> Size {
        Size::new(
            size.width.clamp(self.min.width, self.max.width),
            size.height.clamp(self.min.height, self.max.height),
        )
    }
}
```

### レイアウトの例：Text

```rust
impl RenderObject for TextRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // 1. 固有サイズを測定
        let intrinsic = self.measure_text();

        // 2. 制約を適用
        let size = constraints.constrain(intrinsic);

        // 3. フレームを設定（相対座標）
        self.frame = Rect::new(Point::ZERO, size);

        size
    }
}
```

### レイアウトの例：VStack

```rust
impl RenderObject for VStackRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let mut y_offset = 0.0;
        let mut max_width = 0.0;

        // 子供をレイアウト
        for child in &mut self.children {
            let child_constraints = LayoutConstraints::loose(Size::new(
                constraints.max.width,
                f32::MAX, // 高さは制限しない
            ));

            let child_size = child.layout(child_constraints);

            // 子のフレームを配置
            child.set_frame(Rect::new(
                Point::new(0.0, y_offset),
                child_size,
            ));

            y_offset += child_size.height + self.spacing;
            max_width = max_width.max(child_size.width);
        }

        let total_height = y_offset - self.spacing;
        let size = Size::new(max_width, total_height);

        self.frame = Rect::new(Point::ZERO, size);
        size
    }
}
```

---

## 21. Buffer & Compositing

各RenderObjectは自身の描画バッファを持ち、Compositorがこれをウィンドウバッファに合成します。

### Buffer構造

```rust
pub struct Buffer {
    width: u32,
    height: u32,
    data: Vec<u32>, // BGRA形式
}

impl Buffer {
    pub fn new(size: Size) -> Self {
        let width = size.width as u32;
        let height = size.height as u32;
        Self {
            width,
            height,
            data: vec![0; (width * height) as usize],
        }
    }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn as_slice(&self) -> &[u32] { &self.data }
    pub fn as_mut_slice(&mut self) -> &mut [u32] { &mut self.data }

    /// 別のバッファをこのバッファにブレンドしてコピー
    pub fn composite(
        &mut self,
        src: &Buffer,
        dst_x: i32,
        dst_y: i32,
        opacity: f32,
    ) {
        for y in 0..src.height {
            for x in 0..src.width {
                let src_x = x as i32;
                let src_y = y as i32;
                let dst_x = dst_x + src_x;
                let dst_y = dst_y + src_y;

                if dst_x >= 0 && dst_x < self.width as i32
                    && dst_y >= 0 && dst_y < self.height as i32
                {
                    let src_pixel = src.data[(src_y * src.width + src_x) as usize];
                    let dst_idx = (dst_y * self.width as i32 + dst_x) as usize;

                    // Alphaブレンディング
                    self.data[dst_idx] = blend_pixels(
                        self.data[dst_idx],
                        src_pixel,
                        opacity,
                    );
                }
            }
        }
    }
}

fn blend_pixels(dst: u32, src: u32, opacity: f32) -> u32 {
    // BGRAの各チャンネルを抽出
    let dst_b = (dst & 0xFF) as u32;
    let dst_g = ((dst >> 8) & 0xFF) as u32;
    let dst_r = ((dst >> 16) & 0xFF) as u32;
    let dst_a = ((dst >> 24) & 0xFF) as u32;

    let src_b = (src & 0xFF) as u32;
    let src_g = ((src >> 8) & 0xFF) as u32;
    let src_r = ((src >> 16) & 0xFF) as u32;
    let src_a = ((src >> 24) & 0xFF) as u32;

    // Alphaブレンディング
    let a = src_a as f32 * opacity;
    let inv_a = 255.0 - a;

    let b = (src_b as f32 * a + dst_b as f32 * inv_a / 255.0) as u32;
    let g = (src_g as f32 * a + dst_g as f32 * inv_a / 255.0) as u32;
    let r = (src_r as f32 * a + dst_r as f32 * inv_a / 255.0) as u32;
    let a_final = (dst_a as f32 + (255.0 - dst_a as f32) * a / 255.0) as u32;

    (b & 0xFF) | ((g & 0xFF) << 8) | ((r & 0xFF) << 16) | ((a_final as u32 & 0xFF) << 24)
}
```

### RenderObjectでのBuffer使用

```rust
pub struct TextRenderObject {
    // ...
    buffer: Option<Buffer>,
    frame: Rect,
}

impl RenderObject for TextRenderObject {
    fn render(&mut self) {
        if !self.is_dirty() {
            return;
        }

        // バッファを作成
        self.buffer = Some(Buffer::new(self.frame.size));

        // バッファにテキストを描画
        if let Some(buffer) = &mut self.buffer {
            draw_text(
                buffer.as_mut_slice(),
                buffer.width(),
                buffer.height(),
                &self.view.content,
                0,
                0,
                self.view.font_size,
                self.view.color.as_bgra(),
            );
        }

        self.clear_dirty();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn get_buffer_mut(&mut self) -> Option<&mut Buffer> {
        self.buffer.as_mut()
    }
}
```

### コンテナRenderObject（バッファなし）

```rust
pub struct VStackRenderObject {
    children: Vec<Box<dyn RenderObject>>,
    // buffer: Option<Buffer>,  ← コンテナはバッファを持たない
    frame: Rect,
}

impl RenderObject for VStackRenderObject {
    fn render(&mut self) {
        // 子供だけをレンダリング
        for child in &mut self.children {
            child.render();
        }
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        None  // コンテナはバッファを持たない
    }
}
```

---

## 22. Compositor

CompositorはRenderObjectツリーを巡回し、各バッファをウィンドウバッファに合成します。

```ascii
Window Buffer
      ^
      | composite()
      |
[Compositor]
      |
      +-- [RenderElement (VStack)]
      |      +-- [RenderElement (Text)] --> Buffer A ─┐
      |      +-- [RenderElement (Button)] --> Buffer B ─┼─> Composite
      |                                                │
      +-- [RenderElement (HStack)]                     │
             +-- [RenderElement (Image)] --> Buffer C ─┘
```

### Compositorの実装

```rust
pub struct Compositor {
    window_buffer: Buffer,
}

impl Compositor {
    pub fn new(window_size: Size) -> Self {
        Self {
            window_buffer: Buffer::new(window_size),
        }
    }

    /// RenderObjectツリーを巡回して合成
    pub fn composite_tree(&mut self, root: &dyn RenderObject) {
        // 背景クリア
        self.clear(Color::WHITE);

        // 深さ優先で巡回
        self.composite_node(root, Point::ZERO);
    }

    fn composite_node(&mut self, node: &dyn RenderObject, origin: Point) {
        let frame = node.frame();
        let absolute_origin = origin + frame.origin;

        // 子供を先に処理（後方の要素ほど手前に表示されるため、手前から）
        for child in node.children().iter() {
            self.composite_node(child.as_ref(), absolute_origin);
        }

        // 自身のバッファがあれば合成
        if let Some(buffer) = node.get_buffer() {
            self.window_buffer.composite(
                buffer,
                absolute_origin.x as i32,
                absolute_origin.y as i32,
                1.0, // opacity
            );
        }
    }

    fn clear(&mut self, color: Color) {
        let pixel = color.as_bgra();
        for px in self.window_buffer.as_mut_slice() {
            *px = pixel;
        }
    }

    pub fn window_buffer(&self) -> &Buffer {
        &self.window_buffer
    }
}
```

### アルファブレンディングと不透明度

View Modifiersを使って不透明度を制御できます。

```rust
pub struct Opacity<V: View> {
    inner: V,
    value: f32,
}

impl<V: View> Opacity<V> {
    pub fn new(inner: V, value: f32) -> Self {
        Self { inner, value }
    }
}

impl<V: View> View for Opacity<V> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            RenderOpacity::new(self.value),
            vec![self.inner.create_element()],
        ))
    }
}
```

---

## 23. Rendering Pipeline Complete Flow

完全なレンダリングパイプラインの流れです。

```ascii
1. State Update
   |
   v
2. Mark Dirty (ComponentElement)
   |
   v
3. Build Phase: view.body() → 新しいView
   |
   v
4. Reconciliation: 古いElement vs 新しいView
   |
   v
5. Layout Phase: dirtyなRenderObjectをレイアウト
   |
   v
6. Render Phase: dirtyなRenderObjectがバッファに描画
   |
   v
7. Composite Phase: Compositorがウィンドウバッファに合成
   |
   v
8. Present: ウィンドウシステムに提示
```

---

## 24. RenderObject & Bufferの関係性

RenderObjectとBufferの所有関係を明確にします。

| RenderObject種類 | Bufferの所有 | 役割 |
|-----------------|-------------|------|
| **Leaf (Text, Image)** | 所有する | 自身の描画内容を保持 |
| **Container (VStack, HStack)** | 所有しない | 子供の配置のみ担当 |
| **Modifier (Padding, Frame)** | 所有しない | 子のフレーム変換のみ |

この設計により：
- メモリ効率：不要な中間バッファを作らない
- 描画効率：変更された部分のみバッファを更新
- 合成の柔軟性：Compositorが自由にブレンドできる

---

## 25. Application & Main Loop (アプリケーションとメインループ)

アプリケーションのライフサイクルとイベントループを管理するための`Application`トレイトです。

### Applicationトレイトの定義

```rust
use std::time::{Duration, Instant};

pub trait Application: View {
    /// アプリケーションの本体を返す
    fn body(&self) -> impl View;

    /// アプリケーションを初期化
    fn init(&mut self) {
        // デフォルト実装: 何もしない
    }

    /// メインループを実行
    fn run(&mut self)
    where
        Self: Sized,
    {
        // 1. 自分自身(View)からElementTreeを構築
        let root_element = self.create_element();

        // 2. 初期レイアウトでウィンドウサイズを決定
        let mut pipeline = RenderingPipeline::new();
        pipeline.set_root(root_element);

        // 3. 初期化
        self.init();

        // 4. 初期レイアウトを実行（Window Viewの情報を抽出）
        let (app_id, window_title, window_size) = pipeline.layout_initial();

        // 5. プラットフォームウィンドウを作成
        // デフォルトではSWSバックエンドが使用される
        let mut platform_window: Box<dyn PlatformWindow> = Box::new(
            SWSPlatformWindow::new(&app_id, &window_title, window_size)
                .expect("Failed to create platform window")
        );

        let target_frame_time = Duration::from_secs_f64(1.0 / 60.0); // 60 FPS
        let mut last_frame = Instant::now();

        loop {
            let frame_start = Instant::now();
            let elapsed = frame_start - last_frame;
            last_frame = frame_start;

            // 4.1 イベント処理
            while let Some(event) = platform_window.poll_event() {
                match event {
                    Event::Quit => return, // アプリケーション終了
                    Event::Resize(size) => {
                        pipeline.resize(size);
                    }
                    event => {
                        pipeline.handle_event(event);
                    }
                }
            }

            // 4.2 レンダリングパイプラインの実行
            let window_buffer = pipeline.render();

            // 4.3 画面に提示
            platform_window.present(window_buffer);

            // 4.4 フレームレート調整
            let frame_time = frame_start.elapsed();
            if frame_time < target_frame_time {
                std::thread::sleep(target_frame_time - frame_time);
            }
        }
    }
}
```

### RenderingPipelineの詳細

```rust
pub struct RenderingPipeline {
    element_tree: ElementTree,
    pipeline_owner: PipelineOwner,
    compositor: Option<Compositor>,
    event_dispatcher: EventDispatcher,
    window_size: Size,
}

impl RenderingPipeline {
    pub fn new() -> Self {
        Self {
            element_tree: ElementTree::new(),
            pipeline_owner: PipelineOwner::new(),
            compositor: None,
            event_dispatcher: EventDispatcher::new(),
            window_size: Size::new(800.0, 600.0),
        }
    }

    /// ルートElementを設定
    pub fn set_root(&mut self, root_element: Box<dyn Element>) {
        self.element_tree.set_root(root_element);
    }

    /// 初期レイアウトを実行し、Windowの情報を返す
    pub fn layout_initial(&mut self) -> (String, String, Size) {
        // 1. Build Phase
        self.pipeline_owner.flush_build(&mut self.element_tree);

        // 2. Layout Phase
        self.pipeline_owner.flush_layout(&mut self.element_tree, self.window_size);

        // 3. Window Viewから情報を抽出
        let (app_id, title, size) = if let Some(root) = self.element_tree.root() {
            self.extract_window_info(root)
        } else {
            (
                "com.example.scarletui".to_string(),
                "ScarletUI".to_string(),
                Size::new(800.0, 600.0),
            )
        };

        // 4. Compositorを作成
        self.compositor = Some(Compositor::new(size));
        self.window_size = size;

        (app_id, title, size)
    }

    /// ElementTreeからWindow Viewを探し、app_id、タイトルとサイズを返す
    fn extract_window_info(&self, root: &dyn Element) -> (String, String, Size) {
        // ElementTreeを深さ優先で探索してWindow Viewを探す
        if let Some(window_view) = self.find_window_view(root) {
            // Windowからapp_id、タイトルとサイズを取得
            let app_id = window_view.app_id();
            let title = window_view.title();
            let size = window_view.window_size();
            (app_id, title, size)
        } else {
            // Windowがない場合はデフォルト値
            (
                "com.example.scarletui".to_string(),
                "ScarletUI".to_string(),
                Size::new(800.0, 600.0),
            )
        }
    }

    /// ElementTreeからWindow Viewを再帰的に探す
    fn find_window_view(&self, element: &dyn Element) -> Option<&Window<dyn View>> {
        // ElementのViewがWindowかどうかをチェック
        // 実装はAnyによるダウンキャストで行う
        if let Some(window) = element.as_any().downcast_ref::<Window<dyn View>>() {
            return Some(window);
        }

        // 子Elementを再帰的に探索
        for child in element.children() {
            if let Some(window) = self.find_window_view(child) {
                return Some(window);
            }
        }

        None
    }

    pub fn resize(&mut self, new_size: Size) {
        self.window_size = new_size;
        self.compositor = Some(Compositor::new(new_size));

        // 全体にレイアウト再計算を要求
        if let Some(root) = self.element_tree.root_mut() {
            root.mark_dirty(DirtyFlags::LAYOUT);
        }
    }

    pub fn handle_event(&mut self, event: Event) {
        // イベントディスパッチャーで処理
        self.event_dispatcher.dispatch(&mut self.element_tree, event);

        // イベント処理でStateが変更された場合、再ビルドが必要
        // Stateのコールバック内でComponentElementがdirtyフラグを立てる
    }

    pub fn render(&mut self) -> &Buffer {
        // 1. Build/Layout/Paint Phase
        self.pipeline_owner.flush(&mut self.element_tree, self.window_size);

        // 2. 合成フェーズ
        if let Some(root) = self.element_tree.root() {
            let render_object = root.render_object();
            self.compositor.as_mut().unwrap().composite_tree(render_object);
        }

        self.compositor.as_ref().unwrap().window_buffer()
    }
}
```

### PipelineOwnerのflushメソッド

```rust
impl PipelineOwner {
    /// ダーティなノードを処理して、画面を最新状態にする
    pub fn flush(&mut self, element_tree: &mut ElementTree, window_size: Size) {
        // 1. Build Phase: Stateが変わったViewを再ビルド
        self.flush_build(element_tree);

        // 2. Layout Phase: レイアウトを再計算
        self.flush_layout(element_tree, window_size);

        // 3. Paint Phase: 再描画が必要なノードを描画
        self.flush_paint(element_tree);
    }

    fn flush_build(&mut self, element_tree: &mut ElementTree) {
        let dirty_build = std::mem::take(&mut self.dirty_build);

        for id in dirty_build {
            if let Some(element) = element_tree.get_mut(id) {
                element.rebuild(&mut self.state_registry);
            }
        }
    }

    fn flush_layout(&mut self, element_tree: &mut ElementTree, window_size: Size) {
        let dirty_layout = std::mem::take(&mut self.dirty_layout);

        // 依存順にソートしてレイアウト（親→子の順）
        let sorted = self.topological_sort(element_tree, dirty_layout);

        for id in sorted {
            if let Some(element) = element_tree.get_mut(id) {
                element.layout(element_tree, LayoutConstraints::loose(window_size));
            }
        }
    }

    fn flush_paint(&mut self, element_tree: &mut ElementTree) {
        let dirty_paint = std::mem::take(&mut self.dirty_paint);

        for id in dirty_paint {
            if let Some(element) = element_tree.get_mut(id) {
                element.render();
            }
        }
    }
}
```

### Window View

Windowも単なるViewとして実装されます：

```rust
/// WindowはViewのラッパー
pub struct Window<V: View> {
    app_id: String,
    title: String,
    size: Size,
    child: V,
    resizable: bool,
    decorated: bool,
}

impl<V: View> Window<V> {
    pub fn new(title: impl Into<String>, child: V) -> Self {
        Self {
            app_id: "com.example.scarletui".to_string(),
            title: title.into(),
            size: Size::new(800.0, 600.0),
            child,
            resizable: true,
            decorated: true,
        }
    }

    pub fn app_id(mut self, app_id: impl Into<String>) -> Self {
        self.app_id = app_id.into();
        self
    }

    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    pub fn decorated(mut self, decorated: bool) -> Self {
        self.decorated = decorated;
        self
    }

    pub fn app_id(&self) -> String {
        self.app_id.clone()
    }

    pub fn title(&self) -> String {
        self.title.clone()
    }

    pub fn window_size(&self) -> Size {
        self.size
    }

    /// Anyによるダウンキャスト用
    pub fn as_any(&self) -> &dyn Any {
        self
    }
}

impl<V: View> View for Window<V> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            RenderWindow::new(self.title.clone(), self.size),
            vec![self.child.create_element()],
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
```

### ウィンドウの使用例

```rust
impl MyApp {
    fn body(&self) -> impl View {
        Window::new("My Application", self.content())
            .size(Size::new(1024.0, 768.0))
            .resizable(true)
            .decorated(true)
    }
}
```

### PlatformWindow バックエンドの抽象化

**注意**: `Window` (View) と `PlatformWindow` (OSウィンドウ) は別物です。

ScarletUIはプラットフォーム依存のウィンドウ機能を抽象化する`PlatformWindow`トレイトを提供します。これにより、異なるウィンドウシステム間での移植性が向上します。

#### PlatformWindow トレイト

```rust
/// プラットフォーム固有のウィンドウ機能を抽象化するトレイト
pub trait PlatformWindow {
    /// 新しいウィンドウを作成
    fn new(app_id: &str, title: &str, size: Size) -> Result<Self>
    where
        Self: Sized;

    /// イベントをポーリング（Noneならイベントなし）
    fn poll_event(&mut self) -> Option<Event>;

    /// バッファを画面に提示
    fn present(&mut self, buffer: &Buffer);

    /// ウィンドウタイトルを設定
    fn set_title(&mut self, title: &str);

    /// ウィンドウサイズを取得
    fn size(&self) -> Size;

    /// ウィンドウをリサイズ
    fn resize(&mut self, width: u32, height: u32) -> Result<()>;

    /// ウィンドウを閉じる
    fn close(&mut self) -> Result<()>;
}
```

#### バックエンド実装

```ascii
┌─────────────────────────────────────────────────────────────────┐
│                  PLATFORM WINDOW BACKENDS                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ScarletUI Application                                         │
│      │                                                          │
│      └── Box<dyn PlatformWindow>                               │
│              │                                                 │
│              ├── impl PlatformWindow for SWSPlatformWindow     │
│              │                                                  │
│              ├── (future) impl PlatformWindow for SDL2Window   │
│              │                                                  │
│              └── (future) impl PlatformWindow for WinitWindow  │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

#### SWSバックエンド実装

Scarlet OSでは`SWSPlatformWindow`がデフォルトの実装として提供されます：

```rust
/// SWS (Scarlet Window Server) バックエンド
pub struct SWSPlatformWindow {
    conn: sws_client::Connection,
    surface_id: u32,
    current_size: Size,
}

impl PlatformWindow for SWSPlatformWindow {
    fn new(app_id: &str, title: &str, size: Size) -> Result<Self> {
        let mut conn = sws_client::Connection::connect_default()?;
        let surface_id = conn.create_surface(
            app_id, title, "",
            size.width as u32,
            size.height as u32
        )?;
        Ok(Self {
            conn,
            surface_id,
            current_size: size,
        })
    }

    fn poll_event(&mut self) -> Option<Event> {
        self.conn.dispatch().ok()?;
        self.conn.poll_event().map(|ev| match ev {
            sws_client::Event::Input(input) => Event::Input(input),
            sws_client::Event::SurfaceConfigure { width, height, .. } => {
                self.current_size = Size::new(width as f32, height as f32);
                Event::Resize(self.current_size)
            }
            _ => Event::Unknown,
        })
    }

    fn present(&mut self, buffer: &Buffer) {
        // バッファをSWSサーフェスの共有メモリにコピー
        if let Some(surface) = self.conn.surface_mut(self.surface_id) {
            surface.with_buffer(|shm_buf, width, height| {
                // バッファコピー（BGRA形式）
                let src = buffer.as_bgra();
                let dst = &mut shm_buf[..width * height * 4];
                dst.copy_from_slice(src);
            });
            self.conn.commit(self.surface_id).ok();
        }
    }

    fn set_title(&mut self, title: &str) {
        let _ = self.conn.set_window_title(self.surface_id, title);
    }

    fn size(&self) -> Size {
        self.current_size
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.conn.resize_window(self.surface_id, width, height)?;
        self.current_size = Size::new(width as f32, height as f32);
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        self.conn.destroy_surface(self.surface_id)?;
        Ok(())
    }
}
```

#### SWSのアーキテクチャ

```
┌─────────────────────────────────────────────────────────────────┐
│                      SWS ARCHITECTURE                            │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ScarletUI Application                                         │
│      │                                                          │
│      │  SWSPlatformWindow                                      │
│      │      └── sws_client::Connection                         │
│      │              ├─ create_surface() ────┐                  │
│      │              ├─ commit()             │                  │
│      │              ├─ dispatch()           │                  │
│      │              └─ poll_event()         │                  │
│      │                                │                         │
│  ┌───┴────────────────────────────┴─────┐                       │
│  │         Shared Memory (Zero-copy)    │                       │
│  └──────────────────┬────────────────────┘                       │
│                     │ IPC (Unix-domain socket)                  │
│  ┌──────────────────┴────────────────────────┐                 │
│  │        Scarlet Window Server (sws)        │                 │
│  │     - Compositor                          │                 │
│  │     - Window Manager                      │                 │
│  │     - Input dispatch                      │                 │
│  └───────────────────────────────────────────┘                 │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

#### SWSプロトコルの主要機能

ScarletUIは以下のSWS機能を利用できます：

| 機能 | SWS API | ScarletUI使用例 |
|------|---------|----------------|
| ウィンドウ作成 | `create_surface()` | Application::run() |
| タイトル設定 | `set_window_title()` | Window Viewのtitleプロパティ |
| リサイズ | `resize_window()` | Window Viewのsize変更時 |
| 最小化/最大化 | `minimize_window()`, `maximize_window()` | Windowコントロール |
| 透明度 | `set_window_opacity()` | Window Viewのopacity |
| 親子関係 | `set_window_parent()` | モーダルダイアログ等 |
| ウィンドウタイプ | `set_window_type()` | Normal, AlwaysOnTop, Taskbar等 |

詳細は以下のドキュメントを参照：
- `docs/sws_ipc_protocol.md` - SWS IPCプロトコル仕様
- `docs/sws_client.md` - クライアントライブラリドキュメント
- `docs/sws_buffer_transport.md` - バッファ転送（共有メモリ）設計

### 2種類の「Window」の区別

| 種類 | 型 | 役割 | ライフサイクル |
|------|------|------|--------------|
| **Window (View)** | `Window<V: View>` | View階層の一部。宣言的UIの一部としてWindowを表現 | View/Elementと同じ |
| **PlatformWindow** | `dyn PlatformWindow` | プラットフォーム固有のウィンドウ操作。イベント処理、描画 | アプリケーションと同じ |

```ascii
┌─────────────────────────────────────────────────────────────────┐
│                    TWO TYPES OF WINDOW                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  Application::run()                                              │
│      │                                                          │
│      ├── body() → Window::new("Title", content)                 │
│      │              │                                          │
│      │              └── ViewとしてElementに変換                   │
│      │                      │                                   │
│      │                      ▼                                   │
│      │              RenderingPipeline                            │
│      │                      │                                   │
│      │                      └── layout_initial() で               │
│      │                          (title, size) を抽出          │
│      │                                  │                       │
│      │                                  ▼                       │
│      └── PlatformWindow::new(app_id, title, size)                │
│                      │                                          │
│                      ├── デフォルト: SWSPlatformWindow           │
│                      │      └─ sws_client::Connection         │
│                      │           └─ create_surface()          │
│                      │                                          │
│                      └── (将来的な実装)                         │
│                           ├── SDL2Window                       │
│                           └── WinitWindow                      │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 完全なアプリケーション例

```rust
#[derive(View, Clone)]
struct CounterApp {
    #[state]
    count: i32,
}

impl CounterApp {
    fn body(&self) -> impl View {
        Window::new("Counter Demo",
            vstack! {
                Text::new("Counter Demo")
                    .font_size(24.0)
                    .padding(EdgeInsets::all(10.0)),

                Text::new(format!("Count: {}", self.count))
                    .font_size(48.0)
                    .padding(EdgeInsets::all(20.0)),

                hstack! {
                    Button::new("-")
                        .on_click({
                            let count = self.count.clone();
                            move || {
                                count.update(|c| *c -= 1);
                            }
                        }),

                    Spacer::new(),

                    Button::new("+")
                        .on_click({
                            let count = self.count.clone();
                            move || {
                                count.update(|c| *c += 1);
                            }
                        }),
                }
                .spacing(10.0)
                .padding(EdgeInsets::horizontal(20.0)),
            }
            .spacing(20.0)
            .padding(EdgeInsets::all(20.0))
            .background(Color::WHITE)
        )
        .app_id("com.example.counter")
        .size(Size::new(400.0, 500.0))
        .resizable(true)
    }
}

impl Application for CounterApp {
    // body()メソッドはCounterAppで既に定義済み
    // init()はデフォルト実装を使用
}

fn main() {
    let mut app = CounterApp {
        count: State::initial(StateId::new(1), 0),
    };
    app.run();
}
```

### フレームレートとパフォーマンス

| 項目 | 設計値 | 説明 |
|------|--------|------|
| **目標FPS** | 60 | 1秒間に60フレーム（約16.67ms/フレーム） |
| **差分更新** |.dirtyなノードのみ | 変更された部分のみを再計算・再描画 |
| **イベント駆動** | イベントがあるまでアイドル | State更新時にフレームをスケジュール |
| **垂直同期** | オプション | WindowBackendの実装次第 |

### アプリケーションのライフサイクル

```ascii
┌─────────────────────────────────────────────────────────────────┐
│                     APPLICATION LIFECYCLE                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  1. INITIALIZATION                                              │
│     ├─ Application構造体の作成（struct literal）                 │
│     ├─ PipelineOwner::new() → StateRegistry作成                  │
│     ├─ State::initial() → RegistryにState登録                    │
│     └─ ElementTree構築                                          │
│                                                                 │
│  2. MAIN LOOP (毎フレーム)                                      │
│     ├─ Event Polling                                           │
│     │   ├─ Quit → return (アプリ終了)                           │
│     │   ├─ Resize → 再レイアウト                                │
│     │   └─ Input Event → EventDispatcher                        │
│     │                                                          │
│     ├─ Rendering Pipeline                                      │
│     │   ├─ Build Phase (dirty_buildがある場合)                  │
│     │   │   └─ ComponentElement.rebuild()                       │
│     │   │                                                      │
│     │   ├─ Layout Phase (dirty_layoutがある場合)                │
│     │   │   └─ RenderObject.layout()                            │
│     │   │                                                      │
│     │   ├─ Paint Phase (dirty_paintがある場合)                  │
│     │   │   └─ RenderObject.render()                            │
│     │   │                                                      │
│     │   └─ Composite Phase (常に実行)                           │
│     │       └─ Compositor.composite_tree()                      │
│     │                                                          │
│     ├─ Present to Window                                       │
│     │                                                          │
│     └─ Frame Rate Control                                      │
│         └─ target: 60 FPS                                       │
│                                                                 │
│  3. SHUTDOWN                                                    │
│     └─ リソースの解放                                            │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### State更新から画面表示までの詳細フロー

```ascii
[User clicks button]
        |
        v
[EventDispatcher]
        |
        v
[Button.on_click callback]
        |
        v
[State::update()] ──► [StateRegistry内のStateを更新]
        |
        v
[State::notify()] ──► [コールバック発火]
        |
        v
[ComponentElement::mark_dirty(BUILD)]
        |
        v
[PipelineOwner.dirty_buildにElementId追加]
        |
        v
[次のフレーム]
        |
        v
[PipelineOwner::flush()]
    ├─ Build Phase
    │   └─ view.body() → 新しいView
    │       └─ Reconciliation
    │           ├─ 型が同じ → RenderObject.update()
    │           │       └─ mark_dirty(LAYOUT/PAINT)
    │           └─ 型が違う → 新しいElement作成
    │
    ├─ Layout Phase
    │   └─ RenderObject.layout()
    │       └─ frameを更新
    │
    ├─ Paint Phase
    │   └─ RenderObject.render()
    │       └─ Bufferを更新
    │
    └─ Composite Phase
        └─ Compositor.composite_tree()
            └─ Window Bufferに合成
                |
                v
            [Window::present()]
                |
                v
            [画面に表示]
```

### Applicationトレイトの利点

1. **シンプルさ**: ApplicationはViewのサブタイプ。`body()`メソッドを実装するだけ
2. **宣言的Window制御**: Windowも単なるViewとして`body()`内で宣言
3. **一貫性**: 全てのアプリケーションで同じ初期化・終了フロー
4. **テスト容易性**: `body()`だけでUIをテスト可能
5. **柔軟性**: 複数Window、条件付きWindowなどが表現可能
6. **型安全性**: `Box<dyn View>`を使わず、`impl View`で静的に型付け

### Viewトレイトとの統合

`Application: View` という設計により、Applicationは以下のデフォルト実装を持ちます：

```rust
impl<V: View> View for V where V: Application {
    fn create_element(&self) -> Box<dyn Element> {
        // body()の結果をElementに変換
        self.body().create_element()
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        self.body().listenables()
    }
}
```

これにより、ユーザーは `#[derive(View)]` して`body()`を実装するだけでApplicationとして動作します。

### WindowもViewとして統合

```ascii
┌─────────────────────────────────────────────────────────────────┐
│                    WINDOW AS VIEW                              │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  Application                                                    │
│      │                                                         │
│      └── body() → Window::new("Title", content)                │
│                       └── .size(Size::new(...))                  │
│                       │                                        │
│                       ├── ViewとしてElementに変換               │
│                       ├── RenderObjectとしてレイアウト            │
│                       └── RenderingPipelineが検出してOS Window作成 │
│                                                                 │
│  【メリット】                                                   │
│  - WindowもただのViewなので柔軟性が高い                         │
│  - 条件分岐でWindowを出し入れ可能                                │
│  - 複数Windowも naturally supported                             │
│  - View修飾子（.resizable(), .decorated()）が使える              │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```