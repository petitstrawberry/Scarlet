/* Freestanding position-independent fixture. x18 belongs to the host ABI. */
.text
.p2align 2
.globl _scarlet_fixture_entry
_scarlet_fixture_entry:
    adrp x9, _scarlet_fixture_data@PAGE
    add x9, x9, _scarlet_fixture_data@PAGEOFF
    ldr x10, [x9, #8]
    ldr x0, [x10]
    add x0, x0, #1
    str x0, [x10]
    adrp x10, _scarlet_fixture_bss@PAGE
    add x10, x10, _scarlet_fixture_bss@PAGEOFF
    ldr x11, [x10]
    add x11, x11, #1
    str x11, [x10]
    ldur x1, [sp, #-8]
    ldr x2, [x2]
    ldr x10, [x9, #16]
    br x10

.globl _scarlet_fixture_helper
_scarlet_fixture_helper:
    add x0, x0, #7
    ret

.globl _scarlet_fixture_checkpoint
_scarlet_fixture_checkpoint:
    brk #0x5101
.globl _scarlet_fixture_spin
_scarlet_fixture_spin:
    b _scarlet_fixture_spin

.globl _scarlet_fixture_tls_write
_scarlet_fixture_tls_write:
    brk #0x5500
.globl _scarlet_fixture_tls_read
_scarlet_fixture_tls_read:
    brk #0x5400
    b _scarlet_fixture_checkpoint

.data
.p2align 3
.globl _scarlet_fixture_data
_scarlet_fixture_data:
    .quad 40, 0, 0

.globl _scarlet_fixture_bss
.zerofill __DATA,__bss,_scarlet_fixture_bss,8,3
