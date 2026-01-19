# ScarletUI アーキテクチャ

## 概要

ScarletUIは**データファーストのMVCアーキテクチャ**を採用したモダンなUIフレームワークです。Druid、Flutter、AppKitのベストプラクティスを組み合わせて設計されています。

## 設計原則

1. **データファースト**: すべての状態は`DataContext<T>`を通じて管理
2. **合成可能性（Composability）**: 小さなViewを組み合わせて複雑なUIを構築
3. **単方向データフロー**: データは上から下へ、イベントは下から上へ
4. **予測可能性**: 明確なフェーズ分離と実行順序
5. **効率性**: O(1)操作、最小限の再計算、部分再描画

---

## 全体アーキテクチャ

```
┌─────────────────────────────────────────────────────────┐
│                    Application                          │
├─────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  │
│  │   Window     │  │   Navigator  │  │    Layout    │  │
│  │  Management  │  │    System    │  │    Engine    │  │
│  └──────────────┘  └──────────────┘  └──────────────┘  │
├─────────────────────────────────────────────────────────┤
│                      View System                        │
│  ┌──────────────────────────────────────────────────┐  │
│  │  View Tree (Hierarchy)                           │  │
│  │  ┌─────────┐    ┌─────────┐    ┌─────────┐      │  │
│  │  │VStack   │───▶│HStack   │───▶│ Button  │      │  │
│  │  └─────────┘    └─────────┘    └─────────┘      │  │
│  └──────────────────────────────────────────────────┘  │
│                         ↓                               │
│  ┌──────────────────────────────────────────────────┐  │
│  │  Modifiers (SwiftUI-style chaining)              │  │
│  │  .padding().frame().background().repaint_boundary()│  │
│  └──────────────────────────────────────────────────┘  │
├─────────────────────────────────────────────────────────┤
│                   Data Flow Layer                       │
│  ┌──────────────┐    ┌──────────────┐                 │
│  │ DataContext  │───▶│   Observable  │                 │
│  │  ⟨State⟩     │    │   ⟨Proxy⟩    │                 │
│  └──────────────┘    └──────────────┘                 │
├─────────────────────────────────────────────────────────┤
│                   Event & Render                        │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐             │
│  │  Event   │─▶│  Layout  │─▶│  Paint   │             │
│  │  Phase   │  │  Phase   │  │  Phase   │             │
│  └──────────┘  └──────────┘  └──────────┘             │
│                                                  ↓       │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐             │
│  │ Compose  │─▶│  Buffer  │─▶│ Present  │             │
│  │  Phase   │  │  Pool    │  │  Phase   │             │
│  └──────────┘  └──────────┘  └──────────┘             │
└─────────────────────────────────────────────────────────┘
```

---

## コアコンポーネント

### 1. Viewトレイト

すべてのUI要素の基本インターフェース：

```rust
pub trait View {
    /// 一意の識別子
    fn id(&self) -> ViewId;

    /// レイアウト計算
    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size;

    /// 描画
    fn draw(&self, ctx: &mut PaintCtx, frame: Rect);

    /// イベント処理
    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow;

    /// 状態更新
    fn update(&mut self, ctx: &mut UpdateCtx);
}
```

**特徴:**
- コンポジション可能：小さなViewを組み合わせて複雑なUIを構築
- 再帰的な階層構造
- トレイトベースで柔軟な実装

### 2. DataContext（状態管理）

すべてのアプリケーション状態を管理するデータコンテナ：

```rust
pub struct DataContext<T> {
    inner: Arc<Mutex<DataContextInner<T>>>,
}

struct DataContextInner<T> {
    data: T,
    version: u64,
    observers: HashMap<ViewId, ObserverInfo>,
    dirty_views: HashSet<ViewId>,
}

impl<T> DataContext<T> {
    /// 新しいデータコンテキストを作成
    pub fn new(value: T) -> Self;

    /// 読み取りアクセス
    pub fn get(&self) -> T
    where
        T: Clone;

    /// 書き込みアクセス（自動無効化）
    pub fn modify<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R;

    /// 監視者を追加
    pub fn subscribe(&self, view_id: ViewId) -> u64;
}
```

