use alloc::{string::String, vec::Vec};

use wasmparser::{
    BlockType, ConstExpr, ElementItems, ElementKind, ExternalKind, FunctionBody, MemArg, Parser,
    Payload, TypeRef, VisitOperator,
};

use crate::arch::ArchBackend;
use crate::code::{CodeBuffer, ExecutableSlab};
use crate::control::{BranchKind, ControlFrame, ControlKind, ControlStack, Label, LabelId};
use crate::frame::FrameLayout;
use crate::helpers::{
    helper_call, helper_call_indirect, helper_global_get, helper_global_set, helper_i32_load,
    helper_i32_load8_s, helper_i32_load8_u, helper_i32_load16_s, helper_i32_load16_u,
    helper_i32_store, helper_i32_store8, helper_i32_store16, helper_i64_load8_s,
    helper_i64_load8_u, helper_i64_load16_s, helper_i64_load16_u, helper_i64_load32_s,
    helper_i64_load32_u, helper_i64_store8, helper_i64_store16, helper_i64_store32,
    helper_memory_copy, helper_memory_fill, helper_memory_grow, helper_memory_size, helper_trap,
};
use crate::module::{FuncType, MemoryInfo, ValType};
use crate::runtime::VmContext;
use crate::{
    CompileError, CompiledFn, CompiledModule, ExportEntry, FunctionEntry, RawValue, TrapCode,
};

pub const HELPER_SLOT_COUNT: u16 = 33;
const HELPER_RET_SLOT: u16 = 0;
const HELPER_ARG0_SLOT: u16 = 0;
const HELPER_ARG1_SLOT: u16 = 1;
const HELPER_CALL_FUNC_SLOT: u16 = 0;
const HELPER_CALL_ARGS_BASE: u16 = 1;
const MAX_CALL_ARGS: usize = 32;

