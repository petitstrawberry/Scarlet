// Freestanding AArch64 JIT smoke test. Build normally for Linux or define
// SCARLET_NATIVE and mark the ELF OSABI 83 for the native syscall convention.
#ifdef SCARLET_NATIVE
#define GETPID 7
#define MPROTECT 702
#define EXIT 1
#else
#define GETPID 172
#define MPROTECT 226
#define EXIT 93
#endif

.text
.global _start
_start:
    mov x8, #GETPID
    svc #0
    mov x20, x0
#ifdef SCARLET_NATIVE
    mov x0, #-1
    mov x1, #0
    mov x2, #16384
    mov x3, #3
    mov x4, #0x22
    mov x8, #700
#else
    mov x0, #0
    mov x1, #16384
    mov x2, #3
    mov x3, #0x22
    mov x4, #-1
    mov x8, #222
#endif
    mov x5, #0
    svc #0
    tbnz x0, #63, failed
    mov x19, x0

    adr x1, template
    mov x2, #0
copy_word:
    ldr w3, [x1, x2]
    str w3, [x19, x2]
    add x2, x2, #4
    cmp x2, #(template_end-template)
    b.lo copy_word

    mov x21, #1
publish:
    dc cvau, x19
    dsb ish
    ic ivau, x19
    dsb ish
    isb
    mov x2, #5
    bl protect
    mov x8, #GETPID
    blr x19

    mov w1, #1
    movk w1, #0xd400, lsl #16
    add x1, x1, x20
    add x1, x1, x21
    cmp x0, x1
    b.ne failed
    cmp x21, #2
    b.eq passed
    mov x2, #3
    bl protect
    adr x1, second_add
    ldr w2, [x1]
    str w2, [x19, #(first_add-template)]
    mov x21, #2
    b publish

protect:
    mov x0, x19
    mov x1, #16384
    mov x8, #MPROTECT
    svc #0
    cbnz x0, failed
    ret

failed:
    mov x0, #1
    b exit
passed:
    mov x0, #0
exit:
    mov x8, #EXIT
    svc #0
    brk #1

.section .rodata
.balign 4
template:
    b generated
literal:
    .word 0xd4000001
generated:
    svc #0
    ldr w1, literal
    add x0, x0, x1
first_add:
    add x0, x0, #1
    ret
template_end:
second_add:
    add x0, x0, #2
