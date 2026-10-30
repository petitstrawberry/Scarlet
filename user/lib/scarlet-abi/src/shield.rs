//! Fixed-width native policy records. All reserved fields must be zero.
//! ShieldPolicyCreate(labels, label_count, rules, rule_count) copies them
//! synchronously and returns an immutable policy management capability.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawLabel {
    pub id: u32,
    pub name_len: u32,
    pub name: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawRule {
    pub source: u32,
    pub target: u32,
    pub class: u32,
    pub reserved: u32,
    pub permissions: u64,
}
const _: [(); 16] = [(); core::mem::size_of::<RawLabel>()];
const _: [(); 24] = [(); core::mem::size_of::<RawRule>()];

/// A conditional label transition. Principal is an ABI-neutral identity value;
/// all principal slots must match. Reserved must be zero.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawTransition {
    pub source: u32,
    pub target: u32,
    pub principal: u32,
    pub reserved: u32,
}
const _: [(); 16] = [(); core::mem::size_of::<RawTransition>()];

/// File-label-dependent exec selection. Query previews omit the principal;
/// execution always checks all native principal slots. Reserved must be zero.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawExecTransition {
    pub source: u32,
    pub resource: u32,
    pub target: u32,
    pub principal: u32,
    pub reserved: u64,
}
const _: [(); 24] = [(); core::mem::size_of::<RawExecTransition>()];

/// Native file kinds: regular=1, directory=2, symlink=3, char=4,
/// block=5, socket=6, pipe=7. Flags bit zero selects the default rule.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawFileCreate {
    pub source:u32,pub parent:u32,pub target:u32,pub kind:u32,pub flags:u32,pub reserved:u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawFileRelabel {pub source:u32,pub old:u32,pub target:u32,pub kind:u32}
const _: [();24]=[();core::mem::size_of::<RawFileCreate>()];
const _: [();16]=[();core::mem::size_of::<RawFileRelabel>()];
