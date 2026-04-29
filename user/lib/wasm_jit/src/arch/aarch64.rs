use jit_assembler::aarch64::{Aarch64InstructionBuilder, Register, reg};
use jit_assembler::common::InstructionBuilder;

use crate::arch::ArchBackend;
use crate::code::CodeBuffer;
use crate::control::{BranchKind, Fixup, LabelId};
const SAVED_FP_OFFSET: u32 = 0;
const SAVED_LR_OFFSET: u32 = 8;
const SAVED_X19_OFFSET: u32 = 16;
const CTX_SPILL_OFFSET: u32 = 24;
const FP_SPILL_OFFSET: u32 = 32;
const CALLEE_SAVE_BYTES: u32 = 24;
const CALL_SPILL_BYTES: u32 = 16;
const VMCTX_MEMORY_BASE_OFFSET: u32 = 0;
const LSLV_OPCODE: u8 = 0b001000;
const LSRV_OPCODE: u8 = 0b001001;
const ASRV_OPCODE: u8 = 0b001010;

pub struct Aarch64Backend {
    prologue_stack_size: u32,
}

impl Aarch64Backend {
    pub const fn new() -> Self {
        Self {
            prologue_stack_size: 0,
        }
    }

    fn align_up(value: u32, align: u32) -> u32 {
        let mask = align - 1;
        (value + mask) & !mask
    }

    fn saved_stack_size(&self) -> u32 {
        self.prologue_stack_size
    }

    fn mem_base_reg(&self) -> Register {
        reg::X19
    }

    fn emit_builder<F>(&self, code: &mut CodeBuffer, f: F)
    where
        F: FnOnce(&mut Aarch64InstructionBuilder),
    {
        let mut builder = Aarch64InstructionBuilder::new();
        f(&mut builder);
        let bytes = builder.instructions().to_bytes();
        code.emit_bytes(&bytes);
    }

    fn emit_raw(&self, code: &mut CodeBuffer, word: u32) {
        code.emit_u32(word);
    }

    fn scratch_excluding(&self, regs: &[Register]) -> Register {
        for reg in [self.tmp2(), self.tmp1(), self.tmp0()] {
            if !regs.contains(&reg) {
                return reg;
            }
        }
        self.tmp2()
    }

    fn encode_cmp_reg(rn: Register, rm: Register) -> u32 {
        0xEB00001F | (u32::from(rm.value()) << 16) | (u32::from(rn.value()) << 5)
    }

    fn encode_cset(rd: Register, cond: u8) -> u32 {
        0x9A9F07E0 | (u32::from(cond ^ 1) << 12) | u32::from(rd.value())
    }

    fn encode_csel(rd: Register, rn: Register, rm: Register, cond: u8) -> u32 {
        0x9A800000
            | (u32::from(rm.value()) << 16)
            | (u32::from(cond) << 12)
            | (u32::from(rn.value()) << 5)
            | u32::from(rd.value())
    }

    fn encode_clz(rd: Register, rn: Register) -> u32 {
        0xDAC01000 | (u32::from(rn.value()) << 5) | u32::from(rd.value())
    }

    fn encode_rbit(rd: Register, rn: Register) -> u32 {
        0xDAC00000 | (u32::from(rn.value()) << 5) | u32::from(rd.value())
    }

    fn encode_rorv(rd: Register, rn: Register, rm: Register) -> u32 {
        0x9AC02C00
            | (u32::from(rm.value()) << 16)
            | (u32::from(rn.value()) << 5)
            | u32::from(rd.value())
    }

    fn encode_ldrb(rt: Register, rn: Register, imm12: u16) -> u32 {
        0x39400000 | (u32::from(imm12) << 10) | (u32::from(rn.value()) << 5) | u32::from(rt.value())
    }

    fn encode_ldrsb(rt: Register, rn: Register, imm12: u16) -> u32 {
        0x39800000 | (u32::from(imm12) << 10) | (u32::from(rn.value()) << 5) | u32::from(rt.value())
    }

    fn encode_ldrh(rt: Register, rn: Register, imm12: u16) -> u32 {
        0x79400000 | (u32::from(imm12) << 10) | (u32::from(rn.value()) << 5) | u32::from(rt.value())
    }

