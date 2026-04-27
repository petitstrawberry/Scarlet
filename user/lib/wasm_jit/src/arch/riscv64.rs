//! RISC-V 64-bit code generation backend for the Wasm JIT.

use jit_assembler::common::InstructionBuilder;
use jit_assembler::riscv64::{Register, Riscv64InstructionBuilder, reg};

use crate::arch::ArchBackend;
use crate::code::CodeBuffer;
use crate::control::{BranchKind, Fixup, LabelId};

const SLOT_SIZE: u32 = 8;
const SAVED_REG_BYTES: u32 = 16;
const CALL_SPILL_BYTES: u32 = 16;
const RA_SAVE_OFFSET: u32 = 0;
const S0_SAVE_OFFSET: u32 = 8;
const CTX_SPILL_OFFSET: u32 = 16;
const FP_SPILL_OFFSET: u32 = 24;
const VMCTX_MEMORY_BASE_OFFSET: i16 = 0;

pub struct Riscv64Backend {
    frame_size: u32,
    saved_ra_offset: u32,
    saved_s0_offset: u32,
}

impl Riscv64Backend {
    pub const fn new() -> Self {
        Self {
            frame_size: 0,
            saved_ra_offset: 0,
            saved_s0_offset: 0,
        }
    }

    fn align_up(value: u32, align: u32) -> u32 {
        let mask = align - 1;
        (value + mask) & !mask
    }

    fn frame_bytes(frame_slots: u16) -> u32 {
        frame_slots as u32 * SLOT_SIZE
    }

    fn total_stack_bytes(frame_slots: u16) -> u32 {
        Self::align_up(
            Self::frame_bytes(frame_slots) + SAVED_REG_BYTES + CALL_SPILL_BYTES,
            16,
        )
    }

    fn saved_reg_offset(frame_slots: u16, save_offset: u32) -> u32 {
        Self::frame_bytes(frame_slots) + save_offset
    }

    fn saved_reg_offset_from_base(save_offset: u32) -> u32 {
        save_offset
    }

    fn emit_with_builder(
        code: &mut CodeBuffer,
        build: impl FnOnce(&mut Riscv64InstructionBuilder),
    ) {
        let mut builder = Riscv64InstructionBuilder::new();
        build(&mut builder);
        let bytes = builder.instructions().to_bytes();
        code.emit_bytes(&bytes);
    }

    fn scratch_excluding(&self, reg_a: Register, reg_b: Register) -> Register {
        for reg in [self.tmp2(), self.tmp1(), self.tmp0()] {
            if reg != reg_a && reg != reg_b {
                return reg;
            }
        }
        self.tmp2()
    }

    fn scratch_excluding3(&self, reg_a: Register, reg_b: Register, reg_c: Register) -> Register {
        for reg in [self.tmp2(), self.tmp1(), self.tmp0()] {
            if reg != reg_a && reg != reg_b && reg != reg_c {
                return reg;
            }
        }
        self.tmp2()
    }

    fn emit_raw(&self, code: &mut CodeBuffer, word: u32) {
        code.emit_u32(word);
    }

    fn encode_i_type(opcode: u32, funct3: u32, rd: Register, rs1: Register, imm12: u32) -> u32 {
        ((imm12 & 0xfff) << 20)
            | (u32::from(rs1.value()) << 15)
            | (funct3 << 12)
            | (u32::from(rd.value()) << 7)
            | opcode
    }

    fn encode_r_type(
        opcode: u32,
        funct3: u32,
        funct7: u32,
        rd: Register,
        rs1: Register,
        rs2: Register,
    ) -> u32 {
        (funct7 << 25)
            | (u32::from(rs2.value()) << 20)
            | (u32::from(rs1.value()) << 15)
            | (funct3 << 12)
            | (u32::from(rd.value()) << 7)
            | opcode
    }

    fn emit_load_narrow(
        &mut self,
        code: &mut CodeBuffer,
        base: Register,
        offset: i32,
        emit_small: impl FnOnce(&mut Riscv64InstructionBuilder, Register, Register, i16),
        dst: Register,
    ) {
        if (-2048..=2047).contains(&offset) {
            Self::emit_with_builder(code, |b| emit_small(b, dst, base, offset as i16));
            return;
        }

        let scratch = self.scratch_excluding(dst, base);
        self.emit_li(code, scratch, offset as i64);
        Self::emit_with_builder(code, |b| {
            b.add(scratch, base, scratch);
            emit_small(b, dst, scratch, 0);
        });
    }

