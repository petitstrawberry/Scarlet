.section .text.init
.global _start

.equ UART_BASE, 0x10000000
.equ SBI_LEGACY_PUTCHAR, 0x01
.equ SBI_SRST, 0x53525354

_start:
    la s0, message
1:
    lb a0, 0(s0)
    beqz a0, 2f
    li a7, SBI_LEGACY_PUTCHAR
    ecall
    addi s0, s0, 1
    j 1b
2:
    li a7, SBI_SRST
    li a0, 0
    li a1, 0
    ecall
3:
    wfi
    j 3b

.section .rodata
message:
    .asciz "Hello from guest via SBI!\n"