    fn encode_strb(rt: Register, rn: Register, imm12: u16) -> u32 {
        0x39000000 | (u32::from(imm12) << 10) | (u32::from(rn.value()) << 5) | u32::from(rt.value())
    }

    fn encode_strh(rt: Register, rn: Register, imm12: u16) -> u32 {
        0x79000000 | (u32::from(imm12) << 10) | (u32::from(rn.value()) << 5) | u32::from(rt.value())
    }

    fn emit_load_narrow(
        &self,
        code: &mut CodeBuffer,
        dst: Register,
        base: Register,
        offset: i32,
        encode: impl FnOnce(Register, Register, u16) -> u32,
    ) {
        if offset >= 0 && offset < 4096 {
            self.emit_raw(code, encode(dst, base, offset as u16));
            return;
        }

        let scratch = self.scratch_excluding(&[dst, base]);
        self.emit_mov_imm(code, scratch, offset as u64);
        self.emit_builder(code, |builder| {
            builder.add(scratch, base, scratch);
        });
        self.emit_raw(code, encode(dst, scratch, 0));
    }

    fn emit_store_narrow(
        &self,
        code: &mut CodeBuffer,
        base: Register,
        offset: i32,
        src: Register,
        encode: impl FnOnce(Register, Register, u16) -> u32,
    ) {
        if offset >= 0 && offset < 4096 {
            self.emit_raw(code, encode(src, base, offset as u16));
            return;
        }

        let scratch = self.scratch_excluding(&[base, src]);
        self.emit_mov_imm(code, scratch, offset as u64);
        self.emit_builder(code, |builder| {
            builder.add(scratch, base, scratch);
        });
        self.emit_raw(code, encode(src, scratch, 0));
    }

    fn emit_mov_imm(&self, code: &mut CodeBuffer, dst: Register, imm: u64) {
        self.emit_builder(code, |builder| {
            builder.mov_imm(dst, imm);
        });
    }

    fn emit_add_imm(&self, code: &mut CodeBuffer, dst: Register, base: Register, imm: u32) {
        self.emit_builder(code, |builder| {
            if dst != base {
                if base == reg::SP {
                    builder.addi(dst, reg::SP, 0);
                } else {
                    builder.mov(dst, base);
                }
            }

            let mut remaining = imm;
            while remaining != 0 {
                let chunk = core::cmp::min(remaining, 4095) as u16;
                builder.addi(dst, dst, chunk);
                remaining -= u32::from(chunk);
            }
        });
    }

    fn emit_sub_imm(&self, code: &mut CodeBuffer, dst: Register, base: Register, imm: u32) {
        self.emit_builder(code, |builder| {
            if dst != base {
                if base == reg::SP {
                    builder.addi(dst, reg::SP, 0);
                } else {
                    builder.mov(dst, base);
                }
            }

            let mut remaining = imm;
            while remaining != 0 {
                let chunk = core::cmp::min(remaining, 4095) as u16;
                builder.subi(dst, dst, chunk);
                remaining -= u32::from(chunk);
            }
        });
    }

    fn slot_offset(slot: u16) -> u32 {
        u32::from(slot) * 8
    }

    fn scaled_u64_offset(byte_offset: u32) -> Option<u16> {
        if byte_offset % 8 == 0 {
            let scaled = byte_offset / 8;
            if scaled < 4096 {
                return Some(scaled as u16);
            }
        }
        None
    }

    fn encode_ldr_u64(rt: Register, rn: Register, scaled_imm12: u16) -> u32 {
        0xF9400000
            | (u32::from(scaled_imm12) << 10)
            | (u32::from(rn.value()) << 5)
            | u32::from(rt.value())
    }

    fn encode_str_u64(rt: Register, rn: Register, scaled_imm12: u16) -> u32 {
        0xF9000000
            | (u32::from(scaled_imm12) << 10)
            | (u32::from(rn.value()) << 5)
            | u32::from(rt.value())
    }

    fn encode_b_placeholder() -> u32 {
        0x14000000
    }

    fn encode_cbz_placeholder(rt: Register) -> u32 {
        0xB4000000 | u32::from(rt.value())
    }

    fn encode_cbnz_placeholder(rt: Register) -> u32 {
        0xB5000000 | u32::from(rt.value())
    }

    fn encode_blr(rn: Register) -> u32 {
        0xD63F0000 | (u32::from(rn.value()) << 5)
    }