**使用例:**

```rust
// データの定義
struct AppState {
    count: i32,
    text: String,
}

// データコンテキストの作成
let data = DataContext::new(AppState {
    count: 0,
    text: String::from("Hello"),
});

// 読み取り
let count = data.get().count;

// 書き込み（自動無効化）
data.modify(|state| {
    state.count += 1;  // 自動で再描画リクエスト
});
```

#### bindable! マクロ

SwiftUIの `@State` のような簡潔な構文で状態を作成：

```rust
use scarlet_ui::bindable;

fn build_ui() {
    // プリミティブ値
    let enabled = bindable!(false);
    let volume = bindable!(50.0);

    // 構造体
    let state = bindable!(AppState {
        count: 0,
        text: String::from("Hello"),
    });

    // UIにバインド
    let toggle = Toggle::bind(&enabled);
    let slider = Slider::bind(&volume, 0.0, 100.0);
}
```

### 3. 制約ベースレイアウト

Flutter風の制約システム：

```rust
pub struct LayoutConstraints {
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}
```

**レイアウトフロー:**

1. 親は子に制約を渡す
2. 子は制約内で最適なサイズを計算
3. 子はサイズを親に返す
4. 親は子の位置を決定

**実装例:**

```rust
impl View for Button {
    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        // 子（ラベル）をレイアウト
        let child_constraints = LayoutConstraints::new(
            constraints.min_width - 20,  // パディングを考慮
            constraints.max_width - 20,
            constraints.min_height - 10,
            constraints.max_height - 10,
        );

        let label_size = self.label.layout(ctx, child_constraints);

        // パディングを追加して最終サイズを計算
        Size::new(
            label_size.width + 20,
            label_size.height + 10,
        )
    }
}
```

### 4. 修飾子（Modifiers）

SwiftUI風のメソッドチェーン：

```rust
pub trait ViewExt: View + Sized where Self: 'static {
    fn padding(self, padding: u32) -> Padding<Self>;
    fn frame(self, width: u32, height: u32) -> Frame<Self>;
    fn background(self, color: Color) -> Background<Self>;
    fn repaint_boundary(self) -> RepaintBoundaryWrapper<Self>;
}
```

**使用例:**

```rust
let button = Button::new("Click Me")
    .padding(10)
    .frame(200, 50)
    .background(Color::BUTTON_NORMAL)
    .repaint_boundary();
```

**修飾子の実装:**

```rust
pub struct Padding<T> {
    child: T,
    top: u32,
    right: u32,
    bottom: u32,
    left: u32,
    cached_size: Size,
}

impl<T: View> View for Padding<T> {
    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        // 子の制約を計算（パディングを引く）
        let child_constraints = constraints - self.padding();
        let child_size = self.child.layout(ctx, child_constraints);

        // パディングを加えて返す
        child_size + self.padding()
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        // 子の描画位置を計算
        let child_frame = frame.inset(self.padding);
        self.child.draw(ctx, child_frame);
    }
}
```

---

## レンダリングパイプライン

### フェーズ分離

処理を5つの明確なフェーズに分離：

```
1. Event Phase    → イベントを配布・処理
2. Layout Phase   → レイアウトを計算
3. Paint Phase    → 描画コマンドを生成
4. Compose Phase  → レイヤーを合成
5. Present Phase  → 画面に表示
```

**利点:**
- 並列化可能
- 予測可能な実行順序
- デバッグが容易
- パフォーマンスのボトルネックを特定しやすい

### Event Phase

```
User Input → Event → EventTree → View.event()
                                      ↓
                                  Action/State Change
```

```rust
// イベント処理の例
impl View for Button {
    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        match &event.kind {
            EventKind::MouseDown { button, .. } if *button == MouseButton::Left => {
                self.is_pressed = true;
                ctx.request_paint();  // 再描画リクエスト
                ControlFlow::Continue
            }
            _ => ControlFlow::Continue,
        }
    }
}
```

