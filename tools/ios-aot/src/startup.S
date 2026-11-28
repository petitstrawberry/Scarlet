// x2 points above the initial stack to { constructors..., 0, entry }.
// Each constructor follows AAPCS64. Restore argc/argv and the original SP.
stp x0, x1, [sp, #-16]!
stp x19, x30, [sp, #-16]!
mov x19, x2
1:
ldr x9, [x19], #8
cbz x9, 2f
blr x9
b 1b
2:
ldr x9, [x19]
ldp x19, x30, [sp], #16
ldp x0, x1, [sp], #16
br x9
.p2align 14