    fn emit_store_narrow(
        &mut self,
        code: &mut CodeBuffer,
        base: Register,
        offset: i32,
        src: Register,
        emit_small: impl FnOnce(&mut Riscv64InstructionBuilder, Register, Register, i16),
    ) {
        if (-2048..=2047).contains(&offset) {
            Self::emit_with_builder(code, |b| emit_small(b, base, src, offset as i16));
            return;
        }

        let scratch = self.scratch_excluding(base, src);
        self.emit_li(code, scratch, offset as i64);
        Self::emit_with_builder(code, |b| {
            b.add(scratch, base, scratch);
            emit_small(b, scratch, src, 0);
        });
    }

    fn emit_addi_large(&mut self, code: &mut CodeBuffer, dst: Register, src: Register, imm: i32) {
        if let Ok(imm12) = i16::try_from(imm) {
            if (-2048..=2047).contains(&imm) {
                Self::emit_with_builder(code, |b| {
                    b.addi(dst, src, imm12);
                });
                return;
            }
        }

        let scratch = self.scratch_excluding(dst, src);
        self.emit_li(code, scratch, imm as i64);
        Self::emit_with_builder(code, |b| {
            b.add(dst, src, scratch);
        });
    }

    fn emit_load_base_offset(
        &mut self,
        code: &mut CodeBuffer,
        dst: Register,
        base: Register,
        offset: u32,
    ) {
        if let Ok(imm12) = i16::try_from(offset) {
            if offset <= 2047 {
                Self::emit_with_builder(code, |b| {
                    b.ld(dst, base, imm12);
                });
                return;
            }
        }

        let scratch = self.scratch_excluding(dst, base);
        self.emit_li(code, scratch, offset as i64);
        Self::emit_with_builder(code, |b| {
            b.add(scratch, base, scratch);
            b.ld(dst, scratch, 0);
        });
    }

    fn emit_store_base_offset(
        &mut self,
        code: &mut CodeBuffer,
        base: Register,
        offset: u32,
        src: Register,
    ) {
        if let Ok(imm12) = i16::try_from(offset) {
            if offset <= 2047 {
                Self::emit_with_builder(code, |b| {
                    b.sd(base, src, imm12);
                });
                return;
            }
        }

        let scratch = self.scratch_excluding(base, src);
        self.emit_li(code, scratch, offset as i64);
        Self::emit_with_builder(code, |b| {
            b.add(scratch, base, scratch);
            b.sd(scratch, src, 0);
        });
    }
}

impl Default for Riscv64Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl ArchBackend for Riscv64Backend {
    type Reg = Register;

    fn emit_prologue(&mut self, code: &mut CodeBuffer, frame_slots: u16) {
        self.frame_size = Self::total_stack_bytes(frame_slots);
        self.saved_ra_offset = Self::saved_reg_offset(frame_slots, RA_SAVE_OFFSET);
        self.saved_s0_offset = Self::saved_reg_offset(frame_slots, S0_SAVE_OFFSET);

        self.emit_addi_large(code, reg::SP, reg::SP, -(self.frame_size as i32));
        self.emit_store_base_offset(code, reg::SP, self.saved_ra_offset, reg::RA);
        self.emit_store_base_offset(code, reg::SP, self.saved_s0_offset, reg::S0);

        Self::emit_with_builder(code, |b| {
            b.ld(reg::S0, self.ctx_reg(), VMCTX_MEMORY_BASE_OFFSET);
        });
    }

    fn emit_epilogue(&mut self, code: &mut CodeBuffer) {
        self.emit_load_base_offset(code, reg::RA, reg::SP, self.saved_ra_offset);
        self.emit_load_base_offset(code, reg::S0, reg::SP, self.saved_s0_offset);
        self.emit_addi_large(code, reg::SP, reg::SP, self.frame_size as i32);
        Self::emit_with_builder(code, |b| {
            b.ret();
        });
    }

    fn emit_load_slot(&mut self, code: &mut CodeBuffer, dst: Self::Reg, slot: u16) {
        self.emit_load_base_offset(code, dst, self.fp_reg(), slot as u32 * SLOT_SIZE);
    }

    fn emit_store_slot(&mut self, code: &mut CodeBuffer, slot: u16, src: Self::Reg) {
        self.emit_store_base_offset(code, self.fp_reg(), slot as u32 * SLOT_SIZE, src);
    }

    fn emit_load_slot_offset(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        slot: u16,
        byte_offset: u32,
    ) {
        let offset = slot as u32 * SLOT_SIZE + byte_offset;
        self.emit_load_base_offset(code, dst, self.fp_reg(), offset);
    }

