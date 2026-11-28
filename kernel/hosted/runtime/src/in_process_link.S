// The signed app's read-only guest loader veneers. The carrier identifies each
// operation by the instruction address and the authenticated breakpoint.
.text
.p2align 2
.globl _scarlet_loader_gate_begin, _scarlet_loader_gate_end
.globl _scarlet_loader_open, _scarlet_loader_open_trap, _scarlet_loader_continue
.globl _scarlet_loader_symbol, _scarlet_loader_close, _scarlet_loader_error
_scarlet_loader_gate_begin:
_scarlet_loader_open:
    stp x29, x30, [sp, #-16]!
    mov x29, sp
_scarlet_loader_open_trap:
    brk #0x5700
1:
    cbz x1, 2f
    blr x1
_scarlet_loader_continue:
    brk #0x5700
    b 1b
2:
    ldp x29, x30, [sp], #16
    ret
_scarlet_loader_symbol:
    brk #0x5700
    ret
_scarlet_loader_close:
    brk #0x5700
    ret
_scarlet_loader_error:
    brk #0x5700
    ret
_scarlet_loader_gate_end:

// Guest usercopy resolves one page at a time. Keep the full readable range
// isolated from neighboring host constants.
.section __TEXT,__scarlet_msg,regular
.p2align 14
.globl _scarlet_loader_messages_begin, _scarlet_loader_messages_end
.globl _scarlet_loader_error_open, _scarlet_loader_error_symbol
.globl _scarlet_loader_error_handle, _scarlet_loader_error_flags, _scarlet_loader_error_incomplete
_scarlet_loader_messages_begin:
_scarlet_loader_error_open:
    .asciz "Cannot load the signed Scarlet shared object"
_scarlet_loader_error_symbol:
    .asciz "Undefined Scarlet shared object symbol"
_scarlet_loader_error_handle:
    .asciz "Invalid or closed Scarlet shared object handle"
_scarlet_loader_error_flags:
    .asciz "Scarlet dlopen requires RTLD_NOW | RTLD_GLOBAL"
_scarlet_loader_error_incomplete:
    .asciz "Scarlet shared object initialization was interrupted"
.p2align 14
_scarlet_loader_messages_end:
