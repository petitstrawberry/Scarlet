use alloc::{string::String, vec::Vec};

use wasmparser::{
    BlockType, ExternalKind, FunctionBody, MemArg, Parser, Payload, TypeRef, VisitOperator,
};

use crate::arch::ArchBackend;
use crate::code::{CodeBuffer, ExecutableSlab};
use crate::control::{BranchKind, ControlFrame, ControlKind, ControlStack, Label, LabelId};
use crate::frame::FrameLayout;
use crate::helpers::{helper_call, helper_i32_load, helper_i32_store, helper_trap};
use crate::module::{FuncType, MemoryInfo, ValType};
use crate::runtime::VmContext;
use crate::{
    CompileError, CompiledFn, CompiledModule, ExportEntry, FunctionEntry, RawValue, TrapCode,
};

const HELPER_SLOT_COUNT: u16 = 17;
const HELPER_RET_SLOT: u16 = 0;
const HELPER_ARG0_SLOT: u16 = 0;
const HELPER_ARG1_SLOT: u16 = 1;
const HELPER_CALL_FUNC_SLOT: u16 = 0;
const HELPER_CALL_ARGS_BASE: u16 = 1;
const MAX_CALL_ARGS: usize = 16;

pub struct ValueStack {
    height: u16,
    max_height: u16,
    temp_base: u16,
    slots: Vec<u16>,
}

impl ValueStack {
    pub fn new(temp_base: u16) -> Self {
        Self {
            height: 0,
            max_height: 0,
            temp_base,
            slots: Vec::new(),
        }
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn depth(&self) -> u16 {
        self.slots.len() as u16
    }

    pub fn push(&mut self) -> u16 {
        let slot = self.temp_base + self.height;
        self.height += 1;
        if self.height > self.max_height {
            self.max_height = self.height;
        }
        self.slots.push(slot);
        slot
    }

    pub fn repush(&mut self, slot: u16) {
        self.height += 1;
        if self.height > self.max_height {
            self.max_height = self.height;
        }
        self.slots.push(slot);
    }

    pub fn pop(&mut self) -> u16 {
        self.height -= 1;
        self.slots.pop().unwrap_or(self.temp_base)
    }

    pub fn peek(&self) -> Option<u16> {
        self.slots.last().copied()
    }

    pub fn truncate_keep(&mut self, entry_depth: u16, keep: u8) {
        let keep_len = keep as usize;
        let mut kept = if keep_len == 0 {
            Vec::new()
        } else {
            self.slots
                .split_off(self.slots.len().saturating_sub(keep_len))
        };
        self.slots.truncate(entry_depth as usize);
        self.height = self.slots.len() as u16;
        self.slots.append(&mut kept);
        self.height = self.slots.len() as u16;
        if self.height > self.max_height {
            self.max_height = self.height;
        }
    }

    pub fn max_height(&self) -> u16 {
        self.max_height
    }
}

pub struct ImportInfo {
    pub module: String,
    pub name: String,
    pub type_index: Option<u32>,
}

pub struct CompiledFunction {
    pub code: CodeBuffer,
    pub frame_layout: FrameLayout,
    pub param_count: u16,
    pub local_count: u16,
}

pub struct ModuleCompiler<B: ArchBackend> {
    backend: B,
    func_types: Vec<FuncType>,
    func_type_indices: Vec<u32>,
    imports: Vec<ImportInfo>,
    exports: Vec<(String, u32)>,
    memories: Vec<MemoryInfo>,
    compiled_functions: Vec<CompiledFunction>,
    imported_func_count: u32,
    all_func_type_indices: Vec<u32>,
    data_segments: Vec<crate::DataSegment>,
}

impl<B: ArchBackend> ModuleCompiler<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            func_types: Vec::new(),
            func_type_indices: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            memories: Vec::new(),
            compiled_functions: Vec::new(),
            imported_func_count: 0,
            all_func_type_indices: Vec::new(),
            data_segments: Vec::new(),
        }
    }

    pub fn compile(&mut self, wasm_bytes: &[u8]) -> Result<CompiledModule, CompileError> {
        self.func_types.clear();
        self.func_type_indices.clear();
        self.imports.clear();
        self.exports.clear();
        self.memories.clear();
        self.compiled_functions.clear();
        self.imported_func_count = 0;
        self.all_func_type_indices.clear();
        self.data_segments.clear();

        for payload in Parser::new(0).parse_all(wasm_bytes) {
            match payload.map_err(|_| CompileError::InvalidWasm("parse error"))? {
                Payload::TypeSection(reader) => {
                    for ft in reader.into_iter_err_on_gc_types() {
                        let ft = ft.map_err(|_| CompileError::InvalidWasm("type section"))?;
                        let mut params = Vec::new();
                        for param in ft.params() {
                            params.push(convert_val_type(*param)?);
                        }

                        let mut results = Vec::new();
                        for result in ft.results() {
                            results.push(convert_val_type(*result)?);
                        }

                        self.func_types.push(FuncType { params, results });
                    }
                }
                Payload::ImportSection(reader) => {
                    for import in reader {
                        let import =
                            import.map_err(|_| CompileError::InvalidWasm("import section"))?;
                        let type_index = match import.ty {
                            TypeRef::Func(type_index) => {
                                self.imported_func_count += 1;
                                self.all_func_type_indices.push(type_index);
                                Some(type_index)
                            }
                            _ => None,
                        };

                        self.imports.push(ImportInfo {
                            module: String::from(import.module),
                            name: String::from(import.name),
                            type_index,
                        });
                    }
                }
                Payload::FunctionSection(reader) => {
                    for type_index in reader {
                        let type_index = type_index
                            .map_err(|_| CompileError::InvalidWasm("function section"))?;
                        self.func_type_indices.push(type_index);
                        self.all_func_type_indices.push(type_index);
                    }
                }
                Payload::MemorySection(reader) => {
                    for memory in reader {
                        let memory =
                            memory.map_err(|_| CompileError::InvalidWasm("memory section"))?;
                        self.memories.push(MemoryInfo {
                            initial_pages: u32::try_from(memory.initial)
                                .map_err(|_| CompileError::InvalidWasm("memory initial"))?,
                            max_pages: memory
                                .maximum
                                .map(u32::try_from)
                                .transpose()
                                .map_err(|_| CompileError::InvalidWasm("memory max"))?,
                        });
                    }
                }
                Payload::ExportSection(reader) => {
                    for export in reader {
                        let export =
                            export.map_err(|_| CompileError::InvalidWasm("export section"))?;
                        if export.kind == ExternalKind::Func {
                            self.exports.push((String::from(export.name), export.index));
                        }
                    }
                }
                Payload::CodeSectionEntry(func_body) => {
                    let local_index = self.compiled_functions.len();
                    let type_index = *self
                        .func_type_indices
                        .get(local_index)
                        .ok_or(CompileError::InvalidWasm("code/function count mismatch"))?;
                    let func_type = self
                        .func_types
                        .get(type_index as usize)
                        .ok_or(CompileError::InvalidWasm("function type index"))?;

                    let compiler = FunctionCompiler::new(
                        &mut self.backend,
                        func_type,
                        &self.func_types,
                        &self.all_func_type_indices,
                        self.imported_func_count,
                    );
                    let compiled = compiler.compile(&func_body)?;
                    self.compiled_functions.push(compiled);
                }
                Payload::DataSection(reader) => {
                    for data_entry in reader {
                        let data_entry =
                            data_entry.map_err(|_| CompileError::InvalidWasm("data section"))?;
                        let (offset, data) = match data_entry.kind {
                            wasmparser::DataKind::Active { offset_expr, .. } => {
                                let mut offset_reader = offset_expr.get_binary_reader();
                                let offset_op = offset_reader
                                    .read_operator()
                                    .map_err(|_| CompileError::InvalidWasm("data offset"))?;
                                let offset = match offset_op {
                                    wasmparser::Operator::I32Const { value } => value as u32,
                                    wasmparser::Operator::I64Const { value } => value as u32,
                                    wasmparser::Operator::GlobalGet { .. } => 0,
                                    _ => 0,
                                };
                                (offset, data_entry.data)
                            }
                            wasmparser::DataKind::Passive => (0, data_entry.data),
                        };
                        self.data_segments.push(crate::DataSegment {
                            offset,
                            data: data.to_vec(),
                        });
                    }
                }
                _ => {}
            }
        }

        let mut slab_bytes = Vec::new();
        let mut offsets = Vec::with_capacity(self.compiled_functions.len());
        for compiled in &self.compiled_functions {
            offsets.push(slab_bytes.len());
            slab_bytes.extend_from_slice(&compiled.code.bytes);
        }

        let (slab_ptr, slab_len) = if slab_bytes.is_empty() {
            (core::ptr::null_mut(), 0)
        } else {
            let exec_ptr = crate::engine::alloc_exec_memory(slab_bytes.len());
            if exec_ptr.is_null() {
                return Err(CompileError::CodeGen("executable memory allocation failed"));
            }
            unsafe {
                core::ptr::copy_nonoverlapping(slab_bytes.as_ptr(), exec_ptr, slab_bytes.len());
            }
            (exec_ptr, slab_bytes.len())
        };

        let mut functions = Vec::with_capacity(self.compiled_functions.len());
        for (compiled, offset) in self.compiled_functions.iter().zip(offsets.iter()) {
            let code = if slab_ptr.is_null() {
                empty_compiled_fn
            } else {
                unsafe { core::mem::transmute::<*mut u8, CompiledFn>(slab_ptr.add(*offset)) }
            };
            functions.push(FunctionEntry {
                code,
                frame_slots: compiled.frame_layout.total_slots,
                param_count: compiled.param_count,
                local_count: compiled.local_count,
                max_stack: compiled.frame_layout.total_slots
                    - compiled.param_count
                    - compiled.local_count
                    - HELPER_SLOT_COUNT,
            });
        }

        let mut exports = Vec::new();
        for (name, wasm_func_index) in &self.exports {
            if *wasm_func_index < self.imported_func_count {
                return Err(CompileError::UnsupportedFeature("imported function export"));
            }

            exports.push(ExportEntry {
                name: name.clone(),
                func_index: *wasm_func_index - self.imported_func_count,
            });
        }

        let mut imports = Vec::new();
        for imp in &self.imports {
            if let Some(type_index) = imp.type_index {
                imports.push(crate::ImportedFuncEntry {
                    module: imp.module.clone(),
                    name: imp.name.clone(),
                    type_index,
                });
            }
        }

        Ok(CompiledModule {
            code: ExecutableSlab {
                ptr: slab_ptr,
                len: slab_len,
            },
            functions: functions.into_boxed_slice(),
            exports: exports.into_boxed_slice(),
            imported_funcs: imports.into_boxed_slice(),
            data_segments: core::mem::take(&mut self.data_segments).into_boxed_slice(),
        })
    }
}