    fn emit_li(&mut self, code: &mut CodeBuffer, dst: Self::Reg, imm: i64) {
        if let Ok(imm32) = i32::try_from(imm) {
            Self::emit_with_builder(code, |b| {
                b.li(dst, imm32);
            });
            return;
        }

        let bits = imm as u64;
        let upper = (bits >> 32) as u32;
        let lower = bits as u32;
        let scratch = self.scratch_excluding(dst, reg::ZERO);

        Self::emit_with_builder(code, |b| {
            b.li(dst, upper as i32);
            b.slli(dst, dst, 32);
            b.li(scratch, lower as i32);
            b.slli(scratch, scratch, 32);
            b.srli(scratch, scratch, 32);
            b.or(dst, dst, scratch);
        });
    }

    fn emit_add(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        Self::emit_with_builder(code, |b| {
            b.add(dst, lhs, rhs);
        });
    }

    fn emit_sub(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        Self::emit_with_builder(code, |b| {
            b.sub(dst, lhs, rhs);
        });
    }

    fn emit_mul(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        Self::emit_with_builder(code, |b| {
            b.mul(dst, lhs, rhs);
        });
    }

    fn emit_div_s(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        Self::emit_with_builder(code, |b| {
            b.div(dst, lhs, rhs);
        });
    }

    fn emit_div_u(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        Self::emit_with_builder(code, |b| {
            b.divu(dst, lhs, rhs);
        });
    }

    fn emit_rem_s(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        Self::emit_with_builder(code, |b| {
            b.rem(dst, lhs, rhs);
        });
    }

    fn emit_rem_u(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        Self::emit_with_builder(code, |b| {
            b.remu(dst, lhs, rhs);
        });
    }

    fn emit_and(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        Self::emit_with_builder(code, |b| {
            b.and(dst, lhs, rhs);
        });
    }

    fn emit_or(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        Self::emit_with_builder(code, |b| {
            b.or(dst, lhs, rhs);
        });
    }

    fn emit_xor(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        Self::emit_with_builder(code, |b| {
            b.xor(dst, lhs, rhs);
        });
    }

    fn emit_shl(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        Self::emit_with_builder(code, |b| {
            b.sll(dst, lhs, rhs);
        });
    }

    fn emit_shr_u(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        Self::emit_with_builder(code, |b| {
            b.srl(dst, lhs, rhs);
        });
    }

    fn emit_shr_s(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        Self::emit_with_builder(code, |b| {
            b.sra(dst, lhs, rhs);
        });
    }

    fn emit_eqz(&mut self, code: &mut CodeBuffer, dst: Self::Reg, src: Self::Reg) {
        Self::emit_with_builder(code, |b| {
            b.sltiu(dst, src, 1);
        });
    }

    fn emit_slt(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        Self::emit_with_builder(code, |b| {
            b.slt(dst, lhs, rhs);
        });
    }

    fn emit_sltu(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        Self::emit_with_builder(code, |b| {
            b.sltu(dst, lhs, rhs);
        });
    }

    fn emit_snez(&mut self, code: &mut CodeBuffer, dst: Self::Reg, src: Self::Reg) {
        Self::emit_with_builder(code, |b| {
            b.sltu(dst, reg::ZERO, src);
        });
    }

    fn emit_clz(&mut self, code: &mut CodeBuffer, dst: Self::Reg, src: Self::Reg) {
        self.emit_raw(code, Self::encode_i_type(0x1b, 0b001, dst, src, 0x600));
    }

    fn emit_ctz(&mut self, code: &mut CodeBuffer, dst: Self::Reg, src: Self::Reg) {
        self.emit_raw(code, Self::encode_i_type(0x1b, 0b001, dst, src, 0x601));
    }

    fn emit_rotl(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_raw(
            code,
            Self::encode_r_type(0x3b, 0b001, 0b0110000, dst, lhs, rhs),
        );
    }

    fn emit_load8_u(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        base: Self::Reg,
        offset: i32,
    ) {
        self.emit_load_narrow(
            code,
            base,
            offset,
            |b, rd, rs1, imm| {
                b.lbu(rd, rs1, imm);
            },
            dst,
        );
    }

    fn emit_load8_s(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        base: Self::Reg,
        offset: i32,
    ) {
        self.emit_load_narrow(
            code,
            base,
            offset,
            |b, rd, rs1, imm| {
                b.lb(rd, rs1, imm);
            },
            dst,
        );
    }

