//! Logical resource descriptors, validated resource tables, and branded references.

use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;
use core::ops::{BitOr, BitOrAssign};

use super::pipeline::RenderPipelineDesc;
use super::{Error, Extent2D, PixelRect, Result};

/// Maximum textures held by one [`ResourceTable`].
pub const MAX_TEXTURES: usize = 1_024;
/// Maximum buffers held by one [`ResourceTable`].
pub const MAX_BUFFERS: usize = 1_024;
/// Maximum samplers held by one [`ResourceTable`].
pub const MAX_SAMPLERS: usize = 256;
/// Maximum render pipelines held by one [`ResourceTable`].
pub const MAX_RENDER_PIPELINES: usize = 256;

/// Portable texture pixel format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    /// Eight-bit blue, green, red, and alpha channels.
    Bgra8Unorm,
    /// Eight-bit red, green, blue, and alpha channels.
    Rgba8Unorm,
    /// One eight-bit normalized red channel.
    R8Unorm,
}

impl TextureFormat {
    /// Return the number of bytes per tightly packed pixel.
    ///
    /// # Returns
    ///
    /// The portable byte size for this format.
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Bgra8Unorm | Self::Rgba8Unorm => 4,
            Self::R8Unorm => 1,
        }
    }
}

/// Bitflag-like allowed operations for a texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureUsage(u8);

impl TextureUsage {
    /// Allow shader sampling.
    pub const SAMPLED: Self = Self(1 << 0);
    /// Allow use as a render-pass color attachment.
    pub const RENDER_ATTACHMENT: Self = Self(1 << 1);
    /// Allow use as a texture copy source.
    pub const COPY_SRC: Self = Self(1 << 2);
    /// Allow use as a texture upload or copy destination.
    pub const COPY_DST: Self = Self(1 << 3);
    /// Allow eventual presentation by a backend.
    pub const PRESENT: Self = Self(1 << 4);

    /// Return no usage flags.
    ///
    /// # Returns
    ///
    /// An empty usage set.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Return whether all flags in `other` are present.
    ///
    /// # Arguments
    ///
    /// * `other` - Flags to test.
    ///
    /// # Returns
    ///
    /// `true` when every requested flag is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Combine this usage set with another.
    ///
    /// # Arguments
    ///
    /// * `other` - Flags to add.
    ///
    /// # Returns
    ///
    /// The union of both usage sets.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl BitOr for TextureUsage {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}
impl BitOrAssign for TextureUsage {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = self.union(rhs);
    }
}

/// Validated descriptor for a logical texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureDesc {
    format: TextureFormat,
    extent: Extent2D,
    usage: TextureUsage,
}

impl TextureDesc {
    /// Construct a texture descriptor.
    ///
    /// # Arguments
    ///
    /// * `format` - Pixel format.
    /// * `extent` - Non-zero texture dimensions.
    /// * `usage` - Allowed texture operations.
    ///
    /// # Returns
    ///
    /// A descriptor, or [`Error::InvalidDescriptor`] for empty usage.
    pub const fn new(format: TextureFormat, extent: Extent2D, usage: TextureUsage) -> Result<Self> {
        if usage.0 == 0 {
            Err(Error::InvalidDescriptor)
        } else {
            Ok(Self {
                format,
                extent,
                usage,
            })
        }
    }

    /// Return the pixel format.
    ///
    /// # Returns
    /// The texture format.
    pub const fn format(self) -> TextureFormat {
        self.format
    }
    /// Return the dimensions.
    ///
    /// # Returns
    /// The non-zero texture extent.
    pub const fn extent(self) -> Extent2D {
        self.extent
    }
    /// Return allowed operations.
    ///
    /// # Returns
    /// The texture usage flags.
    pub const fn usage(self) -> TextureUsage {
        self.usage
    }
}

/// Bitflag-like allowed operations for a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferUsage(u8);

