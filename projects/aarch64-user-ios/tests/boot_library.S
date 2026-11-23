// Both the PLT call into this DSO and its GOT reference need symbol binding.
.data
.p2align 3
.globl scarlet_boot_pid_syscall
scarlet_boot_pid_syscall:
    .quad 0

.text
.p2align 2
.type initialize_syscall,%function
initialize_syscall:
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