pub struct FunctionCompiler<'ctx, B: ArchBackend> {
    backend: &'ctx mut B,
    code: CodeBuffer,
    value_stack: ValueStack,
    control_stack: ControlStack,
    control_frames: Vec<ControlFrame>,
    labels: Vec<Label>,
    frame_layout: FrameLayout,
    param_count: u16,
    result_count: u16,
    local_count: u16,
    func_types: &'ctx [FuncType],
    all_func_type_indices: &'ctx [u32],
    imported_func_count: u32,
    function_epilogue_label: LabelId,
}

impl<'ctx, B: ArchBackend> FunctionCompiler<'ctx, B> {
    pub fn new(
        backend: &'ctx mut B,
        func_type: &'ctx FuncType,
        func_types: &'ctx [FuncType],
        all_func_type_indices: &'ctx [u32],
        imported_func_count: u32,
    ) -> Self {
        Self {
            backend,
            code: CodeBuffer::new(),
            value_stack: ValueStack::new(HELPER_SLOT_COUNT + func_type.params.len() as u16),
            control_stack: ControlStack::new(),
            control_frames: Vec::new(),
            labels: Vec::new(),
            frame_layout: FrameLayout::new(func_type.params.len() as u16, 0, 0),
            param_count: func_type.params.len() as u16,
            result_count: func_type.results.len() as u16,
            local_count: 0,
            func_types,
            all_func_type_indices,
            imported_func_count,
            function_epilogue_label: 0,
        }
    }