impl BufferUsage {
    /// Allow use as a vertex buffer.
    pub const VERTEX: Self = Self(1 << 0);
    /// Allow use as an index buffer.
    pub const INDEX: Self = Self(1 << 1);
    /// Allow use as a copy source.
    pub const COPY_SRC: Self = Self(1 << 2);
    /// Allow writes through upload commands.
    pub const COPY_DST: Self = Self(1 << 3);
    /// Return no usage flags.
    ///
    /// # Returns
    /// An empty usage set.
    pub const fn empty() -> Self {
        Self(0)
    }
    /// Return whether all flags in `other` are present.
    ///
    /// # Arguments
    ///
    /// * `other` - Flags to test.
    ///
    /// # Returns
    /// `true` when every requested flag is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    /// Combine this usage set with another.
    ///
    /// # Arguments
    ///
    /// * `other` - Flags to add.
    ///
    /// # Returns
    /// The union of both usage sets.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl BitOr for BufferUsage {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}
impl BitOrAssign for BufferUsage {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = self.union(rhs);
    }
}

/// Validated descriptor for a logical buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferDesc {
    size: u64,
    usage: BufferUsage,
}

impl BufferDesc {
    /// Construct a buffer descriptor.
    ///
    /// # Arguments
    ///
    /// * `size` - Non-zero size in bytes.
    /// * `usage` - Allowed buffer operations.
    ///
    /// # Returns
    /// A descriptor, or [`Error::InvalidDescriptor`] for empty size or usage.
    pub const fn new(size: u64, usage: BufferUsage) -> Result<Self> {
        if size == 0 || usage.0 == 0 {
            Err(Error::InvalidDescriptor)
        } else {
            Ok(Self { size, usage })
        }
    }
    /// Return the size in bytes.
    ///
    /// # Returns
    /// The non-zero buffer size.
    pub const fn size(self) -> u64 {
        self.size
    }
    /// Return allowed operations.
    ///
    /// # Returns
    /// The buffer usage flags.
    pub const fn usage(self) -> BufferUsage {
        self.usage
    }
}

/// Texture filtering mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// Select the nearest texel.
    Nearest,
    /// Linearly filter neighboring texels.
    Linear,
}

/// Texture coordinate addressing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressMode {
    /// Clamp coordinates to the edge texel.
    ClampToEdge,
    /// Repeat the texture.
    Repeat,
    /// Repeat while mirroring every other copy.
    MirrorRepeat,
}

/// Portable logical sampler descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplerDesc {
    min_filter: FilterMode,
    mag_filter: FilterMode,
    address_u: AddressMode,
    address_v: AddressMode,
}

impl SamplerDesc {
    /// Construct a sampler descriptor.
    ///
    /// # Arguments
    ///
    /// * `min_filter` - Minification filter.
    /// * `mag_filter` - Magnification filter.
    /// * `address_u` - Horizontal addressing.
    /// * `address_v` - Vertical addressing.
    ///
    /// # Returns
    /// A portable sampler descriptor.
    pub const fn new(
        min_filter: FilterMode,
        mag_filter: FilterMode,
        address_u: AddressMode,
        address_v: AddressMode,
    ) -> Self {
        Self {
            min_filter,
            mag_filter,
            address_u,
            address_v,
        }
    }
    /// Return the minification filter.
    ///
    /// # Returns
    /// The configured filter.
    pub const fn min_filter(self) -> FilterMode {
        self.min_filter
    }
    /// Return the magnification filter.
    ///
    /// # Returns
    /// The configured filter.
    pub const fn mag_filter(self) -> FilterMode {
        self.mag_filter
    }
    /// Return horizontal addressing.
    ///
    /// # Returns
    /// The configured U-coordinate mode.
    pub const fn address_u(self) -> AddressMode {
        self.address_u
    }
    /// Return vertical addressing.
    ///
    /// # Returns
    /// The configured V-coordinate mode.
    pub const fn address_v(self) -> AddressMode {
        self.address_v
    }
}

