//! Scalar representation at an ABI boundary, independent of the build host.
//!
//! A model describes pointer/register width and byte order, not a calling
//! convention. Register allocation, wide register pairs, structure alignment,
//! error conventions and accessible virtual-address limits belong to the ABI
//! adapter. In particular, a representable address is not permission to access it.

/// Width of a pointer or a register word in the selected ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordWidth {
    Bits32,
    Bits64,
}

impl WordWidth {
    pub const fn bytes(self) -> usize {
        match self {
            Self::Bits32 => 4,
            Self::Bits64 => 8,
        }
    }

    pub const fn max_unsigned(self) -> u64 {
        match self {
            Self::Bits32 => u32::MAX as u64,
            Self::Bits64 => u64::MAX,
        }
    }

    fn check(self, value: u64) -> Result<u64, AbiDataError> {
        if value > self.max_unsigned() {
            Err(AbiDataError::ValueOutOfRange)
        } else {
            Ok(value)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    Little,
    Big,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiDataError {
    BufferTooShort,
    ValueOutOfRange,
    AddressOverflow,
}

/// An unsigned register bit pattern with an explicit signed interpretation.
/// It cannot implicitly become a pointer or a file offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterWord {
    bits: u64,
    width: WordWidth,
}

impl RegisterWord {
    pub const fn unsigned(self) -> u64 {
        self.bits
    }

    /// Sign-extend from the ABI's width, not the build host's width.
    pub const fn signed(self) -> i64 {
        match self.width {
            WordWidth::Bits32 => self.bits as u32 as i32 as i64,
            WordWidth::Bits64 => self.bits as i64,
        }
    }

    pub fn to_usize(self) -> Result<usize, AbiDataError> {
        usize::try_from(self.bits).map_err(|_| AbiDataError::ValueOutOfRange)
    }
}

/// A numerically representable user address; no mapping or access is implied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserAddress {
    value: u64,
    width: WordWidth,
}

impl UserAddress {
    pub const fn value(self) -> u64 {
        self.value
    }

    pub const fn is_null(self) -> bool {
        self.value == 0
    }

    /// Check again when entering the host's address space (e.g. a VM lookup).
    pub fn to_usize(self) -> Result<usize, AbiDataError> {
        usize::try_from(self.value).map_err(|_| AbiDataError::ValueOutOfRange)
    }

    pub fn checked_add(self, offset: u64) -> Result<Self, AbiDataError> {
        let value = self
            .value
            .checked_add(offset)
            .filter(|value| *value <= self.width.max_unsigned())
            .ok_or(AbiDataError::AddressOverflow)?;
        Ok(Self { value, ..self })
    }

    pub fn element(self, index: u64, stride: u64) -> Result<Self, AbiDataError> {
        let offset = index
            .checked_mul(stride)
            .ok_or(AbiDataError::AddressOverflow)?;
        self.checked_add(offset)
    }

    /// Validate every addressed byte. Empty ranges are allowed, including at
    /// zero. A one-byte range at the largest address is representable; no
    /// unrepresentable one-past-end pointer needs to be formed.
    pub fn check_range(self, byte_len: u64) -> Result<(), AbiDataError> {
        if byte_len != 0 {
            self.checked_add(byte_len - 1)?;
        }
        Ok(())
    }
}

/// The pointer/register data model selected by an ABI adapter.
/// Mixed pointer/register widths require a separate adapter; this is not an
/// assertion that all possible calling conventions have equal widths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbiDataModel {
    pub word_width: WordWidth,
    pub byte_order: ByteOrder,
}

impl AbiDataModel {
    pub const fn new(word_width: WordWidth, byte_order: ByteOrder) -> Self {
        Self {
            word_width,
            byte_order,
        }
    }

    /// Only for the current same-width native ABI. A future compatibility ABI
    /// must select its caller's model explicitly instead of using this constant.
    pub const NATIVE: Self = Self::new(
        if usize::BITS == 32 {
            WordWidth::Bits32
        } else {
            WordWidth::Bits64
        },
        if cfg!(target_endian = "little") {
            ByteOrder::Little
        } else {
            ByteOrder::Big
        },
    );

    pub fn register(self, bits: u64) -> Result<RegisterWord, AbiDataError> {
        Ok(RegisterWord {
            bits: self.word_width.check(bits)?,
            width: self.word_width,
        })
    }