    fn encode_data_proc_2src(opcode: u8, rd: Register, rn: Register, rm: Register) -> u32 {
        Self::encode_data_proc_2src_sf(1, opcode, rd, rn, rm)
    }

    fn encode_data_proc_2src_sf(
        sf: u8,
        opcode: u8,
        rd: Register,
        rn: Register,
        rm: Register,
    ) -> u32 {
        (u32::from(sf) << 31)
            | (0b11010110u32 << 21)
            | (u32::from(rm.value()) << 16)
            | (u32::from(opcode) << 10)
            | (u32::from(rn.value()) << 5)
            | u32::from(rd.value())
    }

    fn encode_add_sub_reg(sf: u8, op: u8, rm: Register, rn: Register, rd: Register) -> u32 {
        (u32::from(sf) << 31)
            | (u32::from(op) << 30)
            | (0b01011 << 24)
            | (u32::from(rm.value()) << 16)
            | (u32::from(rn.value()) << 5)
            | u32::from(rd.value())
    }

    fn encode_logical_reg(sf: u8, opc: u8, rm: Register, rn: Register, rd: Register) -> u32 {
        (u32::from(sf) << 31)
            | (u32::from(opc) << 29)
            | (0b01010 << 24)
            | (u32::from(rm.value()) << 16)
            | (u32::from(rn.value()) << 5)
            | u32::from(rd.value())
    }

    fn encode_multiply(
        sf: u8,
        op31: u8,
        rm: Register,
        o0: u8,
        ra: Register,
        rn: Register,
        rd: Register,
    ) -> u32 {
        (u32::from(sf) << 31)
            | (0b11011 << 24)
            | (u32::from(op31) << 21)
            | (u32::from(rm.value()) << 16)
            | (u32::from(o0) << 15)
            | (u32::from(ra.value()) << 10)
            | (u32::from(rn.value()) << 5)
            | u32::from(rd.value())
    }

    fn encode_divide(sf: u8, opcode: u8, rm: Register, rn: Register, rd: Register) -> u32 {
        (u32::from(sf) << 31)
            | (0b11010110 << 21)
            | (u32::from(rm.value()) << 16)
            | (u32::from(opcode) << 10)
            | (u32::from(rn.value()) << 5)
            | u32::from(rd.value())
    }

    fn emit_load_mem(
        &self,
        code: &mut CodeBuffer,
        dst: Register,
        base: Register,
        byte_offset: u32,
    ) {
        if let Some(scaled) = Self::scaled_u64_offset(byte_offset) {
            self.emit_raw(code, Self::encode_ldr_u64(dst, base, scaled));
            return;
        }

        let scratch = if dst != base { dst } else { self.tmp2() };
        self.emit_add_imm(code, scratch, base, byte_offset);
        self.emit_raw(code, Self::encode_ldr_u64(dst, scratch, 0));
    }

    fn emit_store_mem(
        &self,
        code: &mut CodeBuffer,
        base: Register,
        byte_offset: u32,
        src: Register,
    ) {
        let scratch = if self.tmp2() != src && self.tmp2() != base {
            self.tmp2()
        } else if self.tmp1() != src && self.tmp1() != base {
            self.tmp1()
        } else {
            self.tmp0()
        };

        if let Some(scaled) = Self::scaled_u64_offset(byte_offset) {
            self.emit_raw(code, Self::encode_str_u64(src, base, scaled));
            return;
        }

        self.emit_add_imm(code, scratch, base, byte_offset);
        self.emit_raw(code, Self::encode_str_u64(src, scratch, 0));
    }
}

impl Default for Aarch64Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl ArchBackend for Aarch64Backend {
    type Reg = Register;

    fn emit_prologue(&mut self, code: &mut CodeBuffer, frame_slots: u16) {
        let stack_size = Self::align_up(
            u32::from(frame_slots) * 8 + CALLEE_SAVE_BYTES + CALL_SPILL_BYTES,
            16,
        );
        self.prologue_stack_size = stack_size;
        self.emit_sub_imm(code, reg::SP, reg::SP, stack_size);
        self.emit_store_mem(code, reg::SP, SAVED_FP_OFFSET, reg::FP);
        self.emit_store_mem(code, reg::SP, SAVED_LR_OFFSET, reg::LR);
        self.emit_store_mem(code, reg::SP, SAVED_X19_OFFSET, self.mem_base_reg());
        self.emit_load_mem(
            code,
            self.mem_base_reg(),
            self.ctx_reg(),
            VMCTX_MEMORY_BASE_OFFSET,
        );
    }