/// Borrowed pixel data and layout for one texture upload.
#[derive(Debug, Clone, Copy)]
pub struct TextureWrite<'data> {
    destination: PixelRect,
    bytes_per_row: u32,
    data: &'data [u8],
}

impl<'data> TextureWrite<'data> {
    /// Construct a texture upload description.
    ///
    /// # Arguments
    ///
    /// * `destination` - Non-empty destination rectangle.
    /// * `bytes_per_row` - Source byte stride, validated against the texture format when recorded.
    /// * `data` - Borrowed source bytes.
    ///
    /// # Returns
    /// The upload description, or [`Error::InvalidValue`] for a zero stride.
    pub const fn new(
        destination: PixelRect,
        bytes_per_row: u32,
        data: &'data [u8],
    ) -> Result<Self> {
        if bytes_per_row == 0 {
            Err(Error::InvalidValue)
        } else {
            Ok(Self {
                destination,
                bytes_per_row,
                data,
            })
        }
    }
    /// Return the destination rectangle.
    ///
    /// # Returns
    /// The upload destination.
    pub const fn destination(self) -> PixelRect {
        self.destination
    }
    /// Return the source row stride.
    ///
    /// # Returns
    /// Bytes between source row starts.
    pub const fn bytes_per_row(self) -> u32 {
        self.bytes_per_row
    }
    /// Return the borrowed source bytes.
    ///
    /// # Returns
    /// The source byte slice.
    pub const fn data(self) -> &'data [u8] {
        self.data
    }
}

/// Table that owns validated logical resource descriptors.
pub struct ResourceTable {
    textures: RefCell<Vec<TextureDesc>>,
    buffers: RefCell<Vec<BufferDesc>>,
    samplers: RefCell<Vec<SamplerDesc>>,
    pipelines: RefCell<Vec<RenderPipelineDesc>>,
}

impl ResourceTable {
    /// Construct an empty resource table.
    ///
    /// # Returns
    /// An empty table with bounded resource categories.
    pub const fn new() -> Self {
        Self {
            textures: RefCell::new(Vec::new()),
            buffers: RefCell::new(Vec::new()),
            samplers: RefCell::new(Vec::new()),
            pipelines: RefCell::new(Vec::new()),
        }
    }

