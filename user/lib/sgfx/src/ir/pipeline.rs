//! Portable render-pipeline descriptors and draw state.

use alloc::vec::Vec;

use super::{Color, Error, Result, TextureFormat, Transform};

/// Maximum attributes accepted by one portable vertex-buffer layout.
pub const MAX_VERTEX_ATTRIBUTES: usize = 16;

/// Comparison applied between an incoming fragment depth and stored depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareFunction {
    /// Never pass the comparison.
    Never,
    /// Pass when the incoming value is less than the stored value.
    Less,
    /// Pass when both values are equal.
    Equal,
    /// Pass when the incoming value is less than or equal to the stored value.
    LessEqual,
    /// Pass when the incoming value is greater than the stored value.
    Greater,
    /// Pass when the values are not equal.
    NotEqual,
    /// Pass when the incoming value is greater than or equal to the stored value.
    GreaterEqual,
    /// Always pass the comparison.
    Always,
}

/// Portable depth-test and depth-write pipeline state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepthState {
    format: TextureFormat,
    compare: CompareFunction,
    write_enabled: bool,
}

impl DepthState {
    /// Construct depth-test and depth-write state.
    ///
    /// # Arguments
    ///
    /// * `format` - Required render-pass depth attachment format.
    /// * `compare` - Comparison applied to incoming fragment depth.
    /// * `write_enabled` - Whether passing fragments update stored depth.
    ///
    /// # Returns
    /// The requested portable depth state.
    pub const fn new(format: TextureFormat, compare: CompareFunction, write_enabled: bool) -> Self {
        Self {
            format,
            compare,
            write_enabled,
        }
    }

    /// Return the required depth attachment format.
    ///
    /// # Returns
    /// The configured depth texture format.
    pub const fn format(self) -> TextureFormat {
        self.format
    }

    /// Return the depth comparison function.
    ///
    /// # Returns
    /// The configured comparison.
    pub const fn compare(self) -> CompareFunction {
        self.compare
    }

    /// Return whether passing fragments write depth.
    ///
    /// # Returns
    /// `true` when depth writes are enabled.
    pub const fn write_enabled(self) -> bool {
        self.write_enabled
    }
}

/// Format of one vertex attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexFormat {
    /// Two 32-bit floating-point components.
    Float32x2,
    /// Three 32-bit floating-point components.
    Float32x3,
    /// Four 32-bit floating-point components.
    Float32x4,
    /// Four normalized unsigned 8-bit components.
    Unorm8x4,
}

impl VertexFormat {
    /// Return the byte size of one attribute value.
    ///
    /// # Returns
    ///
    /// The packed format size in bytes.
    pub const fn byte_size(self) -> u32 {
        match self {
            Self::Float32x2 => 8,
            Self::Float32x3 => 12,
            Self::Float32x4 => 16,
            Self::Unorm8x4 => 4,
        }
    }
}

/// One attribute within an interleaved vertex buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexAttribute {
    location: u32,
    format: VertexFormat,
    offset: u32,
}

impl VertexAttribute {
    /// Construct a vertex attribute.
    ///
    /// # Arguments
    ///
    /// * `location` - Shader-visible attribute location.
    /// * `format` - Packed attribute format.
    /// * `offset` - Byte offset within one vertex.
    ///
    /// # Returns
    /// The attribute; layout construction validates its range.
    pub const fn new(location: u32, format: VertexFormat, offset: u32) -> Self {
        Self {
            location,
            format,
            offset,
        }
    }
    /// Return the shader-visible location.
    ///
    /// # Returns
    /// The location number.
    pub const fn location(self) -> u32 {
        self.location
    }
    /// Return the packed format.
    ///
    /// # Returns
    /// The attribute format.
    pub const fn format(self) -> VertexFormat {
        self.format
    }
    /// Return the byte offset within each vertex.
    ///
    /// # Returns
    /// The byte offset.
    pub const fn offset(self) -> u32 {
        self.offset
    }
}

/// Owned validated description of one interleaved vertex buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexBufferLayout {
    stride: u32,
    attributes: Vec<VertexAttribute>,
}

