use alloc::vec::Vec;

pub type LabelId = u32;

pub struct Label {
    pub bound_offset: Option<u32>,
}

impl Label {
    pub fn new() -> Self {
        Self { bound_offset: None }
    }

    pub fn bind(&mut self, offset: u32) {
        self.bound_offset = Some(offset);
    }
}

pub enum BranchKind {
    Unconditional,
    ConditionalZero,
    ConditionalNotZero,
}

pub struct Fixup {
    pub at_offset: u32,
    pub kind: BranchKind,
    pub target: LabelId,
}

pub enum ControlKind {
    Block,
    Loop,
    If,
}

pub struct ControlFrame {
    pub kind: ControlKind,
    pub entry_stack_height: u16,
    pub result_arity: u8,
    pub branch_target: LabelId,
    pub end_label: LabelId,
    pub else_label: Option<LabelId>,
}

pub struct ControlStack {
    frames: Vec<ControlFrame>,
}

impl ControlStack {
    pub fn new() -> Self {
        Self { frames: Vec::new() }
    }

    pub fn push(&mut self, frame: ControlFrame) {
        self.frames.push(frame);
    }

    pub fn pop(&mut self) -> Option<ControlFrame> {
        self.frames.pop()
    }

    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    pub fn get_branch_target(&self, depth: u32) -> Option<LabelId> {
        let idx = self.frames.len().checked_sub(depth as usize + 1)?;
        Some(self.frames[idx].branch_target)
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}
