pub mod aarch64;
pub mod riscv64;

use crate::code::CodeBuffer;
use crate::control::LabelId;

pub trait ArchBackend {
    type Reg: Copy + Clone;

    fn emit_prologue(&mut self, code: &mut CodeBuffer, frame_slots: u16);
    fn emit_epilogue(&mut self, code: &mut CodeBuffer);

    fn emit_load_slot(&mut self, code: &mut CodeBuffer, dst: Self::Reg, slot: u16);
    fn emit_store_slot(&mut self, code: &mut CodeBuffer, slot: u16, src: Self::Reg);
    fn emit_load_slot_offset(
        &mut self,
        code: &mut CodeBuffer,
        dst: Self::Reg,
        slot: u16,
        byte_offset: u32,
    );

    fn emit_li(&mut self, code: &mut CodeBuffer, dst: Self::Reg, imm: i64);
    fn emit_add(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg);
    fn emit_sub(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg);
    fn emit_mul(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg);
    fn emit_div_s(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg);
    fn emit_div_u(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg);
    fn emit_rem_s(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg);
    fn emit_rem_u(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg);
    fn emit_and(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg);
    fn emit_or(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg);
    fn emit_xor(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg);
    fn emit_shl(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg);
    fn emit_shr_u(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg);
    fn emit_shr_s(&mut self, code: &mut CodeBuffer, dst: Self::Reg, lhs: Self::Reg, rhs: Self::Reg);
    fn emit_eqz(&mut self, code: &mut CodeBuffer, dst: Self::Reg, src: Self::Reg);

    fn emit_jump(&mut self, code: &mut CodeBuffer, label: LabelId);
    fn emit_branch_zero(&mut self, code: &mut CodeBuffer, reg: Self::Reg, label: LabelId);
    fn emit_branch_not_zero(&mut self, code: &mut CodeBuffer, reg: Self::Reg, label: LabelId);
    fn emit_call_host(&mut self, code: &mut CodeBuffer, addr: usize);
    fn emit_retval(&mut self, code: &mut CodeBuffer, src: Self::Reg);

    fn ctx_reg(&self) -> Self::Reg;
    fn fp_reg(&self) -> Self::Reg;
    fn tmp0(&self) -> Self::Reg;
    fn tmp1(&self) -> Self::Reg;
    fn tmp2(&self) -> Self::Reg;
    fn ret_reg(&self) -> Self::Reg;
}
