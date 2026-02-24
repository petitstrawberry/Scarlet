# Minimal guest "init" that uses SBI for console output
# Simulates a basic boot process with init

.section .text
.globl _start

.set SBI_PUTCHAR, 0x01
.set SBI_SHUTDOWN, 0x53525354

_start:
    # Set up stack (use end of 128MB RAM)
    li sp, 0x88000000
    
    # Print boot message
    la a0, boot_msg
    jal print_string
    
    # Print "init: starting..." 
    la a0, init_msg
    jal print_string
    
    # Simulate init process
    la a0, init_start
    jal print_string
    
    # Simulate some work
    li t0, 5
work_loop:
    la a0, dot_msg
    jal print_string
    
    # Simple delay
    li t1, 100000
delay:
    addi t1, t1, -1
    bnez t1, delay
    
    addi t0, t0, -1
    bnez t0, work_loop
    
    # Print newline
    la a0, newline
    jal print_string
    
    # Print "init: boot complete"
    la a0, init_done
    jal print_string
    
    # Print "init: executing shell..."
    la a0, shell_msg
    jal print_string
    
    # Simulate shell prompt
    la a0, prompt
    jal print_string
    
    # Print "Hello from guest init!"
    la a0, hello_msg
    jal print_string
    
    # Shutdown
    la a0, shutdown_msg
    jal print_string
    
    # SBI shutdown
    li a7, SBI_SHUTDOWN
    li a0, 0  # shutdown type
    li a1, 0  # reason
    ecall
    
    # Should not reach here
    j _start

# Print null-terminated string at a0
print_string:
    mv t0, a0
ps_loop:
    lb a1, 0(t0)
    beqz a1, ps_done
    li a7, SBI_PUTCHAR
    ecall
    addi t0, t0, 1
    j ps_loop
ps_done:
    ret

.section .rodata
boot_msg:
    .asciz "[guest] Booting guest kernel...\n"
init_msg:
    .asciz "[guest] init: starting...\n"
init_start:
    .asciz "[guest] init: mounting filesystems"
dot_msg:
    .asciz "."
newline:
    .asciz "\n"
init_done:
    .asciz "[guest] init: boot complete\n"
shell_msg:
    .asciz "[guest] init: starting shell\n"
prompt:
    .asciz "[guest] # "
hello_msg:
    .asciz "[guest] Hello from guest init!\n"
shutdown_msg:
    .asciz "[guest] init: requesting shutdown\n"
