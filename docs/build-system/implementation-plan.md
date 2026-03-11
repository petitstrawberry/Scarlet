# BSP-Rooted Build System Implementation Plan

## Goals

- Make the **BSP project** the main Scarlet project model
- Keep `scarlet-config.toml` as the source of truth for build composition
- Generate `.scarlet/scarlet-modules/` inside the BSP project before Cargo runs
- Preserve the existing kernel boot and initcall contract
- Avoid per-build `Cargo.toml` editing in BSP projects
- Keep kernel, drivers, ABIs, filesystems, and subsystems as real reusable crates

## Key Decision: Is `cargo scarlet` required?

### Short answer

**No, `cargo scarlet` is not strictly required for the architecture to work.**

The architecture only requires a build tool that can:

1. read `scarlet-config.toml`
2. generate `.scarlet/scarlet-modules/`
3. invoke Cargo for the BSP project

That tool could initially be implemented in several forms:

- a top-level `cargo make` task in the Scarlet repository
- an `xtask`-style helper binary
- a standalone `scarlet-build` binary
- a Cargo subcommand such as `cargo scarlet`

### Recommended decision

**Do not start with `cargo scarlet` as a hard requirement.**

Start with a plain build tool implementation first, then add a `cargo scarlet` UX layer once the generation model is stable.

Why:

- the hard problem is **config resolution and generated dependency synthesis**, not the CLI shape
- delaying the Cargo subcommand keeps the early implementation simpler
- the same generator core can later be wrapped by `cargo scarlet`
- this avoids coupling architectural validation to a specific command-dispatch model too early

So the recommended sequence is:

1. implement generator core
2. integrate it with repo-local `cargo make`
3. validate BSP-rooted workflow end to end
4. only then promote the UX to `cargo scarlet`

## Scope & Assumptions

- Current Scarlet repository layout remains the implementation baseline during development
- The long-term user-facing model is a BSP-rooted external project
- Kernel remains the stable `scarlet` core crate
- User-facing kernel/module distribution can assume registry/online acquisition as a normal case
- Current in-tree Scarlet development still uses local-path workflows
- Kernel source may come from a local tree, vendored checkout, git repository, or registry-like distribution source
- Module crates provide `force_link()` anchors and integrate with the existing initcall model
- Generated state lives at `<bsp-project-root>/.scarlet/scarlet-modules/`
- `target/` is not used as the canonical location for generated source inputs

## Architecture Overview

### User-facing model

```text
my-board-project/
├─ scarlet-config.toml
├─ Cargo.toml
├─ src/main.rs
└─ .scarlet/
   └─ scarlet-modules/
```

### Build-time flow

```text
scarlet-config.toml
        │
        ▼
generator resolves kernel + module dependency graph
        │
        ▼
generate .scarlet/scarlet-modules/Cargo.toml
generate .scarlet/scarlet-modules/src/lib.rs
        │
        ▼
run cargo in BSP project root
        │
        ▼
BSP links kernel core + generated module crate
        │
        ▼
selected module crates survive linking
        │
        ▼
kernel executes existing initcall pipeline
```

## Implementation Plan

### Phase 1 — Define and stabilize the contract

#### A. Documentation contract
- Finalize the build-system architecture docs
- Finalize the `scarlet-config.toml` specification
- Finalize the expected BSP-rooted project shape
- Keep `scarlet-config.toml` `.config`-style and move module metadata out of the config format

#### B. Generated crate contract
- Standardize the generated package name: `scarlet-modules`
- Standardize generated location: `.scarlet/scarlet-modules/`
- Standardize generated API surface: `pub fn force_link()`

#### C. Module crate contract
- Define the required module crate anchor (`force_link()`)
- Document that the anchor must retain the same object/module region that contains initcall registrations

### Phase 2 — Build the generator core

#### A. Parse and validate config
- Parse `scarlet-config.toml`
- Validate config version
- Validate board target info
- Validate kernel source info for registry / git / path acquisition
- Validate that all configured module options exist in the selected registry/catalog
- Validate that each module entry uses exactly one valid dependency-style source form plus an explicit `enabled` field
- Validate that every version-based module entry explicitly names its registry
- Validate registry-defined feature requirements and module conflicts
- Validate that no enabled option depends on another option explicitly set to `false`
- Validate the generated graph with `cargo metadata` after synthesis

