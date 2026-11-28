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
#ifdef SCARLET_ALTERNATE_LIBRARY
    stp x19, x30, [sp, #-16]!
    adr x0, self_path
    mov x1, #0x102 // RTLD_NOW | RTLD_GLOBAL
    bl dlopen
    cbz x0, 1f
    mov x19, x0
    adr x1, self_symbol
    bl dlsym
    cbz x0, 1f
    mov x0, x19
    bl dlclose
    cbnz x0, 1f
    mov x0, #1
    adr x1, reentry_message
    mov x2, #reentry_size
    mov x8, #201
    svc #0
1:
    ldp x19, x30, [sp], #16
#endif
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
#ifdef SCARLET_ALTERNATE_LIBRARY
self_path:
    .asciz "/old_root/lib/libboot-check-alt.so"
self_symbol:
    .asciz "scarlet_boot_pid"
reentry_message:
    .ascii "SCARLET_DLOPEN_REENTER_PASS\n"
.set reentry_size, . - reentry_message
#endif

.text
.globl scarlet_boot_clear_syscall
.type scarlet_boot_clear_syscall,%function
scarlet_boot_clear_syscall:
    adrp x9, :got:scarlet_boot_pid_syscall
    ldr x9, [x9, :got_lo12:scarlet_boot_pid_syscall]
    str xzr, [x9]
    ret