    pub fn compile(
        mut self,
        func_body: &FunctionBody<'_>,
    ) -> Result<CompiledFunction, CompileError> {
        let mut locals_reader = func_body
            .get_locals_reader()
            .map_err(|_| CompileError::InvalidWasm("locals reader"))?;
        for _ in 0..locals_reader.get_count() {
            let (count, ty) = locals_reader
                .read()
                .map_err(|_| CompileError::InvalidWasm("locals section"))?;
            let _ = convert_val_type(ty)?;
            self.local_count = self
                .local_count
                .checked_add(count as u16)
                .ok_or(CompileError::CodeGen("too many locals"))?;
        }

        let estimated_stack = u16::try_from(func_body.as_bytes().len())
            .map_err(|_| CompileError::CodeGen("function body too large"))?;
        self.frame_layout = FrameLayout::new(self.param_count, self.local_count, estimated_stack);
        self.frame_layout.stack_base = HELPER_SLOT_COUNT + self.param_count + self.local_count;
        self.frame_layout.total_slots = self.frame_layout.stack_base + estimated_stack;
        self.value_stack = ValueStack::new(self.frame_layout.stack_base);
        self.function_epilogue_label = self.new_label();

        self.backend
            .emit_prologue(&mut self.code, self.frame_layout.total_slots);
        self.copy_params_to_runtime_slots();

        let mut op_reader = func_body
            .get_operators_reader()
            .map_err(|_| CompileError::InvalidWasm("operators reader"))?;
        while !op_reader.eof() {
            op_reader
                .visit_operator(&mut self)
                .map_err(|_| CompileError::InvalidWasm("operator visitor"))??;
        }
        op_reader
            .ensure_end()
            .map_err(|_| CompileError::InvalidWasm("operators finish"))?;

        if self.result_count > 1 {
            return Err(CompileError::UnsupportedFeature("multi-value results"));
        }

        if self.result_count == 1 {
            let result_slot = self
                .value_stack
                .peek()
                .ok_or(CompileError::InvalidWasm("missing return value"))?;
            self.backend
                .emit_load_slot(&mut self.code, self.backend.tmp0(), result_slot);
            self.backend
                .emit_retval(&mut self.code, self.backend.tmp0());
        } else {
            self.emit_zero_return();
        }

        self.bind_label(self.function_epilogue_label);
        self.backend.emit_epilogue(&mut self.code);

        resolve_fixups(&mut self.code, &self.labels)?;

        Ok(CompiledFunction {
            code: self.code,
            frame_layout: self.frame_layout,
            param_count: self.param_count,
            local_count: self.local_count,
        })
    }

    fn copy_params_to_runtime_slots(&mut self) {
        for index in 0..self.param_count {
            let tmp0 = self.backend.tmp0();
            let runtime_slot = self.runtime_param_slot(index as u32);
            self.backend.emit_load_slot(&mut self.code, tmp0, index);
            self.backend
                .emit_store_slot(&mut self.code, runtime_slot, tmp0);
        }
    }

    fn new_label(&mut self) -> LabelId {
        let id = self.labels.len() as LabelId;
        self.labels.push(Label::new());
        id
    }

    fn bind_label(&mut self, label: LabelId) {
        if let Some(entry) = self.labels.get_mut(label as usize) {
            entry.bind(self.code.offset());
        }
    }

    fn runtime_param_slot(&self, index: u32) -> u16 {
        HELPER_SLOT_COUNT + index as u16
    }

    fn runtime_local_slot(&self, index: u32) -> u16 {
        HELPER_SLOT_COUNT + self.param_count + index as u16
    }

    fn wasm_local_slot(&self, index: u32) -> Result<u16, CompileError> {
        if index < self.param_count as u32 {
            Ok(self.runtime_param_slot(index))
        } else {
            let local_index = index - self.param_count as u32;
            if local_index >= self.local_count as u32 {
                Err(CompileError::InvalidWasm("local index"))
            } else {
                Ok(self.runtime_local_slot(local_index))
            }
        }
    }

    fn helper_slot(&self, index: u16) -> u16 {
        index
    }

    fn push_control_frame(&mut self, frame: ControlFrame) {
        self.control_stack.push(ControlFrame {
            kind: match frame.kind {
                ControlKind::Block => ControlKind::Block,
                ControlKind::Loop => ControlKind::Loop,
                ControlKind::If => ControlKind::If,
            },
            entry_stack_height: frame.entry_stack_height,
            result_arity: frame.result_arity,
            branch_target: frame.branch_target,
            end_label: frame.end_label,
            else_label: frame.else_label,
        });
        self.control_frames.push(frame);
    }

    fn pop_control_frame(&mut self) -> Option<ControlFrame> {
        let _ = self.control_stack.pop();
        self.control_frames.pop()
    }

    fn branch_frame(&self, depth: u32) -> Result<&ControlFrame, CompileError> {
        let idx = self
            .control_frames
            .len()
            .checked_sub(depth as usize + 1)
            .ok_or(CompileError::InvalidWasm("branch depth"))?;
        self.control_frames
            .get(idx)
            .ok_or(CompileError::InvalidWasm("branch frame"))
    }

    fn block_result_arity(&self, blockty: BlockType) -> Result<u8, CompileError> {
        match blockty {
            BlockType::Empty => Ok(0),
            BlockType::Type(_) => Ok(1),
            BlockType::FuncType(type_index) => {
                let func_type = self
                    .func_types
                    .get(type_index as usize)
                    .ok_or(CompileError::InvalidWasm("block type index"))?;
                if func_type.results.len() > 1 {
                    Err(CompileError::UnsupportedFeature("multi-value block"))
                } else {
                    Ok(func_type.results.len() as u8)
                }
            }
        }
    }

    fn emit_zero_return(&mut self) {
        self.backend.emit_li(&mut self.code, self.backend.tmp0(), 0);
        self.backend
            .emit_retval(&mut self.code, self.backend.tmp0());
    }

    fn emit_return(&mut self) -> Result<(), CompileError> {
        if self.result_count == 1 {
            let result_slot = self
                .value_stack
                .peek()
                .ok_or(CompileError::InvalidWasm("missing return value"))?;
            self.backend
                .emit_load_slot(&mut self.code, self.backend.tmp0(), result_slot);
            self.backend
                .emit_retval(&mut self.code, self.backend.tmp0());
        } else {
            self.emit_zero_return();
        }
        self.backend
            .emit_jump(&mut self.code, self.function_epilogue_label);
        Ok(())
    }

