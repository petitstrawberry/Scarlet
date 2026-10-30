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

/// Native endpoint labeling authority; separate from ABI permission classes.
pub const SOCKET_LABEL_CLASS:u32=0x5343_0001;
pub const SOCKET_LABEL_CREATE:u64=1;

/// Append-only update of an administrator-owned policy. The complete batch is
/// copied and validated before publication. Existing identities and grants are
/// preserved. A stale expected generation leaves the policy unchanged.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawPolicyUpdate {
    pub version: u32,
    pub flags: u32,
    pub expected_generation: u64,
    pub labels: u64,
    pub label_count: u64,
    pub rules: u64,
    pub rule_count: u64,
    pub transitions: u64,
    pub transition_count: u64,
    pub exec_transitions: u64,
    pub exec_transition_count: u64,
    pub file_create: u64,
    pub file_create_count: u64,
    pub file_relabel: u64,
    pub file_relabel_count: u64,
    pub reserved: [u64; 2],
}
pub const POLICY_UPDATE_VERSION: u32 = 1;
const _: [(); 128] = [(); core::mem::size_of::<RawPolicyUpdate>()];
