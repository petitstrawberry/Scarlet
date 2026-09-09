// SBI ordered boot: a0 = physical hart ID, a1 = physical FDT, satp = 0.
// This stub runs at the image's physical load address. No Rust or absolute
// data pointers are used before installing the linked kernel mapping.
.section .init.sbi_entry, "ax"
.option push
.option arch, +m
.option norelax
.option nopic
.balign 4
.global _sbi_start
_sbi_start:
    csrci sstatus, 2
    csrw sie, zero
    csrw sscratch, zero
    mv s0, a0
    mv s1, a1
    la s2, __KERNEL_SPACE_START
    la t0, SBI_BOOT_ROOT
    mv t1, t0
    li t2, 1024
1:
    sw zero, 0(t1)
    addi t1, t1, 4
    addi t2, t2, -1
    bnez t2, 1b

    // Identity and linked aliases cover only the bounded bootstrap image.
    mv t1, s2
    la t2, __KERNEL_SPACE_END
    lui t3, %hi(KERNEL_VIRTUAL_BASE)
    addi t3, t3, %lo(KERNEL_VIRTUAL_BASE)
2:
    srli t4, t1, 12
    slli t4, t4, 10
    ori t4, t4, 0xcf
    srli t5, t1, 22
    slli t5, t5, 2
    add t5, t0, t5
    sw t4, 0(t5)
    srli t5, t3, 22
    slli t5, t5, 2
    add t5, t0, t5
    sw t4, 0(t5)
    li t6, 0x400000
    add t1, t1, t6
    add t3, t3, t6
    bltu t1, t2, 2b

    // Two read-only superpages at the temporary FDT window also cover a DTB
    // crossing a physical superpage boundary. No MMIO/RAM identity blanket.
    srli t1, s1, 22
    slli t1, t1, 20
    ori t1, t1, 0x43
    li t2, 0x880
    add t2, t0, t2
    sw t1, 0(t2)
    li t3, 0x100000
    add t1, t1, t3
    sw t1, 4(t2)

    la sp, KERNEL_STACK
    li t1, {stack_size}
    add sp, sp, t1
    lui t1, %hi(KERNEL_VIRTUAL_BASE)
    addi t1, t1, %lo(KERNEL_VIRTUAL_BASE)
    sub t1, t1, s2
    add sp, sp, t1
    la t2, 3f
    add t2, t2, t1
    srli t0, t0, 12
    li t3, 0x80000000
    or t0, t0, t3
    sfence.vma
    csrw satp, t0
    sfence.vma
    jr t2
3:
    la gp, __global_pointer$
    mv a0, s0
    li t0, 0x3fffff
    and a1, s1, t0
    li t0, 0x88000000
    add a1, a1, t0
    mv a2, s1
    mv a3, s2
    tail {rust_entry}

// HSM supplies a0 = physical hart ID and a1 = Scarlet's logical CPU slot.
// The BSP has finished the boot root before issuing the asynchronous start.
.balign 4
.global _sbi_start_secondary
_sbi_start_secondary:
    csrci sstatus, 2
    csrw sie, zero
    csrw sscratch, zero
    li t0, {max_cpus}
    bgeu a1, t0, 5f
    la t0, __KERNEL_SPACE_START
    lui t1, %hi(KERNEL_VIRTUAL_BASE)
    addi t1, t1, %lo(KERNEL_VIRTUAL_BASE)
    sub t1, t1, t0
    la sp, KERNEL_STACK
    li t0, {stack_size}
    addi t2, a1, 1
    mul t0, t0, t2
    add sp, sp, t0
    add sp, sp, t1
    la t2, 4f
    add t2, t2, t1
    la t0, SBI_BOOT_ROOT
    srli t0, t0, 12
    li t3, 0x80000000
    or t0, t0, t3
    fence r, rw
    sfence.vma
    csrw satp, t0
    sfence.vma
    jr t2
4:
    la gp, __global_pointer$
    tail {secondary_entry}
5:
    wfi
    j 5b
.option pop
