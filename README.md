# Scarlet
Minimal OS written in Rust [WIP]

## Build and run

Recommended way to build is to use Docker container with Dockerfile provided in the repository.

In container:

```bash
cargo run
```

## Structure

- `kernel/src/` - kernel code
    - `arch/` - architecture specific code
        - `riscv64/` - RISC-V 64-bit specific code
    - `driver/` - drivers
    - `board/` - board specific code
    - `mem/` - memory management
    - `sched/` - scheduler
    - `traits/` - traits
    - `library/` - library code (e.g. std)

### arch

We must implement `arch` module for each architecture we want to support. It should contain architecture specific code.

Below is an example of how to export architecture specific implementation in `arch` module.
We can add more architectures by adding more modules.

```rust
#[cfg(target_arch = "riscv64")]
pub mod riscv64;
#[cfg(target_arch = "riscv64")]
pub use riscv64::*;
```

#### Required

仕様が固まり次第追加していきます

- `arch::init()` - initialize architecture specific code
- `arch::mmu_init()` - initialize MMU and enable paging
- `arch::enable_interrupts()` - enable interrupts
- `arch::disable_interrupts()` - disable interrupts
- `arch::earlycon::early_putc()` - early console output (before serial device is initialized)
- `arch::Vcpu` - architecture specific vCPU data structure
    - `arch::Vcpu.new()` - create new vCPU
    - `arch::Vcpu.swicth()` - switch context to vCPU (Trap context will be replaced with vCPU context)
    - `arch::Vcpu.jump()` - jump to vCPU
- `arch::Registers` - architecture specific register set
- `arch::ArchTimer` - architecture specific timer
    - `arch::ArchTimer.init()` - initialize timer
    - `arch::ArchTimer.start()` - start timer
    - `arch::ArchTimer.stop()` - stop timer
    - `arch::ArchTimer.is_running()` - check if timer is running
    - `arch::ArchTimer.get_time_us()` - get current time in microseconds
    - `arch::ArchTimer.set_interval_us()` - set timer interval in microseconds

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details