    /// Encode a signed register argument/result without silently truncating it.
    pub fn signed_register(self, value: i64) -> Result<RegisterWord, AbiDataError> {
        let bits = match self.word_width {
            WordWidth::Bits32 => {
                i32::try_from(value).map_err(|_| AbiDataError::ValueOutOfRange)? as u32 as u64
            }
            WordWidth::Bits64 => value as u64,
        };
        self.register(bits)
    }

    /// Also validates fixed-width pointer slots (e.g. a u64 in a 32-bit ABI).
    pub fn user_address(self, value: u64) -> Result<UserAddress, AbiDataError> {
        Ok(UserAddress {
            value: self.word_width.check(value)?,
            width: self.word_width,
        })
    }

    pub fn read_word(self, bytes: &[u8], offset: usize) -> Result<RegisterWord, AbiDataError> {
        let bits = match self.word_width {
            WordWidth::Bits32 => self.read_u32(bytes, offset)? as u64,
            WordWidth::Bits64 => self.read_u64(bytes, offset)?,
        };
        self.register(bits)
    }

    pub fn write_word(
        self,
        bytes: &mut [u8],
        offset: usize,
        bits: u64,
    ) -> Result<(), AbiDataError> {
        self.word_width.check(bits)?;
        match self.word_width {
            WordWidth::Bits32 => self.write_u32(bytes, offset, bits as u32),
            WordWidth::Bits64 => self.write_u64(bytes, offset, bits),
        }
    }

    pub fn read_u32(self, bytes: &[u8], offset: usize) -> Result<u32, AbiDataError> {
        let raw = read_bytes(bytes, offset)?;
        Ok(match self.byte_order {
            ByteOrder::Little => u32::from_le_bytes(raw),
            ByteOrder::Big => u32::from_be_bytes(raw),
        })
    }

    /// Fixed-width data (time, counters, file sizes) does not shrink with words.
    pub fn read_u64(self, bytes: &[u8], offset: usize) -> Result<u64, AbiDataError> {
        let raw = read_bytes(bytes, offset)?;
        Ok(match self.byte_order {
            ByteOrder::Little => u64::from_le_bytes(raw),
            ByteOrder::Big => u64::from_be_bytes(raw),
        })
    }

    /// Fixed-width signed data, including file offsets, independent of words.
    pub fn read_i64(self, bytes: &[u8], offset: usize) -> Result<i64, AbiDataError> {
        self.read_u64(bytes, offset).map(|bits| bits as i64)
    }

    pub fn write_u32(
        self,
        bytes: &mut [u8],
        offset: usize,
        value: u32,
    ) -> Result<(), AbiDataError> {
        let raw = match self.byte_order {
            ByteOrder::Little => value.to_le_bytes(),
            ByteOrder::Big => value.to_be_bytes(),
        };
        write_bytes(bytes, offset, &raw)
    }

    pub fn write_u64(
        self,
        bytes: &mut [u8],
        offset: usize,
        value: u64,
    ) -> Result<(), AbiDataError> {
        let raw = match self.byte_order {
            ByteOrder::Little => value.to_le_bytes(),
            ByteOrder::Big => value.to_be_bytes(),
        };
        write_bytes(bytes, offset, &raw)
    }

    pub fn write_i64(
        self,
        bytes: &mut [u8],
        offset: usize,
        value: i64,
    ) -> Result<(), AbiDataError> {
        self.write_u64(bytes, offset, value as u64)
    }
}

// Copy bytes rather than dereferencing a typed pointer: callers may pass an
// unaligned user record, and its alignment need not match the build host.
fn read_bytes<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], AbiDataError> {
    let end = offset.checked_add(N).ok_or(AbiDataError::BufferTooShort)?;
    let slice = bytes.get(offset..end).ok_or(AbiDataError::BufferTooShort)?;
    let mut raw = [0; N];
    raw.copy_from_slice(slice);
    Ok(raw)
}

fn write_bytes(bytes: &mut [u8], offset: usize, raw: &[u8]) -> Result<(), AbiDataError> {
    let end = offset
        .checked_add(raw.len())
        .ok_or(AbiDataError::BufferTooShort)?;
    let slice = bytes
        .get_mut(offset..end)
        .ok_or(AbiDataError::BufferTooShort)?;
    slice.copy_from_slice(raw);
    Ok(())
}

const _: () = assert!(usize::BITS == 32 || usize::BITS == 64);