### Layout Phase

```
Root
  ↓
layout(constraints) → Size
  ↓
Children.layout(child_constraints) → ChildSizes
  ↓
Calculate Positions
  ↓
Return Size
```

### Paint Phase

```
Root
  ↓
draw(frame)
  ↓
Children.draw(child_frames)
  ↓
Generate Draw Commands
```

### Compose Phase

```
ViewBuffer ──┐
             ├─▶ Compositor ─▶ Final Frame
ViewBuffer ──┘
```

---

## パフォーマンス最適化

### 1. ダーティ追跡（Dirty Tracking）

AppKit風のHashSetベース追跡：

```rust
pub struct RenderTracker {
    dirty_views: HashSet<ViewId>,
}

impl RenderTracker {
    /// ダーティフラグを設定（O(1)）
    pub fn mark_dirty(&mut self, view_id: ViewId, flags: DirtyFlags) {
        self.dirty_views.insert(view_id);
    }

    /// ダーティチェック（O(1)）
    pub fn is_dirty(&self, view_id: ViewId) -> bool {
        self.dirty_views.contains(&view_id)
    }

    /// クリア
    pub fn clear(&mut self) {
        self.dirty_views.clear();
    }
}
```

**特徴:**
- O(1)のダーティチェック
- 再帰的なダーティ伝播なし
- 最小限の再描画

### 2. RepaintBoundary（部分再描画）

```rust
pub struct RepaintBoundary {
    id: ViewId,
    child: Box<dyn View>,
    buffer: Option<ViewBuffer>,  // オフスクリーンバッファ
    opaque: bool,                 // 不透明度ヒント
}
```

**動作:**

```
Parent View
  ↓ (changes)
RepaintBoundary
  ↓ (isolated)
Child View (in buffer)
  ↓
Composite buffer to parent
```

**使用例:**

```rust
// 高頻度更新のアニメーション
let animated_view = AnimatedView::new()
    .repaint_boundary();  // 親への再描画伝播を防止
```

### 3. BufferPool（バッファプール）

Grow-only戦略によるメモリ管理：

```rust
pub struct BufferPool {
    available: Vec<ViewBuffer>,
    in_use: HashMap<ViewId, ViewBuffer>,
    max_pool_size: usize,
    total_memory: usize,
}

impl BufferPool {
    pub fn acquire(&mut self, view_id: ViewId, size: Size) -> Option<ViewBuffer> {
        // 再利用可能なバッファを検索
        let buffer = self.find_reusable_buffer(size)
            .unwrap_or_else(|| ViewBuffer::new(size));

        self.total_memory += buffer.memory_usage();
        self.in_use.insert(view_id, buffer.clone());
        Some(buffer)
    }

    pub fn release(&mut self, view_id: ViewId) {
        if let Some(buffer) = self.in_use.remove(&view_id) {
            self.available.push(buffer);
        }
    }
}
```

**Grow-only戦略:**
- バッファは成長するだけで、縮小しない
- メモリフラグメンテーションを防止
- パフォーマンスの安定化

### 4. O(1)操作

```rust
// ビュー検索
view_registry.get(view_id)  // O(1) HashMap

// ダーティチェック
tracker.is_dirty(view_id)    // O(1) HashSet

// 無効化通知
observer.notify()            // O(1) HashSet
```

---

## データフロー

### 上り（Data Flow）

```
DataContext<T>
    ↓ (変更)
DataVersion.increment()
    ↓
Observers.notify()
    ↓
DirtyFlags.set(view_id)
    ↓
RenderTracker.mark_dirty()
    ↓
再描画リクエスト
```

### 下り（Event Flow）

```
User Input
    ↓
Event
    ↓
Event Phase
    ↓
View.event()
    ↓
Action実行
    ↓
DataContext.mutate()
    ↓
State更新
    ↓
無効化伝播
    ↓
再描画
```

---

## 実装例

### カスタムViewの実装

