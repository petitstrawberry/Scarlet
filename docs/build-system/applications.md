# Native Scarlet applications

System applications live in `/applications/<lowercase-slug>.app`. An app is a
finished directory containing native executables, resources and exactly one root
`<stable-app-id>.desktop` descriptor. The app ID comes from that filename and is
independent of the directory slug. A bundle remains a group of image layers.

`app.toml` is the single editable metadata and build recipe. The SDK generates
the runtime descriptor; recipes are not copied into the finished app. Example:

```toml
[app]
id = "org.example.player"
slug = "player"
name = "Player"
exec = "bin/player"
args = ["%F"]
icon = "resources/icon.png"
mime-types = ["audio/wav"]

[build]
kind = "cargo"
source = "."
package = "player"
bin = "player"
features = ["native"]
default-features = false

[[resources]]
source = "art/icon.png"
to = "resources/icon.png"
```

Build in the application's Scarlet development environment:

```sh
cargo scarlet app build --source . --target aarch64-unknown-scarlet \
  --release --output dist/player.app
```

The output must be new; an existing app is never overwritten. A failed build
leaves no published app. Source output must match the recipe's slug. A prebuilt
app can be copied to another lowercase slug without changing its ID:

```sh
cargo scarlet app build --source dist/player.app \
  --target aarch64-unknown-scarlet --output release/renamed.app
```

The first implementation supports one native AArch64 or RISC-V64 target per app.
It checks ELFOSABI_SCARLET, architecture, file types, executable permissions and
`/bin/scarlet-ld` for any interpreter. It rejects escaping source/resource paths,
absolute runtime paths, `..`, symlinks, multiple descriptors, mixed-ABI ELF files,
and shipped `app.toml`, `Cargo.toml` or `bundle.toml` recipes. Runtime paths use
portable ASCII path components; arguments may contain spaces and Unicode.

Other build recipes are practical for audited builds and existing binaries:

```toml
[build]
kind = "script"
source = "build-native.sh"
```

The executable script receives `TARGET OUTPUT_FILE` as arguments, runs in the
recipe directory and must write the executable to the supplied path. Its
shebang selects the interpreter. Cadence's recipe calls its existing `install`
script, preserving the PIC std build and final staged ELF audit. Its window ID
and title are also derived from app.toml at build time so launch/focus uses the
same stable identity as the catalog. `kind =
"prebuilt"` with `source = "artifacts/player"` packages an existing native ELF;
resources still come from the recipe. Additional native executables can be
included through resources, subject to the same target validation.

## Image layers

Both source recipes and finished apps use the same packaging and validation:

```toml
[[images.rootfs.layers]]
kind = "app"
source = "../../cadence/platforms/scarlet/app.toml"
to = "/applications/cadence.app"
```

A `kind = "app"` layer in a bundle uses paths relative to that bundle. Set its
source to a prebuilt `.app` directory to stage it without rebuilding. The app
layer records its finished content hash in `scarlet.lock`. Target selection
comes from the image's native userspace target. No installer or live filesystem
cleanup is performed by this workflow.

## Discovery and reconciliation

stemd reads `/applications/*.app` and the existing `/etc/stemd.d/apps` catalog
at startup. Bundle-relative Exec, Icon paths and background paths are Scarlet
app-format semantics, marked by `X-Scarlet-AppFormat=1` and
`X-Scarlet-Target=<native-target>` in the generated descriptor. Bare Icon values
remain theme icon names. Legacy desktop entries retain their existing semantics.

Applications continue to use the existing ListApplications, launch/focus, MIME
and shell activation interfaces. An app replaces a legacy descriptor with the
same ID. Duplicate app IDs, malformed apps or other read errors reject the next
catalog and preserve the previous catalog; diagnostics identify the app path.
Catalog order is sorted by stable ID for deterministic MIME fallback.

