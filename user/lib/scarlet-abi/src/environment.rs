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
        if model.read_u32(bytes, 0)? as usize != Self::encoded_size(model) {
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