```rust
use scarlet_ui::*;

struct CounterView {
    id: ViewId,
    count: Arc<RwLock<i32>>,
}

impl CounterView {
    fn new(count: Arc<RwLock<i32>>) -> Self {
        Self {
            id: ViewId::new(),
            count,
        }
    }
}

impl View for CounterView {
    fn id(&self) -> ViewId {
        self.id
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        Size::new(200, 100)
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        let count = *self.count.read().unwrap();
        // テキスト描画
        // TODO: 実装
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        match &event.kind {
            EventKind::MouseDown { button, .. } if *button == MouseButton::Left => {
                // カウント増加
                *self.count.write().unwrap() += 1;
                ctx.request_paint();
            }
            _ => {}
        }
        ControlFlow::Continue
    }

    fn update(&mut self, _ctx: &mut UpdateCtx) {
        // 定期更新処理
    }
}
```

### レイアウトの使用

```rust
use scarlet_ui::*;

let ui = VStack::new()
    .spacing(10)
    .children(vec![
        Box::new(Text::new("Counter App").font_size(24)),
        Box::new(HStack::new()
            .spacing(10)
            .children(vec![
                Box::new(Button::new("Increment")),
                Box::new(Button::new("Decrement")),
            ])
        ),
        Box::new(Text::new(format!("Count: {}", count))),
    ]);
```

### 修飾子のチェーン

```rust
let styled_button = Button::new("Click Me")
    .padding(15)
    .frame(200, 50)
    .background(Color::PRIMARY)
    .repaint_boundary_opaque();
```

---

## レイアウトコンテナ

`child()` メソッドは自動的に `Box::new()` を処理するため、`Box::new()` を書く必要がありません：

```rust
// VStack（垂直スタック）
let vstack = VStack::new()
    .spacing(10)
    .alignment(CrossAxisAlignment::Center)
    .child(Text::new("Title"))
    .child(Text::new("Subtitle"))
    .child(Button::new("Action"));

// HStack（水平スタック）
let hstack = HStack::new()
    .spacing(15)
    .alignment(MainAxisAlignment::Center)
    .child(Button::new("Cancel"))
    .child(Button::new("OK"));

// ZStack（重ね合わせ）
let zstack = ZStack::new()
    .child(Image::new("background.png"))
    .child(Text::new("Overlay Text"));
```

**特徴:**
- ジェネリックな `child<V: View>` メソッド
- 自動的に `Box` へ変換
- ビルダーパターンでメソッドチェーン可能

### view! マクロ（SwiftUI風）

宣言的なマクロを使うとさらに簡潔に書けます：

```rust
use scarlet_ui::view;

let ui = view! {
    VStack(spacing: 16) {
        Text("Title")
        Text("Subtitle")
        Button("Action")
    }
};
```

**ネストした例:**

```rust
let complex_ui = view! {
    VStack(spacing: 20) {
        Text("Header").set_color(Color::BLACK)
        HStack(spacing: 10) {
            Button("Cancel")
            Button("OK")
        }
    }
};
```

**マクロの特徴:**
- `Box::new()` が不要
- 名前付きパラメータ: `VStack(spacing: 16)`
- メソッドチェーン: `Text("Hello").set_color(Color::BLACK)`
- 入れ子のコンテナを自然に記述

---

## コントロール

### 利用可能なコントロール

1. **Text** - テキスト表示
2. **Button** - クリック可能なボタン
3. **Image** - 画像表示
4. **TextField** - テキスト入力
5. **Toggle** - スイッチ/チェックボックス
6. **Slider** - 値の選択

詳細は `src/view/controls/` を参照してください。

---

## 設計のトレードオフ

### 選択した設計

| 項目 | 選択 | 理由 |
|------|------|------|
| 状態管理 | DataContext<T> | 型安全、単一の真実のソース |
| レイアウト | 制約ベース | 柔軟性、レスポンシブ対応 |
| 修飾子 | メソッドチェーン | 可読性、SwiftUIライク |
| ダーティ追跡 | HashSet | O(1)操作、単純実装 |
| バッファ戦略 | Grow-only | メモリ効率、安定性 |