    fn emit_epilogue(&mut self, code: &mut CodeBuffer) {
        self.emit_load_mem(code, self.mem_base_reg(), reg::SP, 16);
        self.emit_load_mem(code, reg::LR, reg::SP, 8);
        self.emit_load_mem(code, reg::FP, reg::SP, 0);
        self.emit_add_imm(code, reg::SP, reg::SP, self.saved_stack_size());
        self.emit_builder(code, |builder| {
            builder.ret();
        });
    }

    fn emit_load_slot(&mut self, code: &mut CodeBuffer, dst: Self::Reg, slot: u16) {
        self.emit_load_mem(code, dst, self.fp_reg(), Self::slot_offset(slot));
    }

    fn emit_store_slot(&mut self, code: &mut CodeBuffer, slot: u16, src: Self::Reg) {
        self.emit_store_mem(code, self.fp_reg(), Self::slot_offset(slot), src);
    }

    fn emit_load_slot_offset(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        slot: u16,
        byte_offset: u32,
    ) {
        self.emit_load_mem(
            code,
            dst,
            self.fp_reg(),
            Self::slot_offset(slot) + byte_offset,
        );
    }

    fn emit_li(&mut self, code: &mut CodeBuffer, dst: Self::Reg, imm: i64) {
        self.emit_mov_imm(code, dst, imm as u64);
    }

    fn emit_add(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_builder(code, |builder| {
            builder.add(dst, lhs, rhs);
        });
    }

    fn emit_addw(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_raw(code, Self::encode_add_sub_reg(0, 0, rhs, lhs, dst));
    }

    fn emit_sub(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_builder(code, |builder| {
            builder.sub(dst, lhs, rhs);
        });
    }

    fn emit_subw(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_raw(code, Self::encode_add_sub_reg(0, 1, rhs, lhs, dst));
    }

    fn emit_mul(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_builder(code, |builder| {
            builder.mul(dst, lhs, rhs);
        });
    }

    fn emit_mulw(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_raw(
            code,
            Self::encode_multiply(0, 0b000, rhs, 0, reg::WZR, lhs, dst),
        );
    }

    fn emit_div_s(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        self.emit_builder(code, |builder| {
            builder.sdiv(dst, lhs, rhs);
        });
    }

    fn emit_divw(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_raw(code, Self::encode_divide(0, 0b000011, rhs, lhs, dst));
    }

    fn emit_div_u(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        self.emit_builder(code, |builder| {
            builder.udiv(dst, lhs, rhs);
        });
    }

    fn emit_divuw(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        self.emit_raw(code, Self::encode_divide(0, 0b000010, rhs, lhs, dst));
    }

    fn emit_rem_s(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        let scratch = if dst != self.tmp0() && lhs != self.tmp0() && rhs != self.tmp0() {
            self.tmp0()
        } else if dst != self.tmp1() && lhs != self.tmp1() && rhs != self.tmp1() {
            self.tmp1()
        } else {
            self.tmp2()
        };
        self.emit_builder(code, |builder| {
            builder.sdiv(scratch, lhs, rhs);
            builder.msub(dst, scratch, rhs, lhs);
        });
    }

    fn emit_remw(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        let scratch = if dst != self.tmp0() && lhs != self.tmp0() && rhs != self.tmp0() {
            self.tmp0()
        } else if dst != self.tmp1() && lhs != self.tmp1() && rhs != self.tmp1() {
            self.tmp1()
        } else {
            self.tmp2()
        };
        self.emit_raw(code, Self::encode_divide(0, 0b000011, rhs, lhs, scratch));
        self.emit_raw(
            code,
            Self::encode_multiply(0, 0b000, rhs, 1, lhs, scratch, dst),
        );
    }

    fn emit_rem_u(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        let scratch = if dst != self.tmp0() && lhs != self.tmp0() && rhs != self.tmp0() {
            self.tmp0()
        } else if dst != self.tmp1() && lhs != self.tmp1() && rhs != self.tmp1() {
            self.tmp1()
        } else {
            self.tmp2()
        };
        self.emit_builder(code, |builder| {
            builder.udiv(scratch, lhs, rhs);
            builder.msub(dst, scratch, rhs, lhs);
        });
    }

