.text
.p2align 2
.globl _probe_enter
_probe_enter:
    brk #0x5600
.globl _probe_return
_probe_return:
    ret