    fn emit_binary_op(
        &mut self,
        op: fn(&mut B, &mut CodeBuffer, B::Reg, B::Reg, B::Reg),
    ) -> Result<(), CompileError> {
        let rhs = self.value_stack.pop();
        let lhs = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), lhs);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp1(), rhs);
        op(
            self.backend,
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        let result = self.value_stack.push();
        self.backend
            .emit_store_slot(&mut self.code, result, self.backend.tmp0());
        Ok(())
    }

    fn emit_memory_addr(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        let addr_slot = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), addr_slot);
        self.backend.emit_li(
            &mut self.code,
            self.backend.tmp1(),
            i64::try_from(memarg.offset).map_err(|_| CompileError::CodeGen("memarg offset"))?,
        );
        self.backend.emit_add(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        Ok(())
    }

    fn emit_call_wrapper_result(&mut self, result_slot: u16) {
        let tmp0 = self.backend.tmp0();
        let helper_ret_slot = self.helper_slot(HELPER_RET_SLOT);
        self.backend
            .emit_load_slot(&mut self.code, tmp0, helper_ret_slot);
        self.backend
            .emit_store_slot(&mut self.code, result_slot, tmp0);
    }

    fn visit_block_impl(&mut self, blockty: BlockType) -> Result<(), CompileError> {
        let end_label = self.new_label();
        self.push_control_frame(ControlFrame {
            kind: ControlKind::Block,
            entry_stack_height: self.value_stack.depth(),
            result_arity: self.block_result_arity(blockty)?,
            branch_target: end_label,
            end_label,
            else_label: None,
        });
        Ok(())
    }

    fn visit_loop_impl(&mut self, blockty: BlockType) -> Result<(), CompileError> {
        let start_label = self.new_label();
        let end_label = self.new_label();
        self.bind_label(start_label);
        self.push_control_frame(ControlFrame {
            kind: ControlKind::Loop,
            entry_stack_height: self.value_stack.depth(),
            result_arity: self.block_result_arity(blockty)?,
            branch_target: start_label,
            end_label,
            else_label: None,
        });
        Ok(())
    }

    fn visit_if_impl(&mut self, blockty: BlockType) -> Result<(), CompileError> {
        let cond_slot = self.value_stack.pop();
        let else_label = self.new_label();
        let end_label = self.new_label();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), cond_slot);
        self.backend
            .emit_branch_zero(&mut self.code, self.backend.tmp0(), else_label);
        self.push_control_frame(ControlFrame {
            kind: ControlKind::If,
            entry_stack_height: self.value_stack.depth(),
            result_arity: self.block_result_arity(blockty)?,
            branch_target: end_label,
            end_label,
            else_label: Some(else_label),
        });
        Ok(())
    }

    fn visit_else_impl(&mut self) -> Result<(), CompileError> {
        let mut frame = self
            .pop_control_frame()
            .ok_or(CompileError::InvalidWasm("else without if"))?;
        if !matches!(frame.kind, ControlKind::If) {
            return Err(CompileError::InvalidWasm("else without if"));
        }

        self.backend.emit_jump(&mut self.code, frame.end_label);
        self.value_stack.truncate_keep(frame.entry_stack_height, 0);
        if let Some(else_label) = frame.else_label.take() {
            self.bind_label(else_label);
        }
        self.push_control_frame(frame);
        Ok(())
    }

    fn visit_end_impl(&mut self) -> Result<(), CompileError> {
        if let Some(frame) = self.pop_control_frame() {
            self.value_stack
                .truncate_keep(frame.entry_stack_height, frame.result_arity);
            if matches!(frame.kind, ControlKind::If) {
                if let Some(else_label) = frame.else_label {
                    self.bind_label(else_label);
                }
            }
            self.bind_label(frame.end_label);
        }
        Ok(())
    }

    fn visit_br_impl(&mut self, relative_depth: u32) -> Result<(), CompileError> {
        let (branch_target, entry_stack_height, result_arity, is_loop) = {
            let frame = self.branch_frame(relative_depth)?;
            (
                frame.branch_target,
                frame.entry_stack_height,
                frame.result_arity,
                matches!(frame.kind, ControlKind::Loop),
            )
        };
        self.backend.emit_jump(&mut self.code, branch_target);
        let keep = if is_loop { 0 } else { result_arity };
        self.value_stack.truncate_keep(entry_stack_height, keep);
        Ok(())
    }

    fn visit_br_if_impl(&mut self, relative_depth: u32) -> Result<(), CompileError> {
        let cond_slot = self.value_stack.pop();
        let branch_target = self.branch_frame(relative_depth)?.branch_target;
        let tmp0 = self.backend.tmp0();
        self.backend.emit_load_slot(&mut self.code, tmp0, cond_slot);
        self.backend
            .emit_branch_not_zero(&mut self.code, tmp0, branch_target);
        Ok(())
    }

    fn visit_return_impl(&mut self) -> Result<(), CompileError> {
        self.emit_return()
    }

    fn visit_unreachable_impl(&mut self) -> Result<(), CompileError> {
        self.backend.emit_call_host(
            &mut self.code,
            host_unreachable_wrapper as *const () as usize,
        );
        Ok(())
    }

    fn visit_local_get_impl(&mut self, local_index: u32) -> Result<(), CompileError> {
        let local_slot = self.wasm_local_slot(local_index)?;
        let stack_slot = self.value_stack.push();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), local_slot);
        self.backend
            .emit_store_slot(&mut self.code, stack_slot, self.backend.tmp0());
        Ok(())
    }

    fn visit_local_set_impl(&mut self, local_index: u32) -> Result<(), CompileError> {
        let src_slot = self.value_stack.pop();
        let dst_slot = self.wasm_local_slot(local_index)?;
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), src_slot);
        self.backend
            .emit_store_slot(&mut self.code, dst_slot, self.backend.tmp0());
        Ok(())
    }

    fn visit_local_tee_impl(&mut self, local_index: u32) -> Result<(), CompileError> {
        let value_slot = self.value_stack.pop();
        let local_slot = self.wasm_local_slot(local_index)?;
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), value_slot);
        self.backend
            .emit_store_slot(&mut self.code, local_slot, self.backend.tmp0());
        self.value_stack.repush(value_slot);
        Ok(())
    }

    fn visit_i32_const_impl(&mut self, value: i32) -> Result<(), CompileError> {
        let slot = self.value_stack.push();
        self.backend
            .emit_li(&mut self.code, self.backend.tmp0(), value as i64);
        self.backend
            .emit_store_slot(&mut self.code, slot, self.backend.tmp0());
        Ok(())
    }

    fn visit_i64_const_impl(&mut self, value: i64) -> Result<(), CompileError> {
        let slot = self.value_stack.push();
        self.backend
            .emit_li(&mut self.code, self.backend.tmp0(), value);
        self.backend
            .emit_store_slot(&mut self.code, slot, self.backend.tmp0());
        Ok(())
    }

    fn visit_i32_add_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_add)
    }

    fn visit_i32_sub_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_sub)
    }

    fn visit_i32_mul_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_mul)
    }

    fn visit_i32_div_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_div_s)
    }

    fn visit_i32_div_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_div_u)
    }

    fn visit_i32_rem_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_rem_s)
    }

    fn visit_i32_rem_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_rem_u)
    }

    fn visit_i32_and_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_and)
    }

    fn visit_i32_or_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_or)
    }

    fn visit_i32_xor_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_xor)
    }

    fn visit_i32_shl_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_shl)
    }

    fn visit_i32_shr_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_shr_u)
    }

    fn visit_i32_shr_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_shr_s)
    }

    fn visit_i64_add_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_add)
    }

    fn visit_i64_sub_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_sub)
    }

    fn visit_i64_mul_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_mul)
    }

    fn visit_i64_div_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_div_s)
    }

    fn visit_i64_div_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_div_u)
    }

    fn visit_i64_rem_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_rem_s)
    }

    fn visit_i64_rem_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_rem_u)
    }

    fn visit_i64_and_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_and)
    }

    fn visit_i64_or_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_or)
    }

    fn visit_i64_xor_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_xor)
    }

    fn visit_i64_shl_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_shl)
    }

    fn visit_i64_shr_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_shr_u)
    }

    fn visit_i64_shr_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_shr_s)
    }

    fn visit_i32_eqz_impl(&mut self) -> Result<(), CompileError> {
        let input = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), input);
        self.backend
            .emit_eqz(&mut self.code, self.backend.tmp0(), self.backend.tmp0());
        let result = self.value_stack.push();
        self.backend
            .emit_store_slot(&mut self.code, result, self.backend.tmp0());
        Ok(())
    }

    fn visit_i32_load_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let tmp0 = self.backend.tmp0();
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, tmp0);
        self.backend
            .emit_call_host(&mut self.code, host_i32_load_wrapper as *const () as usize);
        let result = self.value_stack.push();
        self.emit_call_wrapper_result(result);
        Ok(())
    }

    fn visit_i32_store_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        let value_slot = self.value_stack.pop();
        let addr_slot = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), addr_slot);
        self.backend.emit_li(
            &mut self.code,
            self.backend.tmp1(),
            i64::try_from(memarg.offset).map_err(|_| CompileError::CodeGen("memarg offset"))?,
        );
        self.backend.emit_add(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp1(), value_slot);
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let helper_arg1 = self.helper_slot(HELPER_ARG1_SLOT);
        let tmp0 = self.backend.tmp0();
        let tmp1 = self.backend.tmp1();
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, tmp0);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg1, tmp1);
        self.backend
            .emit_call_host(&mut self.code, host_i32_store_wrapper as *const () as usize);
        Ok(())
    }

    fn visit_i64_load_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let tmp0 = self.backend.tmp0();
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, tmp0);
        self.backend
            .emit_call_host(&mut self.code, host_i64_load_wrapper as *const () as usize);
        let result = self.value_stack.push();
        self.emit_call_wrapper_result(result);
        Ok(())
    }

    fn visit_i64_store_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        let value_slot = self.value_stack.pop();
        let addr_slot = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), addr_slot);
        self.backend.emit_li(
            &mut self.code,
            self.backend.tmp1(),
            i64::try_from(memarg.offset).map_err(|_| CompileError::CodeGen("memarg offset"))?,
        );
        self.backend.emit_add(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp1(), value_slot);
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let helper_arg1 = self.helper_slot(HELPER_ARG1_SLOT);
        let tmp0 = self.backend.tmp0();
        let tmp1 = self.backend.tmp1();
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, tmp0);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg1, tmp1);
        self.backend
            .emit_call_host(&mut self.code, host_i64_store_wrapper as *const () as usize);
        Ok(())
    }

    fn visit_call_impl(&mut self, function_index: u32) -> Result<(), CompileError> {
        let type_index = *self
            .all_func_type_indices
            .get(function_index as usize)
            .ok_or(CompileError::InvalidWasm("call function index"))?;
        let func_type = self
            .func_types
            .get(type_index as usize)
            .ok_or(CompileError::InvalidWasm("call type index"))?;

        if func_type.params.len() > MAX_CALL_ARGS {
            return Err(CompileError::UnsupportedFeature("call argument count"));
        }
        if func_type.results.len() > 1 {
            return Err(CompileError::UnsupportedFeature("multi-value call"));
        }

        for arg_index in (0..func_type.params.len()).rev() {
            let arg_slot = self.value_stack.pop();
            let helper_arg_slot = self.helper_slot(HELPER_CALL_ARGS_BASE + arg_index as u16);
            let tmp0 = self.backend.tmp0();
            self.backend.emit_load_slot(&mut self.code, tmp0, arg_slot);
            self.backend
                .emit_store_slot(&mut self.code, helper_arg_slot, tmp0);
        }

        if function_index < self.imported_func_count {
            self.backend.emit_li(
                &mut self.code,
                self.backend.tmp0(),
                i64::from(function_index),
            );
            let helper_func_slot = self.helper_slot(HELPER_CALL_FUNC_SLOT);
            let tmp0 = self.backend.tmp0();
            self.backend
                .emit_store_slot(&mut self.code, helper_func_slot, tmp0);
            self.backend.emit_call_host(
                &mut self.code,
                host_import_call_wrapper as *const () as usize,
            );
        } else {
            self.backend.emit_li(
                &mut self.code,
                self.backend.tmp0(),
                i64::from(function_index - self.imported_func_count),
            );
            let helper_func_slot = self.helper_slot(HELPER_CALL_FUNC_SLOT);
            let tmp0 = self.backend.tmp0();
            self.backend
                .emit_store_slot(&mut self.code, helper_func_slot, tmp0);
            self.backend
                .emit_call_host(&mut self.code, host_call_wrapper as *const () as usize);
        }

        if func_type.results.len() == 1 {
            let result = self.value_stack.push();
            self.emit_call_wrapper_result(result);
        }

        Ok(())
    }
}