    fn emit_remuw(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        let scratch = if dst != self.tmp0() && lhs != self.tmp0() && rhs != self.tmp0() {
            self.tmp0()
        } else if dst != self.tmp1() && lhs != self.tmp1() && rhs != self.tmp1() {
            self.tmp1()
        } else {
            self.tmp2()
        };
        self.emit_raw(code, Self::encode_divide(0, 0b000010, rhs, lhs, scratch));
        self.emit_raw(
            code,
            Self::encode_multiply(0, 0b000, rhs, 1, lhs, scratch, dst),
        );
    }

    fn emit_and(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_builder(code, |builder| {
            builder.and(dst, lhs, rhs);
        });
    }

    fn emit_or(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_builder(code, |builder| {
            builder.or(dst, lhs, rhs);
        });
    }

    fn emit_xor(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_builder(code, |builder| {
            builder.xor(dst, lhs, rhs);
        });
    }

    fn emit_shl(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_raw(
            code,
            Self::encode_data_proc_2src(LSLV_OPCODE, dst, lhs, rhs),
        );
    }

    fn emit_sllw(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_raw(
            code,
            Self::encode_data_proc_2src_sf(0, LSLV_OPCODE, dst, lhs, rhs),
        );
    }

    fn emit_shr_u(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        self.emit_raw(
            code,
            Self::encode_data_proc_2src(LSRV_OPCODE, dst, lhs, rhs),
        );
    }

    fn emit_srlw(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_raw(
            code,
            Self::encode_data_proc_2src_sf(0, LSRV_OPCODE, dst, lhs, rhs),
        );
    }

    fn emit_shr_s(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        lhs: Self::Reg,
        rhs: Self::Reg,
    ) {
        self.emit_raw(
            code,
            Self::encode_data_proc_2src(ASRV_OPCODE, dst, lhs, rhs),
        );
    }

    fn emit_sraw(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_raw(
            code,
            Self::encode_data_proc_2src_sf(0, ASRV_OPCODE, dst, lhs, rhs),
        );
    }

    fn emit_eqz(&mut self, code: &mut CodeBuffer, dst: Self::Reg, src: Self::Reg) {
        let neg = if dst != self.tmp0() && src != self.tmp0() {
            self.tmp0()
        } else if dst != self.tmp1() && src != self.tmp1() {
            self.tmp1()
        } else {
            self.tmp2()
        };
        let shift_reg = if dst != self.tmp0() && src != self.tmp0() && neg != self.tmp0() {
            self.tmp0()
        } else if dst != self.tmp1() && src != self.tmp1() && neg != self.tmp1() {
            self.tmp1()
        } else {
            self.tmp2()
        };
        self.emit_builder(code, |builder| {
            builder.sub(neg, reg::XZR, src);
            builder.or(dst, src, neg);
        });
        self.emit_mov_imm(code, shift_reg, 63);
        self.emit_raw(
            code,
            Self::encode_data_proc_2src(LSRV_OPCODE, dst, dst, shift_reg),
        );
        self.emit_mov_imm(code, shift_reg, 1);
        self.emit_builder(code, |builder| {
            builder.xor(dst, dst, shift_reg);
        });
    }

    fn emit_slt(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_raw(code, Self::encode_cmp_reg(lhs, rhs));
        self.emit_raw(code, Self::encode_cset(dst, 0b1011));
    }

    fn emit_sltu(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        self.emit_raw(code, Self::encode_cmp_reg(lhs, rhs));
        self.emit_raw(code, Self::encode_cset(dst, 0b0011));
    }

    fn emit_snez(&mut self, code: &mut CodeBuffer, dst: Self::Reg, src: Self::Reg) {
        self.emit_raw(code, Self::encode_cmp_reg(src, reg::XZR));
        self.emit_raw(code, Self::encode_cset(dst, 0b0001));
    }

    fn emit_clz(&mut self, code: &mut CodeBuffer, dst: Self::Reg, src: Self::Reg) {
        self.emit_raw(code, Self::encode_clz(dst, src));
    }

    fn emit_ctz(&mut self, code: &mut CodeBuffer, dst: Self::Reg, src: Self::Reg) {
        self.emit_raw(code, Self::encode_rbit(dst, src));
        self.emit_raw(code, Self::encode_clz(dst, dst));
    }

