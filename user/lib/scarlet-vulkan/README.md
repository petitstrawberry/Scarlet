# Scarlet Native Vulkan integration

The Rust API owns an `ash::Entry` and the native handle to the installed Khronos
loader. It must outlive all Vulkan objects created through that entry.

`bundle.toml` is this package's installation recipe: it selects the native
Khronos loader and SGFX ICD, then installs the ICD manifest and library symlink.
The SDK retrieves pinned repositories, runs their Cargo builds, records output
hashes and stages the libraries. Build definitions remain with their owners:
Khronos loader CMake/CRT integration in its fork, and ICD linking in SGFX.
Headers/LLVM libc and cross tools are declared Nix development dependencies.
The install script only deploys resources; it does not fetch, hash, compile or
manage dependency caches. This recipe is included by the existing SGFX bundle.

The installed loader is currently verified on AArch64. Other architectures skip
its Cargo layers. Myrica/WebKit's Vulkan rasterization is separate unfinished
work; installing this runtime does not accelerate its current Cairo rendering.