impl VertexBufferLayout {
    /// Construct a validated interleaved vertex-buffer layout.
    ///
    /// # Arguments
    ///
    /// * `stride` - Non-zero bytes between consecutive vertices.
    /// * `attributes` - Owned attributes contained in every vertex.
    ///
    /// # Returns
    /// The layout, or [`Error::InvalidDescriptor`] for empty or over-limit
    /// attributes, duplicate locations, overflowing offsets, or attributes
    /// beyond `stride`.
    pub fn new(stride: u32, attributes: Vec<VertexAttribute>) -> Result<Self> {
        if stride == 0 || attributes.is_empty() || attributes.len() > MAX_VERTEX_ATTRIBUTES {
            return Err(Error::InvalidDescriptor);
        }
        for (index, attribute) in attributes.iter().enumerate() {
            if attribute
                .offset
                .checked_add(attribute.format.byte_size())
                .is_none_or(|end| end > stride)
                || attributes[..index]
                    .iter()
                    .any(|other| other.location == attribute.location)
            {
                return Err(Error::InvalidDescriptor);
            }
        }
        Ok(Self { stride, attributes })
    }

    /// Return the interleaved byte stride.
    ///
    /// # Returns
    /// The non-zero stride.
    pub const fn stride(&self) -> u32 {
        self.stride
    }
    /// Return the owned-layout attributes.
    ///
    /// # Returns
    /// Attributes in their declared order.
    pub fn attributes(&self) -> &[VertexAttribute] {
        &self.attributes
    }
}

/// Primitive assembly topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveTopology {
    /// Assemble independent triangles.
    TriangleList,
}

/// Format of indices in an index buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexFormat {
    /// Unsigned 16-bit indices.
    Uint16,
    /// Unsigned 32-bit indices.
    Uint32,
}

impl IndexFormat {
    /// Return the byte size of one index.
    ///
    /// # Returns
    /// The encoded index size in bytes.
    pub const fn byte_size(self) -> u64 {
        match self {
            Self::Uint16 => 2,
            Self::Uint32 => 4,
        }
    }
}

/// Portable fragment operation supported by the logical IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentProgram {
    /// Emit the draw uniform color.
    Solid,
    /// Emit interpolated vertex color modulated by the draw uniform color.
    VertexColor,
    /// Sample one texture using the specified alpha interpretation.
    Texture(TextureSampleMode),
    /// Sample one texture and modulate it by interpolated vertex color.
    ///
    /// Location `1` supplies the color and location `2` supplies the texture
    /// coordinates. The draw uniform color is applied after both values.
    TextureVertexColor(TextureSampleMode),
}

/// Alpha interpretation for a sampled texture fragment program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureSampleMode {
    /// Use sampled red, green, blue, and alpha components.
    Rgba,
    /// Use sampled RGB and treat alpha as one.
    RgbIgnoreAlpha,
    /// Use sampled alpha as a coverage mask for uniform color.
    AlphaMask,
}

/// Per-draw uniforms shared by the portable fragment programs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawUniforms {
    transform: Transform,
    color: Color,
}

impl DrawUniforms {
    /// Construct per-draw transform and color uniforms.
    ///
    /// # Arguments
    ///
    /// * `transform` - Finite general 4×4 transform.
    /// * `color` - Finite color multiplier.
    ///
    /// # Returns
    /// The requested uniforms.
    pub const fn new(transform: Transform, color: Color) -> Self {
        Self { transform, color }
    }
    /// Return the transform.
    ///
    /// # Returns
    /// The general draw transform.
    pub const fn transform(self) -> Transform {
        self.transform
    }
    /// Return the color multiplier.
    ///
    /// # Returns
    /// The finite draw color.
    pub const fn color(self) -> Color {
        self.color
    }
}

/// Source or destination factor in a blend component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendFactor {
    /// Factor zero.
    Zero,
    /// Factor one.
    One,
    /// Source alpha factor.
    SourceAlpha,
    /// One minus source alpha factor.
    OneMinusSourceAlpha,
    /// Destination alpha factor.
    DestinationAlpha,
    /// One minus destination alpha factor.
    OneMinusDestinationAlpha,
}

/// Arithmetic operation for a blend component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendOp {
    /// Add source and destination terms.
    Add,
    /// Subtract destination from source.
    Subtract,
    /// Subtract source from destination.
    ReverseSubtract,
}

/// Blend factors and operation for color or alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlendComponent {
    source_factor: BlendFactor,
    destination_factor: BlendFactor,
    operation: BlendOp,
}

impl BlendComponent {
    /// Construct one blend component.
    ///
    /// # Arguments
    ///
    /// * `source_factor` - Multiplier for source output.
    /// * `destination_factor` - Multiplier for destination output.
    /// * `operation` - Arithmetic operation.
    ///
    /// # Returns
    /// The requested blend component.
    pub const fn new(
        source_factor: BlendFactor,
        destination_factor: BlendFactor,
        operation: BlendOp,
    ) -> Self {
        Self {
            source_factor,
            destination_factor,
            operation,
        }
    }
    /// Return the source multiplier.
    ///
    /// # Returns
    /// The configured source factor.
    pub const fn source_factor(self) -> BlendFactor {
        self.source_factor
    }
    /// Return the destination multiplier.
    ///
    /// # Returns
    /// The configured destination factor.
    pub const fn destination_factor(self) -> BlendFactor {
        self.destination_factor
    }
    /// Return the blend arithmetic operation.
    ///
    /// # Returns
    /// The configured operation.
    pub const fn operation(self) -> BlendOp {
        self.operation
    }
}