    fn emit_rotl(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg) {
        let tmp = self.scratch_excluding(&[dst, lhs, rhs]);
        self.emit_mov_imm(code, tmp, 32);
        self.emit_builder(code, |builder| {
            builder.sub(tmp, tmp, rhs);
        });
        self.emit_raw(code, Self::encode_rorv(dst, lhs, tmp));
    }

    fn emit_load8_u(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        base: Self::Reg,
        offset: i32,
    ) {
        self.emit_load_narrow(code, dst, base, offset, Self::encode_ldrb);
    }

    fn emit_load8_s(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        base: Self::Reg,
        offset: i32,
    ) {
        self.emit_load_narrow(code, dst, base, offset, Self::encode_ldrsb);
    }

    fn emit_load16_u(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        base: Self::Reg,
        offset: i32,
    ) {
        self.emit_load_narrow(code, dst, base, offset, Self::encode_ldrh);
    }

    fn emit_store8(&mut self, code: &mut CodeBuffer, base: Self::Reg, offset: i32, src: Self::Reg) {
        self.emit_store_narrow(code, base, offset, src, Self::encode_strb);
    }

    fn emit_store16(
        &mut self,
        code: &mut CodeBuffer,
        base: Self::Reg,
        offset: i32,
        src: Self::Reg,
    ) {
        self.emit_store_narrow(code, base, offset, src, Self::encode_strh);
    }

    fn emit_select(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        val_a: Self::Reg,
        val_b: Self::Reg,
        cond: Self::Reg,
    ) {
        self.emit_raw(code, Self::encode_cmp_reg(cond, reg::XZR));
        self.emit_raw(code, Self::encode_csel(dst, val_a, val_b, 0b0001));
    }

    fn emit_jump(&mut self, code: &mut CodeBuffer, label: LabelId) {
        let at_offset = code.offset();
        code.add_fixup(Fixup {
            at_offset,
            kind: BranchKind::Unconditional,
            target: label,
        });
        self.emit_raw(code, Self::encode_b_placeholder());
    }

    fn emit_branch_zero(&mut self, code: &mut CodeBuffer, reg: Self::Reg, label: LabelId) {
        let at_offset = code.offset();
        code.add_fixup(Fixup {
            at_offset,
            kind: BranchKind::ConditionalZero,
            target: label,
        });
        self.emit_raw(code, Self::encode_cbz_placeholder(reg));
    }

    fn emit_branch_not_zero(&mut self, code: &mut CodeBuffer, reg: Self::Reg, label: LabelId) {
        let at_offset = code.offset();
        code.add_fixup(Fixup {
            at_offset,
            kind: BranchKind::ConditionalNotZero,
            target: label,
        });
        self.emit_raw(code, Self::encode_cbnz_placeholder(reg));
    }

    fn emit_call_host(&mut self, code: &mut CodeBuffer, addr: usize) {
        self.emit_store_mem(code, reg::SP, CTX_SPILL_OFFSET, self.ctx_reg());
        self.emit_store_mem(code, reg::SP, FP_SPILL_OFFSET, self.fp_reg());
        self.emit_mov_imm(code, self.tmp0(), addr as u64);
        self.emit_raw(code, Self::encode_blr(self.tmp0()));
        self.emit_load_mem(code, self.ctx_reg(), reg::SP, CTX_SPILL_OFFSET);
        self.emit_load_mem(code, self.fp_reg(), reg::SP, FP_SPILL_OFFSET);
    }

    fn emit_retval(&mut self, code: &mut CodeBuffer, src: Self::Reg) {
        if src != self.ret_reg() {
            self.emit_builder(code, |builder| {
                builder.mov(self.ret_reg(), src);
            });
        }
    }

    fn ctx_reg(&self) -> Self::Reg {
        reg::X0
    }

    fn fp_reg(&self) -> Self::Reg {
        reg::X1
    }

    fn tmp0(&self) -> Self::Reg {
        reg::X9
    }

    fn tmp1(&self) -> Self::Reg {
        reg::X10
    }

    fn tmp2(&self) -> Self::Reg {
        reg::X11
    }

    fn ret_reg(&self) -> Self::Reg {
        reg::X0
    }
}
