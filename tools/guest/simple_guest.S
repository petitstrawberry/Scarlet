# Very simple guest - just prints H and halts
.section .text
.globl _start

_start:
    # Set stack
    li sp, 0x88000000
    
    # Print 'H' using SBI legacy putchar
    li a7, 0x01      # SBI_PUTCHAR
    li a0, 0x48      # 'H'
    ecall
    
    # Print 'i'
    li a7, 0x01
    li a0, 0x69      # 'i'
    ecall
    
    # Print '\n'
    li a7, 0x01
    li a0, 0x0a
    ecall
    
    # Shutdown using SBI SRST
    li a7, 0x53525354   # SBI_SRST
    li a0, 0            # shutdown type
    li a1, 0            # reason
    ecall
    
    # Should not reach here, but loop if we do
halt:
    wfi
    j halt
