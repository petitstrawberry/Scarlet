# ScarletUI 性能最適化の試みと教訓

**日付**: 2026-01-17
**目的**: settingsウィンドウ開閉時の266KBリークと、徐々に重くなる問題の解決

## 結論

**全ての修正は効果がなかった**。根本原因は「そもそもWindow起動時点から異様に重い」ことだった。

## 実施した修正（参考用）

### 1. Stateコールバックの自動cleanup（未使用）

**場所**: `user/lib/scarlet-ui/src/state.rs`, `user/lib/scarlet-ui/src/view/controls.rs`

**内容**:
- `StateInner<T>` に `unsubscribe_view()` メソッド追加
- ReactiveLabel, Text, ProgressBar, ListView に Drop trait 実装追加
- cleanup_handlers を使ってunsubscribeを実行

**問題**: Dropが呼ばれていなかった（アプリが正常に終了していないため）

```rust
// State.rs
impl<T> StateInner<T> {
    fn unsubscribe_view(&mut self, handle: &ViewRefreshHandle) {
        let mut view_handles = self.view_handles.lock();
        view_handles.retain(|weak| {
            if let Some(strong) = weak.upgrade() {
                !Arc::ptr_eq(&strong, &handle.needs_refresh)
            } else {
                false
            }
        });
    }
}

// controls.rs - Text
pub struct Text {
    // ...
    cleanup_handlers: Vec<Box<dyn FnOnce() + Send>>,
}

impl Drop for Text {
    fn drop(&mut self) {
        for handler in self.cleanup_handlers.drain(..) {
            handler();
        }
    }
}
```

### 2. VIEW_REFRESH_QUEUE削除（未使用）

**場所**: `user/lib/scarlet-ui/src/state.rs:46, 647-654`

**内容**: 使用されないグローバルキューによるメモリリークを解消

```rust
// 削除
static VIEW_REFRESH_QUEUE: Mutex<Vec<ViewRefreshHandle>> = Mutex::new(Vec::new());

pub fn take_pending_refreshes() -> Vec<ViewRefreshHandle> {
    core::mem::take(&mut *VIEW_REFRESH_QUEUE.lock())
}
```

### 3. BTreeMapの線形探索修正（未使用）

**場所**: `user/lib/scarlet-ui/src/application.rs`

**内容**: `self.windows.get_mut(&surface_id)` を直接使用

### 4. notify_observersのロック削減（未使用）

**場所**: `user/lib/scarlet-ui/src/state.rs:119-159`

**内容**: コールバックのロック回数を3回→2回に削減

### 5. レイアウトアルゴリズム最適化（未使用）

**場所**: `user/lib/scarlet-ui/src/view/containers.rs`

**内容**: VStack/HStackの反復回数を3パス→2パスに削減

### 6. Application::run()のシグネチャ変更（未使用）

**場所**: `user/lib/scarlet-ui/src/application.rs`

**内容**:
- `pub fn run(&mut self) -> !` → `pub fn run(mut self) -> i32`
- `terminate()` を削除し、`break 'event_loop` に変更
- イベントループ終了後に `return 0`

**問題**: Dropが呼ばれていなかった

## 教訓

### 1. Dropが呼ばれない時の調査

`std::task::exit(0)` はDropを呼ばない。代わりに：
1. 明示的にクリーンアップしてからexitする
2. または、所有権を使って関数からreturnしてDropを呼ばせる

でも今回、**そもそもアプリが正常に動作していなかった**ので、Dropが呼ばれることはありませんでした。

### 2. 問題の根本原因の特定

「メモリリークだ」と思っていても、実は「パフォーマンスが悪くてアプリが応答していないだけ」だった可能性があります。

**調査すべきこと**:
1. プロファイリングをして、どこがボトルネックになっているか
2. Layoutが何回呼ばれているか、どのViewが重いのか
3. State更新が何回起きているか、notify_observersが何回呼ばれているか

### 3. 正常な動作の定義

- settingsウィンドウが開いてすぐ閉じる → 異常
- ウィンドウ開閉を繰り返すと重くなる → 異常
- 起動時点から既に重い → **これが本当の問題**

「最初から重い」ということは、最近の変更ではなく、**最初から設計が重い**可能性があります。

## 次のステップ

1. **プロファイリング**: どこが重いのかを特定
2. **Layoutの最適化**: 3パスではなく1パスに
3. **State更新の削減**: 不必要なnotify_observersを減らす
4. **キャッシュの活用**: measure_text_sizedの結果をキャッシュ

## 変更履歴

- `806f93ec`: 正常だったポイント（reset先）
- `66e32ff8`〜`a28df561`: 問題があるコミット群
