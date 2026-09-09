/* Shared XLEN-sized register loads/stores for trap and context assembly. */
.set SCARLET_WORD_BYTES, {word_bytes}
.set SCARLET_TRAPFRAME_SIZE, {trapframe_size}
.macro SCARLET_L reg, address
    .if SCARLET_WORD_BYTES == 8
        ld \reg, \address
    .else
        lw \reg, \address
    .endif
.endm
.macro SCARLET_S reg, address
    .if SCARLET_WORD_BYTES == 8
        sd \reg, \address
    .else
        sw \reg, \address
    .endif
.endm
