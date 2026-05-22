# Multi-Architecture Support

Scarlet supports multiple target architectures, enabling execution of binaries across different CPU architectures through a transparent ABI conversion layer.

## Supported Architectures

### Currently Supported

- **RISC-V 64-bit (riscv64)** - Primary development architecture
- **AArch64 (ARM 64-bit)** - In development, basic support available

### Architecture Selection

The architecture is controlled via the `ARCH` environment variable throughout the build process:

```bash
# RISC-V 64 (default)
cargo make build-riscv64
cargo make test-riscv64

# AArch64
cargo make build-aarch64
cargo make test-aarch64
```

## Building for Multiple Architectures

### Kernel Build

The kernel build automatically selects the target based on the `ARCH` variable:

```bash
# Build RISC-V kernel
cargo make build-riscv64

# Build AArch64 kernel
cargo make build-aarch64
```

Target specifications are located in `kernel/targets/`:
- `riscv64gc-unknown-none-elf.json`
- `aarch64-unknown-none-elf.json`

### User Space Build

User-space programs follow the same convention:

```bash
# Build for RISC-V
cd user/bin
cargo build --target riscv64gc-unknown-scarlet-elf

# Build for AArch64
cd user/bin
cargo build --target aarch64-unknown-scarlet-elf
```

Target specifications are in `user/targets/`:
- `riscv64gc-unknown-scarlet-elf.json`
- `aarch64-unknown-scarlet-elf.json`

## Linux Rootfs for Multiple Architectures

### Building Buildroot

```bash
# RISC-V (uses buildroot-riscv64 config)
bash tools/linux/build_buildroot.sh

# AArch64 (uses buildroot-aarch64 config)
ARCH=aarch64 bash tools/linux/build_buildroot.sh
```

Buildroot/userland artifact generation runs on Linux hosts. This can be
`scarlet-dev`, a Linux VM, or a Linux Nix shell; macOS host execution is stopped
with guidance by the helper scripts.

### Building User Programs

Cross-compiled programs (like `zathura`, `green`, and `fbdoom`) for
Linux:

```bash
# RISC-V
bash tools/linux/build_user_programs.sh

# AArch64
ARCH=aarch64 bash tools/linux/build_user_programs.sh
```

This uses the appropriate toolchain:
- RISC-V: `riscv64-buildroot-linux-musl-gcc`
- AArch64: `aarch64-buildroot-linux-musl-gcc`

### Deploying Rootfs

```bash
# Deploy RISC-V rootfs
bash tools/linux/deploy_rootfs.sh

# Deploy AArch64 rootfs
ARCH=aarch64 bash tools/linux/deploy_rootfs.sh
```

After deployment, rootfs is organized by architecture:
```
mkfs/rootfs/system/
  ├── linux-riscv64/
  └── linux-aarch64/
```

## Running on QEMU

### RISC-V

```bash
cargo make debug-riscv64      # Debug mode
cargo make debug-test-riscv64 # Test mode
cargo make run-riscv64        # Limine-based boot path
```

Or manually:
```bash
bash kernel/tools/run.sh
```

### AArch64

```bash
cargo make debug-aarch64      # Debug mode
cargo make debug-test-aarch64 # Test mode
cargo make run-aarch64        # Limine-based boot path
```

Or manually:
```bash
bash kernel/tools/run_aarch64.sh
```

## Testing

Tests are architecture-specific and run on QEMU:

```bash
# RISC-V tests
cargo make test-riscv64

# AArch64 tests
cargo make test-aarch64
```

Test execution:
- `kernel/tools/test.sh` - RISC-V test runner
- `kernel/tools/test_aarch64.sh` - AArch64 test runner

## Architecture-Specific Code

### Directory Structure

Architecture-specific code is organized as:

```
kernel/src/arch/
  ├── mod.rs          # Common trait definitions
  ├── riscv64/        # RISC-V implementation
  │   ├── mod.rs
  │   ├── context.rs
  │   ├── switch.rs
  │   ├── trap.rs
  │   └── ...
  └── aarch64/        # AArch64 implementation
      ├── mod.rs
      ├── context.rs
      ├── switch.rs
      ├── trap.rs
      └── ...
```

### Conditional Compilation

Architecture-specific features are selected via cfg attributes:

```rust
#[cfg(target_arch = "riscv64")]
use arch::riscv64::*;

#[cfg(target_arch = "aarch64")]
use arch::aarch64::*;
```

## Linker Scripts

Each architecture has dedicated linker scripts:

**Kernel:**
- `kernel/lds/riscv64_qemu_virt.ld`
- `kernel/lds/aarch64_qemu_virt.ld`

**User space:**
- `user/lds/user.ld` (RISC-V)
- `user/lds/user_aarch64.ld` (AArch64)

## Device Tree Files

QEMU virt machine device trees:

- `virt-riscv64.dts` / `virt-riscv64.dtb` - RISC-V device tree
- `virt-aarch64.dts` - AArch64 device tree (generated on-the-fly by QEMU)

The Limine AArch64 workflow materializes the generated QEMU `virt` DTB into `mkfs/dist/virt-aarch64-limine.dtb` so Scarlet can keep using its existing FDT-based `BootInfo` handoff.

## Key Differences Between Architectures

> **Note**: For detailed virtual memory layout and HHDM design, see [memory-map.md](memory-map.md).

### RISC-V 64

- Register naming: `x0-x31`, `pc`, CSRs
- Calling convention: `a0-a7` (args), `t0-t6` (temp), `s0-s11` (saved)
- Privilege levels: M-mode, S-mode, U-mode
- Page table: Sv48 (4-level paging, 48-bit VA)
- Interrupt handling: PLIC (Platform-Level Interrupt Controller)

### AArch64

- Register naming: `x0-x30`, `sp`, `pc`, system registers
- Calling convention: `x0-x7` (args), `x9-x15` (temp), `x19-x28` (saved), `x29` (FP), `x30` (LR)
- Exception levels: EL0 (user), EL1 (kernel), EL2 (hypervisor), EL3 (secure monitor)
- Page table: 4KB/16KB/64KB granules, up to 4-level paging
- Interrupt handling: GIC (Generic Interrupt Controller)

## Notes

- Both architectures share the same high-level kernel logic (task scheduling, memory management, filesystem, etc.)
- Low-level primitives (context switching, trap handling, MMU setup) are architecture-specific
- User-space programs must be compiled for the target architecture
- Cross-architecture binary execution is not supported (no dynamic translation)

## Adding a New Architecture

To add support for a new architecture:

1. Create `kernel/src/arch/<new_arch>/` directory
2. Implement required traits defined in `kernel/src/arch/mod.rs`
3. Add target JSON specification to `kernel/targets/`
4. Create linker script in `kernel/lds/`
5. Add build/test/run scripts in `kernel/tools/`
6. Update Makefile.toml with new targets
7. Add Buildroot configuration for the architecture
8. Test thoroughly with QEMU or real hardware

Refer to the existing `riscv64` and `aarch64` implementations as reference.
