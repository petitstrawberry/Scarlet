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
    - `drivers/` - drivers
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

- `arch::init()` - initialize architecture specific code
- `arch::trap_init()` - initialize traps
- `arch::enable_interrupts()` - enable interrupts
- `arch::disable_interrupts()` - disable interrupts
- `arch::ArchTimer` - architecture specific timer
    - `arch::ArchTimer::init()` - initialize timer
    - `arch::ArchTimer::start()` - start timer
    - `arch::ArchTimer::stop()` - stop timer
    - `arch::is_running()` - check if timer is running
    - `arch::get_time_us()` - get current time in microseconds
    - `arch::ArchTimer::set_interval_us()` - set timer interval in microseconds

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details
