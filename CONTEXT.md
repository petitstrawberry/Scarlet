# Scarlet U-SHV Development Context

## Goal

Implement safe, host-side interrupt routing and controller abstraction for the Type-2 hypervisor (U-SHV) to support booting Linux guests on RISC-V. Current focus: Get UART interrupts working in a simple test guest that should:
1. Receive external interrupts when typing characters
2. Read the character from UART RBR
3. Echo the character back
4. Quit when 'q' is pressed

## Key Fixes Made

### 1. PC Advancement for Compressed Instructions (trap.rs)

**Problem:** QEMU doesn't correctly set the htinst transformation bit for compressed instructions. The `c.sw a2,40(a0)` was being treated as 32-bit, causing PC to advance by 4 instead of 2.

**Solution:** Check if htinst contains a valid 32-bit RISC-V opcode. If not, assume compressed instruction.

### 2. KB_XLATE Constant Value (timer.rs)

**Problem:** `KB_XLATE` was set to `0x01` (KB_MEDIUMRAW) instead of `0` (KB_XLATE). This caused stdin to return Linux keycodes instead of ASCII characters.

**Solution:** Changed to `const KB_XLATE: usize = 0;`

### 3. Guest Binary Format

**Problem:** ushv was loading ELF directly instead of raw binary.

**Solution:** Convert with `objcopy -O binary`

## Current Status

**Working:**
- ✅ Guest boots and initializes PLIC correctly
- ✅ UART thread receives stdin input (after KB_XLATE fix)
- ✅ PLIC set_pending is called correctly
- ✅ VcpuIrqSink inject interrupt syscall succeeds (result=0)

**Not Working:**
- ❌ Guest doesn't receive/process the interrupt
- Need to debug kernel's `inject_pending_interrupts` / hvip settings

## Remaining Tasks

1. Debug kernel interrupt injection (hvip/vsie)
2. Clean up debug logs after fix confirmed
3. Test complete UART echo flow

## Relevant Files

- `kernel/src/arch/riscv64/hv/trap.rs` - MMIO decode, htinst parsing
- `kernel/src/arch/riscv64/hv/vm.rs` - VcpuObject, interrupt injection
- `user/bin/src/ushv/riscv64/timer.rs` - UART input thread, KB_XLATE fix
- `user/bin/src/ushv/devices/plic.rs` - PLIC device
- `user/bin/src/ushv/machine/mod.rs` - VcpuIrqSink