### 将来の拡張

- **ナビゲーションシステム**: TabView, NavigationView
- **アニメーション**: トランジション、スプリングアニメーション
- **テーマ**: ダークモード、カスタムテーマ
- **国際化**: RTL対応、複数言語
- **アクセシビリティ**: スクリーンリーダー、キーボードナビゲーション

---

## リアクティブプログラミング

ScarletUIは**完全にリアクティブなデータフロー**をサポートしています。複数のViewが同じ状態を監視し、変更時に自動的に更新されます。

### Two-Way Data Binding

UIコントロールと `DataContext` の双方向バインディング：

```rust
use scarlet_ui::*;

fn build_ui() {
    // bindable! マクロで状態を作成
    let enabled = bindable!(false);
    let volume = bindable!(50.0);

    // UIコントロールをバインド
    let toggle = Toggle::bind(&enabled);
    let slider = Slider::bind(&volume, 0.0, 100.0);

    // 値を表示
    let enabled_text = Text::bind(&enabled, |e| if *e { "ON" } else { "OFF" });
    let volume_text = Text::bind(&volume, |v| format!("Volume: {}", v));
}
```

**データフロー:**

```
ユーザーがToggleをクリック
    ↓
ToggleがDataContext<bool>を更新
    ↓
全observer（Textなど）が自動的に再描画
```

### 構造体とのバインディング

複雑な状態は構造体で管理し、Lensを使って個別フィールドにアクセス：

```rust
struct AudioState {
    volume: f32,
    bass: f32,
    treble: f32,
}

fn build_audio_ui() {
    let state = bindable!(AudioState {
        volume: 50.0,
        bass: 30.0,
        treble: 70.0,
    });

    // Lensでvolumeフィールドにフォーカス
    let volume_lens = FnLens::new(
        |s: &AudioState| &s.volume,
        |s: &mut AudioState| &mut s.volume
    );

    // 子DataContextを作成
    let volume_data = state.child(volume_lens);

    // UIにバインド
    VStack::new()
        .spacing(16)
        .child(Text::bind(&volume_data, |v| format!("Volume: {}", *v)))
        .child(Slider::bind(&volume_data, 0.0, 100.0));
}
```

### 複数Viewの連携

```rust
struct CounterState {
    count: i32,
}

fn build_counter() {
    let state = bindable!(CounterState { count: 0 });

    VStack::new()
        .spacing(16)
        .child(Text::bind(&state, |s| format!("Count: {}", s.count)))
        .child(
            Button::new("Increment")
                .set_action(Arc::new(|| {
                    state.modify(|s| s.count += 1);
                }))
        )
        .child(
            Button::new("Decrement")
                .set_action(Arc::new(|| {
                    state.modify(|s| s.count -= 1);
                }))
        );
}
```

### パフォーマンス特性

リアクティブシステムはO(1)で動作：

```rust
// データ変更時
state.modify(|s| {
    s.count += 1;
    // ↓ 自動的に全observerに通知（HashSetでO(1)）
});
```

**特徴:**
- **O(1)通知** - HashSet-based dirty tracking
- **双方向バインディング** - UIコントロールが自動的にデータを更新
- **自動伝播** - データ変更時に全observerが自動更新
- **部分再描画** - 変更したViewのみ再描画
- **Lens対応** - 構造体のサブフィールドにフォーカス可能

---

## まとめ

ScarletUIのアーキテクチャは：

1. **データファースト**: 状態が中心で、UIは自動的に追従
2. **効率的**: O(1)操作、部分再描画、バッファプール
3. **合成可能**: 小さなViewを組み合わせて複雑なUIを構築
4. **予測可能**: 明確なフェーズ分離、単方向データフロー
5. **拡張可能**: トレイトベース、カスタムViewの容易な追加

これにより、高速でメンテナンスしやすく、スケーラブルなUIフレームワークを実現しています。
