# Runtime Delegation for Binary Execution

## Overview

Scarlet supports delegating binary execution to userland runtimes, enabling:
- **Cross-architecture emulation**: Run binaries from different architectures (e.g., MS-DOS via DOSBox)
- **Alternative runtime environments**: Execute binaries requiring special runtimes (Wasm, Java bytecode, etc.)
- **Kernel modularity**: Keep complex runtimes in userspace to maintain kernel security and simplicity

## How It Works

Instead of loading and executing a binary directly in the kernel, the system can:
1. Detect that a binary requires a runtime
2. Execute the runtime binary with the target binary as an argument
3. The runtime handles loading and executing the target binary in userspace

## Architecture

### Components

1. **RuntimeConfig**: Configuration structure defining how to delegate execution
   - `runtime_path`: Path to the runtime executable
   - `runtime_abi`: Optional ABI specification for the runtime
   - `runtime_args`: Additional arguments to pass to the runtime

2. **AbiModule::get_runtime_config()**: Method to determine if runtime delegation is needed
   - Each ABI module can implement custom logic to detect when delegation is required
   - Returns `Some(RuntimeConfig)` if delegation is needed, `None` otherwise

3. **TransparentExecutor**: Handles the delegation process
   - Checks for runtime configuration before direct execution
   - Constructs runtime arguments: `[runtime_path, runtime_args..., target_binary, target_args...]`
   - Executes the runtime with proper ABI

## Usage Example

### Example 1: MS-DOS Emulation via DOSBox

```rust
impl AbiModule for MsDosAbi {
    fn get_runtime_config(
        &self,
        file_object: &crate::object::KernelObject,
        file_path: &str,
    ) -> Option<RuntimeConfig> {
        // Check if this is an MS-DOS executable
        if self.is_msdos_binary(file_object) {
            Some(RuntimeConfig {
                runtime_path: "/system/linux/bin/dosbox".to_string(),
                runtime_abi: Some("linux-riscv64".to_string()),
                runtime_args: vec![],
            })
        } else {
            None
        }
    }
}
```

When executing an MS-DOS binary:
1. System detects it's an MS-DOS binary
2. Runtime delegation is configured to use DOSBox
3. DOSBox is executed as: `dosbox /path/to/program.exe [args...]`
4. DOSBox (Linux binary) runs in Linux ABI and emulates MS-DOS

### Example 2: WebAssembly Runtime

```rust
impl AbiModule for WasmAbi {
    fn get_runtime_config(
        &self,
        file_object: &crate::object::KernelObject,
        file_path: &str,
    ) -> Option<RuntimeConfig> {
        // Check if this is a Wasm binary
        if file_path.ends_with(".wasm") {
            Some(RuntimeConfig {
                runtime_path: "/system/scarlet/bin/wasm-runtime".to_string(),
                runtime_abi: None, // Auto-detect (likely Scarlet native)
                runtime_args: vec!["--wasm".to_string()],
            })
        } else {
            None
        }
    }
}
```

When executing a Wasm binary:
1. System detects it's a `.wasm` file
2. Runtime delegation configured to use Scarlet-native Wasm runtime
3. Wasm runtime executed as: `wasm-runtime --wasm /path/to/app.wasm [args...]`
4. Wasm runtime (Scarlet binary) runs in Scarlet ABI and executes Wasm code

### Example 3: Cross-Architecture Emulation

```rust
impl AbiModule for X86LinuxAbi {
    fn get_runtime_config(
        &self,
        file_object: &crate::object::KernelObject,
        file_path: &str,
    ) -> Option<RuntimeConfig> {
        // Check if this is an x86-64 Linux binary on RISC-V
        #[cfg(target_arch = "riscv64")]
        if self.is_x86_64_binary(file_object) {
            return Some(RuntimeConfig {
                runtime_path: "/system/linux/bin/qemu-x86_64".to_string(),
                runtime_abi: Some("linux-riscv64".to_string()),
                runtime_args: vec![],
            });
        }
        
        None // Native architecture, execute directly
    }
}
```

When executing an x86-64 Linux binary on RISC-V:
1. System detects architecture mismatch
2. Runtime delegation configured to use QEMU user-mode
3. QEMU executed as: `qemu-x86_64 /path/to/x86-program [args...]`
4. QEMU (RISC-V binary) emulates x86-64 execution

## Implementation Guidelines

### For ABI Module Developers

1. **Binary Detection**: Implement logic to detect when runtime delegation is needed
   - Check file format, magic bytes, extensions, etc.
   - Consider architecture compatibility

2. **Runtime Configuration**: Return appropriate `RuntimeConfig`
   - Specify correct runtime path
   - Set runtime ABI if known (or `None` for auto-detection)
   - Include any runtime-specific arguments

3. **Fallback**: Return `None` when direct execution is possible
   - Native binaries don't need runtime delegation
   - Only delegate when necessary

### Best Practices

1. **Performance**: Runtime delegation adds overhead, use only when needed
2. **Security**: Ensure runtime binaries are trusted and properly sandboxed
3. **Compatibility**: Test runtime delegation with various binary types
4. **Documentation**: Document which binaries require runtimes and why

## Architecture Benefits

### Kernel Simplicity
- Complex emulation logic stays in userspace
- Kernel remains small and maintainable
- Easier to update runtimes without kernel changes

### Security
- Runtimes run with user privileges
- Kernel attack surface reduced
- Runtime bugs don't compromise kernel

### Flexibility
- Easy to add new runtime support
- Can swap runtime implementations
- Multiple runtimes for same binary format

### Compatibility
- Support legacy formats without kernel bloat
- Enable cross-architecture execution
- Run binaries requiring special environments

## Future Enhancements

1. **Runtime Registry**: Centralized runtime configuration database
2. **Runtime Caching**: Keep runtimes loaded for faster execution
3. **Runtime Selection**: Choose best runtime based on performance/compatibility
4. **Runtime Sandboxing**: Enhanced isolation for untrusted runtimes
5. **Resource Limits**: Control runtime resource usage

## Related Documentation

- [ABI Module System](./abi/)
- [Linux ABI Status](./abi/linux/status.md)
