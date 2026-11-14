//! Fixed-width native policy records. All reserved fields must be zero.
//! ShieldPolicyCreate(labels, label_count, rules, rule_count) copies them
//! synchronously and returns an immutable policy management capability.

/// Bounded policy capacities shared by the kernel and native SDK.
pub const MAX_LABELS: usize = 2048;
pub const MAX_RULES: usize = 16384;
pub const MAX_LABEL_BYTES: usize = 255;

/// Persistent Shield file-label metadata is kept in this reserved trusted
/// xattr namespace. The suffix is the lowercase hexadecimal stable domain.
pub const FILE_LABEL_METADATA_PREFIX: &[u8] = b"trusted.scarlet.shield.";
pub const FILE_LABEL_METADATA_NAME_LEN: usize = 55;
pub const FILE_LABEL_DOMAIN_LEN: usize = 16;
pub const FILE_LABEL_METADATA_MAX_BYTES: usize = 512;
pub const FILE_LABEL_VALUE_PREFIX: &[u8] = b"SLB1";
const _: [(); FILE_LABEL_METADATA_NAME_LEN] =
    [(); FILE_LABEL_METADATA_PREFIX.len() + FILE_LABEL_DOMAIN_LEN * 2];

/// Convert a stable file-persistence domain into its reserved native xattr
/// name. Label metadata never contains policy addresses or numeric label IDs.
pub const fn file_label_metadata_name(
    domain: &[u8; FILE_LABEL_DOMAIN_LEN],
) -> [u8; FILE_LABEL_METADATA_NAME_LEN] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut name = [0u8; FILE_LABEL_METADATA_NAME_LEN];
    let mut index = 0;
    while index < FILE_LABEL_METADATA_PREFIX.len() {
        name[index] = FILE_LABEL_METADATA_PREFIX[index];
        index += 1;
    }
    let mut byte = 0;
    while byte < FILE_LABEL_DOMAIN_LEN {
        name[index] = HEX[(domain[byte] >> 4) as usize];
        name[index + 1] = HEX[(domain[byte] & 0x0f) as usize];
        index += 2;
        byte += 1;
    }
    name
}

/// Public xattr APIs must reject this namespace; only inode metadata backends
/// may read or write the reserved key.
pub fn is_file_label_metadata_name(name: &[u8]) -> bool {
    name.starts_with(FILE_LABEL_METADATA_PREFIX)
}

/// Encode a symbolic label into the versioned persistent representation.
pub fn encode_file_label(name: &str, output: &mut [u8]) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_LABEL_BYTES || bytes.contains(&0) {
        return None;
    }
    let length = FILE_LABEL_VALUE_PREFIX.len().checked_add(bytes.len())?;
    if length > FILE_LABEL_METADATA_MAX_BYTES || output.len() < length {
        return None;
    }
    output[..FILE_LABEL_VALUE_PREFIX.len()].copy_from_slice(FILE_LABEL_VALUE_PREFIX);
    output[FILE_LABEL_VALUE_PREFIX.len()..length].copy_from_slice(bytes);
    Some(length)
}

/// Decode only the versioned symbolic name. Policy lookup is intentionally a
/// separate step so an unknown name is never treated as an unlabeled file.
pub fn decode_file_label(value: &[u8]) -> Option<&str> {
    let name = value.strip_prefix(FILE_LABEL_VALUE_PREFIX)?;
    if value.len() > FILE_LABEL_METADATA_MAX_BYTES
        || name.is_empty()
        || name.len() > MAX_LABEL_BYTES
        || name.contains(&0)
    {
        return None;
    }
    core::str::from_utf8(name).ok()
}

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
    pub source: u32,
    pub parent: u32,
    pub target: u32,
    pub kind: u32,
    pub flags: u32,
    pub reserved: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawFileRelabel {
    pub source: u32,
    pub old: u32,
    pub target: u32,
    pub kind: u32,
}
const _: [(); 24] = [(); core::mem::size_of::<RawFileCreate>()];
const _: [(); 16] = [(); core::mem::size_of::<RawFileRelabel>()];

/// Native endpoint labeling authority; separate from ABI permission classes.
pub const SOCKET_LABEL_CLASS: u32 = 0x5343_0001;
pub const SOCKET_LABEL_CREATE: u64 = 1;

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

/// Version two retains the complete version-one prefix and reserved bytes.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawPolicyUpdateWithSets {
    pub base: RawPolicyUpdate,
    pub sets: u64,
    pub set_count: u64,
    pub members: u64,
    pub member_count: u64,
    pub set_grants: u64,
    pub set_grant_count: u64,
    pub set_files: u64,
    pub set_file_count: u64,
    pub reserved: [u64; 2],
}
pub const POLICY_UPDATE_SETS_VERSION: u32 = 2;
/// Version three appends bounded principal conditions to the version-two prefix.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawPolicyUpdateWithPrincipalRanges {
    pub base: RawPolicyUpdateWithSets,
    pub principal_ranges: u64,
    pub principal_range_count: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawPrincipalRangeTransition {
    pub source: u32,
    pub target: u32,
    pub first: u32,
    pub last: u32,
}
pub const POLICY_UPDATE_PRINCIPAL_RANGES_VERSION: u32 = 3;
const _: [(); 224] = [(); core::mem::size_of::<RawPolicyUpdateWithPrincipalRanges>()];
const _: [(); 16] = [(); core::mem::size_of::<RawPrincipalRangeTransition>()];
pub const MAX_LABEL_SETS: usize = 256;
pub const MAX_SET_MEMBERS: usize = 16384;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawLabelSet {
    pub id: u32,
    pub reserved: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawSetMember {
    pub set: u32,
    pub label: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawLabelSelector {
    pub kind: u32,
    pub id: u32,
}
impl RawLabelSelector {
    pub const fn label(id: u32) -> Self {
        Self { kind: 0, id }
    }
    pub const fn set(id: u32) -> Self {
        Self { kind: 1, id }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawSetGrant {
    pub source: RawLabelSelector,
    pub target: RawLabelSelector,
    pub class: u32,
    pub reserved: u32,
    pub permissions: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawSetFileRule {
    pub source: RawLabelSelector,
    pub origin: RawLabelSelector,
    pub target: RawLabelSelector,
    pub kind: u32,
    /// 1: creation authorization, 2: relabel authorization. No default selection.
    pub action: u32,
    /// Bit 0 requires different old and new labels (relabel rules only).
    pub flags: u32,
    pub reserved: u32,
}
const _: [(); 208] = [(); core::mem::size_of::<RawPolicyUpdateWithSets>()];
const _: [(); 8] = [(); core::mem::size_of::<RawLabelSet>()];
const _: [(); 8] = [(); core::mem::size_of::<RawSetMember>()];
const _: [(); 32] = [(); core::mem::size_of::<RawSetGrant>()];
const _: [(); 40] = [(); core::mem::size_of::<RawSetFileRule>()];