macro_rules! function_compiler_visit_one {
    (@$proposal:ident $op:ident { blockty: $argty:ty } => visit_block ($($ann:tt)*)) => {
        fn visit_block(&mut self, blockty: $argty) -> Self::Output {
            self.visit_block_impl(blockty)
        }
    };
    (@$proposal:ident $op:ident { blockty: $argty:ty } => visit_loop ($($ann:tt)*)) => {
        fn visit_loop(&mut self, blockty: $argty) -> Self::Output {
            self.visit_loop_impl(blockty)
        }
    };
    (@$proposal:ident $op:ident { blockty: $argty:ty } => visit_if ($($ann:tt)*)) => {
        fn visit_if(&mut self, blockty: $argty) -> Self::Output {
            self.visit_if_impl(blockty)
        }
    };
    (@$proposal:ident $op:ident => visit_else ($($ann:tt)*)) => {
        fn visit_else(&mut self) -> Self::Output {
            self.visit_else_impl()
        }
    };
    (@$proposal:ident $op:ident => visit_end ($($ann:tt)*)) => {
        fn visit_end(&mut self) -> Self::Output {
            self.visit_end_impl()
        }
    };
    (@$proposal:ident $op:ident { relative_depth: $argty:ty } => visit_br ($($ann:tt)*)) => {
        fn visit_br(&mut self, relative_depth: $argty) -> Self::Output {
            self.visit_br_impl(relative_depth)
        }
    };
    (@$proposal:ident $op:ident { relative_depth: $argty:ty } => visit_br_if ($($ann:tt)*)) => {
        fn visit_br_if(&mut self, relative_depth: $argty) -> Self::Output {
            self.visit_br_if_impl(relative_depth)
        }
    };
    (@$proposal:ident $op:ident => visit_return ($($ann:tt)*)) => {
        fn visit_return(&mut self) -> Self::Output {
            self.visit_return_impl()
        }
    };
    (@$proposal:ident $op:ident => visit_unreachable ($($ann:tt)*)) => {
        fn visit_unreachable(&mut self) -> Self::Output {
            self.visit_unreachable_impl()
        }
    };
    (@$proposal:ident $op:ident { local_index: $argty:ty } => visit_local_get ($($ann:tt)*)) => {
        fn visit_local_get(&mut self, local_index: $argty) -> Self::Output {
            self.visit_local_get_impl(local_index)
        }
    };
    (@$proposal:ident $op:ident { local_index: $argty:ty } => visit_local_set ($($ann:tt)*)) => {
        fn visit_local_set(&mut self, local_index: $argty) -> Self::Output {
            self.visit_local_set_impl(local_index)
        }
    };
    (@$proposal:ident $op:ident { local_index: $argty:ty } => visit_local_tee ($($ann:tt)*)) => {
        fn visit_local_tee(&mut self, local_index: $argty) -> Self::Output {
            self.visit_local_tee_impl(local_index)
        }
    };
    (@$proposal:ident $op:ident { value: $argty:ty } => visit_i32_const ($($ann:tt)*)) => {
        fn visit_i32_const(&mut self, value: $argty) -> Self::Output {
            self.visit_i32_const_impl(value)
        }
    };
    (@$proposal:ident $op:ident { value: $argty:ty } => visit_i64_const ($($ann:tt)*)) => {
        fn visit_i64_const(&mut self, value: $argty) -> Self::Output {
            self.visit_i64_const_impl(value)
        }
    };
    (@$proposal:ident $op:ident => visit_i32_add ($($ann:tt)*)) => {
        fn visit_i32_add(&mut self) -> Self::Output { self.visit_i32_add_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_sub ($($ann:tt)*)) => {
        fn visit_i32_sub(&mut self) -> Self::Output { self.visit_i32_sub_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_mul ($($ann:tt)*)) => {
        fn visit_i32_mul(&mut self) -> Self::Output { self.visit_i32_mul_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_div_s ($($ann:tt)*)) => {
        fn visit_i32_div_s(&mut self) -> Self::Output { self.visit_i32_div_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_div_u ($($ann:tt)*)) => {
        fn visit_i32_div_u(&mut self) -> Self::Output { self.visit_i32_div_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_rem_s ($($ann:tt)*)) => {
        fn visit_i32_rem_s(&mut self) -> Self::Output { self.visit_i32_rem_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_rem_u ($($ann:tt)*)) => {
        fn visit_i32_rem_u(&mut self) -> Self::Output { self.visit_i32_rem_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_and ($($ann:tt)*)) => {
        fn visit_i32_and(&mut self) -> Self::Output { self.visit_i32_and_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_or ($($ann:tt)*)) => {
        fn visit_i32_or(&mut self) -> Self::Output { self.visit_i32_or_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_xor ($($ann:tt)*)) => {
        fn visit_i32_xor(&mut self) -> Self::Output { self.visit_i32_xor_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_shl ($($ann:tt)*)) => {
        fn visit_i32_shl(&mut self) -> Self::Output { self.visit_i32_shl_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_shr_u ($($ann:tt)*)) => {
        fn visit_i32_shr_u(&mut self) -> Self::Output { self.visit_i32_shr_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_shr_s ($($ann:tt)*)) => {
        fn visit_i32_shr_s(&mut self) -> Self::Output { self.visit_i32_shr_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_add ($($ann:tt)*)) => {
        fn visit_i64_add(&mut self) -> Self::Output { self.visit_i64_add_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_sub ($($ann:tt)*)) => {
        fn visit_i64_sub(&mut self) -> Self::Output { self.visit_i64_sub_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_mul ($($ann:tt)*)) => {
        fn visit_i64_mul(&mut self) -> Self::Output { self.visit_i64_mul_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_div_s ($($ann:tt)*)) => {
        fn visit_i64_div_s(&mut self) -> Self::Output { self.visit_i64_div_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_div_u ($($ann:tt)*)) => {
        fn visit_i64_div_u(&mut self) -> Self::Output { self.visit_i64_div_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_rem_s ($($ann:tt)*)) => {
        fn visit_i64_rem_s(&mut self) -> Self::Output { self.visit_i64_rem_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_rem_u ($($ann:tt)*)) => {
        fn visit_i64_rem_u(&mut self) -> Self::Output { self.visit_i64_rem_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_and ($($ann:tt)*)) => {
        fn visit_i64_and(&mut self) -> Self::Output { self.visit_i64_and_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_or ($($ann:tt)*)) => {
        fn visit_i64_or(&mut self) -> Self::Output { self.visit_i64_or_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_xor ($($ann:tt)*)) => {
        fn visit_i64_xor(&mut self) -> Self::Output { self.visit_i64_xor_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_shl ($($ann:tt)*)) => {
        fn visit_i64_shl(&mut self) -> Self::Output { self.visit_i64_shl_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_shr_u ($($ann:tt)*)) => {
        fn visit_i64_shr_u(&mut self) -> Self::Output { self.visit_i64_shr_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_shr_s ($($ann:tt)*)) => {
        fn visit_i64_shr_s(&mut self) -> Self::Output { self.visit_i64_shr_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_eqz ($($ann:tt)*)) => {
        fn visit_i32_eqz(&mut self) -> Self::Output { self.visit_i32_eqz_impl() }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i32_load ($($ann:tt)*)) => {
        fn visit_i32_load(&mut self, memarg: $argty) -> Self::Output {
            self.visit_i32_load_impl(memarg)
        }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i32_store ($($ann:tt)*)) => {
        fn visit_i32_store(&mut self, memarg: $argty) -> Self::Output {
            self.visit_i32_store_impl(memarg)
        }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i64_load ($($ann:tt)*)) => {
        fn visit_i64_load(&mut self, memarg: $argty) -> Self::Output {
            self.visit_i64_load_impl(memarg)
        }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i64_store ($($ann:tt)*)) => {
        fn visit_i64_store(&mut self, memarg: $argty) -> Self::Output {
            self.visit_i64_store_impl(memarg)
        }
    };
    (@$proposal:ident $op:ident { function_index: $argty:ty } => visit_call ($($ann:tt)*)) => {
        fn visit_call(&mut self, function_index: $argty) -> Self::Output {
            self.visit_call_impl(function_index)
        }
    };
    (@$proposal:ident $op:ident { $($arg:ident: $argty:ty),* } => $visit:ident ($($ann:tt)*)) => {
        fn $visit(&mut self, $($arg: $argty),*) -> Self::Output {
            $(let _ = $arg;)*
            Err(CompileError::UnsupportedFeature(stringify!($op)))
        }
    };
    (@$proposal:ident $op:ident => $visit:ident ($($ann:tt)*)) => {
        fn $visit(&mut self) -> Self::Output {
            Err(CompileError::UnsupportedFeature(stringify!($op)))
        }
    };
}

macro_rules! function_compiler_visit_operator {
    ($( @$proposal:ident $op:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident ($($ann:tt)*))*) => {
        $(
            function_compiler_visit_one!(@$proposal $op $({ $($arg: $argty),* })? => $visit ($($ann)*));
        )*
    };
}

impl<'a, 'ctx, B: ArchBackend> VisitOperator<'a> for FunctionCompiler<'ctx, B> {
    type Output = Result<(), CompileError>;

    wasmparser::for_each_visit_operator!(function_compiler_visit_operator);
}

fn convert_val_type(value: wasmparser::ValType) -> Result<ValType, CompileError> {
    match value {
        wasmparser::ValType::I32 => Ok(ValType::I32),
        wasmparser::ValType::I64 => Ok(ValType::I64),
        wasmparser::ValType::F32 => Ok(ValType::F32),
        wasmparser::ValType::F64 => Ok(ValType::F64),
        _ => Err(CompileError::UnsupportedFeature("value type")),
    }
}

unsafe extern "C" fn empty_compiled_fn(_ctx: *mut VmContext, _frame: *mut RawValue) -> RawValue {
    0
}

unsafe extern "C" fn host_i32_load_wrapper(ctx: *mut VmContext, frame: *mut RawValue) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        let value = helper_i32_load(ctx, addr);
        *frame.add(HELPER_RET_SLOT as usize) = value;
    }
    0
}

unsafe extern "C" fn host_i32_store_wrapper(ctx: *mut VmContext, frame: *mut RawValue) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        let value = *frame.add(HELPER_ARG1_SLOT as usize) as u32;
        helper_i32_store(ctx, addr, value);
    }
    0
}

unsafe extern "C" fn host_i64_load_wrapper(ctx: *mut VmContext, frame: *mut RawValue) -> RawValue {
    unsafe {
        let ctx_ref = &mut *ctx;
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u64;
        if !ctx_ref.check_memory(addr, 8) {
            ctx_ref.set_trap(TrapCode::MemoryOutOfBounds);
            *frame.add(HELPER_RET_SLOT as usize) = 0;
            return 0;
        }

        let ptr = ctx_ref.memory_base.add(addr as usize) as *const u64;
        *frame.add(HELPER_RET_SLOT as usize) = core::ptr::read_unaligned(ptr);
    }
    0
}

unsafe extern "C" fn host_i64_store_wrapper(ctx: *mut VmContext, frame: *mut RawValue) -> RawValue {
    unsafe {
        let ctx_ref = &mut *ctx;
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u64;
        if !ctx_ref.check_memory(addr, 8) {
            ctx_ref.set_trap(TrapCode::MemoryOutOfBounds);
            return 0;
        }

        let value = *frame.add(HELPER_ARG1_SLOT as usize);
        let ptr = ctx_ref.memory_base.add(addr as usize) as *mut u64;
        core::ptr::write_unaligned(ptr, value);
    }
    0
}

unsafe extern "C" fn host_call_wrapper(ctx: *mut VmContext, frame: *mut RawValue) -> RawValue {
    unsafe {
        let func_index = *frame.add(HELPER_CALL_FUNC_SLOT as usize) as u32;
        let args_ptr = frame.add(HELPER_CALL_ARGS_BASE as usize) as *const RawValue;
        let value = helper_call(ctx, func_index, args_ptr);
        *frame.add(HELPER_RET_SLOT as usize) = value;
    }
    0
}

unsafe extern "C" fn host_import_call_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let import_index = *frame.add(HELPER_CALL_FUNC_SLOT as usize) as u32;
        let args_ptr = frame.add(HELPER_CALL_ARGS_BASE as usize) as *const RawValue;
        let value = crate::wasi::dispatch_imported(ctx, import_index, args_ptr);
        *frame.add(HELPER_RET_SLOT as usize) = value;
    }
    0
}

unsafe extern "C" fn host_unreachable_wrapper(
    ctx: *mut VmContext,
    _frame: *mut RawValue,
) -> RawValue {
    unsafe { helper_trap(ctx, TrapCode::Unreachable) }
}

fn resolve_fixups(code: &mut CodeBuffer, labels: &[Label]) -> Result<(), CompileError> {
    for index in 0..code.fixups.len() {
        let (at_offset, target_label, kind) = {
            let fixup = &code.fixups[index];
            let kind = match fixup.kind {
                BranchKind::Unconditional => PatchBranchKind::Unconditional,
                BranchKind::ConditionalZero => PatchBranchKind::ConditionalZero,
                BranchKind::ConditionalNotZero => PatchBranchKind::ConditionalNotZero,
            };
            (fixup.at_offset, fixup.target, kind)
        };

        let target = labels
            .get(target_label as usize)
            .and_then(|label| label.bound_offset)
            .ok_or(CompileError::CodeGen("unbound label"))?;
        let delta = i64::from(target) - i64::from(at_offset);

        #[cfg(target_arch = "aarch64")]
        patch_aarch64_fixup(code, at_offset, kind, delta)?;

        #[cfg(target_arch = "riscv64")]
        patch_riscv64_fixup(code, at_offset, kind, delta)?;
    }

    Ok(())
}

#[cfg(target_arch = "aarch64")]
fn patch_aarch64_fixup(
    code: &mut CodeBuffer,
    at_offset: u32,
    kind: PatchBranchKind,
    delta: i64,
) -> Result<(), CompileError> {
    if delta & 0b11 != 0 {
        return Err(CompileError::CodeGen("unaligned branch target"));
    }

    let original = read_u32_at(code, at_offset)?;
    let patched = match kind {
        PatchBranchKind::Unconditional => {
            let imm26 =
                i32::try_from(delta >> 2).map_err(|_| CompileError::CodeGen("branch range"))?;
            if !(-(1 << 25)..(1 << 25)).contains(&imm26) {
                return Err(CompileError::CodeGen("branch range"));
            }
            (original & !0x03ff_ffff) | ((imm26 as u32) & 0x03ff_ffff)
        }
        PatchBranchKind::ConditionalZero | PatchBranchKind::ConditionalNotZero => {
            let imm19 =
                i32::try_from(delta >> 2).map_err(|_| CompileError::CodeGen("branch range"))?;
            if !(-(1 << 18)..(1 << 18)).contains(&imm19) {
                return Err(CompileError::CodeGen("branch range"));
            }
            (original & !0x00ff_ffe0) | (((imm19 as u32) & 0x7ffff) << 5)
        }
    };

    code.patch_u32_at(at_offset, patched);
    Ok(())
}

#[cfg(target_arch = "riscv64")]
fn patch_riscv64_fixup(
    code: &mut CodeBuffer,
    at_offset: u32,
    kind: PatchBranchKind,
    delta: i64,
) -> Result<(), CompileError> {
    if delta & 0b1 != 0 {
        return Err(CompileError::CodeGen("unaligned branch target"));
    }

    let original = read_u32_at(code, at_offset)?;
    let patched = match kind {
        PatchBranchKind::Unconditional => {
            let imm = i32::try_from(delta).map_err(|_| CompileError::CodeGen("branch range"))?;
            if !(-(1 << 20)..(1 << 20)).contains(&(imm >> 1)) {
                return Err(CompileError::CodeGen("branch range"));
            }

            let imm20 = ((imm >> 20) & 0x1) as u32;
            let imm10_1 = ((imm >> 1) & 0x3ff) as u32;
            let imm11 = ((imm >> 11) & 0x1) as u32;
            let imm19_12 = ((imm >> 12) & 0xff) as u32;

            (original & 0x000f_f07f)
                | (imm20 << 31)
                | (imm19_12 << 12)
                | (imm11 << 20)
                | (imm10_1 << 21)
        }
        PatchBranchKind::ConditionalZero | PatchBranchKind::ConditionalNotZero => {
            let imm = i32::try_from(delta).map_err(|_| CompileError::CodeGen("branch range"))?;
            if !(-(1 << 12)..(1 << 12)).contains(&(imm >> 1)) {
                return Err(CompileError::CodeGen("branch range"));
            }

            let imm12 = ((imm >> 12) & 0x1) as u32;
            let imm10_5 = ((imm >> 5) & 0x3f) as u32;
            let imm4_1 = ((imm >> 1) & 0xf) as u32;
            let imm11 = ((imm >> 11) & 0x1) as u32;

            (original & 0x01ff_07f) | (imm12 << 31) | (imm10_5 << 25) | (imm4_1 << 8) | (imm11 << 7)
        }
    };

    code.patch_u32_at(at_offset, patched);
    Ok(())
}

fn read_u32_at(code: &CodeBuffer, offset: u32) -> Result<u32, CompileError> {
    let offset = offset as usize;
    let bytes = code
        .bytes
        .get(offset..offset + 4)
        .ok_or(CompileError::CodeGen("fixup read"))?;
    let mut word = [0u8; 4];
    word.copy_from_slice(bytes);
    Ok(u32::from_le_bytes(word))
}
#[derive(Clone, Copy)]
enum PatchBranchKind {
    Unconditional,
    ConditionalZero,
    ConditionalNotZero,
}