#### B. Resolve module dependencies
- Resolve kernel dependency source from config
- Load registry/catalog metadata for the selected board/profile
- Read dependency-style module entries from `[modules]`
- Convert resolved kernel/module sources into Cargo dependency entries
- Filter only `enabled = true` module options into the generated crate

#### C. Generate BSP-local crate
- Write `.scarlet/scarlet-modules/Cargo.toml`
- Write `.scarlet/scarlet-modules/src/lib.rs`
- Make output deterministic
- Ensure regeneration is idempotent when config does not change
- Run `cargo metadata` on the BSP project after generation and treat resolution/feature breakage as a hard error

### Phase 3 — Integrate with the in-tree Scarlet repository

#### A. Prototype integration in current repo
- Update the in-tree BSP prototype manifests to include the static dependency on `.scarlet/scarlet-modules`
- Add a generation pre-step before BSP Cargo commands
- Keep the repo as the implementation testbed for the external BSP-rooted model

#### B. `cargo make` integration
- Add a `generate-scarlet-modules` task
- Make BSP build/check/clippy/run tasks depend on generation
- Ensure clean workflows preserve reproducibility

#### C. Validation
- Build RISC-V BSP through the generation flow
- Build AArch64 BSP through the generation flow
- Verify that selected modules remain linked
- Verify that initcalls still execute through the existing kernel path

### Phase 4 — BSP-rooted external project workflow

#### A. Create a reference BSP project template
- Provide a minimal external BSP project skeleton
- Include `Cargo.toml`, `src/main.rs`, and `scarlet-config.toml`
- Document user-facing registry-based examples and in-tree local-path examples for kernel and modules

#### B. Tooling bootstrap
- Provide a bootstrap command that generates `.scarlet/scarlet-modules` before IDE metadata runs
- Document how rust-analyzer should work with the generated crate

#### C. End-to-end verification
- Create a sample external BSP project
- Run generation + build from the BSP root
- Confirm the workflow no longer depends on monorepo-specific paths

### Phase 5 — CLI productization

This is where `cargo scarlet` becomes useful.

#### A. Promote the generator into a stable tool
- Extract the generator core into a reusable library or dedicated binary
- Define stable CLI arguments and diagnostics

#### B. Add friendly command surface
- `cargo scarlet generate`
- `cargo scarlet build`
- `cargo scarlet check`
- `cargo scarlet menuconfig` (future)

#### C. Keep CLI thin
- The CLI should be a thin UX wrapper around the validated generator/build pipeline
- It should not become the place where build semantics are duplicated in a second implementation

## Acceptance Criteria

The plan is considered complete when all of the following are true:

1. BSP-rooted project model is fully documented
2. `scarlet-config.toml` is documented and validated by tooling
3. `.scarlet/scarlet-modules/` is generated from config before Cargo runs
4. BSP manifests remain static across module selection changes
5. selected module crates are retained in the final kernel image
6. existing initcall execution remains the runtime initialization mechanism
7. both in-tree prototype BSPs build through the new generation flow
8. at least one external BSP-rooted sample project works end to end

## Risks

### 1. IDE and metadata timing
- rust-analyzer and `cargo metadata` expect path dependencies to exist before build
- mitigation: explicit generation/bootstrap step

### 2. Force-link contract drift
- module authors may implement `force_link()` incorrectly
- mitigation: document and test the module crate contract carefully

### 3. Monorepo path leakage
- prototype paths from the Scarlet repo may accidentally leak into the external project model
- mitigation: maintain an external sample BSP project as a first-class validation target

### 4. CLI over-design too early
- starting with `cargo scarlet` could distract from the actual dependency-synthesis problem
- mitigation: delay Cargo-subcommand UX until after generator correctness is proven

## Recommended Immediate Next Steps

1. implement the generator core
2. integrate generation into current `cargo make` BSP tasks
3. patch the in-tree BSP prototype to depend on `.scarlet/scarlet-modules`
4. prove force-link + initcall retention with one real module
5. only then decide whether to expose the workflow as `cargo scarlet`

## Summary

`cargo scarlet` is a good eventual UX, but it is **not the first thing that must exist**.

The first real milestone is simpler:

> given a BSP-rooted project and `scarlet-config.toml`, generate `.scarlet/scarlet-modules`, then build successfully with ordinary Cargo.