    /// Define a texture descriptor and return its table-branded reference.
    ///
    /// # Arguments
    ///
    /// * `desc` - Validated texture descriptor.
    ///
    /// # Returns
    /// A texture reference, or a bounded-allocation error.
    pub fn define_texture(&self, desc: TextureDesc) -> Result<TextureRef<'_>> {
        let index = Self::push(&self.textures, desc, MAX_TEXTURES)?;
        Ok(TextureRef { owner: self, index })
    }
    /// Define a buffer descriptor and return its table-branded reference.
    ///
    /// # Arguments
    ///
    /// * `desc` - Validated buffer descriptor.
    ///
    /// # Returns
    /// A buffer reference, or a bounded-allocation error.
    pub fn define_buffer(&self, desc: BufferDesc) -> Result<BufferRef<'_>> {
        let index = Self::push(&self.buffers, desc, MAX_BUFFERS)?;
        Ok(BufferRef { owner: self, index })
    }
    /// Define a sampler descriptor and return its table-branded reference.
    ///
    /// # Arguments
    ///
    /// * `desc` - Sampler descriptor.
    ///
    /// # Returns
    /// A sampler reference, or a bounded-allocation error.
    pub fn define_sampler(&self, desc: SamplerDesc) -> Result<SamplerRef<'_>> {
        let index = Self::push(&self.samplers, desc, MAX_SAMPLERS)?;
        Ok(SamplerRef { owner: self, index })
    }
    /// Define a render-pipeline descriptor and return its table-branded reference.
    ///
    /// # Arguments
    ///
    /// * `desc` - Validated owned pipeline descriptor.
    ///
    /// # Returns
    /// A pipeline reference, or a bounded-allocation error.
    pub fn define_render_pipeline(
        &self,
        desc: RenderPipelineDesc,
    ) -> Result<RenderPipelineRef<'_>> {
        let index = Self::push(&self.pipelines, desc, MAX_RENDER_PIPELINES)?;
        Ok(RenderPipelineRef { owner: self, index })
    }

    fn push<T>(items: &RefCell<Vec<T>>, value: T, maximum: usize) -> Result<usize> {
        let mut items = items.borrow_mut();
        if items.len() >= maximum {
            return Err(Error::ResourceLimitExceeded);
        }
        items.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
        let index = items.len();
        items.push(value);
        Ok(index)
    }
    pub(crate) fn texture(&self, reference: TextureRef<'_>) -> Result<TextureDesc> {
        if !core::ptr::eq(reference.owner, self) {
            return Err(Error::ResourceTableMismatch);
        }
        self.textures
            .borrow()
            .get(reference.index)
            .copied()
            .ok_or(Error::InvalidDescriptor)
    }
    pub(crate) fn buffer(&self, reference: BufferRef<'_>) -> Result<BufferDesc> {
        if !core::ptr::eq(reference.owner, self) {
            return Err(Error::ResourceTableMismatch);
        }
        self.buffers
            .borrow()
            .get(reference.index)
            .copied()
            .ok_or(Error::InvalidDescriptor)
    }
    pub(crate) fn sampler(&self, reference: SamplerRef<'_>) -> Result<SamplerDesc> {
        if !core::ptr::eq(reference.owner, self) {
            return Err(Error::ResourceTableMismatch);
        }
        self.samplers
            .borrow()
            .get(reference.index)
            .copied()
            .ok_or(Error::InvalidDescriptor)
    }
    /// Borrow a validated pipeline descriptor without cloning its owned layout.
    pub(crate) fn with_pipeline<T>(
        &self,
        reference: RenderPipelineRef<'_>,
        access: impl FnOnce(&RenderPipelineDesc) -> T,
    ) -> Result<T> {
        if !core::ptr::eq(reference.owner, self) {
            return Err(Error::ResourceTableMismatch);
        }
        let pipelines = self.pipelines.borrow();
        let descriptor = pipelines
            .get(reference.index)
            .ok_or(Error::InvalidDescriptor)?;
        Ok(access(descriptor))
    }
    pub(crate) fn same_texture(&self, left: TextureRef<'_>, right: TextureRef<'_>) -> Result<bool> {
        self.texture(left)?;
        self.texture(right)?;
        Ok(left.index == right.index)
    }
}

impl Default for ResourceTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Reference to a texture retained by one [`ResourceTable`].
#[derive(Clone, Copy)]
pub struct TextureRef<'r> {
    pub(crate) owner: &'r ResourceTable,
    pub(crate) index: usize,
}
/// Reference to a buffer retained by one [`ResourceTable`].
#[derive(Clone, Copy)]
pub struct BufferRef<'r> {
    pub(crate) owner: &'r ResourceTable,
    pub(crate) index: usize,
}
/// Reference to a sampler retained by one [`ResourceTable`].
#[derive(Clone, Copy)]
pub struct SamplerRef<'r> {
    pub(crate) owner: &'r ResourceTable,
    pub(crate) index: usize,
}
/// Reference to a render pipeline retained by one [`ResourceTable`].
#[derive(Clone, Copy)]
pub struct RenderPipelineRef<'r> {
    pub(crate) owner: &'r ResourceTable,
    pub(crate) index: usize,
}

macro_rules! impl_resource_ref_traits {
    ($reference:ident) => {
        impl fmt::Debug for $reference<'_> {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($reference), "(..)"))
            }
        }

        impl PartialEq for $reference<'_> {
            fn eq(&self, other: &Self) -> bool {
                core::ptr::eq(self.owner, other.owner) && self.index == other.index
            }
        }

        impl Eq for $reference<'_> {}
    };
}

impl_resource_ref_traits!(TextureRef);
impl_resource_ref_traits!(BufferRef);
impl_resource_ref_traits!(SamplerRef);
impl_resource_ref_traits!(RenderPipelineRef);
