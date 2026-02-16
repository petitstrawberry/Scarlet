.section .text.init
.global _start

.equ UART_BASE, 0x10000000

_start:
    # Print "Hello from guest!"
    la a0, message
1:
    lb t0, 0(a0)
    beqz t0, 2f
    li t1, UART_BASE
    sb t0, 0(t1)
    addi a0, a0, 1
    j 1b
2:
    # Infinite loop (shutdown)
3:
    wfi
    j 3b

.section .rodata
message:
    .asciz "Hello from guest!\n"