/// Complete independent color and alpha blend configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlendState {
    color: BlendComponent,
    alpha: BlendComponent,
}

impl BlendState {
    /// Replace destination color and alpha with source output.
    pub const REPLACE: Self = Self {
        color: BlendComponent::new(BlendFactor::One, BlendFactor::Zero, BlendOp::Add),
        alpha: BlendComponent::new(BlendFactor::One, BlendFactor::Zero, BlendOp::Add),
    };
    /// Source-over blending for straight-alpha source RGB.
    pub const SOURCE_OVER_STRAIGHT_ALPHA: Self = Self {
        color: BlendComponent::new(
            BlendFactor::SourceAlpha,
            BlendFactor::OneMinusSourceAlpha,
            BlendOp::Add,
        ),
        alpha: BlendComponent::new(
            BlendFactor::One,
            BlendFactor::OneMinusSourceAlpha,
            BlendOp::Add,
        ),
    };
    /// Destination-in masking by source alpha.
    pub const DESTINATION_IN: Self = Self {
        color: BlendComponent::new(BlendFactor::Zero, BlendFactor::SourceAlpha, BlendOp::Add),
        alpha: BlendComponent::new(BlendFactor::Zero, BlendFactor::SourceAlpha, BlendOp::Add),
    };

    /// Construct a blend state from independent color and alpha components.
    ///
    /// # Arguments
    ///
    /// * `color` - RGB blend component.
    /// * `alpha` - Alpha blend component.
    ///
    /// # Returns
    /// The requested blend state.
    pub const fn new(color: BlendComponent, alpha: BlendComponent) -> Self {
        Self { color, alpha }
    }
    /// Return the RGB blend component.
    ///
    /// # Returns
    /// The color component.
    pub const fn color(self) -> BlendComponent {
        self.color
    }
    /// Return the alpha blend component.
    ///
    /// # Returns
    /// The alpha component.
    pub const fn alpha(self) -> BlendComponent {
        self.alpha
    }
}

/// Triangle faces discarded by rasterization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CullMode {
    /// Rasterize both faces.
    None,
    /// Discard front-facing triangles.
    Front,
    /// Discard back-facing triangles.
    Back,
}

/// Winding direction treated as front-facing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontFace {
    /// Clockwise winding is front-facing.
    Clockwise,
    /// Counter-clockwise winding is front-facing.
    CounterClockwise,
}

/// Rasterization state for one render pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RasterState {
    cull_mode: CullMode,
    front_face: FrontFace,
}

impl RasterState {
    /// Construct rasterization state.
    ///
    /// # Arguments
    ///
    /// * `cull_mode` - Faces to discard.
    /// * `front_face` - Winding treated as front-facing.
    ///
    /// # Returns
    /// The requested raster state.
    pub const fn new(cull_mode: CullMode, front_face: FrontFace) -> Self {
        Self {
            cull_mode,
            front_face,
        }
    }
    /// Return the face-culling mode.
    ///
    /// # Returns
    /// The configured culling mode.
    pub const fn cull_mode(self) -> CullMode {
        self.cull_mode
    }
    /// Return the front-face winding.
    ///
    /// # Returns
    /// The configured front-face winding.
    pub const fn front_face(self) -> FrontFace {
        self.front_face
    }
}

/// Owned portable descriptor for a render pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderPipelineDesc {
    target_format: TextureFormat,
    topology: PrimitiveTopology,
    vertex_buffer: VertexBufferLayout,
    fragment: FragmentProgram,
    blend: BlendState,
    raster: RasterState,
    depth: Option<DepthState>,
}

