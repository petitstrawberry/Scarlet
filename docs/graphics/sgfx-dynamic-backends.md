# Native SGFX drivers

Native AArch64 and RISC-V64 images load VirGL from
`/lib/sgfx/libsgfx_scarlet_virgl.so` through `/bin/scarlet-ld`.
The adjacent `scarlet-virgl.sgfx-driver` manifest selects backend ABI 2.
RISC-V32 retains its compatibility backend and only prepares the coordinated
SGFX source configuration; no driver library or manifest is installed there.
Userspace dependencies explicitly enable `backend-dynamic`. Native 64-bit
VirGL and Maxwell use installed drivers. Adreno remains built into the clients
until its dynamic driver is available. Integration CI rejects static VirGL and
Maxwell implementations anywhere in the native 64-bit userspace dependency graph.
The native 64-bit VirGL static feature has been removed; RISC-V32 continues
to select its compatibility backend through `backend-scarlet-virgl`.
Shared NV12/YCbCr image import is supported by the Maxwell backend. The VirGL
backend does not implement that import path.

Use a Scarlet compiler with native cdylib support, DSO-safe standard-library
TLS, and the GNU ELF OSABI fix (`petitstrawberry/rust` commit `71dd0425890`).
The linker can tag retained constructor sections with OSABI 3; the native
compiler accepts that tag and marks its completed executable/shared-object
output as Scarlet OSABI 83.

For the local compiler, enter the usual Nix development shell and run from
the Scarlet repository:

```sh
source scripts/scarlet-rust-dev.sh
scarlet-rust-use-local ../rust-scarlet
```

The local stage1 sysroot needs both the host libraries (including
`proc_macro`) and libraries for the selected Scarlet target. Cargo and native
build tools continue to come from the development shell. Return to the
packaged compiler with `scarlet-rust-use-cached`.

Native projects include `bundles/sgfx-native` before their userspace packages.
Its script builds the pinned SGFX plugin, verifies its ELF header, imports,
exports and relocations, and installs the plugin and manifest. It also writes
`.scarlet/sgfx/userspace.toml`, selected by the project's `userspace.cargo-config`.
Published UI/backend dependencies retain their shared SGFX core revision.
This config supplies the linker input needed for `scarlet-ld`'s `dlopen`, `dlsym`
and `dlerror` imports without replacing those Git dependencies with local paths. Driver discovery uses `dlopen`; applications have no
`DT_NEEDED` dependency on the VirGL plugin.

To prepare that configuration for an individual application build:

```sh
python3 tools/sgfx-native-build.py --project projects/aarch64-limine-full
cargo --config projects/aarch64-limine-full/.scarlet/sgfx/userspace.toml \
  build --manifest-path user/std-bin/Cargo.toml \
  --target aarch64-unknown-scarlet --bin sgfx-probe
```

Python comes from the Nix shell (3.11 or newer). `--sgfx-source /path/to/sgfx`
can reuse an existing checkout at the pinned revision. The driver, Cargo
configuration and ELF report are generated under the project's `.scarlet/sgfx`
directory. The build uses the selected compiler; it does not rewrite an ELF
header or change a packaged sysroot.

Run `sgfx-probe` in the guest to see `linkage: dynamic` and the loaded library
path. Missing or incompatible drivers fail device creation without falling
back to static VirGL. `SGFX_DRIVER_PATH` overrides manifest search directories
and `SGFX_BACKEND` selects a manifest name.