Explicit reconciliation is available as the no-argument SBUS method
`org.scarlet-os.stemd.ReloadApplications` on `/org/scarlet/os/stemd`, service
`org.scarlet-os.stemd`, returning the catalog count as a string. Native socket
clients can send byte `0x08` to `/tmp/stemd.sock` (empty payload). Reload builds
the next catalog before atomically replacing it. Removed entries disappear;
running processes remain alive. Copy/delete changes require this explicit
reload; there is no filesystem watcher.

## Native filesystem layout and current limits

Native libraries use `/lib`; SGFX native drivers and manifests use `/lib/sgfx`.
Commands and services use `/bin`; the native ELF interpreter is
`/bin/scarlet-ld`. Native staging no longer includes the old `/system` tree.
`/systems` remains the separate ABI backing-root namespace. Linux SGFX retains
its `/usr/lib/sgfx` view and Linux interpreter paths. Native and Linux DSOs must
remain separate.

The desktop bundle selects exact published Cadence and GUI application commits.
Their coordinated ScarletUI/SGFX graph uses the native driver directory.
`SCARLET_SGFX_SOURCE` can select an existing SGFX checkout for image builds;
its HEAD must match the configured immutable SGFX revision. This selects the
same published source, without changing the manifests' dependency pins.
An explicit `SGFX_DRIVER_PATH=/lib/sgfx` is available for native diagnostics;
do not pass that native override to Linux-view programs.

The native loader searches adjacent to a dependency's requesting object and
then `/lib`; bare runtime dlopen names search `/lib`. It does not translate an
app's `bin` directory into its `lib` directory, implement private app library
search, or honor RPATH/RUNPATH. User-scoped applications, Linux-ABI app bundles,
installation/update, signing and universal apps need separate design. Opening
`.app` directories from Files is a follow-up; launching through the catalog is
supported. Host builds and staging checks do not verify a physical Switch,
SWS/SAS runtime, audible audio or GPU rendering.

## Pinned release graph

The app rollout selects these exact published commits. The shared core/ABI
identity remains independent of the runtime discovery revision.

| Dependency | Commit |
| --- | --- |
| sgfx | `20bb5b8cc19445520d08c31c3971ac06244bc1d9` |
| sdk | `9b3a257e02a5fddd37cf785912269a3e8702f10f` |
| sgfx-core | `040dbb5cf42b75489504e1765183d37f6131f54c` |
| scarlet-ui | `1b0f08aeb0a7057c9a6d0d7312554372411afff8` |
| resonara | `ae33ff44e3a246c49351bc719944ce9f7c44a087` |
| cadence | `318353de2e97733979001b4ad70879a7a6825d61` |
| carmine | `106809d9a6d5d0ec1a31a1b7b83e41e68878ee89` |
| myrica | `60b52e4c2eff17ea629c4df17ddc02fa00bacaa9` |
| vellum | `87a0f17173a7be09ead750f913b64b1fc5e0c785` |
| boxcraft | `a341f37b323572815871df1a95c8be812b9cdf86` |

Cadence's CLAP GUI dependency is coordinated through the dependency-only
Resonara update. No DSP or optimization code is part of this rollout.
The desktop bundle consumes Cadence's published Git bundle at
`platforms/scarlet`; it does not require a sibling Cadence checkout.

For subsequent releases, publish SDK/SGFX first, then ScarletUI, the CLAP GUI
dependency and native applications. Update the desktop bundle and userspace
Cargo/flake locks, publish Scarlet, then update Switch's distribution and
runtime pins and regenerate its console manifests and image locks with the
supported tools. Keep shared core identity consistent throughout this graph.

Rebuild clean staging, inspect native paths and ELF linkage, then check app
registration, launch/focus, MIME activation and explicit reload in a complete
native desktop VM. Host parser tests and isolated loader VM tests alone do not
verify those desktop operations. Physical Switch GPU/audio verification remains
a separate deployment step.
