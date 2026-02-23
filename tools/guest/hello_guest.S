# Simple guest program for testing ushv
# Prints "Hello from guest!\n" using SBI legacy putchar

.section .text
.globl _start

.set UART_BASE, 0x10000000

_start:
    # Set up stack (use end of 128MB RAM)
    li sp, 0x88000000
    
    # Print "Hello from guest!\n"
    la a0, message
print_loop:
    lb a1, 0(a0)
    beqz a1, done_print
    
    # SBI legacy putchar (ecall with a7=0x01)
    li a7, 0x01
    ecall
    
    addi a0, a0, 1
    j print_loop

done_print:
    # Infinite loop
    wfi
    j done_print

.section .rodata
message:
    .asciz "Hello from guest!\n"
