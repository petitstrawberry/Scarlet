//! Byte decoder for the native-word Environment execution record.
//!
//! The 64-bit layout is the existing native ABI. The 32-bit layout describes the
//! same fields using four-byte words, for foundation tests and future adapters;
//! it does not register a new syscall or enable a compatibility execution mode.

use crate::data_model::{AbiDataError, AbiDataModel, UserAddress};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentExecError {
    Encoding(AbiDataError),
    UnsupportedSize,
    UnsupportedFlags,
    InvalidPlacement,
}

impl From<AbiDataError> for EnvironmentExecError {
    fn from(error: AbiDataError) -> Self {
        Self::Encoding(error)
    }
}

/// Decoded values, never a `repr(C)` view over user memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvironmentExec {
    pub argv: UserAddress,
    pub envp: UserAddress,
    pub cwd: UserAddress,
    pub handles: UserAddress,
    pub handle_count: u64,
}

impl EnvironmentExec {
    pub const MAX_ENCODED_SIZE: usize = 48;

    pub const fn encoded_size(model: AbiDataModel) -> usize {
        8 + 5 * model.word_width.bytes()
    }

    pub fn decode(model: AbiDataModel, bytes: &[u8]) -> Result<Self, EnvironmentExecError> {
        Self::decode_sized(model, bytes, Self::encoded_size(model))
    }

    fn decode_sized(
        model: AbiDataModel,
        bytes: &[u8],
        size: usize,
    ) -> Result<Self, EnvironmentExecError> {
        if model.read_u32(bytes, 0)? as usize != size {
            return Err(EnvironmentExecError::UnsupportedSize);
        }
        if model.read_u32(bytes, 4)? != 0 {
            return Err(EnvironmentExecError::UnsupportedFlags);
        }
        let word = |index| model.read_word(bytes, 8 + index * model.word_width.bytes());
        let address = |index| model.user_address(word(index)?.unsigned());
        Ok(Self {
            argv: address(0)?,
            envp: address(1)?,
            cwd: address(2)?,
            handles: address(3)?,
            handle_count: word(4)?.unsigned(),
        })
    }
}

/// The child's job-control placement when it becomes scheduler-visible.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SpawnPlacement {
    #[default]
    Inherit,
    NewProcessGroup,
    JoinProcessGroup(u32),
    NewSession,
}

impl SpawnPlacement {
    /// The record stores the placement tag and the namespace-local group ID.
    pub const fn encode(self) -> (u32, u32) {
        match self {
            Self::Inherit => (0, 0),
            Self::NewProcessGroup => (1, 0),
            Self::JoinProcessGroup(group) => (2, group),
            Self::NewSession => (3, 0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvironmentSpawn {
    pub exec: EnvironmentExec,
    pub placement: SpawnPlacement,
}

impl EnvironmentSpawn {
    pub const MAX_ENCODED_SIZE: usize = EnvironmentExec::MAX_ENCODED_SIZE + 8;

    pub const fn encoded_size(model: AbiDataModel) -> usize {
        EnvironmentExec::encoded_size(model) + 8
    }

    pub fn decode(model: AbiDataModel, bytes: &[u8]) -> Result<Self, EnvironmentExecError> {
        let base = EnvironmentExec::encoded_size(model);
        let size = model.read_u32(bytes, 0)? as usize;
        let placement = if size == base {
            SpawnPlacement::Inherit
        } else if size == Self::encoded_size(model) {
            match (
                model.read_u32(bytes, base)?,
                model.read_u32(bytes, base + 4)?,
            ) {
                (0, 0) => SpawnPlacement::Inherit,
                (1, 0) => SpawnPlacement::NewProcessGroup,
                (2, group) if group != 0 => SpawnPlacement::JoinProcessGroup(group),
                (3, 0) => SpawnPlacement::NewSession,
                _ => return Err(EnvironmentExecError::InvalidPlacement),
            }
        } else {
            return Err(EnvironmentExecError::UnsupportedSize);
        };
        Ok(Self {
            exec: EnvironmentExec::decode_sized(model, bytes, size)?,
            placement,
        })
    }
}