impl RenderPipelineDesc {
    /// Construct an owned render-pipeline descriptor.
    ///
    /// # Arguments
    ///
    /// * `target_format` - Required color attachment format.
    /// * `topology` - Primitive topology.
    /// * `vertex_buffer` - Owned interleaved vertex layout.
    /// * `fragment` - Portable fragment program.
    /// * `blend` - Color and alpha blend state.
    /// * `raster` - Face and winding state.
    ///
    /// # Returns
    /// The requested owned descriptor, or [`Error::InvalidDescriptor`] when
    /// portable vertex conventions are absent. Location `0` is position and
    /// must use `Float32x2`, `Float32x3`, or `Float32x4`. `VertexColor`
    /// requires location `1` as `Float32x3`, `Float32x4`, or `Unorm8x4`; `Texture` requires
    /// location `1` as `Float32x2` texture coordinates; `TextureVertexColor` requires
    /// location `1` as color and location `2` as `Float32x2` texture coordinates.
    pub fn new(
        target_format: TextureFormat,
        topology: PrimitiveTopology,
        vertex_buffer: VertexBufferLayout,
        fragment: FragmentProgram,
        blend: BlendState,
        raster: RasterState,
    ) -> Result<Self> {
        let position = vertex_buffer
            .attributes()
            .iter()
            .find(|attribute| attribute.location() == 0)
            .map(|attribute| attribute.format());
        if !matches!(
            position,
            Some(VertexFormat::Float32x2 | VertexFormat::Float32x3 | VertexFormat::Float32x4)
        ) {
            return Err(Error::InvalidDescriptor);
        }
        let location_one = vertex_buffer
            .attributes()
            .iter()
            .find(|attribute| attribute.location() == 1)
            .map(|attribute| attribute.format());
        let location_two = vertex_buffer
            .attributes()
            .iter()
            .find(|attribute| attribute.location() == 2)
            .map(|attribute| attribute.format());
        let fragment_attributes_are_valid = match fragment {
            FragmentProgram::Solid => true,
            FragmentProgram::VertexColor => matches!(
                location_one,
                Some(VertexFormat::Float32x3 | VertexFormat::Float32x4 | VertexFormat::Unorm8x4)
            ),
            FragmentProgram::Texture(_) => matches!(location_one, Some(VertexFormat::Float32x2)),
            FragmentProgram::TextureVertexColor(_) => {
                matches!(
                    location_one,
                    Some(
                        VertexFormat::Float32x3 | VertexFormat::Float32x4 | VertexFormat::Unorm8x4
                    )
                ) && matches!(location_two, Some(VertexFormat::Float32x2))
            }
        };
        if !fragment_attributes_are_valid {
            return Err(Error::InvalidDescriptor);
        }
        Ok(Self {
            target_format,
            topology,
            vertex_buffer,
            fragment,
            blend,
            raster,
            depth: None,
        })
    }

    /// Configure depth testing for this pipeline.
    ///
    /// # Arguments
    ///
    /// * `depth` - Depth attachment format, comparison, and write state.
    ///
    /// # Returns
    /// The updated pipeline descriptor, or [`Error::InvalidDescriptor`] when
    /// the requested format is not a depth format.
    pub fn with_depth_state(mut self, depth: DepthState) -> Result<Self> {
        if depth.format() != TextureFormat::Depth32Float {
            return Err(Error::InvalidDescriptor);
        }
        self.depth = Some(depth);
        Ok(self)
    }

    /// Configure depth testing through the depth-stencil pipeline slot.
    ///
    /// Scarlet currently exposes depth without stencil; this discoverable
    /// alias leaves room for stencil state in a future compatible API.
    ///
    /// # Arguments
    ///
    /// * `depth` - Depth attachment format, comparison, and write state.
    ///
    /// # Returns
    /// The updated pipeline descriptor, or [`Error::InvalidDescriptor`] when
    /// the requested format is not a depth format.
    pub fn with_depth_stencil(self, depth: DepthState) -> Result<Self> {
        self.with_depth_state(depth)
    }
    /// Return the required color attachment format.
    ///
    /// # Returns
    /// The target texture format.
    pub const fn target_format(&self) -> TextureFormat {
        self.target_format
    }
    /// Return primitive assembly topology.
    ///
    /// # Returns
    /// The configured topology.
    pub const fn topology(&self) -> PrimitiveTopology {
        self.topology
    }
    /// Return the vertex-buffer layout.
    ///
    /// # Returns
    /// The owned layout by shared reference.
    pub const fn vertex_buffer(&self) -> &VertexBufferLayout {
        &self.vertex_buffer
    }
    /// Return the fragment program.
    ///
    /// # Returns
    /// The portable fragment operation.
    pub const fn fragment(&self) -> FragmentProgram {
        self.fragment
    }
    /// Return blend state.
    ///
    /// # Returns
    /// The configured blend state.
    pub const fn blend(&self) -> BlendState {
        self.blend
    }
    /// Return rasterization state.
    ///
    /// # Returns
    /// The configured raster state.
    pub const fn raster(&self) -> RasterState {
        self.raster
    }

    /// Return optional depth-test and depth-write state.
    ///
    /// # Returns
    /// The configured depth state, or `None` when depth testing is disabled.
    pub const fn depth_state(&self) -> Option<DepthState> {
        self.depth
    }
}
