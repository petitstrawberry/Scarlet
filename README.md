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
- `arch::Arch` - Struct that contains architecture specific functions
    - `arch::Arch::new()` - create object`
    - `arch::Arch.get_cpuid()` - get CPU ID
    - `arch::Arch.get_trapframe_paddr()` - get physical address of trap frame
    - `arch::Arch.get_trapframe()` - get trap frame
    - `arch::Arch.set_trap_handler()` - set trap handler
- `arch::Trapframe` - Trapframe struct for sharing context between kernel and user space
- `arch::init_arch()` - initialize architecture specific code
- `arch::enable_interrupts()` - enable interrupts
- `arch::disable_interrupts()` - disable interrupts
- `arch::earlycon::early_putc()` - early console output (before serial device is initialized)
- `arch::get_cpu()` - get `Arch` struct of current CPU core
- `arch::get_user_trapvector_paddr()` - get physical address of user trap vector
- `arch::get_kernel_trapvector_paddr()` - get physical address of kernel trap vector
- `arch::get_kernel_trap_handler()` - get physical address of kernel trap handler
- `arch::get_user_trap_handler()` - get physical address of user trap handler
- `arch::set_trapvector()` - set trap vector for current cpu core
- `arch::set_trapframe()` - set trap frame for current cpu core
- `arch::Vcpu` - architecture specific vCPU data structure
    - `arch::Vcpu::new()` - create new vCPU
    - `arch::Vcpu.switch()` - switch context to vCPU (Trap context will be replaced with vCPU context)
    - `arch::Vcpu.jump()` - jump to vCPU
- `arch::Registers` - architecture specific register set
- `arch::ArchTimer` - architecture specific timer
    - `arch::ArchTimer.init()` - initialize timer
    - `arch::ArchTimer.start()` - start timer
    - `arch::ArchTimer.stop()` - stop timer
    - `arch::ArchTimer.is_running()` - check if timer is running
    - `arch::ArchTimer.get_time_us()` - get current time in microseconds
    - `arch::ArchTimer.set_interval_us()` - set timer interval in microseconds

## Documentation

More information can be found at [here](https://docs.scarlet.ichigo.dev/kernel)

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details
