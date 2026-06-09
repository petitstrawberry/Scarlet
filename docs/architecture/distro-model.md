# Scarlet Distro Architecture Design

> Refines and supersedes the original proposal in [GitHub Issue #461](https://github.com/petitstrawberry/Scarlet/issues/461).

## Overview

Scarlet の構成管理を「すべて入りの monorepo」から「再利用可能なコンポーネントで構成される
プラットフォーム」へ移行するためのアーキテクチャ設計。

Zephyr `west`、Yocto/BSP のレイヤーモデル、FreeRTOS 配布形式を参考に、
関心の分離 (Separation of Concerns) を基本原則とする。

## Core Principle: Separation of Concerns

現在の `project` は 3 つの異なる関心を混在させている:

1. **どのハードウェアで動かすか** (Board Support Package)
2. **どのユーザランドを入れるか** (Image Recipe)
3. **どこからコンポーネントを取得するか** (Distro Manifest)

これらを独立したレイヤーに分離する。

## 3-Layer Architecture

```
┌─────────────────────────────────────────────┐
│          Distro Manifest (scarlet.toml)      │
│  WHERE: コンポーネントのソースと revision     │
│  kernel, SDK, packages, libraries            │
│  scarlet.lock で pin                         │
│  local override 可 (scarlet.local.toml)      │
├─────────────────────────────────────────────┤
│          Image Recipe (image.toml)           │
│  WHAT: イメージに何を入れるか                  │
│  ユーザランドの構成宣言                        │
│  BSP を参照する                              │
│  パッケージ・アプリの配置先を指定              │
├─────────────────────────────────────────────┤
│          Board Support Package (bsp.toml)    │
│  HOW: このハードウェアでカーネルを動かす       │
│  カーネル設定、ボード定義、ブート設定          │
│  ユーザランドは触らない                       │
│  実行計画は .scarlet/ 以下に自動生成          │
└─────────────────────────────────────────────┘
```

## File Responsibilities

### scarlet.toml — Distro Manifest

リポジトリルートに配置。ディストリビューション全体の構成を定義する。

```toml
# scarlet.toml
[distro]
name = "scarlet-reference"
version = "0.17.0"

[kernel]
source = { path = "kernel" }
# or: source = { git = "https://github.com/petitstrawberry/scarlet-kernel", branch = "main" }

[packages.coreutils]
source = { path = "packages/coreutils" }

[libs.std]
source = { path = "user/lib/std" }

[libs.ui]
source = { path = "user/lib/scarlet-ui" }
```

```toml
# scarlet.lock (自動生成)
[kernel]
rev = "abc1234"

[packages.coreutils]
rev = "def5678"
```

```toml
# scarlet.local.toml (gitignored, 開発時の上書き)
[kernel]
source = { path = "../scarlet-kernel" }

[apps.myapp]
source = { path = "../my-scarlet-app" }
```

### bsp.toml + kernel.toml — Board Support Package

`bsps/<name>/` に配置。ハードウェアでカーネルを動かすための設定。
ユーザランドには関与しない。

```toml
# bsps/riscv64-limine/bsp.toml
[board]
name = "riscv64-limine"
target = "riscv64gc-unknown-none-elf"
target_json = "../../kernel/targets/riscv64gc-unknown-none-elf.json"
boot_protocol = "limine"

[boot]
cmdline = "console=ttyS0"
```

```toml
# bsps/riscv64-limine/kernel.toml
[kernel]
package = "scarlet"
source = { path = "../../kernel" }

[kernel.features]
network = true
user-fpu = true
user-vector = true
hypervisor = true
limine = true

[modules]
"scarlet-module-prototype" = { path = "../../modules/scarlet-module-prototype", enabled = false }
```

BSP ディレクトリ構成:

```
bsps/riscv64-limine/
  bsp.toml           # ボード・ブート定義
  kernel.toml        # カーネルビルド設定
  src/main.rs        # カーネルエントリポイント
  Cargo.toml         # カーネルバイナリ crate
  lds/               # リンカスクリプト
  .cargo/config.toml # Cargo ビルド設定
```

### image.toml — Image Recipe

`images/` に配置。ユーザランドの構成を宣言する。
BSP を名前で参照し、どのパッケージ・アプリを入れるかを指定する。

```toml
# images/full.toml
[bsp]
name = "riscv64-limine"
source = { path = "../bsps/riscv64-limine" }

[[package]]
name = "coreutils"
bins = ["cat", "ls", "cp", "mv", "rm", "mkdir", "echo", "uname"]
install_dir = "/system/scarlet/bin"

[[app]]
name = "init"
install = "/system/scarlet/bin/init"

[[app]]
name = "sh"
install = "/system/scarlet/bin/sh"

[[app]]
name = "scarlet_desktop"
install = "/system/scarlet/bin/scarlet_desktop"

[[app]]
name = "terminal"
install = "/system/scarlet/bin/terminal"

[[app]]
name = "gpu_probe"
install = "/system/scarlet/bin/gpu_probe"
```

```toml
# images/minimal.toml (同じ BSP、別の構成)
[bsp]
name = "riscv64-limine"
source = { path = "../bsps/riscv64-limine" }

[[app]]
name = "init"
install = "/system/scarlet/bin/init"

[[app]]
name = "sh"
install = "/system/scarlet/bin/sh"
```

## Generated Artifacts

`cargo-scarlet` は `.scarlet/` 以下にビルド成果物を生成する。
現在の `image.steps` はこの生成物の一部となり、ユーザーは直接触らない。

```
.scarlet/
  images/
    <image-name>/
      bsp.toml             # コピー/解決済みの BSP 設定
      kernel.toml          # コピー/解決済みのカーネル設定
      image-plan.toml      # image.toml から展開された実行計画
      initramfs-*.cpio     # ビルド済み initramfs
      rootfs-*.ext2        # ビルド済み rootfs
      boot-*.img           # ビルド済みブートイメージ
```

## SDK Commands (cargo-scarlet)

```bash
# コンポーネントの取得・更新
cargo scarlet update

# BSP + Image を指定してビルド
cargo scarlet build --bsp bsps/riscv64-limine --image images/full

# イメージの生成
cargo scarlet image --bsp bsps/riscv64-limine --image images/full

# 実行
cargo scarlet run --bsp bsps/riscv64-limine --image images/full

# 新規作成
cargo scarlet new bsp my-board --target riscv64gc-unknown-none-elf
cargo scarlet new image my-image --bsp bsps/riscv64-limine
cargo scarlet new module my-module
cargo scarlet new app my-app

# ローカル開発用 override
cargo scarlet local kernel ../scarlet-kernel
```

## Migration from Current Structure

### Phase 1: Introduce image.toml and split scarlet-config.toml

- Add `image.toml` support to `cargo-scarlet`
- Split `scarlet-config.toml` into `bsp.toml` + `kernel.toml`
- Migrate `mkfs/make_initramfs.sh` logic into `image.toml` driven composition
- Keep backward compatibility with existing `scarlet-config.toml`

### Phase 2: Restructure directories

- `projects/` → `bsps/`
- Add `images/` directory with image recipes
- Keep `user/bin/` monolith for now (split separately)

### Phase 3: Split user/bin monolith

- Break `user/bin/Cargo.toml` into individual app crates
- Enable per-app builds
- `image.toml` can reference apps by name

### Phase 4: Add scarlet.toml (Distro Manifest)

- Introduce `scarlet.toml` for component source management
- Add `scarlet.lock` for revision pinning
- Add `scarlet.local.toml` for development overrides
- Support external git/path sources

### Phase 5: Repository split (when needed)

- Move kernel to `petitstrawberry/scarlet-kernel` when external contributors need it
- Move coreutils to `petitstrawberry/scarlet-coreutils`
- Keep `petitstrawberry/Scarlet` as the reference distro

## Relationship to Current Code

| Current | New | Notes |
|---|---|---|
| `scarlet-config.toml` | `bsp.toml` + `kernel.toml` | Split by concern |
| `[[image.steps]]` | `image.toml` → generated plan | User writes recipe, system generates steps |
| `mkfs/make_initramfs.sh` | `cargo-scarlet image` | Shell scripts → tool-driven |
| `projects/<name>/` | `bsps/<name>/` + `images/<name>.toml` | BSP and Image separated |
| `cargo-scarlet` (current) | Extended with new subcommands | Evolved, not replaced |
| N/A | `scarlet.toml` + `scarlet.lock` | New: distro manifest |

## Design Decisions

### Why BSP does not include userland

BSP の責務は「カーネルをこのハードウェアで動かす」ことに限定する。
ユーザランドの構成は image.toml 側で判断する。

例: GPU 搭載ボードに `gpu_probe` を入れたい場合、
BSP に gpu_probe を追加するのではなく、
そのボード向けの image.toml に gpu_probe を含める。

これにより、同じ BSP で「フル構成」と「最小構成」を自由に作れる。

### Why image.steps stays as generated artifact

`image.steps`（現 `[[image.steps]]`）の実行モデルは健全。
問題は「ユーザーが直接書いている」こと。
`image.toml` から自動生成される中間表現として `.scarlet/image-plan.toml` に配置する。
エスケープハッチとして直接 `image-plan.toml` を編集することも可能にする。

### Why kernel.toml is separate from bsp.toml

同じボードで異なるカーネル設定を使うケースがある:

- デバッグ用（profiler 有効）
- リリース用（profiler 無効）
- 実験用（特定機能の有効化/無効化）

`bsp.toml`（ハードウェア定義）と `kernel.toml`（ビルド設定）の分離により、
ハードウェア設定を変えずにカーネル設定だけ差し替えられる。

### Why not Git submodules

Git submodules は構成管理の基本メカニズムとしては不適:

- ブランチ単位の追跡しかできない（revision pin が弱い）
- ローカル override の仕組みがない
- 複数の submodule にまたがる atomic な更新が困難
- ユーザー体験が悪い（clone 時の --recursive、submodule update 等）

代わりに `scarlet.toml` + `scarlet.lock` による manifest/lock モデルを採用する。
これは Zephyr `west`、Bazel WORKSPACE、Cargo Cargo.lock と同じアプローチ。