#[derive(Clone, Copy)]
enum GlobalInitValue {
    I32(u32),
    I64(u64),
    Global(u32),
}

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
    global_types: Vec<wasmparser::GlobalType>,
    global_init_values: Vec<GlobalInitValue>,
    imported_global_count: usize,
    table: Vec<u32>,
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
            global_types: Vec::new(),
            global_init_values: Vec::new(),
            imported_global_count: 0,
            table: Vec::new(),
        }
    }

    fn resolve_data_offset(
        &self,
        offset_expr: wasmparser::ConstExpr<'_>,
    ) -> Result<u32, CompileError> {
        let mut reader = offset_expr.get_binary_reader();
        let offset_op = reader
            .read_operator()
            .map_err(|_| CompileError::InvalidWasm("data offset"))?;
        match offset_op {
            wasmparser::Operator::I32Const { value } => Ok(value as u32),
            wasmparser::Operator::I64Const { value } => Ok(value as u32),
            wasmparser::Operator::GlobalGet { global_index } => {
                let idx = global_index as usize;
                if idx < self.imported_global_count {
                    return Err(CompileError::UnsupportedFeature(
                        "imported global in data offset",
                    ));
                }
                let local_idx = idx - self.imported_global_count;
                let init = self
                    .global_init_values
                    .get(local_idx)
                    .ok_or(CompileError::InvalidWasm("data offset global index"))?;
                match init {
                    GlobalInitValue::I32(v) => Ok(*v),
                    GlobalInitValue::I64(v) => Ok(*v as u32),
                    GlobalInitValue::Global(_) => Err(CompileError::UnsupportedFeature(
                        "nested global in data offset",
                    )),
                }
            }
            _ => Err(CompileError::UnsupportedFeature("data offset expression")),
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
        self.global_types.clear();
        self.global_init_values.clear();
        self.imported_global_count = 0;
        self.table.clear();

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
                            TypeRef::Global(global_type) => {
                                self.imported_global_count += 1;
                                self.global_types.push(global_type);
                                self.global_init_values.push(GlobalInitValue::I64(0));
                                None
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
                Payload::TableSection(reader) => {
                    for table in reader {
                        let table =
                            table.map_err(|_| CompileError::InvalidWasm("table section"))?;
                        let initial = usize::try_from(table.ty.initial)
                            .map_err(|_| CompileError::InvalidWasm("table initial"))?;
                        if self.table.len() < initial {
                            self.table.resize(initial, u32::MAX);
                        }
                    }
                }
                Payload::GlobalSection(reader) => {
                    for global in reader {
                        let global =
                            global.map_err(|_| CompileError::InvalidWasm("global section"))?;
                        self.global_types.push(global.ty);
                        self.global_init_values
                            .push(parse_global_init_expr(global.init_expr)?);
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

                    let compiler = FunctionCompiler::new_with_debug(
                        &mut self.backend,
                        func_type,
                        &self.func_types,
                        &self.all_func_type_indices,
                        self.imported_func_count,
                        local_index,
                    );
                    let global_func_index = self.imported_func_count as u32 + local_index as u32;
                    let compiled = compiler.compile(&func_body).map_err(|e| match e {
                        CompileError::FuncCompileError { .. } => e,
                        other => CompileError::FuncCompileError {
                            func_index: global_func_index,
                            inner: match other {
                                CompileError::InvalidWasm(s) => s,
                                CompileError::UnsupportedFeature(s) => s,
                                CompileError::CodeGen(s) => s,
                                CompileError::FuncCompileError { inner, .. } => inner,
                            },
                        },
                    })?;
                    self.compiled_functions.push(compiled);
                }
                Payload::DataSection(reader) => {
                    for data_entry in reader {
                        let data_entry =
                            data_entry.map_err(|_| CompileError::InvalidWasm("data section"))?;
                        match data_entry.kind {
                            wasmparser::DataKind::Active { offset_expr, .. } => {
                                let offset = self.resolve_data_offset(offset_expr)?;
                                self.data_segments.push(crate::DataSegment {
                                    offset,
                                    data: data_entry.data.to_vec(),
                                });
                            }
                            wasmparser::DataKind::Passive => {
                                // Passive segments are loaded via memory.init at runtime.
                                // Skip them here; they require bulk-memory opcode support.
                            }
                        }
                    }
                }
                Payload::ElementSection(reader) => {
                    for element in reader {
                        let element =
                            element.map_err(|_| CompileError::InvalidWasm("element section"))?;
                        let base = match element.kind {
                            ElementKind::Active {
                                table_index,
                                offset_expr,
                            } => {
                                if table_index.unwrap_or(0) != 0 {
                                    return Err(CompileError::UnsupportedFeature("multi-table"));
                                }
                                usize::try_from(parse_const_expr_u32(offset_expr)?)
                                    .map_err(|_| CompileError::InvalidWasm("element offset"))?
                            }
                            ElementKind::Passive | ElementKind::Declared => continue,
                        };

                        let ElementItems::Functions(functions) = element.items else {
                            return Err(CompileError::UnsupportedFeature("non-function elements"));
                        };
                        let count = usize::try_from(functions.count())
                            .map_err(|_| CompileError::InvalidWasm("element count"))?;
                        let required = base
                            .checked_add(count)
                            .ok_or(CompileError::InvalidWasm("element size"))?;
                        if self.table.len() < required {
                            self.table.resize(required, u32::MAX);
                        }
                        for (idx, function_index) in functions.into_iter().enumerate() {
                            self.table[base + idx] = function_index
                                .map_err(|_| CompileError::InvalidWasm("element function index"))?;
                        }
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

        let globals = build_globals(
            &self.global_types,
            &self.global_init_values,
            self.imported_global_count,
        )?;

        for global in globals.iter().take(self.imported_global_count) {
            if global.mutable {
                return Err(CompileError::UnsupportedFeature("mutable imported globals"));
            }
        }

        let functions = functions.into_boxed_slice();
        let exports = exports.into_boxed_slice();
        let imported_funcs = imports.into_boxed_slice();
        let data_segments = core::mem::take(&mut self.data_segments).into_boxed_slice();
        let mut globals = globals.into_boxed_slice();
        let table = core::mem::take(&mut self.table).into_boxed_slice();

        crate::runtime::register_module_defaults(
            functions.as_ptr(),
            functions.len(),
            globals.as_mut_ptr(),
            globals.len(),
            self.imported_global_count,
            table.as_ptr(),
            table.len(),
        );

        let min_memory_pages = self.memories.first().map(|m| m.initial_pages).unwrap_or(1);

        Ok(CompiledModule {
            code: ExecutableSlab {
                ptr: slab_ptr,
                len: slab_len,
            },
            functions,
            exports,
            imported_funcs,
            data_segments,
            globals,
            imported_global_count: self.imported_global_count as u32,
            table,
            min_memory_pages,
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
    func_index: usize,
    current_path_reachable: bool,
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
            func_index: 0,
            current_path_reachable: true,
        }
    }

    pub fn new_with_debug(
        backend: &'ctx mut B,
        func_type: &'ctx FuncType,
        func_types: &'ctx [FuncType],
        all_func_type_indices: &'ctx [u32],
        imported_func_count: u32,
        func_index: usize,
    ) -> Self {
        let mut s = Self::new(
            backend,
            func_type,
            func_types,
            all_func_type_indices,
            imported_func_count,
        );
        s.func_index = func_index;
        s
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
        self.current_path_reachable = true;

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
            if let Some(result_slot) = self.value_stack.peek() {
                self.backend
                    .emit_load_slot(&mut self.code, self.backend.tmp0(), result_slot);
                self.backend
                    .emit_retval(&mut self.code, self.backend.tmp0());
            } else if self.current_path_reachable {
                return Err(CompileError::InvalidWasm(
                    "missing implicit function result",
                ));
            }
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
            has_incoming_end: frame.has_incoming_end,
        });
        self.control_frames.push(frame);
    }

    fn branch_frame_mut(&mut self, depth: u32) -> Result<&mut ControlFrame, CompileError> {
        let idx = self
            .control_frames
            .len()
            .checked_sub(depth as usize + 1)
            .ok_or(CompileError::InvalidWasm("branch depth"))?;
        self.control_frames
            .get_mut(idx)
            .ok_or(CompileError::InvalidWasm("branch frame"))
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
                .ok_or(CompileError::InvalidWasm("missing explicit return value"))?;
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

    fn emit_binary_host_cmp(&mut self, host_fn: usize) -> Result<(), CompileError> {
        let rhs_slot = self.value_stack.pop();
        let lhs_slot = self.value_stack.pop();
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let helper_arg1 = self.helper_slot(HELPER_ARG1_SLOT);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), lhs_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), rhs_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg1, self.backend.tmp0());
        self.backend.emit_call_host(&mut self.code, host_fn);
        let result = self.value_stack.push();
        self.emit_call_wrapper_result(result);
        Ok(())
    }

    fn emit_binary_host_fpu(&mut self, host_fn: usize) -> Result<(), CompileError> {
        let rhs_slot = self.value_stack.pop();
        let lhs_slot = self.value_stack.pop();
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let helper_arg1 = self.helper_slot(HELPER_ARG1_SLOT);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), lhs_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), rhs_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg1, self.backend.tmp0());
        self.backend.emit_call_host(&mut self.code, host_fn);
        let result = self.value_stack.push();
        self.emit_call_wrapper_result(result);
        Ok(())
    }

    fn emit_unary_host_op(&mut self, host_fn: usize) -> Result<(), CompileError> {
        let src_slot = self.value_stack.pop();
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), src_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend.emit_call_host(&mut self.code, host_fn);
        let result = self.value_stack.push();
        self.emit_call_wrapper_result(result);
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

    fn emit_helper_call1(&mut self, arg0_slot: u16, host: usize) -> u16 {
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), arg0_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend.emit_call_host(&mut self.code, host);
        let result = self.value_stack.push();
        self.emit_call_wrapper_result(result);
        result
    }

    fn emit_memory_access_wrapper(&mut self, host: usize, result: bool) {
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let tmp0 = self.backend.tmp0();
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, tmp0);
        self.backend.emit_call_host(&mut self.code, host);
        if result {
            let result_slot = self.value_stack.push();
            self.emit_call_wrapper_result(result_slot);
        }
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
            has_incoming_end: false,
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
            has_incoming_end: false,
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
            has_incoming_end: false,
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
        self.current_path_reachable = true;
        self.push_control_frame(frame);
        Ok(())
    }

    fn visit_end_impl(&mut self) -> Result<(), CompileError> {
        if let Some(frame) = self.pop_control_frame() {
            if self.current_path_reachable {
                self.value_stack
                    .truncate_keep(frame.entry_stack_height, frame.result_arity);
            } else if frame.has_incoming_end {
                self.value_stack.truncate_keep(frame.entry_stack_height, 0);
                for _ in 0..frame.result_arity {
                    self.value_stack.push();
                }
            } else {
                self.value_stack.truncate_keep(frame.entry_stack_height, 0);
            }
            if matches!(frame.kind, ControlKind::If) {
                if let Some(else_label) = frame.else_label {
                    self.bind_label(else_label);
                }
            }
            self.bind_label(frame.end_label);
            self.current_path_reachable = self.current_path_reachable || frame.has_incoming_end;
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
        if !is_loop {
            self.branch_frame_mut(relative_depth)?.has_incoming_end = true;
        }
        self.backend.emit_jump(&mut self.code, branch_target);
        let keep = if is_loop { 0 } else { result_arity };
        self.value_stack.truncate_keep(entry_stack_height, keep);
        self.current_path_reachable = false;
        Ok(())
    }

    fn visit_br_if_impl(&mut self, relative_depth: u32) -> Result<(), CompileError> {
        let cond_slot = self.value_stack.pop();
        let frame = self.branch_frame(relative_depth)?;
        let branch_target = frame.branch_target;
        if !matches!(frame.kind, ControlKind::Loop) {
            self.branch_frame_mut(relative_depth)?.has_incoming_end = true;
        }
        let tmp0 = self.backend.tmp0();
        self.backend.emit_load_slot(&mut self.code, tmp0, cond_slot);
        self.backend
            .emit_branch_not_zero(&mut self.code, tmp0, branch_target);
        Ok(())
    }

    fn visit_br_table_impl(&mut self, table: wasmparser::BrTable<'_>) -> Result<(), CompileError> {
        let index_slot = self.value_stack.pop();
        let tmp0 = self.backend.tmp0();
        let tmp1 = self.backend.tmp1();
        self.backend
            .emit_load_slot(&mut self.code, tmp0, index_slot);

        let targets: alloc::vec::Vec<u32> = table
            .targets()
            .collect::<Result<_, _>>()
            .map_err(|_| CompileError::InvalidWasm("br_table targets"))?;

        for target_depth in &targets {
            let label = self.branch_frame(*target_depth)?.branch_target;
            self.backend.emit_branch_zero(&mut self.code, tmp0, label);
            self.backend.emit_li(&mut self.code, tmp1, 1);
            self.backend.emit_sub(&mut self.code, tmp0, tmp0, tmp1);
        }

        let (default_target, default_entry, default_arity, default_is_loop) = {
            let frame = self.branch_frame(table.default())?;
            (
                frame.branch_target,
                frame.entry_stack_height,
                frame.result_arity,
                matches!(frame.kind, ControlKind::Loop),
            )
        };
        for target_depth in &targets {
            if !matches!(self.branch_frame(*target_depth)?.kind, ControlKind::Loop) {
                self.branch_frame_mut(*target_depth)?.has_incoming_end = true;
            }
        }
        if !default_is_loop {
            self.branch_frame_mut(table.default())?.has_incoming_end = true;
        }
        self.backend.emit_jump(&mut self.code, default_target);

        let keep = if default_is_loop { 0 } else { default_arity };
        self.value_stack.truncate_keep(default_entry, keep);
        self.current_path_reachable = false;

        Ok(())
    }

    fn visit_return_impl(&mut self) -> Result<(), CompileError> {
        self.emit_return()?;
        self.value_stack.truncate_keep(0, 0);
        for _ in 0..self.result_count {
            self.value_stack.push();
        }
        self.current_path_reachable = false;
        Ok(())
    }

    fn visit_unreachable_impl(&mut self) -> Result<(), CompileError> {
        self.backend.emit_call_host(
            &mut self.code,
            host_unreachable_wrapper as *const () as usize,
        );
        self.backend
            .emit_jump(&mut self.code, self.function_epilogue_label);
        self.value_stack.truncate_keep(0, 0);
        for _ in 0..self.result_count {
            self.value_stack.push();
        }
        self.current_path_reachable = false;
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

    fn visit_f64_const_impl(&mut self, value: wasmparser::Ieee64) -> Result<(), CompileError> {
        let slot = self.value_stack.push();
        self.backend
            .emit_li(&mut self.code, self.backend.tmp0(), value.bits() as i64);
        self.backend
            .emit_store_slot(&mut self.code, slot, self.backend.tmp0());
        Ok(())
    }

    fn visit_f32_const_impl(&mut self, value: wasmparser::Ieee32) -> Result<(), CompileError> {
        let slot = self.value_stack.push();
        self.backend
            .emit_li(&mut self.code, self.backend.tmp0(), value.bits() as i64);
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
            .emit_li(&mut self.code, self.backend.tmp2(), 32);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp2(),
        );
        self.backend.emit_shr_u(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp2(),
        );
        self.backend
            .emit_eqz(&mut self.code, self.backend.tmp0(), self.backend.tmp0());
        let result = self.value_stack.push();
        self.backend
            .emit_store_slot(&mut self.code, result, self.backend.tmp0());
        Ok(())
    }

    fn visit_cmp_impl(
        &mut self,
        op: fn(&mut B, &mut CodeBuffer, B::Reg, B::Reg, B::Reg),
        swap: bool,
        invert: bool,
    ) -> Result<(), CompileError> {
        let rhs = self.value_stack.pop();
        let lhs = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), lhs);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp1(), rhs);
        let (a, b) = if swap {
            (self.backend.tmp1(), self.backend.tmp0())
        } else {
            (self.backend.tmp0(), self.backend.tmp1())
        };
        op(self.backend, &mut self.code, self.backend.tmp0(), a, b);
        if invert {
            self.backend
                .emit_eqz(&mut self.code, self.backend.tmp0(), self.backend.tmp0());
        }
        let result = self.value_stack.push();
        self.backend
            .emit_store_slot(&mut self.code, result, self.backend.tmp0());
        Ok(())
    }

    fn visit_i32_eq_impl(&mut self) -> Result<(), CompileError> {
        let rhs = self.value_stack.pop();
        let lhs = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), lhs);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp1(), rhs);
        self.backend
            .emit_li(&mut self.code, self.backend.tmp2(), 32);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp2(),
        );
        self.backend.emit_shr_u(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp2(),
        );
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp1(),
            self.backend.tmp1(),
            self.backend.tmp2(),
        );
        self.backend.emit_shr_u(
            &mut self.code,
            self.backend.tmp1(),
            self.backend.tmp1(),
            self.backend.tmp2(),
        );
        self.backend.emit_xor(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        self.backend
            .emit_eqz(&mut self.code, self.backend.tmp0(), self.backend.tmp0());
        let result = self.value_stack.push();
        self.backend
            .emit_store_slot(&mut self.code, result, self.backend.tmp0());
        Ok(())
    }

    fn visit_i32_ne_impl(&mut self) -> Result<(), CompileError> {
        let rhs = self.value_stack.pop();
        let lhs = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), lhs);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp1(), rhs);
        self.backend
            .emit_li(&mut self.code, self.backend.tmp2(), 32);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp2(),
        );
        self.backend.emit_shr_u(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp2(),
        );
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp1(),
            self.backend.tmp1(),
            self.backend.tmp2(),
        );
        self.backend.emit_shr_u(
            &mut self.code,
            self.backend.tmp1(),
            self.backend.tmp1(),
            self.backend.tmp2(),
        );
        self.backend.emit_xor(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        self.backend
            .emit_snez(&mut self.code, self.backend.tmp0(), self.backend.tmp0());
        let result = self.value_stack.push();
        self.backend
            .emit_store_slot(&mut self.code, result, self.backend.tmp0());
        Ok(())
    }

    fn visit_i32_lt_s_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_slt, false, false)
    }
    fn visit_i32_lt_u_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_sltu, false, false)
    }
    fn visit_i32_gt_s_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_slt, true, false)
    }
    fn visit_i32_gt_u_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_sltu, true, false)
    }
    fn visit_i32_le_s_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_slt, true, true)
    }
    fn visit_i32_le_u_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_sltu, true, true)
    }
    fn visit_i32_ge_s_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_slt, false, true)
    }
    fn visit_i32_ge_u_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_sltu, false, true)
    }

    fn visit_i64_eqz_impl(&mut self) -> Result<(), CompileError> {
        self.visit_i32_eqz_impl()
    }
    fn visit_i64_eq_impl(&mut self) -> Result<(), CompileError> {
        self.visit_i32_eq_impl()
    }
    fn visit_i64_ne_impl(&mut self) -> Result<(), CompileError> {
        self.visit_i32_ne_impl()
    }
    fn visit_f64_eq_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_cmp(host_f64_eq_wrapper as *const () as usize)
    }
    fn visit_f64_ne_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_cmp(host_f64_ne_wrapper as *const () as usize)
    }
    fn visit_f32_eq_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_cmp(host_f32_eq_wrapper as *const () as usize)
    }
    fn visit_f32_ne_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_cmp(host_f32_ne_wrapper as *const () as usize)
    }
    fn visit_f64_lt_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_cmp(host_f64_lt_wrapper as *const () as usize)
    }
    fn visit_f64_le_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_cmp(host_f64_le_wrapper as *const () as usize)
    }
    fn visit_f64_gt_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_cmp(host_f64_gt_wrapper as *const () as usize)
    }
    fn visit_f64_ge_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_cmp(host_f64_ge_wrapper as *const () as usize)
    }
    fn visit_f32_lt_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_cmp(host_f32_lt_wrapper as *const () as usize)
    }
    fn visit_f32_le_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_cmp(host_f32_le_wrapper as *const () as usize)
    }
    fn visit_f32_gt_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_cmp(host_f32_gt_wrapper as *const () as usize)
    }
    fn visit_f32_ge_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_cmp(host_f32_ge_wrapper as *const () as usize)
    }
    fn visit_f64_add_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f64_add_wrapper as *const () as usize)
    }
    fn visit_f64_sub_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f64_sub_wrapper as *const () as usize)
    }
    fn visit_f64_mul_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f64_mul_wrapper as *const () as usize)
    }
    fn visit_f64_div_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f64_div_wrapper as *const () as usize)
    }
    fn visit_f32_add_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f32_add_wrapper as *const () as usize)
    }
    fn visit_f32_sub_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f32_sub_wrapper as *const () as usize)
    }
    fn visit_f32_mul_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f32_mul_wrapper as *const () as usize)
    }
    fn visit_f32_div_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f32_div_wrapper as *const () as usize)
    }
    fn visit_f64_abs_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f64_abs_wrapper as *const () as usize)
    }
    fn visit_f64_neg_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f64_neg_wrapper as *const () as usize)
    }
    fn visit_f64_ceil_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f64_ceil_wrapper as *const () as usize)
    }
    fn visit_f64_floor_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f64_floor_wrapper as *const () as usize)
    }
    fn visit_f64_trunc_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f64_trunc_wrapper as *const () as usize)
    }
    fn visit_f64_nearest_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f64_nearest_wrapper as *const () as usize)
    }
    fn visit_f64_sqrt_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f64_sqrt_wrapper as *const () as usize)
    }
    fn visit_f32_abs_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f32_abs_wrapper as *const () as usize)
    }
    fn visit_f32_neg_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f32_neg_wrapper as *const () as usize)
    }
    fn visit_f32_ceil_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f32_ceil_wrapper as *const () as usize)
    }
    fn visit_f32_floor_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f32_floor_wrapper as *const () as usize)
    }
    fn visit_f32_trunc_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f32_trunc_wrapper as *const () as usize)
    }
    fn visit_f32_nearest_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f32_nearest_wrapper as *const () as usize)
    }
    fn visit_f32_sqrt_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f32_sqrt_wrapper as *const () as usize)
    }
    fn visit_f64_min_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f64_min_wrapper as *const () as usize)
    }
    fn visit_f64_max_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f64_max_wrapper as *const () as usize)
    }
    fn visit_f64_copysign_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f64_copysign_wrapper as *const () as usize)
    }
    fn visit_f32_min_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f32_min_wrapper as *const () as usize)
    }
    fn visit_f32_max_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f32_max_wrapper as *const () as usize)
    }
    fn visit_f32_copysign_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_host_fpu(host_f32_copysign_wrapper as *const () as usize)
    }
    fn visit_f32_load_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.visit_i32_load_impl(memarg)
    }
    fn visit_f64_load_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.visit_i64_load_impl(memarg)
    }
    fn visit_f32_store_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.visit_i32_store_impl(memarg)
    }
    fn visit_f64_store_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.visit_i64_store_impl(memarg)
    }
    fn visit_i32_trunc_f64_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i32_trunc_f64_s_wrapper as *const () as usize)
    }
    fn visit_i32_trunc_f64_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i32_trunc_f64_u_wrapper as *const () as usize)
    }
    fn visit_i32_trunc_f32_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i32_trunc_f32_s_wrapper as *const () as usize)
    }
    fn visit_i32_trunc_f32_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i32_trunc_f32_u_wrapper as *const () as usize)
    }
    fn visit_i64_trunc_f64_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i64_trunc_f64_s_wrapper as *const () as usize)
    }
    fn visit_i64_trunc_f64_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i64_trunc_f64_u_wrapper as *const () as usize)
    }
    fn visit_i64_trunc_f32_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i64_trunc_f32_s_wrapper as *const () as usize)
    }
    fn visit_i64_trunc_f32_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i64_trunc_f32_u_wrapper as *const () as usize)
    }
    fn visit_i32_trunc_sat_f64_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i32_trunc_f64_s_wrapper as *const () as usize)
    }
    fn visit_i32_trunc_sat_f64_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i32_trunc_f64_u_wrapper as *const () as usize)
    }
    fn visit_i32_trunc_sat_f32_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i32_trunc_f32_s_wrapper as *const () as usize)
    }
    fn visit_i32_trunc_sat_f32_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i32_trunc_f32_u_wrapper as *const () as usize)
    }
    fn visit_i64_trunc_sat_f64_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i64_trunc_f64_s_wrapper as *const () as usize)
    }
    fn visit_i64_trunc_sat_f64_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i64_trunc_f64_u_wrapper as *const () as usize)
    }
    fn visit_i64_trunc_sat_f32_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i64_trunc_f32_s_wrapper as *const () as usize)
    }
    fn visit_i64_trunc_sat_f32_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_i64_trunc_f32_u_wrapper as *const () as usize)
    }
    fn visit_f64_convert_i32_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f64_convert_i32_s_wrapper as *const () as usize)
    }
    fn visit_f64_convert_i32_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f64_convert_i32_u_wrapper as *const () as usize)
    }
    fn visit_f64_convert_i64_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f64_convert_i64_s_wrapper as *const () as usize)
    }
    fn visit_f64_convert_i64_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f64_convert_i64_u_wrapper as *const () as usize)
    }
    fn visit_f32_convert_i32_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f32_convert_i32_s_wrapper as *const () as usize)
    }
    fn visit_f32_convert_i32_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f32_convert_i32_u_wrapper as *const () as usize)
    }
    fn visit_f32_convert_i64_s_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f32_convert_i64_s_wrapper as *const () as usize)
    }
    fn visit_f32_convert_i64_u_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f32_convert_i64_u_wrapper as *const () as usize)
    }
    fn visit_f64_promote_f32_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f64_promote_f32_wrapper as *const () as usize)
    }
    fn visit_f32_demote_f64_impl(&mut self) -> Result<(), CompileError> {
        self.emit_unary_host_op(host_f32_demote_f64_wrapper as *const () as usize)
    }
    fn visit_i64_lt_u_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_sltu, false, false)
    }
    fn visit_i64_gt_u_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_sltu, true, false)
    }
    fn visit_i64_lt_s_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_slt, false, false)
    }
    fn visit_i64_gt_s_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_slt, true, false)
    }
    fn visit_i64_le_s_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_slt, true, true)
    }
    fn visit_i64_le_u_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_sltu, true, true)
    }
    fn visit_i64_ge_s_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_slt, false, true)
    }
    fn visit_i64_ge_u_impl(&mut self) -> Result<(), CompileError> {
        self.visit_cmp_impl(B::emit_sltu, false, true)
    }

    fn visit_i32_clz_impl(&mut self) -> Result<(), CompileError> {
        let src = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), src);
        self.backend
            .emit_clz(&mut self.code, self.backend.tmp0(), self.backend.tmp0());
        let result = self.value_stack.push();
        self.backend
            .emit_store_slot(&mut self.code, result, self.backend.tmp0());
        Ok(())
    }

    fn visit_i32_ctz_impl(&mut self) -> Result<(), CompileError> {
        let src = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), src);
        self.backend
            .emit_ctz(&mut self.code, self.backend.tmp0(), self.backend.tmp0());
        let result = self.value_stack.push();
        self.backend
            .emit_store_slot(&mut self.code, result, self.backend.tmp0());
        Ok(())
    }

    fn visit_i32_rotl_impl(&mut self) -> Result<(), CompileError> {
        self.emit_binary_op(B::emit_rotl)
    }

    fn visit_i32_rotr_impl(&mut self) -> Result<(), CompileError> {
        let lhs_temp = self.helper_slot(2);
        let shr_temp = self.helper_slot(3);
        let rhs = self.value_stack.pop();
        let lhs = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), lhs);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp1(), rhs);
        self.backend
            .emit_li(&mut self.code, self.backend.tmp2(), 32);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp2(),
        );
        self.backend.emit_shr_u(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp2(),
        );
        self.backend
            .emit_store_slot(&mut self.code, lhs_temp, self.backend.tmp0());
        self.backend
            .emit_li(&mut self.code, self.backend.tmp2(), 59);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp1(),
            self.backend.tmp1(),
            self.backend.tmp2(),
        );
        self.backend.emit_shr_u(
            &mut self.code,
            self.backend.tmp1(),
            self.backend.tmp1(),
            self.backend.tmp2(),
        );
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), lhs_temp);
        self.backend.emit_shr_u(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        self.backend
            .emit_store_slot(&mut self.code, shr_temp, self.backend.tmp0());
        self.backend
            .emit_li(&mut self.code, self.backend.tmp2(), 32);
        self.backend.emit_sub(
            &mut self.code,
            self.backend.tmp2(),
            self.backend.tmp2(),
            self.backend.tmp1(),
        );
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), lhs_temp);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp2(),
        );
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp1(), shr_temp);
        self.backend.emit_or(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        self.backend
            .emit_li(&mut self.code, self.backend.tmp1(), 32);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        self.backend.emit_shr_u(
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

    fn visit_i32_extend8_s_impl(&mut self) -> Result<(), CompileError> {
        let src = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), src);
        self.backend
            .emit_li(&mut self.code, self.backend.tmp1(), 56);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        self.backend.emit_shr_s(
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

    fn visit_i32_extend16_s_impl(&mut self) -> Result<(), CompileError> {
        let src = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), src);
        self.backend
            .emit_li(&mut self.code, self.backend.tmp1(), 48);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        self.backend.emit_shr_s(
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

    fn visit_i64_reinterpret_f64_impl(&mut self) -> Result<(), CompileError> {
        Ok(())
    }

    fn visit_f64_reinterpret_i64_impl(&mut self) -> Result<(), CompileError> {
        Ok(())
    }

    fn visit_f32_reinterpret_i32_impl(&mut self) -> Result<(), CompileError> {
        Ok(())
    }

    fn visit_i32_reinterpret_f32_impl(&mut self) -> Result<(), CompileError> {
        Ok(())
    }

    fn visit_i32_load_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        self.emit_memory_access_wrapper(host_i32_load_wrapper as *const () as usize, true);
        Ok(())
    }

    fn visit_i32_load8_u_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        self.emit_memory_access_wrapper(host_i32_load8_u_wrapper as *const () as usize, true);
        Ok(())
    }

    fn visit_i32_load8_s_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        self.emit_memory_access_wrapper(host_i32_load8_s_wrapper as *const () as usize, true);
        Ok(())
    }

    fn visit_i32_load16_u_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        self.emit_memory_access_wrapper(host_i32_load16_u_wrapper as *const () as usize, true);
        Ok(())
    }

    fn visit_i32_load16_s_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        self.emit_memory_access_wrapper(host_i32_load16_s_wrapper as *const () as usize, true);
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

    fn visit_i32_store8_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        let value_slot = self.value_stack.pop();
        let addr_slot = self.value_stack.pop();
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let helper_arg1 = self.helper_slot(HELPER_ARG1_SLOT);
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
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend
            .emit_store_slot(&mut self.code, helper_arg1, self.backend.tmp1());
        self.backend.emit_call_host(
            &mut self.code,
            host_i32_store8_wrapper as *const () as usize,
        );
        Ok(())
    }

    fn visit_i32_store16_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        let value_slot = self.value_stack.pop();
        let addr_slot = self.value_stack.pop();
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let helper_arg1 = self.helper_slot(HELPER_ARG1_SLOT);
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
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend
            .emit_store_slot(&mut self.code, helper_arg1, self.backend.tmp1());
        self.backend.emit_call_host(
            &mut self.code,
            host_i32_store16_wrapper as *const () as usize,
        );
        Ok(())
    }

    fn visit_i64_load_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        self.emit_memory_access_wrapper(host_i64_load_wrapper as *const () as usize, true);
        Ok(())
    }

    fn visit_i64_load32_u_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        self.emit_memory_access_wrapper(host_i64_load32_u_wrapper as *const () as usize, true);
        Ok(())
    }

    fn visit_i64_load8_u_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        self.emit_memory_access_wrapper(host_i64_load8_u_wrapper as *const () as usize, true);
        Ok(())
    }

    fn visit_i64_load8_s_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        self.emit_memory_access_wrapper(host_i64_load8_s_wrapper as *const () as usize, true);
        Ok(())
    }

    fn visit_i64_load16_u_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        self.emit_memory_access_wrapper(host_i64_load16_u_wrapper as *const () as usize, true);
        Ok(())
    }

    fn visit_i64_load16_s_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        self.emit_memory_access_wrapper(host_i64_load16_s_wrapper as *const () as usize, true);
        Ok(())
    }

    fn visit_i64_load32_s_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        self.emit_memory_addr(memarg)?;
        self.emit_memory_access_wrapper(host_i64_load32_s_wrapper as *const () as usize, true);
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

    fn visit_i64_store32_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
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
        self.backend
            .emit_li(&mut self.code, self.backend.tmp2(), 32);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp1(),
            self.backend.tmp1(),
            self.backend.tmp2(),
        );
        self.backend.emit_shr_u(
            &mut self.code,
            self.backend.tmp1(),
            self.backend.tmp1(),
            self.backend.tmp2(),
        );
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let helper_arg1 = self.helper_slot(HELPER_ARG1_SLOT);
        let tmp0 = self.backend.tmp0();
        let tmp1 = self.backend.tmp1();
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, tmp0);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg1, tmp1);
        self.backend.emit_call_host(
            &mut self.code,
            host_i64_store32_wrapper as *const () as usize,
        );
        Ok(())
    }

    fn visit_i64_store8_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        let value_slot = self.value_stack.pop();
        let addr_slot = self.value_stack.pop();
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let helper_arg1 = self.helper_slot(HELPER_ARG1_SLOT);
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
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend
            .emit_store_slot(&mut self.code, helper_arg1, self.backend.tmp1());
        self.backend.emit_call_host(
            &mut self.code,
            host_i64_store8_wrapper as *const () as usize,
        );
        Ok(())
    }

    fn visit_i64_store16_impl(&mut self, memarg: MemArg) -> Result<(), CompileError> {
        let value_slot = self.value_stack.pop();
        let addr_slot = self.value_stack.pop();
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let helper_arg1 = self.helper_slot(HELPER_ARG1_SLOT);
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
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend
            .emit_store_slot(&mut self.code, helper_arg1, self.backend.tmp1());
        self.backend.emit_call_host(
            &mut self.code,
            host_i64_store16_wrapper as *const () as usize,
        );
        Ok(())
    }

    fn visit_drop_impl(&mut self) -> Result<(), CompileError> {
        self.value_stack.pop();
        Ok(())
    }

    fn visit_i32_wrap_i64_impl(&mut self) -> Result<(), CompileError> {
        let src = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), src);
        // Zero-extend: shift left 32 then right 32 to clear upper bits
        self.backend
            .emit_li(&mut self.code, self.backend.tmp1(), 32);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        self.backend.emit_shr_u(
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

    fn visit_i64_extend_i32_u_impl(&mut self) -> Result<(), CompileError> {
        let src = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), src);
        self.backend
            .emit_li(&mut self.code, self.backend.tmp1(), 32);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        self.backend.emit_shr_u(
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

    fn visit_i64_extend_i32_s_impl(&mut self) -> Result<(), CompileError> {
        let src = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), src);
        self.backend
            .emit_li(&mut self.code, self.backend.tmp1(), 32);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        self.backend.emit_shr_s(
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

    fn visit_i32_popcnt_impl(&mut self) -> Result<(), CompileError> {
        let src = self.value_stack.pop();
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), src);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend.emit_call_host(
            &mut self.code,
            host_i32_popcnt_wrapper as *const () as usize,
        );
        let result = self.value_stack.push();
        self.emit_call_wrapper_result(result);
        Ok(())
    }

    fn visit_i64_popcnt_impl(&mut self) -> Result<(), CompileError> {
        let src = self.value_stack.pop();
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), src);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend.emit_call_host(
            &mut self.code,
            host_i64_popcnt_wrapper as *const () as usize,
        );
        let result = self.value_stack.push();
        self.emit_call_wrapper_result(result);
        Ok(())
    }

    fn visit_i64_clz_impl(&mut self) -> Result<(), CompileError> {
        self.visit_i32_clz_impl()
    }

    fn visit_i64_ctz_impl(&mut self) -> Result<(), CompileError> {
        self.visit_i32_ctz_impl()
    }

    fn visit_i64_rotl_impl(&mut self) -> Result<(), CompileError> {
        self.visit_i32_rotl_impl()
    }

    fn visit_i64_rotr_impl(&mut self) -> Result<(), CompileError> {
        let lhs_temp = self.helper_slot(2);
        let shr_temp = self.helper_slot(3);
        let rhs = self.value_stack.pop();
        let lhs = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), lhs);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp1(), rhs);
        self.backend
            .emit_store_slot(&mut self.code, lhs_temp, self.backend.tmp0());
        self.backend
            .emit_li(&mut self.code, self.backend.tmp2(), 58);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp1(),
            self.backend.tmp1(),
            self.backend.tmp2(),
        );
        self.backend.emit_shr_u(
            &mut self.code,
            self.backend.tmp1(),
            self.backend.tmp1(),
            self.backend.tmp2(),
        );
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), lhs_temp);
        self.backend.emit_shr_u(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
        );
        self.backend
            .emit_store_slot(&mut self.code, shr_temp, self.backend.tmp0());
        self.backend
            .emit_li(&mut self.code, self.backend.tmp2(), 64);
        self.backend.emit_sub(
            &mut self.code,
            self.backend.tmp2(),
            self.backend.tmp2(),
            self.backend.tmp1(),
        );
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), lhs_temp);
        self.backend.emit_shl(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp2(),
        );
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp1(), shr_temp);
        self.backend.emit_or(
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

        self.backend.emit_call_host(
            &mut self.code,
            host_check_trap_wrapper as *const () as usize,
        );
        let trap_check_slot = self.helper_slot(HELPER_ARG1_SLOT);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), trap_check_slot);
        self.backend.emit_branch_not_zero(
            &mut self.code,
            self.backend.tmp0(),
            self.function_epilogue_label,
        );

        if func_type.results.len() == 1 {
            let result = self.value_stack.push();
            self.emit_call_wrapper_result(result);
        }

        Ok(())
    }

    fn visit_global_get_impl(&mut self, global_index: u32) -> Result<(), CompileError> {
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        self.backend
            .emit_li(&mut self.code, self.backend.tmp0(), i64::from(global_index));
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend.emit_call_host(
            &mut self.code,
            host_global_get_wrapper as *const () as usize,
        );
        let result = self.value_stack.push();
        self.emit_call_wrapper_result(result);
        Ok(())
    }

    fn visit_global_set_impl(&mut self, global_index: u32) -> Result<(), CompileError> {
        let value_slot = self.value_stack.pop();
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let helper_arg1 = self.helper_slot(HELPER_ARG1_SLOT);
        self.backend
            .emit_li(&mut self.code, self.backend.tmp0(), i64::from(global_index));
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp1(), value_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend
            .emit_store_slot(&mut self.code, helper_arg1, self.backend.tmp1());
        self.backend.emit_call_host(
            &mut self.code,
            host_global_set_wrapper as *const () as usize,
        );
        Ok(())
    }

    fn visit_call_indirect_impl(
        &mut self,
        type_index: u32,
        table_index: u32,
    ) -> Result<(), CompileError> {
        if table_index != 0 {
            return Err(CompileError::UnsupportedFeature(
                "multi-table call_indirect",
            ));
        }
        let func_type = self
            .func_types
            .get(type_index as usize)
            .ok_or(CompileError::InvalidWasm("call_indirect type index"))?;
        if func_type.params.len() > MAX_CALL_ARGS {
            return Err(CompileError::UnsupportedFeature(
                "call_indirect argument count",
            ));
        }
        if func_type.results.len() > 1 {
            return Err(CompileError::UnsupportedFeature(
                "multi-value call_indirect",
            ));
        }

        let table_slot = self.value_stack.pop();
        let helper_func_slot = self.helper_slot(HELPER_CALL_FUNC_SLOT);
        for arg_index in (0..func_type.params.len()).rev() {
            let arg_slot = self.value_stack.pop();
            let helper_arg_slot = self.helper_slot(HELPER_CALL_ARGS_BASE + arg_index as u16);
            self.backend
                .emit_load_slot(&mut self.code, self.backend.tmp0(), arg_slot);
            self.backend
                .emit_store_slot(&mut self.code, helper_arg_slot, self.backend.tmp0());
        }
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), table_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_func_slot, self.backend.tmp0());
        self.backend.emit_call_host(
            &mut self.code,
            host_call_indirect_wrapper as *const () as usize,
        );

        self.backend.emit_call_host(
            &mut self.code,
            host_check_trap_wrapper as *const () as usize,
        );
        let trap_check_slot = self.helper_slot(HELPER_ARG1_SLOT);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), trap_check_slot);
        self.backend.emit_branch_not_zero(
            &mut self.code,
            self.backend.tmp0(),
            self.function_epilogue_label,
        );

        if func_type.results.len() == 1 {
            let result = self.value_stack.push();
            self.emit_call_wrapper_result(result);
        }
        Ok(())
    }

    fn visit_select_impl(&mut self) -> Result<(), CompileError> {
        let cond_slot = self.value_stack.pop();
        let val2_slot = self.value_stack.pop();
        let val1_slot = self.value_stack.pop();
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), val1_slot);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp1(), val2_slot);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp2(), cond_slot);
        self.backend.emit_select(
            &mut self.code,
            self.backend.tmp0(),
            self.backend.tmp0(),
            self.backend.tmp1(),
            self.backend.tmp2(),
        );
        let result = self.value_stack.push();
        self.backend
            .emit_store_slot(&mut self.code, result, self.backend.tmp0());
        Ok(())
    }

    fn visit_memory_copy_impl(&mut self, dst_mem: u32, src_mem: u32) -> Result<(), CompileError> {
        if dst_mem != 0 || src_mem != 0 {
            return Err(CompileError::UnsupportedFeature("multi-memory copy"));
        }
        let len_slot = self.value_stack.pop();
        let src_slot = self.value_stack.pop();
        let dst_slot = self.value_stack.pop();
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let helper_arg1 = self.helper_slot(HELPER_ARG1_SLOT);
        let helper_arg2 = self.helper_slot(2);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), dst_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), src_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg1, self.backend.tmp0());
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), len_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg2, self.backend.tmp0());
        self.backend.emit_call_host(
            &mut self.code,
            host_memory_copy_wrapper as *const () as usize,
        );
        Ok(())
    }

    fn visit_memory_fill_impl(&mut self, mem: u32) -> Result<(), CompileError> {
        if mem != 0 {
            return Err(CompileError::UnsupportedFeature("multi-memory fill"));
        }
        let len_slot = self.value_stack.pop();
        let value_slot = self.value_stack.pop();
        let dst_slot = self.value_stack.pop();
        let helper_arg0 = self.helper_slot(HELPER_ARG0_SLOT);
        let helper_arg1 = self.helper_slot(HELPER_ARG1_SLOT);
        let helper_arg2 = self.helper_slot(2);
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), dst_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg0, self.backend.tmp0());
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), value_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg1, self.backend.tmp0());
        self.backend
            .emit_load_slot(&mut self.code, self.backend.tmp0(), len_slot);
        self.backend
            .emit_store_slot(&mut self.code, helper_arg2, self.backend.tmp0());
        self.backend.emit_call_host(
            &mut self.code,
            host_memory_fill_wrapper as *const () as usize,
        );
        Ok(())
    }

    fn visit_nop_impl(&mut self) -> Result<(), CompileError> {
        Ok(())
    }

    fn visit_memory_grow_impl(&mut self, mem: u32) -> Result<(), CompileError> {
        if mem != 0 {
            return Err(CompileError::UnsupportedFeature("multi-memory grow"));
        }
        let delta_slot = self.value_stack.pop();
        let result =
            self.emit_helper_call1(delta_slot, host_memory_grow_wrapper as *const () as usize);
        let _ = result;
        Ok(())
    }

    fn visit_memory_size_impl(&mut self, mem: u32) -> Result<(), CompileError> {
        if mem != 0 {
            return Err(CompileError::UnsupportedFeature("multi-memory size"));
        }
        self.backend.emit_call_host(
            &mut self.code,
            host_memory_size_wrapper as *const () as usize,
        );
        let result = self.value_stack.push();
        self.emit_call_wrapper_result(result);
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
    (@$proposal:ident $op:ident { value: $argty:ty } => visit_f64_const ($($ann:tt)*)) => {
        fn visit_f64_const(&mut self, value: $argty) -> Self::Output {
            self.visit_f64_const_impl(value)
        }
    };
    (@$proposal:ident $op:ident { value: $argty:ty } => visit_f32_const ($($ann:tt)*)) => {
        fn visit_f32_const(&mut self, value: $argty) -> Self::Output {
            self.visit_f32_const_impl(value)
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
    (@$proposal:ident $op:ident => visit_i32_eq ($($ann:tt)*)) => {
        fn visit_i32_eq(&mut self) -> Self::Output { self.visit_i32_eq_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_ne ($($ann:tt)*)) => {
        fn visit_i32_ne(&mut self) -> Self::Output { self.visit_i32_ne_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_lt_s ($($ann:tt)*)) => {
        fn visit_i32_lt_s(&mut self) -> Self::Output { self.visit_i32_lt_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_lt_u ($($ann:tt)*)) => {
        fn visit_i32_lt_u(&mut self) -> Self::Output { self.visit_i32_lt_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_gt_s ($($ann:tt)*)) => {
        fn visit_i32_gt_s(&mut self) -> Self::Output { self.visit_i32_gt_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_gt_u ($($ann:tt)*)) => {
        fn visit_i32_gt_u(&mut self) -> Self::Output { self.visit_i32_gt_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_le_s ($($ann:tt)*)) => {
        fn visit_i32_le_s(&mut self) -> Self::Output { self.visit_i32_le_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_le_u ($($ann:tt)*)) => {
        fn visit_i32_le_u(&mut self) -> Self::Output { self.visit_i32_le_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_ge_s ($($ann:tt)*)) => {
        fn visit_i32_ge_s(&mut self) -> Self::Output { self.visit_i32_ge_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_ge_u ($($ann:tt)*)) => {
        fn visit_i32_ge_u(&mut self) -> Self::Output { self.visit_i32_ge_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_eqz ($($ann:tt)*)) => {
        fn visit_i64_eqz(&mut self) -> Self::Output { self.visit_i64_eqz_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_eq ($($ann:tt)*)) => {
        fn visit_i64_eq(&mut self) -> Self::Output { self.visit_i64_eq_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_ne ($($ann:tt)*)) => {
        fn visit_i64_ne(&mut self) -> Self::Output { self.visit_i64_ne_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_lt_u ($($ann:tt)*)) => {
        fn visit_i64_lt_u(&mut self) -> Self::Output { self.visit_i64_lt_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_gt_u ($($ann:tt)*)) => {
        fn visit_i64_gt_u(&mut self) -> Self::Output { self.visit_i64_gt_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_lt_s ($($ann:tt)*)) => {
        fn visit_i64_lt_s(&mut self) -> Self::Output { self.visit_i64_lt_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_gt_s ($($ann:tt)*)) => {
        fn visit_i64_gt_s(&mut self) -> Self::Output { self.visit_i64_gt_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_le_s ($($ann:tt)*)) => {
        fn visit_i64_le_s(&mut self) -> Self::Output { self.visit_i64_le_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_le_u ($($ann:tt)*)) => {
        fn visit_i64_le_u(&mut self) -> Self::Output { self.visit_i64_le_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_ge_s ($($ann:tt)*)) => {
        fn visit_i64_ge_s(&mut self) -> Self::Output { self.visit_i64_ge_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_ge_u ($($ann:tt)*)) => {
        fn visit_i64_ge_u(&mut self) -> Self::Output { self.visit_i64_ge_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_popcnt ($($ann:tt)*)) => {
        fn visit_i32_popcnt(&mut self) -> Self::Output { self.visit_i32_popcnt_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_extend_i32_s ($($ann:tt)*)) => {
        fn visit_i64_extend_i32_s(&mut self) -> Self::Output { self.visit_i64_extend_i32_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_clz ($($ann:tt)*)) => {
        fn visit_i32_clz(&mut self) -> Self::Output { self.visit_i32_clz_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_ctz ($($ann:tt)*)) => {
        fn visit_i32_ctz(&mut self) -> Self::Output { self.visit_i32_ctz_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_rotl ($($ann:tt)*)) => {
        fn visit_i32_rotl(&mut self) -> Self::Output { self.visit_i32_rotl_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_rotr ($($ann:tt)*)) => {
        fn visit_i32_rotr(&mut self) -> Self::Output { self.visit_i32_rotr_impl() }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i32_load ($($ann:tt)*)) => {
        fn visit_i32_load(&mut self, memarg: $argty) -> Self::Output {
            self.visit_i32_load_impl(memarg)
        }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i32_load8_u ($($ann:tt)*)) => {
        fn visit_i32_load8_u(&mut self, memarg: $argty) -> Self::Output { self.visit_i32_load8_u_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i32_load8_s ($($ann:tt)*)) => {
        fn visit_i32_load8_s(&mut self, memarg: $argty) -> Self::Output { self.visit_i32_load8_s_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i32_load16_u ($($ann:tt)*)) => {
        fn visit_i32_load16_u(&mut self, memarg: $argty) -> Self::Output { self.visit_i32_load16_u_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i32_load16_s ($($ann:tt)*)) => {
        fn visit_i32_load16_s(&mut self, memarg: $argty) -> Self::Output { self.visit_i32_load16_s_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i32_store ($($ann:tt)*)) => {
        fn visit_i32_store(&mut self, memarg: $argty) -> Self::Output {
            self.visit_i32_store_impl(memarg)
        }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i32_store8 ($($ann:tt)*)) => {
        fn visit_i32_store8(&mut self, memarg: $argty) -> Self::Output { self.visit_i32_store8_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i32_store16 ($($ann:tt)*)) => {
        fn visit_i32_store16(&mut self, memarg: $argty) -> Self::Output { self.visit_i32_store16_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i64_load ($($ann:tt)*)) => {
        fn visit_i64_load(&mut self, memarg: $argty) -> Self::Output {
            self.visit_i64_load_impl(memarg)
        }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i64_load8_u ($($ann:tt)*)) => {
        fn visit_i64_load8_u(&mut self, memarg: $argty) -> Self::Output { self.visit_i64_load8_u_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i64_load8_s ($($ann:tt)*)) => {
        fn visit_i64_load8_s(&mut self, memarg: $argty) -> Self::Output { self.visit_i64_load8_s_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i64_load16_u ($($ann:tt)*)) => {
        fn visit_i64_load16_u(&mut self, memarg: $argty) -> Self::Output { self.visit_i64_load16_u_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i64_load16_s ($($ann:tt)*)) => {
        fn visit_i64_load16_s(&mut self, memarg: $argty) -> Self::Output { self.visit_i64_load16_s_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i64_load32_u ($($ann:tt)*)) => {
        fn visit_i64_load32_u(&mut self, memarg: $argty) -> Self::Output { self.visit_i64_load32_u_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i64_load32_s ($($ann:tt)*)) => {
        fn visit_i64_load32_s(&mut self, memarg: $argty) -> Self::Output { self.visit_i64_load32_s_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i64_store ($($ann:tt)*)) => {
        fn visit_i64_store(&mut self, memarg: $argty) -> Self::Output {
            self.visit_i64_store_impl(memarg)
        }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i64_store8 ($($ann:tt)*)) => {
        fn visit_i64_store8(&mut self, memarg: $argty) -> Self::Output { self.visit_i64_store8_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i64_store16 ($($ann:tt)*)) => {
        fn visit_i64_store16(&mut self, memarg: $argty) -> Self::Output { self.visit_i64_store16_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_i64_store32 ($($ann:tt)*)) => {
        fn visit_i64_store32(&mut self, memarg: $argty) -> Self::Output {
            self.visit_i64_store32_impl(memarg)
        }
    };
    (@$proposal:ident $op:ident => visit_i64_extend_i32_u ($($ann:tt)*)) => {
        fn visit_i64_extend_i32_u(&mut self) -> Self::Output { self.visit_i64_extend_i32_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_wrap_i64 ($($ann:tt)*)) => {
        fn visit_i32_wrap_i64(&mut self) -> Self::Output { self.visit_i32_wrap_i64_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_extend8_s ($($ann:tt)*)) => {
        fn visit_i32_extend8_s(&mut self) -> Self::Output { self.visit_i32_extend8_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_extend16_s ($($ann:tt)*)) => {
        fn visit_i32_extend16_s(&mut self) -> Self::Output { self.visit_i32_extend16_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_reinterpret_f64 ($($ann:tt)*)) => {
        fn visit_i64_reinterpret_f64(&mut self) -> Self::Output { self.visit_i64_reinterpret_f64_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_reinterpret_i64 ($($ann:tt)*)) => {
        fn visit_f64_reinterpret_i64(&mut self) -> Self::Output { self.visit_f64_reinterpret_i64_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_reinterpret_i32 ($($ann:tt)*)) => {
        fn visit_f32_reinterpret_i32(&mut self) -> Self::Output { self.visit_f32_reinterpret_i32_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_reinterpret_f32 ($($ann:tt)*)) => {
        fn visit_i32_reinterpret_f32(&mut self) -> Self::Output { self.visit_i32_reinterpret_f32_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_eq ($($ann:tt)*)) => {
        fn visit_f64_eq(&mut self) -> Self::Output { self.visit_f64_eq_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_ne ($($ann:tt)*)) => {
        fn visit_f64_ne(&mut self) -> Self::Output { self.visit_f64_ne_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_lt ($($ann:tt)*)) => {
        fn visit_f64_lt(&mut self) -> Self::Output { self.visit_f64_lt_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_le ($($ann:tt)*)) => {
        fn visit_f64_le(&mut self) -> Self::Output { self.visit_f64_le_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_gt ($($ann:tt)*)) => {
        fn visit_f64_gt(&mut self) -> Self::Output { self.visit_f64_gt_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_ge ($($ann:tt)*)) => {
        fn visit_f64_ge(&mut self) -> Self::Output { self.visit_f64_ge_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_eq ($($ann:tt)*)) => {
        fn visit_f32_eq(&mut self) -> Self::Output { self.visit_f32_eq_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_ne ($($ann:tt)*)) => {
        fn visit_f32_ne(&mut self) -> Self::Output { self.visit_f32_ne_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_lt ($($ann:tt)*)) => {
        fn visit_f32_lt(&mut self) -> Self::Output { self.visit_f32_lt_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_le ($($ann:tt)*)) => {
        fn visit_f32_le(&mut self) -> Self::Output { self.visit_f32_le_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_gt ($($ann:tt)*)) => {
        fn visit_f32_gt(&mut self) -> Self::Output { self.visit_f32_gt_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_ge ($($ann:tt)*)) => {
        fn visit_f32_ge(&mut self) -> Self::Output { self.visit_f32_ge_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_add ($($ann:tt)*)) => {
        fn visit_f64_add(&mut self) -> Self::Output { self.visit_f64_add_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_sub ($($ann:tt)*)) => {
        fn visit_f64_sub(&mut self) -> Self::Output { self.visit_f64_sub_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_mul ($($ann:tt)*)) => {
        fn visit_f64_mul(&mut self) -> Self::Output { self.visit_f64_mul_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_div ($($ann:tt)*)) => {
        fn visit_f64_div(&mut self) -> Self::Output { self.visit_f64_div_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_add ($($ann:tt)*)) => {
        fn visit_f32_add(&mut self) -> Self::Output { self.visit_f32_add_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_sub ($($ann:tt)*)) => {
        fn visit_f32_sub(&mut self) -> Self::Output { self.visit_f32_sub_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_mul ($($ann:tt)*)) => {
        fn visit_f32_mul(&mut self) -> Self::Output { self.visit_f32_mul_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_div ($($ann:tt)*)) => {
        fn visit_f32_div(&mut self) -> Self::Output { self.visit_f32_div_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_abs ($($ann:tt)*)) => {
        fn visit_f64_abs(&mut self) -> Self::Output { self.visit_f64_abs_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_neg ($($ann:tt)*)) => {
        fn visit_f64_neg(&mut self) -> Self::Output { self.visit_f64_neg_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_ceil ($($ann:tt)*)) => {
        fn visit_f64_ceil(&mut self) -> Self::Output { self.visit_f64_ceil_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_floor ($($ann:tt)*)) => {
        fn visit_f64_floor(&mut self) -> Self::Output { self.visit_f64_floor_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_trunc ($($ann:tt)*)) => {
        fn visit_f64_trunc(&mut self) -> Self::Output { self.visit_f64_trunc_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_nearest ($($ann:tt)*)) => {
        fn visit_f64_nearest(&mut self) -> Self::Output { self.visit_f64_nearest_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_sqrt ($($ann:tt)*)) => {
        fn visit_f64_sqrt(&mut self) -> Self::Output { self.visit_f64_sqrt_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_abs ($($ann:tt)*)) => {
        fn visit_f32_abs(&mut self) -> Self::Output { self.visit_f32_abs_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_neg ($($ann:tt)*)) => {
        fn visit_f32_neg(&mut self) -> Self::Output { self.visit_f32_neg_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_ceil ($($ann:tt)*)) => {
        fn visit_f32_ceil(&mut self) -> Self::Output { self.visit_f32_ceil_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_floor ($($ann:tt)*)) => {
        fn visit_f32_floor(&mut self) -> Self::Output { self.visit_f32_floor_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_trunc ($($ann:tt)*)) => {
        fn visit_f32_trunc(&mut self) -> Self::Output { self.visit_f32_trunc_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_nearest ($($ann:tt)*)) => {
        fn visit_f32_nearest(&mut self) -> Self::Output { self.visit_f32_nearest_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_sqrt ($($ann:tt)*)) => {
        fn visit_f32_sqrt(&mut self) -> Self::Output { self.visit_f32_sqrt_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_min ($($ann:tt)*)) => {
        fn visit_f64_min(&mut self) -> Self::Output { self.visit_f64_min_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_max ($($ann:tt)*)) => {
        fn visit_f64_max(&mut self) -> Self::Output { self.visit_f64_max_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_copysign ($($ann:tt)*)) => {
        fn visit_f64_copysign(&mut self) -> Self::Output { self.visit_f64_copysign_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_min ($($ann:tt)*)) => {
        fn visit_f32_min(&mut self) -> Self::Output { self.visit_f32_min_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_max ($($ann:tt)*)) => {
        fn visit_f32_max(&mut self) -> Self::Output { self.visit_f32_max_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_copysign ($($ann:tt)*)) => {
        fn visit_f32_copysign(&mut self) -> Self::Output { self.visit_f32_copysign_impl() }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_f32_load ($($ann:tt)*)) => {
        fn visit_f32_load(&mut self, memarg: $argty) -> Self::Output { self.visit_f32_load_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_f64_load ($($ann:tt)*)) => {
        fn visit_f64_load(&mut self, memarg: $argty) -> Self::Output { self.visit_f64_load_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_f32_store ($($ann:tt)*)) => {
        fn visit_f32_store(&mut self, memarg: $argty) -> Self::Output { self.visit_f32_store_impl(memarg) }
    };
    (@$proposal:ident $op:ident { memarg: $argty:ty } => visit_f64_store ($($ann:tt)*)) => {
        fn visit_f64_store(&mut self, memarg: $argty) -> Self::Output { self.visit_f64_store_impl(memarg) }
    };
    (@$proposal:ident $op:ident => visit_i32_trunc_f64_s ($($ann:tt)*)) => {
        fn visit_i32_trunc_f64_s(&mut self) -> Self::Output { self.visit_i32_trunc_f64_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_trunc_f64_u ($($ann:tt)*)) => {
        fn visit_i32_trunc_f64_u(&mut self) -> Self::Output { self.visit_i32_trunc_f64_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_trunc_f32_s ($($ann:tt)*)) => {
        fn visit_i32_trunc_f32_s(&mut self) -> Self::Output { self.visit_i32_trunc_f32_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_trunc_f32_u ($($ann:tt)*)) => {
        fn visit_i32_trunc_f32_u(&mut self) -> Self::Output { self.visit_i32_trunc_f32_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_trunc_f64_s ($($ann:tt)*)) => {
        fn visit_i64_trunc_f64_s(&mut self) -> Self::Output { self.visit_i64_trunc_f64_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_trunc_f64_u ($($ann:tt)*)) => {
        fn visit_i64_trunc_f64_u(&mut self) -> Self::Output { self.visit_i64_trunc_f64_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_trunc_f32_s ($($ann:tt)*)) => {
        fn visit_i64_trunc_f32_s(&mut self) -> Self::Output { self.visit_i64_trunc_f32_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_trunc_f32_u ($($ann:tt)*)) => {
        fn visit_i64_trunc_f32_u(&mut self) -> Self::Output { self.visit_i64_trunc_f32_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_trunc_sat_f64_s ($($ann:tt)*)) => {
        fn visit_i32_trunc_sat_f64_s(&mut self) -> Self::Output { self.visit_i32_trunc_sat_f64_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_trunc_sat_f64_u ($($ann:tt)*)) => {
        fn visit_i32_trunc_sat_f64_u(&mut self) -> Self::Output { self.visit_i32_trunc_sat_f64_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_trunc_sat_f32_s ($($ann:tt)*)) => {
        fn visit_i32_trunc_sat_f32_s(&mut self) -> Self::Output { self.visit_i32_trunc_sat_f32_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i32_trunc_sat_f32_u ($($ann:tt)*)) => {
        fn visit_i32_trunc_sat_f32_u(&mut self) -> Self::Output { self.visit_i32_trunc_sat_f32_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_trunc_sat_f64_s ($($ann:tt)*)) => {
        fn visit_i64_trunc_sat_f64_s(&mut self) -> Self::Output { self.visit_i64_trunc_sat_f64_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_trunc_sat_f64_u ($($ann:tt)*)) => {
        fn visit_i64_trunc_sat_f64_u(&mut self) -> Self::Output { self.visit_i64_trunc_sat_f64_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_trunc_sat_f32_s ($($ann:tt)*)) => {
        fn visit_i64_trunc_sat_f32_s(&mut self) -> Self::Output { self.visit_i64_trunc_sat_f32_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_trunc_sat_f32_u ($($ann:tt)*)) => {
        fn visit_i64_trunc_sat_f32_u(&mut self) -> Self::Output { self.visit_i64_trunc_sat_f32_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_convert_i32_s ($($ann:tt)*)) => {
        fn visit_f64_convert_i32_s(&mut self) -> Self::Output { self.visit_f64_convert_i32_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_convert_i32_u ($($ann:tt)*)) => {
        fn visit_f64_convert_i32_u(&mut self) -> Self::Output { self.visit_f64_convert_i32_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_convert_i64_s ($($ann:tt)*)) => {
        fn visit_f64_convert_i64_s(&mut self) -> Self::Output { self.visit_f64_convert_i64_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_convert_i64_u ($($ann:tt)*)) => {
        fn visit_f64_convert_i64_u(&mut self) -> Self::Output { self.visit_f64_convert_i64_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_convert_i32_s ($($ann:tt)*)) => {
        fn visit_f32_convert_i32_s(&mut self) -> Self::Output { self.visit_f32_convert_i32_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_convert_i32_u ($($ann:tt)*)) => {
        fn visit_f32_convert_i32_u(&mut self) -> Self::Output { self.visit_f32_convert_i32_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_convert_i64_s ($($ann:tt)*)) => {
        fn visit_f32_convert_i64_s(&mut self) -> Self::Output { self.visit_f32_convert_i64_s_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_convert_i64_u ($($ann:tt)*)) => {
        fn visit_f32_convert_i64_u(&mut self) -> Self::Output { self.visit_f32_convert_i64_u_impl() }
    };
    (@$proposal:ident $op:ident => visit_f64_promote_f32 ($($ann:tt)*)) => {
        fn visit_f64_promote_f32(&mut self) -> Self::Output { self.visit_f64_promote_f32_impl() }
    };
    (@$proposal:ident $op:ident => visit_f32_demote_f64 ($($ann:tt)*)) => {
        fn visit_f32_demote_f64(&mut self) -> Self::Output { self.visit_f32_demote_f64_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_popcnt ($($ann:tt)*)) => {
        fn visit_i64_popcnt(&mut self) -> Self::Output { self.visit_i64_popcnt_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_clz ($($ann:tt)*)) => {
        fn visit_i64_clz(&mut self) -> Self::Output { self.visit_i64_clz_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_ctz ($($ann:tt)*)) => {
        fn visit_i64_ctz(&mut self) -> Self::Output { self.visit_i64_ctz_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_rotl ($($ann:tt)*)) => {
        fn visit_i64_rotl(&mut self) -> Self::Output { self.visit_i64_rotl_impl() }
    };
    (@$proposal:ident $op:ident => visit_i64_rotr ($($ann:tt)*)) => {
        fn visit_i64_rotr(&mut self) -> Self::Output { self.visit_i64_rotr_impl() }
    };
    (@$proposal:ident $op:ident { function_index: $argty:ty } => visit_call ($($ann:tt)*)) => {
        fn visit_call(&mut self, function_index: $argty) -> Self::Output {
            self.visit_call_impl(function_index)
        }
    };
    (@$proposal:ident $op:ident { global_index: $argty:ty } => visit_global_get ($($ann:tt)*)) => {
        fn visit_global_get(&mut self, global_index: $argty) -> Self::Output { self.visit_global_get_impl(global_index) }
    };
    (@$proposal:ident $op:ident { global_index: $argty:ty } => visit_global_set ($($ann:tt)*)) => {
        fn visit_global_set(&mut self, global_index: $argty) -> Self::Output { self.visit_global_set_impl(global_index) }
    };
    (@$proposal:ident $op:ident { type_index: $argty1:ty, table_index: $argty2:ty } => visit_call_indirect ($($ann:tt)*)) => {
        fn visit_call_indirect(&mut self, type_index: $argty1, table_index: $argty2) -> Self::Output {
            self.visit_call_indirect_impl(type_index, table_index)
        }
    };
    (@$proposal:ident $op:ident => visit_select ($($ann:tt)*)) => {
        fn visit_select(&mut self) -> Self::Output { self.visit_select_impl() }
    };
    (@$proposal:ident $op:ident { targets: $argty:ty } => visit_br_table ($($ann:tt)*)) => {
        fn visit_br_table(&mut self, targets: $argty) -> Self::Output { self.visit_br_table_impl(targets) }
    };
    (@$proposal:ident $op:ident { dst_mem: $argty1:ty, src_mem: $argty2:ty } => visit_memory_copy ($($ann:tt)*)) => {
        fn visit_memory_copy(&mut self, dst_mem: $argty1, src_mem: $argty2) -> Self::Output {
            self.visit_memory_copy_impl(dst_mem, src_mem)
        }
    };
    (@$proposal:ident $op:ident { mem: $argty:ty } => visit_memory_fill ($($ann:tt)*)) => {
        fn visit_memory_fill(&mut self, mem: $argty) -> Self::Output { self.visit_memory_fill_impl(mem) }
    };
    (@$proposal:ident $op:ident { mem: $argty:ty } => visit_memory_grow ($($ann:tt)*)) => {
        fn visit_memory_grow(&mut self, mem: $argty) -> Self::Output { self.visit_memory_grow_impl(mem) }
    };
    (@$proposal:ident $op:ident { mem: $argty:ty } => visit_memory_size ($($ann:tt)*)) => {
        fn visit_memory_size(&mut self, mem: $argty) -> Self::Output { self.visit_memory_size_impl(mem) }
    };
    (@$proposal:ident Drop => visit_drop ($($ann:tt)*)) => {
        fn visit_drop(&mut self) -> Self::Output {
            self.visit_drop_impl()
        }
    };
    (@$proposal:ident Nop => visit_nop ($($ann:tt)*)) => {
        fn visit_nop(&mut self) -> Self::Output { self.visit_nop_impl() }
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

fn parse_const_expr_u32(expr: ConstExpr<'_>) -> Result<u32, CompileError> {
    let value = parse_global_init_expr(expr)?;
    match value {
        GlobalInitValue::I32(v) => Ok(v),
        GlobalInitValue::I64(v) => Ok(v as u32),
        GlobalInitValue::Global(_) => {
            Err(CompileError::UnsupportedFeature("global.get in const expr"))
        }
    }
}

fn parse_global_init_expr(expr: ConstExpr<'_>) -> Result<GlobalInitValue, CompileError> {
    let mut reader = expr.get_operators_reader();
    let op = reader
        .read()
        .map_err(|_| CompileError::InvalidWasm("global init expr"))?;
    let value = match op {
        wasmparser::Operator::I32Const { value } => GlobalInitValue::I32(value as u32),
        wasmparser::Operator::I64Const { value } => GlobalInitValue::I64(value as u64),
        wasmparser::Operator::GlobalGet { global_index } => GlobalInitValue::Global(global_index),
        _ => return Err(CompileError::UnsupportedFeature("global init expr")),
    };
    reader
        .read()
        .map_err(|_| CompileError::InvalidWasm("global init expr end"))?;
    Ok(value)
}

fn build_globals(
    global_types: &[wasmparser::GlobalType],
    init_values: &[GlobalInitValue],
    imported_global_count: usize,
) -> Result<Vec<crate::GlobalEntry>, CompileError> {
    let mut globals: Vec<crate::GlobalEntry> = Vec::with_capacity(global_types.len());
    for (index, (global_type, init_value)) in
        global_types.iter().zip(init_values.iter()).enumerate()
    {
        let value = if index < imported_global_count {
            0
        } else {
            match init_value {
                GlobalInitValue::I32(v) => u64::from(*v),
                GlobalInitValue::I64(v) => *v,
                GlobalInitValue::Global(src) => {
                    globals
                        .get(*src as usize)
                        .ok_or(CompileError::InvalidWasm("global init global"))?
                        .value
                }
            }
        };
        globals.push(crate::GlobalEntry {
            value,
            mutable: global_type.mutable,
        });
    }
    Ok(globals)
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

unsafe extern "C" fn host_i32_load8_u_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        *frame.add(HELPER_RET_SLOT as usize) = helper_i32_load8_u(ctx, addr);
    }
    0
}

unsafe extern "C" fn host_i32_load8_s_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        *frame.add(HELPER_RET_SLOT as usize) = helper_i32_load8_s(ctx, addr);
    }
    0
}

unsafe extern "C" fn host_i32_load16_u_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        *frame.add(HELPER_RET_SLOT as usize) = helper_i32_load16_u(ctx, addr);
    }
    0
}

unsafe extern "C" fn host_i32_load16_s_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        *frame.add(HELPER_RET_SLOT as usize) = helper_i32_load16_s(ctx, addr);
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

unsafe extern "C" fn host_i32_store8_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        let value = *frame.add(HELPER_ARG1_SLOT as usize) as u32;
        helper_i32_store8(ctx, addr, value);
    }
    0
}

unsafe extern "C" fn host_i32_store16_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        let value = *frame.add(HELPER_ARG1_SLOT as usize) as u32;
        helper_i32_store16(ctx, addr, value);
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

unsafe extern "C" fn host_i64_load32_u_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        *frame.add(HELPER_RET_SLOT as usize) = helper_i64_load32_u(ctx, addr);
    }
    0
}

unsafe extern "C" fn host_i64_load8_u_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        *frame.add(HELPER_RET_SLOT as usize) = helper_i64_load8_u(ctx, addr);
    }
    0
}

unsafe extern "C" fn host_i64_load8_s_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        *frame.add(HELPER_RET_SLOT as usize) = helper_i64_load8_s(ctx, addr);
    }
    0
}

unsafe extern "C" fn host_i64_load16_u_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        *frame.add(HELPER_RET_SLOT as usize) = helper_i64_load16_u(ctx, addr);
    }
    0
}

unsafe extern "C" fn host_i64_load16_s_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        *frame.add(HELPER_RET_SLOT as usize) = helper_i64_load16_s(ctx, addr);
    }
    0
}

unsafe extern "C" fn host_i64_load32_s_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        *frame.add(HELPER_RET_SLOT as usize) = helper_i64_load32_s(ctx, addr);
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

unsafe extern "C" fn host_i64_store8_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        let value = *frame.add(HELPER_ARG1_SLOT as usize);
        helper_i64_store8(ctx, addr, value);
    }
    0
}

unsafe extern "C" fn host_i64_store16_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        let value = *frame.add(HELPER_ARG1_SLOT as usize);
        helper_i64_store16(ctx, addr, value);
    }
    0
}

unsafe extern "C" fn host_i64_store32_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let addr = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        let value = *frame.add(HELPER_ARG1_SLOT as usize);
        helper_i64_store32(ctx, addr, value);
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

unsafe extern "C" fn host_call_indirect_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let table_index = *frame.add(HELPER_CALL_FUNC_SLOT as usize) as u32;
        let args_ptr = frame.add(HELPER_CALL_ARGS_BASE as usize) as *const RawValue;
        let value = helper_call_indirect(ctx, table_index, args_ptr);
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

unsafe extern "C" fn host_global_get_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let index = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        *frame.add(HELPER_RET_SLOT as usize) = helper_global_get(ctx, index);
    }
    0
}

unsafe extern "C" fn host_global_set_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let index = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        let value = *frame.add(HELPER_ARG1_SLOT as usize);
        helper_global_set(ctx, index, value);
    }
    0
}

unsafe extern "C" fn host_memory_copy_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let dst = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        let src = *frame.add(HELPER_ARG1_SLOT as usize) as u32;
        let len = *frame.add(2) as u32;
        helper_memory_copy(ctx, dst, src, len);
    }
    0
}

unsafe extern "C" fn host_memory_fill_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let dst = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        let value = *frame.add(HELPER_ARG1_SLOT as usize) as u32;
        let len = *frame.add(2) as u32;
        helper_memory_fill(ctx, dst, value, len);
    }
    0
}

unsafe extern "C" fn host_memory_grow_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        let delta = *frame.add(HELPER_ARG0_SLOT as usize) as u32;
        *frame.add(HELPER_RET_SLOT as usize) = helper_memory_grow(ctx, delta);
    }
    0
}

unsafe extern "C" fn host_memory_size_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    unsafe {
        *frame.add(HELPER_RET_SLOT as usize) = helper_memory_size(ctx);
    }
    0
}

unsafe extern "C" fn host_i32_popcnt_wrapper(
    _ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    let val = unsafe { *frame.add(HELPER_ARG0_SLOT as usize) as u32 };
    let count = val.count_ones();
    unsafe {
        *frame.add(HELPER_RET_SLOT as usize) = count as RawValue;
    }
    0
}

unsafe extern "C" fn host_i64_popcnt_wrapper(
    _ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    let val = unsafe { *frame.add(HELPER_ARG0_SLOT as usize) };
    let count = val.count_ones();
    unsafe {
        *frame.add(HELPER_RET_SLOT as usize) = count as RawValue;
    }
    0
}

macro_rules! define_fcmp_wrapper {
    ($name:ident, $fTy:ty, $bits_ty:ty, $op:tt) => {
        unsafe extern "C" fn $name(
            _ctx: *mut VmContext,
            frame: *mut RawValue,
        ) -> RawValue {
            let lhs_bits = unsafe { *frame.add(HELPER_ARG0_SLOT as usize) as $bits_ty };
            let rhs_bits = unsafe { *frame.add(HELPER_ARG1_SLOT as usize) as $bits_ty };
            let lhs = <$fTy>::from_bits(lhs_bits);
            let rhs = <$fTy>::from_bits(rhs_bits);
            let result: u32 = if lhs $op rhs { 1 } else { 0 };
            unsafe {
                *frame.add(HELPER_RET_SLOT as usize) = result as RawValue;
            }
            0
        }
    };
}

define_fcmp_wrapper!(host_f64_eq_wrapper, f64, u64, ==);
define_fcmp_wrapper!(host_f64_ne_wrapper, f64, u64, !=);
define_fcmp_wrapper!(host_f64_lt_wrapper, f64, u64, <);
define_fcmp_wrapper!(host_f64_le_wrapper, f64, u64, <=);
define_fcmp_wrapper!(host_f64_gt_wrapper, f64, u64, >);
define_fcmp_wrapper!(host_f64_ge_wrapper, f64, u64, >=);
define_fcmp_wrapper!(host_f32_eq_wrapper, f32, u32, ==);
define_fcmp_wrapper!(host_f32_ne_wrapper, f32, u32, !=);
define_fcmp_wrapper!(host_f32_lt_wrapper, f32, u32, <);
define_fcmp_wrapper!(host_f32_le_wrapper, f32, u32, <=);
define_fcmp_wrapper!(host_f32_gt_wrapper, f32, u32, >);
define_fcmp_wrapper!(host_f32_ge_wrapper, f32, u32, >=);

macro_rules! define_fpu_wrapper {
    ($name:ident, $fTy:ty, $bits_ty:ty, $op:tt) => {
        unsafe extern "C" fn $name(
            _ctx: *mut VmContext,
            frame: *mut RawValue,
        ) -> RawValue {
            let lhs_bits = unsafe { *frame.add(HELPER_ARG0_SLOT as usize) as $bits_ty };
            let rhs_bits = unsafe { *frame.add(HELPER_ARG1_SLOT as usize) as $bits_ty };
            let lhs = <$fTy>::from_bits(lhs_bits);
            let rhs = <$fTy>::from_bits(rhs_bits);
            let result = lhs $op rhs;
            unsafe {
                *frame.add(HELPER_RET_SLOT as usize) = result.to_bits() as RawValue;
            }
            0
        }
    };
}

define_fpu_wrapper!(host_f64_add_wrapper, f64, u64, +);
define_fpu_wrapper!(host_f64_sub_wrapper, f64, u64, -);
define_fpu_wrapper!(host_f64_mul_wrapper, f64, u64, *);
define_fpu_wrapper!(host_f64_div_wrapper, f64, u64, /);
define_fpu_wrapper!(host_f32_add_wrapper, f32, u32, +);
define_fpu_wrapper!(host_f32_sub_wrapper, f32, u32, -);
define_fpu_wrapper!(host_f32_mul_wrapper, f32, u32, *);
define_fpu_wrapper!(host_f32_div_wrapper, f32, u32, /);

macro_rules! define_funary_wrapper {
    ($name:ident, $src_ty:ty, $src_bits_ty:ty, $dst_ty:ty, $op:expr) => {
        unsafe extern "C" fn $name(_ctx: *mut VmContext, frame: *mut RawValue) -> RawValue {
            let bits = unsafe { *frame.add(HELPER_ARG0_SLOT as usize) as $src_bits_ty };
            let val = <$src_ty>::from_bits(bits);
            let result: $dst_ty = ($op)(val);
            unsafe {
                *frame.add(HELPER_RET_SLOT as usize) = result.to_bits() as RawValue;
            }
            0
        }
    };
}

macro_rules! define_fpu_fn_wrapper {
    ($name:ident, $f_ty:ty, $bits_ty:ty, $op:expr) => {
        unsafe extern "C" fn $name(_ctx: *mut VmContext, frame: *mut RawValue) -> RawValue {
            let lhs_bits = unsafe { *frame.add(HELPER_ARG0_SLOT as usize) as $bits_ty };
            let rhs_bits = unsafe { *frame.add(HELPER_ARG1_SLOT as usize) as $bits_ty };
            let lhs = <$f_ty>::from_bits(lhs_bits);
            let rhs = <$f_ty>::from_bits(rhs_bits);
            let result: $f_ty = ($op)(lhs, rhs);
            unsafe {
                *frame.add(HELPER_RET_SLOT as usize) = result.to_bits() as RawValue;
            }
            0
        }
    };
}

macro_rules! define_float_to_int_wrapper {
    ($name:ident, $src_ty:ty, $src_bits_ty:ty, $op:expr) => {
        unsafe extern "C" fn $name(_ctx: *mut VmContext, frame: *mut RawValue) -> RawValue {
            let bits = unsafe { *frame.add(HELPER_ARG0_SLOT as usize) as $src_bits_ty };
            let val = <$src_ty>::from_bits(bits);
            let result: RawValue = ($op)(val);
            unsafe {
                *frame.add(HELPER_RET_SLOT as usize) = result;
            }
            0
        }
    };
}

macro_rules! define_int_to_float_wrapper {
    ($name:ident, $dst_ty:ty, $op:expr) => {
        unsafe extern "C" fn $name(_ctx: *mut VmContext, frame: *mut RawValue) -> RawValue {
            let raw = unsafe { *frame.add(HELPER_ARG0_SLOT as usize) };
            let result: $dst_ty = ($op)(raw);
            unsafe {
                *frame.add(HELPER_RET_SLOT as usize) = result.to_bits() as RawValue;
            }
            0
        }
    };
}

define_funary_wrapper!(host_f64_abs_wrapper, f64, u64, f64, |val: f64| val.abs());
define_funary_wrapper!(host_f64_neg_wrapper, f64, u64, f64, |val: f64| -val);
define_funary_wrapper!(host_f64_ceil_wrapper, f64, u64, f64, |val: f64| libm::ceil(
    val
));
define_funary_wrapper!(host_f64_floor_wrapper, f64, u64, f64, |val: f64| {
    libm::floor(val)
});
define_funary_wrapper!(host_f64_trunc_wrapper, f64, u64, f64, |val: f64| {
    libm::trunc(val)
});
define_funary_wrapper!(host_f64_nearest_wrapper, f64, u64, f64, |val: f64| {
    libm::rint(val)
});
define_funary_wrapper!(host_f64_sqrt_wrapper, f64, u64, f64, |val: f64| libm::sqrt(
    val
));
define_funary_wrapper!(host_f32_abs_wrapper, f32, u32, f32, |val: f32| val.abs());
define_funary_wrapper!(host_f32_neg_wrapper, f32, u32, f32, |val: f32| -val);
define_funary_wrapper!(
    host_f32_ceil_wrapper,
    f32,
    u32,
    f32,
    |val: f32| libm::ceilf(val)
);
define_funary_wrapper!(host_f32_floor_wrapper, f32, u32, f32, |val: f32| {
    libm::floorf(val)
});
define_funary_wrapper!(host_f32_trunc_wrapper, f32, u32, f32, |val: f32| {
    libm::truncf(val)
});
define_funary_wrapper!(host_f32_nearest_wrapper, f32, u32, f32, |val: f32| {
    libm::rintf(val)
});
define_funary_wrapper!(
    host_f32_sqrt_wrapper,
    f32,
    u32,
    f32,
    |val: f32| libm::sqrtf(val)
);
define_funary_wrapper!(host_f64_promote_f32_wrapper, f32, u32, f64, |val: f32| val
    as f64);
define_funary_wrapper!(host_f32_demote_f64_wrapper, f64, u64, f32, |val: f64| val
    as f32);

define_fpu_fn_wrapper!(host_f64_min_wrapper, f64, u64, |lhs: f64, rhs: f64| lhs
    .min(rhs));
define_fpu_fn_wrapper!(host_f64_max_wrapper, f64, u64, |lhs: f64, rhs: f64| lhs
    .max(rhs));
define_fpu_fn_wrapper!(host_f64_copysign_wrapper, f64, u64, |lhs: f64, rhs: f64| {
    lhs.copysign(rhs)
});
define_fpu_fn_wrapper!(host_f32_min_wrapper, f32, u32, |lhs: f32, rhs: f32| lhs
    .min(rhs));
define_fpu_fn_wrapper!(host_f32_max_wrapper, f32, u32, |lhs: f32, rhs: f32| lhs
    .max(rhs));
define_fpu_fn_wrapper!(host_f32_copysign_wrapper, f32, u32, |lhs: f32, rhs: f32| {
    lhs.copysign(rhs)
});

define_float_to_int_wrapper!(
    host_i32_trunc_f64_s_wrapper,
    f64,
    u64,
    |val: f64| (val as i32) as u32 as RawValue
);
define_float_to_int_wrapper!(
    host_i32_trunc_f64_u_wrapper,
    f64,
    u64,
    |val: f64| (val as u32) as RawValue
);
define_float_to_int_wrapper!(
    host_i32_trunc_f32_s_wrapper,
    f32,
    u32,
    |val: f32| (val as i32) as u32 as RawValue
);
define_float_to_int_wrapper!(
    host_i32_trunc_f32_u_wrapper,
    f32,
    u32,
    |val: f32| (val as u32) as RawValue
);
define_float_to_int_wrapper!(
    host_i64_trunc_f64_s_wrapper,
    f64,
    u64,
    |val: f64| (val as i64) as RawValue
);
define_float_to_int_wrapper!(
    host_i64_trunc_f64_u_wrapper,
    f64,
    u64,
    |val: f64| val as u64 as RawValue
);
define_float_to_int_wrapper!(
    host_i64_trunc_f32_s_wrapper,
    f32,
    u32,
    |val: f32| (val as i64) as RawValue
);
define_float_to_int_wrapper!(
    host_i64_trunc_f32_u_wrapper,
    f32,
    u32,
    |val: f32| val as u64 as RawValue
);

define_int_to_float_wrapper!(
    host_f64_convert_i32_s_wrapper,
    f64,
    |raw: RawValue| raw as u32 as i32 as f64
);
define_int_to_float_wrapper!(
    host_f64_convert_i32_u_wrapper,
    f64,
    |raw: RawValue| raw as u32 as f64
);
define_int_to_float_wrapper!(
    host_f64_convert_i64_s_wrapper,
    f64,
    |raw: RawValue| raw as i64 as f64
);
define_int_to_float_wrapper!(host_f64_convert_i64_u_wrapper, f64, |raw: RawValue| raw
    as f64);
define_int_to_float_wrapper!(
    host_f32_convert_i32_s_wrapper,
    f32,
    |raw: RawValue| raw as u32 as i32 as f32
);
define_int_to_float_wrapper!(
    host_f32_convert_i32_u_wrapper,
    f32,
    |raw: RawValue| raw as u32 as f32
);
define_int_to_float_wrapper!(
    host_f32_convert_i64_s_wrapper,
    f32,
    |raw: RawValue| raw as i64 as f32
);
define_int_to_float_wrapper!(host_f32_convert_i64_u_wrapper, f32, |raw: RawValue| raw
    as f32);

unsafe extern "C" fn host_unreachable_wrapper(
    ctx: *mut VmContext,
    _frame: *mut RawValue,
) -> RawValue {
    unsafe { helper_trap(ctx, TrapCode::Unreachable) };
    0
}

unsafe extern "C" fn host_check_trap_wrapper(
    ctx: *mut VmContext,
    frame: *mut RawValue,
) -> RawValue {
    let trapped = unsafe { (*ctx).trap != TrapCode::None };
    unsafe {
        *frame.add(HELPER_ARG1_SLOT as usize) = if trapped { 1 } else { 0 };
    }
    0
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
