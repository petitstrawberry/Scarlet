# ScarletUI Multi-Window / Scene / Backend 抽象化 設計ドキュメント

> 関連 Issue:
> - [#473 — scarlet-ui: multi window](https://github.com/petitstrawberry/Scarlet/issues/473)
> - [#467 — scarlet-ui: host-side live preview backend for app development](https://github.com/petitstrawberry/Scarlet/issues/467)
>
> 状態: **設計段階（未実装）**
>
> 方針: 互換維持ではなく **完全移行**。`Application::body()` を旧 API として残す前提を捨て、`Application::scenes()` を唯一のアプリ UI エントリポイントにする。

---

## 1. 目的

ScarletUI を SwiftUI の `App` / `Scene` / `WindowGroup` に近い形へ移行し、1つのアプリケーションが複数ウィンドウを自然に宣言できるようにする。

同時に、#467 の host-side live preview backend を見据え、`Application::run()` から `SWSPlatformWindow` 直参照を排除する。つまり、本設計は単なる multi-window 化ではなく、以下3つをまとめて扱う。

1. **Scene API への完全移行**
2. **multi-window runtime の導入**
3. **platform backend 抽象化**

これにより、将来的に SWS だけでなく `winit + softbuffer` 等の host backend 上でも同じ `Application` / `Scene` / `RenderingPipeline` を再利用できる。

---

## 2. 決定事項

| 項目 | 決定 |
|------|------|
| App UI エントリ | `Application::scenes()` のみ。`body()` は廃止 |
| App trait | `Application: View` を廃止。アプリは View ではなく Scene provider |
| multi-window | 1ウィンドウ = 1 `RenderingPipeline` + 1 `PlatformWindow` |
| dirty 管理 | `PipelineId` で partition。現状の単一 global dirty set は不可 |
| backend | `Application::run()` は backend 非依存 runner へ移行。SWS は default backend の1つ |
| ライフサイクルフック | `WindowContext` + `dyn PlatformWindow` を受け取る backend-agnostic API にする |
| 終了ポリシー | 最後のウィンドウを閉じたら終了。`exit_when_all_windows_closed()` で変更可能 |
| MVP | 起動時に `scenes()` で宣言した静的複数ウィンドウを開く。動的 `openWindow` は Phase 2 |

---

## 3. 現状の問題

### 3.1 `Application` が単一 View 前提

現在の `Application` は `View` を継承し、`body() -> impl View` で UI を返す。

```rust
pub trait Application: View {
    fn body(&self) -> impl View;
    fn run(&mut self) -> Result<()> { ... }
}
```

これは SwiftUI の `App.body -> some Scene` ではなく、アプリ自体を View として扱う構造であり、複数 top-level window の宣言には向かない。

### 3.2 `Application::run()` が SWS 固定

`application.rs` の `run()` は直接 `SWSPlatformWindow::new_with_menu_and_policies(...)` / `create_with_type_and_menu_and_policies(...)` を呼ぶ。#467 の M0「`Application::run()` から `SWSPlatformWindow` の直参照を外す」と同じ箇所が multi-window 化でもボトルネックになる。

### 3.3 `RenderingPipeline` が1ウィンドウ前提

`RenderingPipeline` は1つの `ElementTree`、1つの `Compositor`、1つの `EventDispatcher` を持つ。これは1ウィンドウ単位としては自然だが、アプリ全体で1つだけ持つ設計は複数ウィンドウに合わない。

### 3.4 global dirty set が multi-pipeline unsafe

現状:

```rust
static GLOBAL_DIRTY_IDS: Mutex<BTreeSet<ElementId>> = Mutex::new(BTreeSet::new());
static GLOBAL_DIRTY_LAYOUT_IDS: Mutex<BTreeSet<ElementId>> = Mutex::new(BTreeSet::new());
static GLOBAL_DIRTY_PAINT_IDS: Mutex<BTreeSet<ElementId>> = Mutex::new(BTreeSet::new());
static GLOBAL_DIRTY_SELF_PAINT_IDS: Mutex<BTreeSet<ElementId>> = Mutex::new(BTreeSet::new());
```

`take_global_dirty_ids()` は集合を丸ごと drain + clear する。複数 pipeline が存在すると、pipeline B が pipeline A の dirty `ElementId` を消費して捨てる可能性がある。

したがって、1ウィンドウ=1 pipeline にするなら、dirty queue は必ず `PipelineId` で分割する。

---

## 4. 新アーキテクチャ概要

```text
Application
  scenes() -> impl Scene
        |
        v
SceneBuilder
  Vec<WindowDeclaration>
        |
        v
ApplicationRunner<B: PlatformBackend>
  Vec<WindowSlot<B::Window>>
        |
        +-- WindowSlot #1
        |     WindowId / SceneWindowKey / PipelineId
        |     RenderingPipeline
        |     B::Window: PlatformWindow
        |
        +-- WindowSlot #2
              WindowId / SceneWindowKey / PipelineId
              RenderingPipeline
              B::Window: PlatformWindow
```

### 4.1 重要な分離

| 概念 | 所有者 | 備考 |
|------|--------|------|
| 宣言された window | `Scene` / `SceneBuilder` | 起動時 declarative model |
| 実行時 window | `WindowSlot` | window id、pipeline、platform window を保持 |
| 描画パイプライン | `RenderingPipeline` | 1 window ごとに独立 |
| OS/backend window | `PlatformWindow` | SWS / host backend で差し替え |
| app state | `Application` と `State<T>` | `State<T>` は Arc 共有。pipeline ごとに subscribe |

---

## 5. App / Scene API

### 5.1 `Application` は View ではなくなる

完全移行なので `Application: View` を廃止する。

```rust
pub trait Application: Clone + 'static {
    /// アプリの top-level scene を宣言する。
    fn scenes(&self) -> impl Scene;

    /// 初期化フック。
    fn init(&mut self) {}

    /// メインループ1 tick ごとの app-level idle hook。
    fn on_idle(&mut self) {}

    /// 各 window が作成された直後に呼ばれる。
    fn on_window_created(
        &mut self,
        _ctx: &WindowContext,
        _window: &mut dyn PlatformWindow,
    ) {
    }

    /// 各 loop tick で window ごとに呼ばれる。
    fn on_window_sync(
        &mut self,
        _ctx: &WindowContext,
        _window: &mut dyn PlatformWindow,
    ) {
    }

    /// window resize hook。
    fn on_window_resize(
        &mut self,
        _ctx: &WindowContext,
        _width: u32,
        _height: u32,
    ) {
    }

    /// 全 window が閉じられたときに app を終了するか。
    fn exit_when_all_windows_closed(&self) -> bool {
        true
    }

    /// debug logging hook。
    fn debug_logging(&self) -> bool {
        false
    }
}
```

`Application::run()` は trait default から薄くする。backend を選ぶ default entry は feature 側に寄せる。

```rust
impl<A: Application> A {
    #[cfg(feature = "sws")]
    pub fn run(&mut self) -> Result<()> {
        self.run_with_backend(SwsBackend::new()?)
    }

    pub fn run_with_backend<B: PlatformBackend>(&mut self, backend: B) -> Result<()> {
        ApplicationRunner::new(backend).run(self)
    }
}
```

Rust では trait に inherent impl は書けないため、実装時には extension trait を使う。

```rust
pub trait ApplicationRunExt: Application {
    fn run_with_backend<B: PlatformBackend>(&mut self, backend: B) -> Result<()>
    where
        Self: Sized,
    {
        ApplicationRunner::new(backend).run(self)
    }

    #[cfg(feature = "sws")]
    fn run(&mut self) -> Result<()>
    where
        Self: Sized,
    {
        self.run_with_backend(SwsBackend::new()?)
    }
}

impl<T: Application> ApplicationRunExt for T {}
```

### 5.2 `Scene`

`Scene` は top-level window declaration を builder に積む。

```rust
pub trait Scene {
    fn build(self, builder: &mut SceneBuilder);
}

pub struct SceneBuilder {
    declarations: Vec<WindowDeclaration>,
}

pub struct WindowDeclaration {
    pub key: SceneWindowKey,
    pub view: Box<dyn View>,
}
```

`Scene` は consuming API にする。`Application::scenes()` を再評価すれば毎回 fresh な scene が得られるため、rebuild 時にも問題ない。

```rust
impl<V> Scene for Window<V>
where
    V: View + Clone + 'static,
{
    fn build(self, builder: &mut SceneBuilder) {
        builder.window("main", self);
    }
}
```

### 5.3 複数 Scene の合成

SwiftUI 的に複数 `Window` を並べられるよう、tuple に `Scene` を実装する。

```rust
impl<A, B> Scene for (A, B)
where
    A: Scene,
    B: Scene,
{
    fn build(self, builder: &mut SceneBuilder) {
        self.0.build(builder);
        self.1.build(builder);
    }
}
```

アリティは `ViewTuple` と同じ方針で macro 展開する。

### 5.4 `WindowGroup`

MVP では1つの `WindowGroup` は1 instance だけ生成する。Phase 2 で `openWindow(id:)` 相当の dynamic spawn に拡張する。

```rust
pub struct WindowGroup<V: View> {
    key: SceneWindowKey,
    window: Window<V>,
}

impl<V> WindowGroup<V>
where
    V: View + Clone + 'static,
{
    pub fn new(key: impl Into<SceneWindowKey>, window: Window<V>) -> Self {
        Self { key: key.into(), window }
    }
}

impl<V> Scene for WindowGroup<V>
where
    V: View + Clone + 'static,
{
    fn build(self, builder: &mut SceneBuilder) {
        builder.window(self.key, self.window);
    }
}
```

### 5.5 利用例

```rust
impl Application for DemoApp {
    fn scenes(&self) -> impl Scene {
        (
            WindowGroup::new(
                "main",
                Window::new("Main", MainView::new(self.state.clone()))
                    .size(Size::new(800.0, 600.0)),
            ),
            WindowGroup::new(
                "inspector",
                Window::new("Inspector", InspectorView::new(self.state.clone()))
                    .size(Size::new(320.0, 600.0)),
            ),
        )
    }
}
```

---

## 6. Identity model

```rust
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SceneWindowKey(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PipelineId(u64);
```

| ID | 役割 |
|----|------|
| `SceneWindowKey` | 宣言上の安定キー。`"main"`, `"settings"`, `"inspector"` など |
| `WindowId` | 実行時に開かれた window instance の ID |
| `PipelineId` | dirty queue partition key。MVP では `WindowId` と 1:1 |

index ではなく `WindowId` を使う。close/remove で slot の index は変わるため、index は永続 identity として不適切。

---

## 7. Backend 抽象化

#467 を見据え、multi-window runner は `SWSPlatformWindow` を知らない。

### 7.1 `PlatformWindow`

既存 trait を backend-neutral に寄せる。

```rust
pub trait PlatformWindow {
    fn poll_event(&mut self) -> Option<Event>;
    fn present(&mut self, buffer: &Buffer);
    fn present_with_damage(&mut self, buffer: &Buffer, damage: Option<&[DamageRect]>);
    fn set_title(&mut self, title: &str);
    fn size(&self) -> Size;
    fn resize(&mut self, width: u32, height: u32) -> Result<()>;
    fn close(&mut self) -> Result<()>;
    fn minimize(&mut self) -> Result<()>;
    fn maximize(&mut self) -> Result<()>;
    fn restore(&mut self) -> Result<()>;
    fn request_move(&mut self) -> Result<()>;
    fn set_resizable(&mut self, resizable: bool) -> Result<()>;
    fn set_opaque(&mut self, opaque: bool) -> Result<()>;
    fn set_menu_titles(&mut self, menu_titles: &str) -> Result<()>;

    /// backend固有拡張用。SWS connection_mut 等はここから downcast する。
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any;

    /// backend が持つ platform-native id。SWS では surface_id。
    fn platform_window_id(&self) -> u64;
}
```

`surface_id()` は SWS 固有名なので、trait 上では `platform_window_id()` に改名する。SWS の `surface_id()` は concrete 型のメソッドとして残してよい。

### 7.2 `PlatformBackend`

Window creation は `PlatformWindow::new` ではなく backend が担う。

```rust
pub trait PlatformBackend {
    type Window: PlatformWindow + 'static;

    fn output_scale_milli(&mut self) -> u32;

    fn create_window(&mut self, request: WindowCreateRequest) -> Result<Self::Window>;

    /// backend 全体のイベントが必要になった場合用。
    /// MVP では None 固定でもよい。
    fn poll_backend_event(&mut self) -> Option<BackendEvent> {
        None
    }
}

pub struct WindowCreateRequest {
    pub app_id: String,
    pub title: String,
    pub size: Size,
    pub window_type: u32,
    pub menu_json: String,
    pub focus_on_create: bool,
    pub active_on_focus: bool,
    pub opaque: bool,
    pub resizable: bool,
}
```

SWS:

```rust
pub struct SwsBackend;

impl PlatformBackend for SwsBackend {
    type Window = SWSPlatformWindow;

    fn create_window(&mut self, request: WindowCreateRequest) -> Result<Self::Window> {
        // 既存 new_with_menu_and_policies / create_with_type... を内部で選択
    }
}
```

Host preview:

```rust
pub struct HostPreviewBackend;

impl PlatformBackend for HostPreviewBackend {
    type Window = HostPreviewWindow; // winit + softbuffer 等
}
```

この分離で #467 の M0「`Application::run()` から SWS 直参照を外す」が multi-window 実装と同時に達成される。

---

## 8. WindowSlot / Runner

### 8.1 `WindowContext`

```rust
pub struct WindowContext {
    pub window_id: WindowId,
    pub scene_key: SceneWindowKey,
    pub pipeline_id: PipelineId,
    pub platform_window_id: u64,
    pub is_primary: bool,
}
```

### 8.2 `WindowSlot`

```rust
struct WindowSlot<W: PlatformWindow> {
    context: WindowContext,
    pipeline: RenderingPipeline,
    window: W,
    presented_this_cycle: bool,
}
```

### 8.3 `ApplicationRunner`

```rust
pub struct ApplicationRunner<B: PlatformBackend> {
    backend: B,
}

impl<B: PlatformBackend> ApplicationRunner<B> {
    pub fn run<A: Application>(&mut self, app: &mut A) -> Result<()> {
        app.init();

        let declarations = collect_scene_declarations(app);
        let mut slots = self.create_slots(app, declarations)?;

        self.run_loop(app, &mut slots)
    }
}
```

---

## 9. SceneWindowRootElement

`ApplicationRootElement` は廃止する。各 window pipeline の root は `SceneWindowRootElement<A>` になる。

役割:

1. `app.scenes()` を再評価する。
2. 自分の `SceneWindowKey` に対応する `WindowDeclaration` を選ぶ。
3. その declaration の `View` から child element を作る。
4. rebuild 時も同じ key を再解決し、既存 child に `update()` する。
5. `mount()` 時に `PipelineId` を dirty callback に渡す。

```rust
struct SceneWindowRootElement<A: Application> {
    id: ElementId,
    app: A,
    scene_key: SceneWindowKey,
    pipeline_id: PipelineId,
    child: Option<Box<dyn Element>>,
}
```

### 9.1 なぜ必要か

View 構築時に State を読むパターンを維持するため。

```rust
Window::new("Counter", Text::new(format!("{}", self.count.get())))
```

この場合、State 更新時に root が rebuild され、`app.scenes()` を再評価しなければ新しい `Text` が作られない。

---

## 10. dirty queue partition

### 10.1 新構造

```rust
struct DirtyQueues {
    build: BTreeSet<ElementId>,
    layout: BTreeSet<ElementId>,
    paint: BTreeSet<ElementId>,
    self_paint: BTreeSet<ElementId>,
}

static GLOBAL_DIRTY: Mutex<BTreeMap<PipelineId, DirtyQueues>> = Mutex::new(BTreeMap::new());
```

### 10.2 mark API

```rust
pub fn mark_element_dirty(owner: PipelineId, id: ElementId);
pub fn mark_element_needs_layout(owner: PipelineId, id: ElementId);
pub fn mark_element_needs_paint(owner: PipelineId, id: ElementId);
pub fn mark_element_needs_self_paint(owner: PipelineId, id: ElementId);
```

### 10.3 `Element::mount` 破壊的変更

```rust
pub struct MountContext {
    pub pipeline_id: PipelineId,
}

pub trait Element {
    fn mount(&mut self, ctx: &MountContext);
    fn unmount(&mut self);
    // ...
}
```

全 Element 実装は子要素へ同じ `MountContext` を伝播する。State subscription callback は `ctx.pipeline_id` を capture する。

---

## 11. run loop

```rust
loop {
    let mut any_event = false;
    let mut any_presented = false;
    let mut close_ids = Vec::new();

    // backend-global events（host backend 用の余地）
    while let Some(event) = self.backend.poll_backend_event() {
        any_event = true;
        handle_backend_event(event, &mut slots, &mut close_ids)?;
    }

    // window-local events
    for slot in slots.iter_mut() {
        slot.presented_this_cycle = false;

        while let Some(event) = slot.window.poll_event() {
            any_event = true;
            handle_window_event(app, slot, event, &mut close_ids)?;
        }
    }

    remove_closed_slots(&mut slots, close_ids)?;

    if slots.is_empty() && app.exit_when_all_windows_closed() {
        return Ok(());
    }

    app.on_idle();

    for slot in slots.iter_mut() {
        app.on_window_sync(&slot.context, &mut slot.window);
        sync_text_input(&mut slot.window, &slot.pipeline);

        if slot.pipeline.has_dirty() && !slot.presented_this_cycle {
            if present_pipeline(&mut slot.pipeline, &mut slot.window) {
                any_presented = true;
                slot.presented_this_cycle = true;
            }
        }
    }

    if !any_event && !any_presented {
        sleep_16ms();
    }
}
```

`Event::Quit` はアプリ全体終了ではなく、その event を出した window slot の close として扱う。アプリ全体終了は「slot が0個になった」または将来の explicit app quit command に委ねる。

---

## 12. backend-specific escape hatch

`terminal/main.rs` のように SWS connection に直接触りたいケースがある。新 API では `dyn PlatformWindow` を downcast する。

```rust
impl Application for TerminalApp {
    fn on_window_created(
        &mut self,
        ctx: &WindowContext,
        window: &mut dyn PlatformWindow,
    ) {
        if let Some(sws) = window.as_any_mut().downcast_mut::<SWSPlatformWindow>() {
            // sws.connection_mut() で text-input context を作る
        }
    }
}
```

Host backend ではこの downcast は失敗する。必要なら別の backend-neutral text input API を `PlatformWindow` に追加する。#467 の host preview ではまず最低限の keyboard/text event mapping を backend 側で行う。

---

## 13. `WindowInfo` / `WindowCreateRequest`

`RenderingPipeline::layout_initial()` が返す `WindowInfo` を backend window 作成要求へ変換する。

```rust
impl WindowCreateRequest {
    pub fn from_window_info(info: WindowInfo, limits: WindowSizeLimits) -> Self {
        Self {
            app_id: info.app_id,
            title: info.title,
            size: info.size,
            window_type: info.window_type,
            menu_json: info.menu_bar.as_ref().map(|m| m.to_json()).unwrap_or_default(),
            focus_on_create: info.focus_on_create,
            active_on_focus: info.active_on_focus,
            opaque: info.opaque,
            resizable: limits.resizable,
        }
    }
}
```

`RenderingPipeline::extract_window_info()` は「1 pipeline 内の root window を探す」用途として残す。アプリ全体の multi-window discovery は `SceneBuilder` 側が担う。

---

## 14. menu / text input / focus

### 14.1 menu

現状 `menu_model::register_menu_callbacks(window_id, menu_bar)` と `invoke_menu_callback(window_id, item_id)` がある。multi-window では window close 時の cleanup が必須。

```rust
pub fn unregister_menu_callbacks(window_id: u32);
```

SWS では `window_id = surface_id`。backend-neutral runner では `platform_window_id` を使うが、`menu_model` が u32 前提なら SWS 専用で保持するか、`u64` 化する。

### 14.2 text input

`sync_text_input` は slot ごとに呼ぶ。text input events は発生元 `PlatformWindow` の slot pipeline にのみ dispatch する。

### 14.3 focus

window 内 focus は `EventDispatcher` ごとに独立。app-level focused window は `WindowContext.window_id` で追跡する。

---

## 15. #467 との関係

この設計は #467 の M0 を内包する。

| #467 milestone | 本設計での対応 |
|----------------|----------------|
| M0: `Application::run()` の backend 抽象化 | `ApplicationRunner<B: PlatformBackend>` により対応 |
| M1: `std` feature 整備 | 本設計の直接対象外。ただし `PlatformBackend` を crate 分離しやすくする |
| M2: host backend 実装 | `HostPreviewBackend: PlatformBackend` として追加可能 |
| M3: host 向け run loop | 同じ `ApplicationRunner` を使う。backend-global event の hook を用意 |
| M4: hot reload | `SceneWindowRootElement` が `app.scenes()` を再評価するため、view tree 差し替えの入口になる |

重要: multi-window 実装で SWS 固定のまま `Vec<SWSPlatformWindow>` にしてしまうと、#467 で再度 runner を剥がす必要が出る。ここで backend abstraction まで入れる方が二度手間を避けられる。

---

## 16. MVP 実装範囲

1. `Application: View` を廃止し、`Application::scenes()` 必須化
2. `Scene` / `SceneBuilder` / `WindowGroup` / tuple Scene 実装追加
3. `WindowId` / `SceneWindowKey` / `PipelineId` / `WindowContext` 追加
4. `PlatformBackend` / `ApplicationRunner` 追加
5. `SwsBackend: PlatformBackend` 実装
6. `Application::run()` を SWS default extension として再提供
7. `WindowSlot` ベースの multi-window run loop 実装
8. `SceneWindowRootElement` 追加、`ApplicationRootElement` 廃止
9. dirty queue を `PipelineId` partition に変更
10. `Element::mount(&MountContext)` へ破壊的変更
11. `PlatformWindow` を backend-neutral に整理し `as_any_mut` / `platform_window_id` 追加
12. `menu_model::unregister_menu_callbacks` 追加
13. 既存 scarlet-ui apps を全て `scenes()` へ移行

---

## 17. Phase 2

- 動的 `open_window(key)` / `close_window(id)` command API
- `WindowGroup` の複数 instance 管理
- `openWindow(value:)` 的な front-or-create セマンティクス
- `scenePhase`
- window duplication / restoration
- host preview backend crate (`scarlet-ui-host`) 実装
- hot reload / live preview
- backend-neutral text input capability

---

## 18. 既存アプリ移行例

### 18.1 Before

```rust
impl Application for DemoApp {
    fn body(&self) -> impl View {
        Window::new("Demo", DemoView::new(self.state.clone()))
            .size(Size::new(800.0, 600.0))
    }
}
```

### 18.2 After

```rust
impl Application for DemoApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new(
            "main",
            Window::new("Demo", DemoView::new(self.state.clone()))
                .size(Size::new(800.0, 600.0)),
        )
    }
}
```

### 18.3 複数 window

```rust
impl Application for DemoApp {
    fn scenes(&self) -> impl Scene {
        (
            WindowGroup::new(
                "main",
                Window::new("Main", MainView::new(self.state.clone()))
                    .size(Size::new(800.0, 600.0)),
            ),
            WindowGroup::new(
                "inspector",
                Window::new("Inspector", InspectorView::new(self.state.clone()))
                    .size(Size::new(320.0, 600.0)),
            ),
        )
    }
}
```

---

## 19. ファイル別変更マップ

| ファイル | 変更 |
|---------|------|
| `src/application.rs` | `Application` trait 再定義、`ApplicationRootElement` 廃止、runner 呼び出しへ移行 |
| `src/scene.rs` | 新規。`Scene`, `SceneBuilder`, `WindowGroup`, IDs, `WindowContext` |
| `src/runner.rs` | 新規。`ApplicationRunner`, `WindowSlot`, main loop |
| `src/platform/mod.rs` | `PlatformWindow` 整理、`PlatformBackend` 追加または `platform/backend.rs` に分離 |
| `src/platform/sws.rs` | `SwsBackend` 実装、`SWSPlatformWindow: PlatformWindow` 調整 |
| `src/pipeline/owner.rs` | dirty queue partition |
| `src/pipeline/rendering.rs` | `PipelineId` 保持、`with_owner` 追加 |
| `src/element/mod.rs` | `mount(&MountContext)` へ破壊的変更 |
| `src/element/*` | mount propagation / dirty mark owner 対応 |
| `src/views/window.rs` | `WindowGroup` もしくは `impl Scene for Window` 周辺 |
| `src/menu_model.rs` | unregister 追加、id 型見直し |
| `src/lib.rs` | new modules / prelude exports |
| `user/bin/src/*.rs` | `body()` から `scenes()` へ移行 |
| `user/std-bin/src/*.rs` | 同上 |

---

## 20. リスク

| リスク | 影響 | 対策 |
|--------|------|------|
| `Element::mount` 破壊的変更が大規模 | 高 | 先に `MountContext` を最小型で導入し、機械的に全 Element へ伝播 |
| backend abstraction と multi-window を同時にやるため差分が大きい | 高 | 実装を PR/commit 単位で M0 dirty partition → Scene API → Backend → Runner に分ける |
| `dyn PlatformWindow` downcast が増える | 中 | SWS 固有機能は extension trait / capability API に順次昇格 |
| host backend が求める event loop と SWS polling loop が違う | 中 | `PlatformBackend::poll_backend_event` を用意し、MVP は polling、host では winit integration を別途設計 |
| `scenes()` が every rebuild で全宣言を再生成する | 低 | MVP では window 数が少ないため許容。Phase 2 で declaration cache 検討 |

---

## 21. 結論

本気で将来の #467 まで見据えるなら、`body()` 互換を残す中途半端な移行は避けるべき。

ScarletUI の新しい中核は次の形にする。

```text
Application::scenes()
  -> Scene declarations
  -> ApplicationRunner<B: PlatformBackend>
  -> WindowSlot { WindowId, PipelineId, RenderingPipeline, PlatformWindow }
```

この設計なら、multi-window と host preview backend の両方に対して一度の大きな整理で土台を作れる。
