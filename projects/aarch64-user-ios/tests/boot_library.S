// Both the PLT call into this DSO and its GOT reference need symbol binding.
.data
.p2align 3
.globl scarlet_boot_pid_syscall
scarlet_boot_pid_syscall:
    .quad 0

.text
.p2align 2
#ifdef SCARLET_ALTERNATE_LIBRARY
    // Keep the alternate implementation at different symbol addresses.
    nop
    nop
#endif
.type initialize_syscall,%function
initialize_syscall:
    mov x0, #1
    adr x1, version_message
    mov x2, #version_size
    mov x8, #201 // StreamWrite
    svc #0
    adrp x9, :got:scarlet_boot_pid_syscall
    ldr x9, [x9, :got_lo12:scarlet_boot_pid_syscall]
    mov x10, #7
    str x10, [x9]
    ret
.section .init_array,"aw",%init_array
    .quad initialize_syscall

.text
.globl scarlet_boot_pid
.type scarlet_boot_pid,%function
scarlet_boot_pid:
    adrp x9, :got:scarlet_boot_pid_syscall
    ldr x9, [x9, :got_lo12:scarlet_boot_pid_syscall]
    ldr x8, [x9]
    svc #0
    ret

.section .rodata
version_message:
#ifdef SCARLET_ALTERNATE_LIBRARY
    .ascii "SCARLET_LIBRARY_B\n"
#else
    .ascii "SCARLET_LIBRARY_A\n"
#endif
.set version_size, . - version_message

.text
.globl scarlet_boot_clear_syscall
.type scarlet_boot_clear_syscall,%function
scarlet_boot_clear_syscall:
    adrp x9, :got:scarlet_boot_pid_syscall
    ldr x9, [x9, :got_lo12:scarlet_boot_pid_syscall]
    str xzr, [x9]
    ret