    fn emit_load16_u(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        base: Self::Reg,
        offset: i32,
    ) {
        self.emit_load_narrow(
            code,
            base,
            offset,
            |b, rd, rs1, imm| {
                b.lhu(rd, rs1, imm);
            },
            dst,
        );
    }

    fn emit_store8(&mut self, code: &mut CodeBuffer, base: Self::Reg, offset: i32, src: Self::Reg) {
        self.emit_store_narrow(code, base, offset, src, |b, rs1, rs2, imm| {
            b.sb(rs1, rs2, imm);
        });
    }

    fn emit_store16(
        &mut self,
        code: &mut CodeBuffer,
        base: Self::Reg,
        offset: i32,
        src: Self::Reg,
    ) {
        self.emit_store_narrow(code, base, offset, src, |b, rs1, rs2, imm| {
            b.sh(rs1, rs2, imm);
        });
    }

    fn emit_select(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        val_a: Self::Reg,
        val_b: Self::Reg,
        cond: Self::Reg,
    ) {
        let mask = self.scratch_excluding3(dst, val_a, val_b);
        Self::emit_with_builder(code, |b| {
            b.xor(dst, val_a, val_b);
            b.sltu(mask, reg::ZERO, cond);
            b.sub(mask, reg::ZERO, mask);
            b.and(dst, dst, mask);
            b.xor(dst, dst, val_b);
        });
    }

    fn emit_jump(&mut self, code: &mut CodeBuffer, label: LabelId) {
        let at_offset = code.offset();
        Self::emit_with_builder(code, |b| {
            b.jal(reg::ZERO, 0);
        });
        code.add_fixup(Fixup {
            at_offset,
            kind: BranchKind::Unconditional,
            target: label,
        });
    }

    fn emit_branch_zero(&mut self, code: &mut CodeBuffer, reg_to_test: Self::Reg, label: LabelId) {
        let at_offset = code.offset();
        Self::emit_with_builder(code, |b| {
            b.bne(reg_to_test, reg::ZERO, 8);
        });
        Self::emit_with_builder(code, |b| {
            b.jal(reg::ZERO, 0);
        });
        code.add_fixup(Fixup {
            at_offset: at_offset + 4,
            kind: BranchKind::Unconditional,
            target: label,
        });
    }

    fn emit_branch_not_zero(
        &mut self,
        code: &mut CodeBuffer,
        reg_to_test: Self::Reg,
        label: LabelId,
    ) {
        let at_offset = code.offset();
        Self::emit_with_builder(code, |b| {
            b.beq(reg_to_test, reg::ZERO, 8);
        });
        Self::emit_with_builder(code, |b| {
            b.jal(reg::ZERO, 0);
        });
        code.add_fixup(Fixup {
            at_offset: at_offset + 4,
            kind: BranchKind::Unconditional,
            target: label,
        });
    }

    fn emit_call_host(&mut self, code: &mut CodeBuffer, addr: usize) {
        let ctx_spill = Self::saved_reg_offset_from_base(CTX_SPILL_OFFSET);
        let fp_spill = Self::saved_reg_offset_from_base(FP_SPILL_OFFSET);
        self.emit_store_base_offset(code, reg::SP, ctx_spill, self.ctx_reg());
        self.emit_store_base_offset(code, reg::SP, fp_spill, self.fp_reg());
        self.emit_li(code, self.tmp2(), addr as i64);
        Self::emit_with_builder(code, |b| {
            b.jalr(reg::RA, self.tmp2(), 0);
        });
        self.emit_load_base_offset(code, self.ctx_reg(), reg::SP, ctx_spill);
        self.emit_load_base_offset(code, self.fp_reg(), reg::SP, fp_spill);
    }

    fn emit_retval(&mut self, code: &mut CodeBuffer, src: Self::Reg) {
        if src == self.ret_reg() {
            return;
        }

        Self::emit_with_builder(code, |b| {
            b.addi(self.ret_reg(), src, 0);
        });
    }

    fn ctx_reg(&self) -> Self::Reg {
        reg::A0
    }

    fn fp_reg(&self) -> Self::Reg {
        reg::A1
    }

    fn tmp0(&self) -> Self::Reg {
        reg::T0
    }

    fn tmp1(&self) -> Self::Reg {
        reg::T1
    }

    fn tmp2(&self) -> Self::Reg {
        reg::T2
    }

    fn ret_reg(&self) -> Self::Reg {
        reg::A0
    }
}
