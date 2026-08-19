//! Value types and validation errors used by the graphics IR.

/// Result type returned by graphics IR operations.
pub type Result<T> = core::result::Result<T, Error>;

/// Reason a graphics IR descriptor or command was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A value was non-finite, zero where non-zero is required, or otherwise malformed.
    InvalidValue,
    /// A descriptor does not describe a portable logical graphics object.
    InvalidDescriptor,
    /// A resource's required usage flag is absent.
    InvalidUsage,
    /// A rectangle, byte range, or index range exceeds its resource.
    OutOfBounds,
    /// Arithmetic needed to validate a range overflowed.
    Overflow,
    /// A resource reference belongs to a different resource table.
    ResourceTableMismatch,
    /// A render pass command was used outside an active render pass.
    RenderPassNotActive,
    /// A copy or upload command was used while a render pass is active.
    RenderPassActive,
    /// A command requires a render pipeline that has not been set.
    PipelineNotSet,
    /// A command requires a vertex buffer that has not been set.
    VertexBufferNotSet,
    /// A command requires an index buffer that has not been set.
    IndexBufferNotSet,
    /// A draw requires uniforms that have not been set.
    UniformsNotSet,
    /// A textured pipeline requires both a texture and sampler binding.
    TextureBindingNotSet,
    /// A render pipeline's target format differs from the active attachment.
    PipelineTargetMismatch,
    /// Sampling the active render attachment would create feedback.
    AttachmentFeedback,
    /// A bounded resource table is full.
    ResourceLimitExceeded,
    /// A bounded command buffer is full.
    CommandLimitExceeded,
    /// Allocation for IR-owned storage failed.
    OutOfMemory,
    /// A command buffer was finished while a render pass remained open.
    RenderPassStillActive,
}

/// A finite RGBA color represented with floating-point components.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    red: f32,
    green: f32,
    blue: f32,
    alpha: f32,
}

impl Color {
    /// Construct a finite RGBA color.
    ///
    /// # Arguments
    ///
    /// * `red` - Red component.
    /// * `green` - Green component.
    /// * `blue` - Blue component.
    /// * `alpha` - Alpha component.
    ///
    /// # Returns
    ///
    /// The color, or [`Error::InvalidValue`] when any component is non-finite.
    pub fn rgba(red: f32, green: f32, blue: f32, alpha: f32) -> Result<Self> {
        if [red, green, blue, alpha]
            .iter()
            .all(|value| value.is_finite())
        {
            Ok(Self {
                red,
                green,
                blue,
                alpha,
            })
        } else {
            Err(Error::InvalidValue)
        }
    }

    /// Return the RGBA components in order.
    ///
    /// # Returns
    ///
    /// `[red, green, blue, alpha]`.
    pub const fn components(self) -> [f32; 4] {
        [self.red, self.green, self.blue, self.alpha]
    }
}

/// Non-zero two-dimensional pixel extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent2D {
    width: u32,
    height: u32,
}

impl Extent2D {
    /// Construct a non-zero pixel extent.
    ///
    /// # Arguments
    ///
    /// * `width` - Width in pixels.
    /// * `height` - Height in pixels.
    ///
    /// # Returns
    ///
    /// The extent, or [`Error::InvalidValue`] for a zero dimension.
    pub const fn new(width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 {
            Err(Error::InvalidValue)
        } else {
            Ok(Self { width, height })
        }
    }

    /// Return the width in pixels.
    ///
    /// # Returns
    ///
    /// The non-zero width.
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Return the height in pixels.
    ///
    /// # Returns
    ///
    /// The non-zero height.
    pub const fn height(self) -> u32 {
        self.height
    }
}

/// A non-empty top-left pixel rectangle with overflow-safe bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl PixelRect {
    /// Construct a non-empty pixel rectangle.
    ///
    /// # Arguments
    ///
    /// * `x` - Left pixel coordinate.
    /// * `y` - Top pixel coordinate.
    /// * `width` - Non-zero width in pixels.
    /// * `height` - Non-zero height in pixels.
    ///
    /// # Returns
    ///
    /// The rectangle, or [`Error::InvalidValue`] when it is empty or overflows.
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Result<Self> {
        if width == 0
            || height == 0
            || x.checked_add(width).is_none()
            || y.checked_add(height).is_none()
        {
            Err(Error::InvalidValue)
        } else {
            Ok(Self {
                x,
                y,
                width,
                height,
            })
        }
    }

    /// Return the left coordinate.
    ///
    /// # Returns
    ///
    /// The left pixel coordinate.
    pub const fn x(self) -> u32 {
        self.x
    }

    /// Return the top coordinate.
    ///
    /// # Returns
    ///
    /// The top pixel coordinate.
    pub const fn y(self) -> u32 {
        self.y
    }

    /// Return the width in pixels.
    ///
    /// # Returns
    ///
    /// The non-zero rectangle width.
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Return the height in pixels.
    ///
    /// # Returns
    ///
    /// The non-zero rectangle height.
    pub const fn height(self) -> u32 {
        self.height
    }

    /// Return whether this rectangle lies wholly within an extent.
    ///
    /// # Arguments
    ///
    /// * `extent` - Bounds to test.
    ///
    /// # Returns
    ///
    /// `true` when all pixels are inside `extent`.
    pub const fn is_within(self, extent: Extent2D) -> bool {
        self.x + self.width <= extent.width && self.y + self.height <= extent.height
    }

    /// Return whether another rectangle has the same width and height.
    ///
    /// # Arguments
    ///
    /// * `other` - Rectangle whose extent should be compared.
    ///
    /// # Returns
    ///
    /// `true` when both extents are equal, regardless of their origins.
    pub const fn same_extent(self, other: Self) -> bool {
        self.width == other.width && self.height == other.height
    }
}

/// A finite general 4×4 transform matrix in column-major order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    columns: [f32; 16],
}

impl Transform {
    /// Construct a finite column-major 4×4 transform.
    ///
    /// # Arguments
    ///
    /// * `columns` - Sixteen matrix elements in column-major order.
    ///
    /// # Returns
    ///
    /// The transform, or [`Error::InvalidValue`] for a non-finite element.
    pub fn from_columns(columns: [f32; 16]) -> Result<Self> {
        if columns.iter().all(|value| value.is_finite()) {
            Ok(Self { columns })
        } else {
            Err(Error::InvalidValue)
        }
    }

    /// Construct the identity transform.
    ///
    /// # Returns
    ///
    /// A finite identity matrix.
    pub const fn identity() -> Self {
        Self {
            columns: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    /// Return the column-major matrix elements.
    ///
    /// # Returns
    ///
    /// The sixteen finite matrix elements.
    pub const fn columns(self) -> [f32; 16] {
        self.columns
    }
}
