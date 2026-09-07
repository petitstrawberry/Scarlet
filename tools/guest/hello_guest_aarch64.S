.section .text
.global _start

.equ UART0, 0x09000000

_start:
    // Load address of message string
    adr x1, msg

loop:
    ldrb w2, [x1], #1
    cbz w2, done

    // Write byte to UART (PL011 on QEMU virt)
    mov x0, #UART0
    strb w2, [x0]

    b loop

done:
    // Halt: WFE loop
halt:
    wfe
    b halt

msg:
    .asciz "Hello from AArch64 guest!\n"
