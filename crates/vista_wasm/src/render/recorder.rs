//! A stand-in for the subset of `wgpu` the renderer uses, for native
//! builds: it records what the renderer asks of the GPU instead of doing
//! it, so a native host can replay it on its own graphics API.
//!
//! `render::gpu`, `render::gpu::weather` and `render::textures` name it
//! `wgpu` in native builds, so the renderer is the same code in the
//! browser and natively. Types, fields and methods keep wgpu's names and
//! shapes; only what the renderer calls exists.
//!
//! Every resource gets an [`Id`] when it is created, and an
//! [`Op::Release`] when its last handle drops. Queue writes are recorded
//! in order as they are made; a command encoder's commands are recorded
//! when it is submitted, as one [`Op::Submit`]. Resources a command
//! buffer uses stay alive until it is submitted, so a release always
//! follows the last use, as in WebGPU.
//!
//! The host drains the operations with [`Recorder::take_ops`], and
//! reports back what only the GPU knows: buffer contents it was asked to
//! read ([`Recorder::complete_map`]), finished work
//! ([`Recorder::work_done`]), errors and a lost device.

use std::borrow::Cow;
use std::ops::{Bound, Deref, DerefMut, Range, RangeBounds};
use std::sync::{Arc, Mutex, MutexGuard};

/// A recorded resource's identity. 0 is never used; [`OUTPUT_TEXTURE`] is
/// the host's render target.
pub type Id = u32;

/// The texture the surface hands out: the host's render target for the
/// frame (see [`Surface::get_current_texture`]). It is never released.
pub const OUTPUT_TEXTURE: Id = 1;

/// The first id given to a resource.
const FIRST_ID: Id = 2;

// --- what is recorded --------------------------------------------------------

/// One recorded operation, in the order the renderer made it.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
  /// A buffer of `size` bytes.
  CreateBuffer {
    id: Id,
    size: u64,
    usage: BufferUsages,
    label: String,
  },
  /// A texture.
  CreateTexture {
    id: Id,
    size: Extent3d,
    mip_level_count: u32,
    dimension: TextureDimension,
    format: TextureFormat,
    usage: TextureUsages,
    label: String,
  },
  /// A view of `texture`. Counts left `None` run to the texture's end.
  CreateView {
    id: Id,
    texture: Id,
    dimension: Option<TextureViewDimension>,
    base_mip_level: u32,
    mip_level_count: Option<u32>,
    base_array_layer: u32,
    array_layer_count: Option<u32>,
  },
  /// A sampler.
  CreateSampler { id: Id, desc: SamplerDesc },
  /// A bind group: each binding and what it binds.
  CreateBindGroup {
    id: Id,
    entries: Vec<(u32, Resource)>,
    label: String,
  },
  /// A shader module, by the name `render::shaders::module_name` gives
  /// its source (empty for a source it does not know).
  CreateShaderModule { id: Id, module: String },
  /// A render pipeline.
  CreateRenderPipeline { id: Id, desc: RenderPipelineDesc },
  /// A compute pipeline.
  CreateComputePipeline {
    id: Id,
    module: Id,
    entry_point: String,
    constants: Vec<(String, f64)>,
    label: String,
  },
  /// Bytes written into a buffer.
  WriteBuffer {
    buffer: Id,
    offset: u64,
    data: Vec<u8>,
  },
  /// Texels written into one mip level of a texture, from `origin`.
  WriteTexture {
    texture: Id,
    mip_level: u32,
    origin: Origin3d,
    data: Vec<u8>,
    bytes_per_row: u32,
    rows_per_image: u32,
    size: Extent3d,
  },
  /// A command buffer, run in order.
  Submit(Vec<Command>),
  /// Read `buffer` back, once the work submitted before it is done, and
  /// hand its bytes to [`Recorder::complete_map`].
  MapRead { buffer: Id },
  /// The frame in [`OUTPUT_TEXTURE`] is finished.
  Present,
  /// The host's render target is now `width` x `height` in `format`.
  ConfigureSurface {
    width: u32,
    height: u32,
    format: TextureFormat,
  },
  /// Nothing will use `id` again.
  Release(Id),
}

/// What a bind group entry binds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resource {
  /// `size` bytes of a buffer from `offset`; `None` to its end.
  Buffer {
    buffer: Id,
    offset: u64,
    size: Option<u64>,
  },
  /// A texture view.
  View(Id),
  /// A sampler.
  Sampler(Id),
}

/// One command of a command buffer.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
  /// Start drawing into these targets.
  BeginRenderPass {
    colour: Vec<Option<ColourTarget>>,
    depth: Option<DepthTarget>,
  },
  /// Stop drawing into them.
  EndRenderPass,
  /// Start dispatching compute work.
  BeginComputePass,
  /// Stop.
  EndComputePass,
  /// Draw with this pipeline.
  SetRenderPipeline(Id),
  /// Dispatch with this pipeline.
  SetComputePipeline(Id),
  /// Bind group `index`, or unbind it.
  SetBindGroup { index: u32, group: Option<Id> },
  /// Vertex buffer `slot`, from byte `offset`.
  SetVertexBuffer {
    slot: u32,
    buffer: Id,
    offset: u64,
    size: Option<u64>,
  },
  /// The index buffer, from byte `offset`.
  SetIndexBuffer {
    buffer: Id,
    format: IndexFormat,
    offset: u64,
  },
  /// The viewport, in pixels.
  SetViewport {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    min_depth: f32,
    max_depth: f32,
  },
  /// A draw. `vertex_index` and `instance_index` include the ranges'
  /// starts, as in WebGPU.
  Draw {
    vertices: Range<u32>,
    instances: Range<u32>,
  },
  /// An indexed draw; `vertex_index` includes `base_vertex`.
  DrawIndexed {
    indices: Range<u32>,
    base_vertex: i32,
    instances: Range<u32>,
  },
  /// A draw whose four arguments are at `offset` in `buffer`.
  DrawIndirect { buffer: Id, offset: u64 },
  /// An indexed draw whose five arguments are at `offset` in `buffer`.
  DrawIndexedIndirect { buffer: Id, offset: u64 },
  /// A compute dispatch.
  Dispatch { x: u32, y: u32, z: u32 },
  /// Copy bytes between buffers.
  CopyBufferToBuffer {
    source: Id,
    source_offset: u64,
    destination: Id,
    destination_offset: u64,
    size: u64,
  },
  /// Copy texels between textures.
  CopyTextureToTexture {
    source: TextureCopy,
    destination: TextureCopy,
    size: Extent3d,
  },
}

/// A colour attachment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColourTarget {
  pub view: Id,
  /// Cleared to this colour first, or `None` to keep what is there.
  pub clear: Option<Color>,
  /// Whether what is drawn is kept.
  pub store: bool,
}

/// The depth attachment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthTarget {
  pub view: Id,
  /// Cleared to this depth first, or `None` to keep what is there.
  pub clear: Option<f32>,
  pub store: bool,
}

/// One side of a texture copy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureCopy {
  pub texture: Id,
  pub mip_level: u32,
  pub origin: Origin3d,
}

/// A sampler's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SamplerDesc {
  pub address_mode_u: AddressMode,
  pub address_mode_v: AddressMode,
  pub address_mode_w: AddressMode,
  pub mag_filter: FilterMode,
  pub min_filter: FilterMode,
  pub mipmap_filter: MipmapFilterMode,
  pub lod_min_clamp: f32,
  pub lod_max_clamp: f32,
  pub compare: Option<CompareFunction>,
  pub anisotropy_clamp: u16,
}

/// A render pipeline's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderPipelineDesc {
  pub label: String,
  pub module: Id,
  pub vertex_entry: String,
  /// `None` for a depth-only pipeline with no fragment stage.
  pub fragment_entry: Option<String>,
  /// Override constants, for both stages.
  pub constants: Vec<(String, f64)>,
  pub buffers: Vec<Option<VertexLayout>>,
  pub targets: Vec<Option<ColorTargetState>>,
  pub topology: PrimitiveTopology,
  pub front_face: FrontFace,
  pub cull_mode: Option<Face>,
  pub depth: Option<DepthStencilState>,
}

/// A vertex buffer's layout.
#[derive(Clone, Debug, PartialEq)]
pub struct VertexLayout {
  pub array_stride: u64,
  pub step_mode: VertexStepMode,
  pub attributes: Vec<VertexAttribute>,
}

// --- the recorder ------------------------------------------------------------

/// A boxed callback, run when the host reports back.
type Callback = Box<dyn FnOnce() + Send>;

/// What a buffer read calls when its bytes arrive.
type MapCallback = Box<dyn FnOnce(Result<(), BufferAsyncError>) + Send>;

#[derive(Default)]
struct State {
  next_id: Id,
  ops: Vec<Op>,
  /// Buffers waiting to be read back, with what to call when they are.
  maps: Vec<(Arc<BufferInner>, MapCallback)>,
  /// Called when the work submitted so far is done.
  work_done: Vec<Callback>,
  uncaptured: Option<Arc<dyn Fn(Error) + Send + Sync>>,
  lost: Option<Box<dyn FnOnce(DeviceLostReason, String) + Send>>,
}

struct Shared {
  state: Mutex<State>,
}

impl Shared {
  fn lock(&self) -> MutexGuard<'_, State> {
    // A panic while recording leaves nothing half-written that matters.
    self
      .state
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
  }

  fn push(&self, op: Op) {
    self.lock().ops.push(op);
  }

  fn id(&self) -> Id {
    let mut state = self.lock();
    let id = state.next_id;
    state.next_id += 1;
    id
  }
}

/// The recording. Clones share it.
#[derive(Clone)]
pub struct Recorder {
  shared: Arc<Shared>,
}

impl Default for Recorder {
  fn default() -> Self {
    Self::new()
  }
}

impl Recorder {
  /// An empty recording.
  pub fn new() -> Self {
    Self {
      shared: Arc::new(Shared {
        state: Mutex::new(State {
          next_id: FIRST_ID,
          ..State::default()
        }),
      }),
    }
  }

  /// The device and queue that record into it.
  pub fn device(&self) -> (Device, Queue) {
    (
      Device {
        shared: Arc::clone(&self.shared),
      },
      Queue {
        shared: Arc::clone(&self.shared),
      },
    )
  }

  /// A surface whose frames are the host's render target, `width` x
  /// `height` in `format`.
  pub fn surface(&self, width: u32, height: u32, format: TextureFormat) -> Surface<'static> {
    Surface {
      shared: Arc::clone(&self.shared),
      config: SurfaceConfiguration {
        format,
        width,
        height,
      },
      _window: std::marker::PhantomData,
    }
  }

  /// Every operation recorded since the last call, in order.
  pub fn take_ops(&self) -> Vec<Op> {
    std::mem::take(&mut self.shared.lock().ops)
  }

  /// The bytes of a buffer an [`Op::MapRead`] asked for. False when no
  /// read of `buffer` is waiting, or the bytes are not its size.
  pub fn complete_map(&self, buffer: Id, bytes: &[u8]) -> bool {
    let waiting = {
      let mut state = self.shared.lock();
      let at = state
        .maps
        .iter()
        .position(|(inner, _)| inner.res.id == buffer);
      at.map(|at| state.maps.remove(at))
    };
    let Some((inner, callback)) = waiting else {
      return false;
    };

    if bytes.len() as u64 != inner.size {
      callback(Err(BufferAsyncError));
      return false;
    }

    *inner.lock_mapped() = Some(bytes.to_vec());
    callback(Ok(()));
    true
  }

  /// A read the host could not make: its callback hears so.
  pub fn fail_map(&self, buffer: Id) {
    let waiting = {
      let mut state = self.shared.lock();
      let at = state
        .maps
        .iter()
        .position(|(inner, _)| inner.res.id == buffer);
      at.map(|at| state.maps.remove(at))
    };

    if let Some((_, callback)) = waiting {
      callback(Err(BufferAsyncError));
    }
  }

  /// The work submitted so far is done.
  pub fn work_done(&self) {
    let callbacks = std::mem::take(&mut self.shared.lock().work_done);

    for callback in callbacks {
      callback();
    }
  }

  /// An error the host's GPU reported.
  pub fn report_error(&self, error: Error) {
    let handler = self.shared.lock().uncaptured.clone();

    if let Some(handler) = handler {
      handler(error);
    }
  }

  /// The host lost its device, for `message`.
  pub fn report_lost(&self, message: String) {
    let callback = self.shared.lock().lost.take();

    if let Some(callback) = callback {
      callback(DeviceLostReason::Unknown, message);
    }
  }
}

/// A resource's id; recording its release when the last handle drops.
struct Res {
  id: Id,
  shared: Arc<Shared>,
}

impl Res {
  fn new(shared: &Arc<Shared>) -> Self {
    Self {
      id: shared.id(),
      shared: Arc::clone(shared),
    }
  }
}

impl Drop for Res {
  fn drop(&mut self) {
    if self.id != OUTPUT_TEXTURE {
      self.shared.push(Op::Release(self.id));
    }
  }
}

impl std::fmt::Debug for Res {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(formatter, "#{}", self.id)
  }
}

/// Something a command buffer keeps alive until it is submitted.
type Keep = Arc<dyn std::any::Any + Send + Sync>;

// --- bit sets ------------------------------------------------------------------

macro_rules! bit_set {
  ($(#[$doc:meta])* $name:ident: $($flag:ident = $bit:expr),* $(,)?) => {
    $(#[$doc])*
    #[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
    pub struct $name(pub u32);

    impl $name {
      $(pub const $flag: Self = Self($bit);)*

      /// No flags.
      pub const fn empty() -> Self {
        Self(0)
      }

      /// Whether every flag of `other` is set.
      pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
      }

      /// Whether any flag of `other` is set.
      pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
      }

      /// Whether no flag is set.
      pub const fn is_empty(self) -> bool {
        self.0 == 0
      }
    }

    impl std::ops::BitOr for $name {
      type Output = Self;

      fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
      }
    }

    impl std::ops::BitOrAssign for $name {
      fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
      }
    }

    impl std::ops::BitAnd for $name {
      type Output = Self;

      fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
      }
    }
  };
}

bit_set!(
  /// How a buffer may be used.
  BufferUsages:
  MAP_READ = 1,
  MAP_WRITE = 2,
  COPY_SRC = 4,
  COPY_DST = 8,
  INDEX = 16,
  VERTEX = 32,
  UNIFORM = 64,
  STORAGE = 128,
  INDIRECT = 256,
  QUERY_RESOLVE = 512,
);

bit_set!(
  /// How a texture may be used.
  TextureUsages:
  COPY_SRC = 1,
  COPY_DST = 2,
  TEXTURE_BINDING = 4,
  STORAGE_BINDING = 8,
  RENDER_ATTACHMENT = 16,
);

bit_set!(
  /// Shader stages that see a binding.
  ShaderStages:
  VERTEX = 1,
  FRAGMENT = 2,
  COMPUTE = 4,
  VERTEX_FRAGMENT = 3,
);

bit_set!(
  /// Colour channels a pipeline writes.
  ColorWrites:
  RED = 1,
  GREEN = 2,
  BLUE = 4,
  ALPHA = 8,
  ALL = 15,
);

bit_set!(
  /// Optional device features. Native hosts offer none.
  Features:
  TIMESTAMP_QUERY = 1,
);

// --- plain types ---------------------------------------------------------------

/// A texture's size.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Extent3d {
  pub width: u32,
  pub height: u32,
  pub depth_or_array_layers: u32,
}

/// A texel position.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Origin3d {
  pub x: u32,
  pub y: u32,
  pub z: u32,
}

impl Origin3d {
  pub const ZERO: Self = Self { x: 0, y: 0, z: 0 };
}

/// A colour, for clears.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
  pub r: f64,
  pub g: f64,
  pub b: f64,
  pub a: f64,
}

impl Color {
  pub const BLACK: Self = Self {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
  };
  pub const TRANSPARENT: Self = Self {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
  };
}

/// Texture formats the renderer uses.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TextureFormat {
  R8Unorm,
  Rg8Unorm,
  Rgba8Unorm,
  Rgba8UnormSrgb,
  Bgra8Unorm,
  Bgra8UnormSrgb,
  Rgba16Float,
  R32Float,
  Depth32Float,
}

impl TextureFormat {
  /// Whether the format stores colour sRGB-encoded.
  pub fn is_srgb(self) -> bool {
    matches!(self, Self::Rgba8UnormSrgb | Self::Bgra8UnormSrgb)
  }

  /// Bytes per texel.
  pub fn block_size(self) -> u32 {
    match self {
      Self::R8Unorm => 1,
      Self::Rg8Unorm => 2,
      Self::Rgba16Float => 8,
      _ => 4,
    }
  }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TextureDimension {
  D1,
  #[default]
  D2,
  D3,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TextureViewDimension {
  D1,
  #[default]
  D2,
  D2Array,
  Cube,
  CubeArray,
  D3,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TextureAspect {
  #[default]
  All,
  DepthOnly,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum AddressMode {
  #[default]
  ClampToEdge,
  Repeat,
  MirrorRepeat,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum FilterMode {
  #[default]
  Nearest,
  Linear,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum MipmapFilterMode {
  #[default]
  Nearest,
  Linear,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CompareFunction {
  Never,
  Less,
  Equal,
  LessEqual,
  Greater,
  NotEqual,
  GreaterEqual,
  Always,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IndexFormat {
  Uint16,
  Uint32,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum VertexStepMode {
  #[default]
  Vertex,
  Instance,
}

/// Vertex attribute formats the renderer uses.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VertexFormat {
  Float32,
  Float32x2,
  Float32x3,
  Float32x4,
  Snorm16x2,
  Snorm16x4,
  Uint32x3,
  Unorm8x4,
  Uint8x4,
}

impl VertexFormat {
  /// Bytes per attribute.
  pub const fn size(self) -> u64 {
    match self {
      Self::Float32 | Self::Snorm16x2 | Self::Unorm8x4 | Self::Uint8x4 => 4,
      Self::Float32x2 | Self::Snorm16x4 => 8,
      Self::Float32x3 | Self::Uint32x3 => 12,
      Self::Float32x4 => 16,
    }
  }
}

/// One vertex attribute.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VertexAttribute {
  pub format: VertexFormat,
  pub offset: u64,
  pub shader_location: u32,
}

/// Attributes packed one after another, as `wgpu::vertex_attr_array!`
/// lays them out.
pub const fn attribute_array<const N: usize>(
  attributes: [(u32, VertexFormat); N],
) -> [VertexAttribute; N] {
  let mut out = [VertexAttribute {
    format: VertexFormat::Float32,
    offset: 0,
    shader_location: 0,
  }; N];
  let mut offset = 0;
  let mut i = 0;

  while i < N {
    out[i] = VertexAttribute {
      format: attributes[i].1,
      offset,
      shader_location: attributes[i].0,
    };
    offset += attributes[i].1.size();
    i += 1;
  }

  out
}

/// As `wgpu::vertex_attr_array!`.
macro_rules! vertex_attr_array {
  ($($location:expr => $format:ident),* $(,)?) => {
    $crate::render::recorder::attribute_array([
      $(($location, $crate::render::recorder::VertexFormat::$format)),*
    ])
  };
}
pub(crate) use vertex_attr_array;

/// A vertex buffer layout.
#[derive(Clone, Debug)]
pub struct VertexBufferLayout<'a> {
  pub array_stride: u64,
  pub step_mode: VertexStepMode,
  pub attributes: &'a [VertexAttribute],
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum PrimitiveTopology {
  PointList,
  LineList,
  LineStrip,
  #[default]
  TriangleList,
  TriangleStrip,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum FrontFace {
  #[default]
  Ccw,
  Cw,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Face {
  Front,
  Back,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum PolygonMode {
  #[default]
  Fill,
  Line,
  Point,
}

/// Primitive assembly and rasterisation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PrimitiveState {
  pub topology: PrimitiveTopology,
  pub strip_index_format: Option<IndexFormat>,
  pub front_face: FrontFace,
  pub cull_mode: Option<Face>,
  pub unclipped_depth: bool,
  pub polygon_mode: PolygonMode,
  pub conservative: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlendFactor {
  Zero,
  One,
  Src,
  OneMinusSrc,
  SrcAlpha,
  OneMinusSrcAlpha,
  Dst,
  OneMinusDst,
  DstAlpha,
  OneMinusDstAlpha,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlendOperation {
  Add,
  Subtract,
  ReverseSubtract,
  Min,
  Max,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendComponent {
  pub src_factor: BlendFactor,
  pub dst_factor: BlendFactor,
  pub operation: BlendOperation,
}

/// Blending for one colour target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendState {
  pub color: BlendComponent,
  pub alpha: BlendComponent,
}

impl BlendState {
  /// As `wgpu::BlendState::ALPHA_BLENDING`.
  pub const ALPHA_BLENDING: Self = Self {
    color: BlendComponent {
      src_factor: BlendFactor::SrcAlpha,
      dst_factor: BlendFactor::OneMinusSrcAlpha,
      operation: BlendOperation::Add,
    },
    alpha: BlendComponent {
      src_factor: BlendFactor::One,
      dst_factor: BlendFactor::OneMinusSrcAlpha,
      operation: BlendOperation::Add,
    },
  };
}

/// A colour target of a pipeline.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorTargetState {
  pub format: TextureFormat,
  pub blend: Option<BlendState>,
  pub write_mask: ColorWrites,
}

/// Depth bias.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DepthBiasState {
  pub constant: i32,
  pub slope_scale: f32,
  pub clamp: f32,
}

/// Stencil testing, which the renderer never uses.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StencilState {
  pub read_mask: u32,
  pub write_mask: u32,
}

/// Depth testing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthStencilState {
  pub format: TextureFormat,
  pub depth_write_enabled: Option<bool>,
  pub depth_compare: Option<CompareFunction>,
  pub stencil: StencilState,
  pub bias: DepthBiasState,
}

/// Multisampling, which the renderer never uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MultisampleState {
  pub count: u32,
  pub mask: u64,
  pub alpha_to_coverage_enabled: bool,
}

impl Default for MultisampleState {
  fn default() -> Self {
    Self {
      count: 1,
      mask: !0,
      alpha_to_coverage_enabled: false,
    }
  }
}

/// Override constants and other compilation settings.
#[derive(Clone, Debug, Default)]
pub struct PipelineCompilationOptions<'a> {
  pub constants: &'a [(&'a str, f64)],
  pub zero_initialize_workgroup_memory: bool,
}

/// The vertex stage.
pub struct VertexState<'a> {
  pub module: &'a ShaderModule,
  pub entry_point: Option<&'a str>,
  pub compilation_options: PipelineCompilationOptions<'a>,
  pub buffers: &'a [Option<VertexBufferLayout<'a>>],
}

/// The fragment stage.
pub struct FragmentState<'a> {
  pub module: &'a ShaderModule,
  pub entry_point: Option<&'a str>,
  pub compilation_options: PipelineCompilationOptions<'a>,
  pub targets: &'a [Option<ColorTargetState>],
}

/// Pipeline caches do not exist natively.
pub struct PipelineCache;

pub struct RenderPipelineDescriptor<'a> {
  pub label: Option<&'a str>,
  pub layout: Option<&'a PipelineLayout>,
  pub vertex: VertexState<'a>,
  pub fragment: Option<FragmentState<'a>>,
  pub primitive: PrimitiveState,
  pub depth_stencil: Option<DepthStencilState>,
  pub multisample: MultisampleState,
  pub multiview_mask: Option<std::num::NonZeroU32>,
  pub cache: Option<&'a PipelineCache>,
}

pub struct ComputePipelineDescriptor<'a> {
  pub label: Option<&'a str>,
  pub layout: Option<&'a PipelineLayout>,
  pub module: &'a ShaderModule,
  pub entry_point: Option<&'a str>,
  pub compilation_options: PipelineCompilationOptions<'a>,
  pub cache: Option<&'a PipelineCache>,
}

pub struct BufferDescriptor<'a> {
  pub label: Option<&'a str>,
  pub size: u64,
  pub usage: BufferUsages,
  pub mapped_at_creation: bool,
}

pub struct TextureDescriptor<'a> {
  pub label: Option<&'a str>,
  pub size: Extent3d,
  pub mip_level_count: u32,
  pub sample_count: u32,
  pub dimension: TextureDimension,
  pub format: TextureFormat,
  pub usage: TextureUsages,
  pub view_formats: &'a [TextureFormat],
}

#[derive(Clone, Debug, Default)]
pub struct TextureViewDescriptor<'a> {
  pub label: Option<&'a str>,
  pub format: Option<TextureFormat>,
  pub dimension: Option<TextureViewDimension>,
  pub usage: Option<TextureUsages>,
  pub aspect: TextureAspect,
  pub base_mip_level: u32,
  pub mip_level_count: Option<u32>,
  pub base_array_layer: u32,
  pub array_layer_count: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct SamplerDescriptor<'a> {
  pub label: Option<&'a str>,
  pub address_mode_u: AddressMode,
  pub address_mode_v: AddressMode,
  pub address_mode_w: AddressMode,
  pub mag_filter: FilterMode,
  pub min_filter: FilterMode,
  pub mipmap_filter: MipmapFilterMode,
  pub lod_min_clamp: f32,
  pub lod_max_clamp: f32,
  pub compare: Option<CompareFunction>,
  pub anisotropy_clamp: u16,
  pub border_color: Option<()>,
}

impl Default for SamplerDescriptor<'_> {
  fn default() -> Self {
    Self {
      label: None,
      address_mode_u: AddressMode::default(),
      address_mode_v: AddressMode::default(),
      address_mode_w: AddressMode::default(),
      mag_filter: FilterMode::default(),
      min_filter: FilterMode::default(),
      mipmap_filter: MipmapFilterMode::default(),
      lod_min_clamp: 0.0,
      lod_max_clamp: 32.0,
      compare: None,
      anisotropy_clamp: 1,
      border_color: None,
    }
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferBindingType {
  Uniform,
  Storage { read_only: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureSampleType {
  Float { filterable: bool },
  Depth,
  Sint,
  Uint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SamplerBindingType {
  Filtering,
  NonFiltering,
  Comparison,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageTextureAccess {
  WriteOnly,
  ReadOnly,
  ReadWrite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingType {
  Buffer {
    ty: BufferBindingType,
    has_dynamic_offset: bool,
    min_binding_size: Option<std::num::NonZeroU64>,
  },
  Sampler(SamplerBindingType),
  Texture {
    sample_type: TextureSampleType,
    view_dimension: TextureViewDimension,
    multisampled: bool,
  },
  StorageTexture {
    access: StorageTextureAccess,
    format: TextureFormat,
    view_dimension: TextureViewDimension,
  },
}

pub struct BindGroupLayoutEntry {
  pub binding: u32,
  pub visibility: ShaderStages,
  pub ty: BindingType,
  pub count: Option<std::num::NonZeroU32>,
}

pub struct BindGroupLayoutDescriptor<'a> {
  pub label: Option<&'a str>,
  pub entries: &'a [BindGroupLayoutEntry],
}

pub struct PipelineLayoutDescriptor<'a> {
  pub label: Option<&'a str>,
  pub bind_group_layouts: &'a [Option<&'a BindGroupLayout>],
  pub immediate_size: u32,
}

/// Part of a buffer, for a binding.
#[derive(Clone)]
pub struct BufferBinding<'a> {
  pub buffer: &'a Buffer,
  pub offset: u64,
  pub size: Option<std::num::NonZeroU64>,
}

#[derive(Clone)]
pub enum BindingResource<'a> {
  Buffer(BufferBinding<'a>),
  Sampler(&'a Sampler),
  TextureView(&'a TextureView),
}

#[derive(Clone)]
pub struct BindGroupEntry<'a> {
  pub binding: u32,
  pub resource: BindingResource<'a>,
}

pub struct BindGroupDescriptor<'a> {
  pub label: Option<&'a str>,
  pub layout: &'a BindGroupLayout,
  pub entries: &'a [BindGroupEntry<'a>],
}

pub enum ShaderSource<'a> {
  Wgsl(Cow<'a, str>),
}

pub struct ShaderModuleDescriptor<'a> {
  pub label: Option<&'a str>,
  pub source: ShaderSource<'a>,
}

pub struct CommandEncoderDescriptor<'a> {
  pub label: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoadOp<V> {
  Clear(V),
  Load,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreOp {
  Store,
  Discard,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Operations<V> {
  pub load: LoadOp<V>,
  pub store: StoreOp,
}

pub struct RenderPassColorAttachment<'a> {
  pub view: &'a TextureView,
  pub depth_slice: Option<u32>,
  pub resolve_target: Option<&'a TextureView>,
  pub ops: Operations<Color>,
}

pub struct RenderPassDepthStencilAttachment<'a> {
  pub view: &'a TextureView,
  pub depth_ops: Option<Operations<f32>>,
  pub stencil_ops: Option<Operations<u32>>,
}

/// Timestamp queries, which native hosts do not offer
/// ([`Device::features`] is empty).
pub struct RenderPassTimestampWrites<'a> {
  pub query_set: &'a QuerySet,
  pub beginning_of_pass_write_index: Option<u32>,
  pub end_of_pass_write_index: Option<u32>,
}

/// As [`RenderPassTimestampWrites`].
pub struct ComputePassTimestampWrites<'a> {
  pub query_set: &'a QuerySet,
  pub beginning_of_pass_write_index: Option<u32>,
  pub end_of_pass_write_index: Option<u32>,
}

#[derive(Default)]
pub struct RenderPassDescriptor<'a> {
  pub label: Option<&'a str>,
  pub color_attachments: &'a [Option<RenderPassColorAttachment<'a>>],
  pub depth_stencil_attachment: Option<RenderPassDepthStencilAttachment<'a>>,
  pub timestamp_writes: Option<RenderPassTimestampWrites<'a>>,
  pub occlusion_query_set: Option<&'a QuerySet>,
  pub multiview_mask: Option<std::num::NonZeroU32>,
}

#[derive(Default)]
pub struct ComputePassDescriptor<'a> {
  pub label: Option<&'a str>,
  pub timestamp_writes: Option<ComputePassTimestampWrites<'a>>,
}

/// Where in a texture a copy starts.
pub struct TexelCopyTextureInfo<'a> {
  pub texture: &'a Texture,
  pub mip_level: u32,
  pub origin: Origin3d,
  pub aspect: TextureAspect,
}

/// How copied texels lie in memory.
#[derive(Clone, Copy, Debug, Default)]
pub struct TexelCopyBufferLayout {
  pub offset: u64,
  pub bytes_per_row: Option<u32>,
  pub rows_per_image: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MapMode {
  Read,
  Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryType {
  Timestamp,
}

pub struct QuerySetDescriptor<'a> {
  pub label: Option<&'a str>,
  pub ty: QueryType,
  pub count: u32,
}

/// A buffer read that failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferAsyncError;

/// A buffer's contents could not be reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MapError;

/// An error the host's GPU reported.
#[derive(Clone, Debug)]
pub enum Error {
  OutOfMemory { source: () },
  Validation { source: (), description: String },
  Internal { source: (), description: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorFilter {
  OutOfMemory,
  Validation,
  Internal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceLostReason {
  Unknown,
  Destroyed,
}

/// An error scope. Native hosts report errors as they happen, so a scope
/// never holds one.
pub struct ErrorScopeGuard;

impl ErrorScopeGuard {
  /// The scope's error: always none.
  pub fn pop(self) -> std::future::Ready<Option<Error>> {
    std::future::ready(None)
  }
}

// --- resources -----------------------------------------------------------------

struct BufferInner {
  res: Res,
  size: u64,
  usage: BufferUsages,
  /// Bytes while the buffer is mapped: written before its first use when
  /// mapped at creation, or read back.
  mapped: Mutex<Option<Vec<u8>>>,
  /// Whether `mapped` is to be written to the buffer on unmapping.
  write_on_unmap: Mutex<bool>,
}

impl BufferInner {
  fn lock_mapped(&self) -> MutexGuard<'_, Option<Vec<u8>>> {
    self
      .mapped
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
  }
}

/// A buffer.
#[derive(Clone)]
pub struct Buffer {
  inner: Arc<BufferInner>,
}

impl std::fmt::Debug for Buffer {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(formatter, "Buffer({:?})", self.inner.res)
  }
}

impl PartialEq for Buffer {
  fn eq(&self, other: &Self) -> bool {
    self.inner.res.id == other.inner.res.id
  }
}

fn range_of(bounds: impl RangeBounds<u64>, size: u64) -> (u64, Option<u64>) {
  let start = match bounds.start_bound() {
    Bound::Included(start) => *start,
    Bound::Excluded(start) => start + 1,
    Bound::Unbounded => 0,
  };
  let end = match bounds.end_bound() {
    Bound::Included(end) => Some(end + 1),
    Bound::Excluded(end) => Some(*end),
    Bound::Unbounded => None,
  };
  (start, end.map(|end| end.min(size).saturating_sub(start)))
}

impl Buffer {
  /// The recorded id.
  pub fn id(&self) -> Id {
    self.inner.res.id
  }

  /// Size in bytes.
  pub fn size(&self) -> u64 {
    self.inner.size
  }

  /// How it may be used.
  pub fn usage(&self) -> BufferUsages {
    self.inner.usage
  }

  /// Part of it.
  pub fn slice(&self, bounds: impl RangeBounds<u64>) -> BufferSlice<'_> {
    let (offset, size) = range_of(bounds, self.inner.size);
    BufferSlice {
      buffer: self,
      offset,
      size,
    }
  }

  /// All of it, for a binding.
  pub fn as_entire_binding(&self) -> BindingResource<'_> {
    BindingResource::Buffer(BufferBinding {
      buffer: self,
      offset: 0,
      size: None,
    })
  }

  /// Unmap it: bytes written while mapped at creation are recorded.
  pub fn unmap(&self) {
    let bytes = self.inner.lock_mapped().take();
    let write = std::mem::take(
      &mut *self
        .inner
        .write_on_unmap
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );

    if let (Some(bytes), true) = (bytes, write) {
      self.inner.res.shared.push(Op::WriteBuffer {
        buffer: self.inner.res.id,
        offset: 0,
        data: bytes,
      });
    }
  }
}

/// Part of a buffer.
#[derive(Clone, Copy)]
pub struct BufferSlice<'a> {
  buffer: &'a Buffer,
  offset: u64,
  size: Option<u64>,
}

/// Mapped bytes, read-only.
pub struct BufferView<'a> {
  guard: MutexGuard<'a, Option<Vec<u8>>>,
  range: Range<usize>,
}

impl Deref for BufferView<'_> {
  type Target = [u8];

  fn deref(&self) -> &[u8] {
    self
      .guard
      .as_deref()
      .map_or(&[], |bytes| &bytes[self.range.clone()])
  }
}

/// Mapped bytes, writable.
pub struct BufferViewMut<'a> {
  guard: MutexGuard<'a, Option<Vec<u8>>>,
  range: Range<usize>,
}

impl Deref for BufferViewMut<'_> {
  type Target = [u8];

  fn deref(&self) -> &[u8] {
    self
      .guard
      .as_deref()
      .map_or(&[], |bytes| &bytes[self.range.clone()])
  }
}

impl DerefMut for BufferViewMut<'_> {
  fn deref_mut(&mut self) -> &mut [u8] {
    let range = self.range.clone();
    self
      .guard
      .as_deref_mut()
      .map_or(&mut [], |bytes| &mut bytes[range])
  }
}

impl<'a> BufferSlice<'a> {
  fn bytes(&self) -> Range<usize> {
    let end = self
      .size
      .map_or(self.buffer.inner.size, |size| self.offset + size);
    self.offset as usize..end as usize
  }

  /// Read the buffer back once the work before it is done; `callback`
  /// hears when it is mapped.
  pub fn map_async(
    &self,
    _mode: MapMode,
    callback: impl FnOnce(Result<(), BufferAsyncError>) + Send + 'static,
  ) {
    let mut state = self.buffer.inner.res.shared.lock();
    state.ops.push(Op::MapRead {
      buffer: self.buffer.inner.res.id,
    });
    state
      .maps
      .push((Arc::clone(&self.buffer.inner), Box::new(callback)));
  }

  /// The mapped bytes.
  pub fn get_mapped_range(&self) -> Result<BufferView<'a>, MapError> {
    let guard = self.buffer.inner.lock_mapped();

    match guard.as_ref() {
      Some(bytes) if self.bytes().end <= bytes.len() => Ok(BufferView {
        range: self.bytes(),
        guard,
      }),
      _ => Err(MapError),
    }
  }

  /// The mapped bytes, to write.
  pub fn get_mapped_range_mut(&self) -> Result<BufferViewMut<'a>, MapError> {
    let guard = self.buffer.inner.lock_mapped();

    match guard.as_ref() {
      Some(bytes) if self.bytes().end <= bytes.len() => Ok(BufferViewMut {
        range: self.bytes(),
        guard,
      }),
      _ => Err(MapError),
    }
  }
}

struct TextureInner {
  res: Res,
  size: Extent3d,
  mip_level_count: u32,
  format: TextureFormat,
}

/// A texture.
#[derive(Clone)]
pub struct Texture {
  inner: Arc<TextureInner>,
}

impl Texture {
  /// The recorded id.
  pub fn id(&self) -> Id {
    self.inner.res.id
  }

  pub fn width(&self) -> u32 {
    self.inner.size.width
  }

  pub fn height(&self) -> u32 {
    self.inner.size.height
  }

  pub fn depth_or_array_layers(&self) -> u32 {
    self.inner.size.depth_or_array_layers
  }

  pub fn mip_level_count(&self) -> u32 {
    self.inner.mip_level_count
  }

  pub fn format(&self) -> TextureFormat {
    self.inner.format
  }

  pub fn size(&self) -> Extent3d {
    self.inner.size
  }

  /// Mip 0 from the origin, for a copy.
  pub fn as_image_copy(&self) -> TexelCopyTextureInfo<'_> {
    TexelCopyTextureInfo {
      texture: self,
      mip_level: 0,
      origin: Origin3d::ZERO,
      aspect: TextureAspect::All,
    }
  }

  /// A view of it.
  pub fn create_view(&self, desc: &TextureViewDescriptor<'_>) -> TextureView {
    let res = Res::new(&self.inner.res.shared);
    res.shared.push(Op::CreateView {
      id: res.id,
      texture: self.inner.res.id,
      dimension: desc.dimension,
      base_mip_level: desc.base_mip_level,
      mip_level_count: desc.mip_level_count,
      base_array_layer: desc.base_array_layer,
      array_layer_count: desc.array_layer_count,
    });
    TextureView {
      inner: Arc::new(ViewInner {
        res,
        _texture: self.clone(),
      }),
    }
  }
}

struct ViewInner {
  res: Res,
  _texture: Texture,
}

/// A view of a texture. Views are equal when they are the same view.
#[derive(Clone)]
pub struct TextureView {
  inner: Arc<ViewInner>,
}

impl TextureView {
  /// The recorded id.
  pub fn id(&self) -> Id {
    self.inner.res.id
  }
}

impl PartialEq for TextureView {
  fn eq(&self, other: &Self) -> bool {
    self.inner.res.id == other.inner.res.id
  }
}

impl std::fmt::Debug for TextureView {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(formatter, "TextureView({:?})", self.inner.res)
  }
}

macro_rules! handle {
  ($(#[$doc:meta])* $name:ident) => {
    $(#[$doc])*
    #[derive(Clone)]
    pub struct $name {
      inner: Arc<Res>,
    }

    impl $name {
      /// The recorded id.
      pub fn id(&self) -> Id {
        self.inner.id
      }
    }

    impl std::fmt::Debug for $name {
      fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}({:?})", stringify!($name), self.inner)
      }
    }
  };
}

handle!(
  /// A sampler.
  Sampler
);
handle!(
  /// A shader module.
  ShaderModule
);
handle!(
  /// A render pipeline.
  RenderPipeline
);

/// A bind group, keeping what it binds alive.
#[derive(Clone)]
pub struct BindGroup {
  inner: Arc<(Res, Vec<Keep>)>,
}

impl BindGroup {
  /// The recorded id.
  pub fn id(&self) -> Id {
    self.inner.0.id
  }
}

/// A compute pipeline.
#[derive(Clone)]
pub struct ComputePipeline {
  inner: Arc<Res>,
}

impl ComputePipeline {
  /// The recorded id.
  pub fn id(&self) -> Id {
    self.inner.id
  }

  /// Bind groups are matched to the pipeline when it is used, so any
  /// layout will do.
  pub fn get_bind_group_layout(&self, _index: u32) -> BindGroupLayout {
    BindGroupLayout
  }
}

/// Bind group layouts record nothing: the host binds by the pipeline's
/// own shaders.
pub struct BindGroupLayout;

/// As [`BindGroupLayout`].
pub struct PipelineLayout;

/// Timestamp query sets, which native hosts do not offer.
pub struct QuerySet;

// --- device and queue -------------------------------------------------------------

/// Creates resources.
#[derive(Clone)]
pub struct Device {
  shared: Arc<Shared>,
}

impl Device {
  /// Native hosts offer no optional features.
  pub fn features(&self) -> Features {
    Features::empty()
  }

  pub fn create_buffer(&self, desc: &BufferDescriptor<'_>) -> Buffer {
    let res = Res::new(&self.shared);
    res.shared.push(Op::CreateBuffer {
      id: res.id,
      size: desc.size,
      usage: desc.usage,
      label: desc.label.unwrap_or_default().to_string(),
    });
    Buffer {
      inner: Arc::new(BufferInner {
        res,
        size: desc.size,
        usage: desc.usage,
        mapped: Mutex::new(desc.mapped_at_creation.then(|| vec![0; desc.size as usize])),
        write_on_unmap: Mutex::new(desc.mapped_at_creation),
      }),
    }
  }

  pub fn create_texture(&self, desc: &TextureDescriptor<'_>) -> Texture {
    let res = Res::new(&self.shared);
    res.shared.push(Op::CreateTexture {
      id: res.id,
      size: desc.size,
      mip_level_count: desc.mip_level_count,
      dimension: desc.dimension,
      format: desc.format,
      usage: desc.usage,
      label: desc.label.unwrap_or_default().to_string(),
    });
    Texture {
      inner: Arc::new(TextureInner {
        res,
        size: desc.size,
        mip_level_count: desc.mip_level_count,
        format: desc.format,
      }),
    }
  }

  pub fn create_sampler(&self, desc: &SamplerDescriptor<'_>) -> Sampler {
    let res = Res::new(&self.shared);
    res.shared.push(Op::CreateSampler {
      id: res.id,
      desc: SamplerDesc {
        address_mode_u: desc.address_mode_u,
        address_mode_v: desc.address_mode_v,
        address_mode_w: desc.address_mode_w,
        mag_filter: desc.mag_filter,
        min_filter: desc.min_filter,
        mipmap_filter: desc.mipmap_filter,
        lod_min_clamp: desc.lod_min_clamp,
        lod_max_clamp: desc.lod_max_clamp,
        compare: desc.compare,
        anisotropy_clamp: desc.anisotropy_clamp,
      },
    });
    Sampler {
      inner: Arc::new(res),
    }
  }

  pub fn create_bind_group_layout(&self, _desc: &BindGroupLayoutDescriptor<'_>) -> BindGroupLayout {
    BindGroupLayout
  }

  pub fn create_pipeline_layout(&self, _desc: &PipelineLayoutDescriptor<'_>) -> PipelineLayout {
    PipelineLayout
  }

  pub fn create_bind_group(&self, desc: &BindGroupDescriptor<'_>) -> BindGroup {
    let res = Res::new(&self.shared);
    let mut keep: Vec<Keep> = Vec::with_capacity(desc.entries.len());
    let entries = desc
      .entries
      .iter()
      .map(|entry| {
        let resource = match &entry.resource {
          BindingResource::Buffer(binding) => {
            keep.push(binding.buffer.inner.clone());
            Resource::Buffer {
              buffer: binding.buffer.id(),
              offset: binding.offset,
              size: binding.size.map(std::num::NonZeroU64::get),
            }
          }
          BindingResource::TextureView(view) => {
            keep.push(view.inner.clone());
            Resource::View(view.id())
          }
          BindingResource::Sampler(sampler) => {
            keep.push(sampler.inner.clone());
            Resource::Sampler(sampler.id())
          }
        };
        (entry.binding, resource)
      })
      .collect();
    res.shared.push(Op::CreateBindGroup {
      id: res.id,
      entries,
      label: desc.label.unwrap_or_default().to_string(),
    });
    BindGroup {
      inner: Arc::new((res, keep)),
    }
  }

  pub fn create_shader_module(&self, desc: ShaderModuleDescriptor<'_>) -> ShaderModule {
    let ShaderSource::Wgsl(source) = &desc.source;
    let res = Res::new(&self.shared);
    res.shared.push(Op::CreateShaderModule {
      id: res.id,
      module: crate::render::shaders::module_name(source)
        .unwrap_or_default()
        .to_string(),
    });
    ShaderModule {
      inner: Arc::new(res),
    }
  }

  pub fn create_render_pipeline(&self, desc: &RenderPipelineDescriptor<'_>) -> RenderPipeline {
    let res = Res::new(&self.shared);
    let constants = desc
      .vertex
      .compilation_options
      .constants
      .iter()
      .map(|(name, value)| ((*name).to_string(), *value))
      .collect();
    let layout = |layout: &Option<VertexBufferLayout<'_>>| {
      layout.as_ref().map(|layout| VertexLayout {
        array_stride: layout.array_stride,
        step_mode: layout.step_mode,
        attributes: layout.attributes.to_vec(),
      })
    };
    res.shared.push(Op::CreateRenderPipeline {
      id: res.id,
      desc: RenderPipelineDesc {
        label: desc.label.unwrap_or_default().to_string(),
        module: desc.vertex.module.id(),
        vertex_entry: desc.vertex.entry_point.unwrap_or("main").to_string(),
        fragment_entry: desc
          .fragment
          .as_ref()
          .map(|fragment| fragment.entry_point.unwrap_or("main").to_string()),
        constants,
        buffers: desc.vertex.buffers.iter().map(layout).collect(),
        targets: desc
          .fragment
          .as_ref()
          .map_or_else(Vec::new, |fragment| fragment.targets.to_vec()),
        topology: desc.primitive.topology,
        front_face: desc.primitive.front_face,
        cull_mode: desc.primitive.cull_mode,
        depth: desc.depth_stencil,
      },
    });
    RenderPipeline {
      inner: Arc::new(res),
    }
  }

  pub fn create_compute_pipeline(&self, desc: &ComputePipelineDescriptor<'_>) -> ComputePipeline {
    let res = Res::new(&self.shared);
    res.shared.push(Op::CreateComputePipeline {
      id: res.id,
      module: desc.module.id(),
      entry_point: desc.entry_point.unwrap_or("main").to_string(),
      constants: desc
        .compilation_options
        .constants
        .iter()
        .map(|(name, value)| ((*name).to_string(), *value))
        .collect(),
      label: desc.label.unwrap_or_default().to_string(),
    });
    ComputePipeline {
      inner: Arc::new(res),
    }
  }

  pub fn create_query_set(&self, _desc: &QuerySetDescriptor<'_>) -> QuerySet {
    QuerySet
  }

  pub fn create_command_encoder(&self, _desc: &CommandEncoderDescriptor<'_>) -> CommandEncoder {
    CommandEncoder {
      inner: Arc::new(Mutex::new(EncoderInner::default())),
    }
  }

  /// Errors are reported as they happen; scopes never hold one.
  pub fn push_error_scope(&self, _filter: ErrorFilter) -> ErrorScopeGuard {
    ErrorScopeGuard
  }

  pub fn on_uncaptured_error(&self, handler: Arc<dyn Fn(Error) + Send + Sync>) {
    self.shared.lock().uncaptured = Some(handler);
  }

  pub fn set_device_lost_callback(
    &self,
    callback: impl FnOnce(DeviceLostReason, String) + Send + 'static,
  ) {
    self.shared.lock().lost = Some(Box::new(callback));
  }
}

/// Writes and submits.
#[derive(Clone)]
pub struct Queue {
  shared: Arc<Shared>,
}

impl Queue {
  pub fn write_buffer(&self, buffer: &Buffer, offset: u64, data: &[u8]) {
    self.shared.push(Op::WriteBuffer {
      buffer: buffer.id(),
      offset,
      data: data.to_vec(),
    });
  }

  pub fn write_texture(
    &self,
    texture: TexelCopyTextureInfo<'_>,
    data: &[u8],
    layout: TexelCopyBufferLayout,
    size: Extent3d,
  ) {
    let start = layout.offset as usize;
    let bytes_per_row = layout
      .bytes_per_row
      .unwrap_or(size.width * texture.texture.format().block_size());
    self.shared.push(Op::WriteTexture {
      texture: texture.texture.id(),
      mip_level: texture.mip_level,
      origin: texture.origin,
      data: data.get(start..).unwrap_or_default().to_vec(),
      bytes_per_row,
      rows_per_image: layout.rows_per_image.unwrap_or(size.height),
      size,
    });
  }

  pub fn submit(&self, buffers: impl IntoIterator<Item = CommandBuffer>) {
    for buffer in buffers {
      self.shared.push(Op::Submit(buffer.commands));
      // What it used may be released now.
      drop(buffer.keep);
    }
  }

  pub fn on_submitted_work_done(&self, callback: impl FnOnce() + Send + 'static) {
    self.shared.lock().work_done.push(Box::new(callback));
  }

  pub fn get_timestamp_period(&self) -> f32 {
    1.0
  }

  /// Show the frame.
  pub fn present(&self, _texture: SurfaceTexture) {
    self.shared.push(Op::Present);
  }
}

// --- commands ----------------------------------------------------------------------

#[derive(Default)]
struct EncoderInner {
  commands: Vec<Command>,
  keep: Vec<Keep>,
}

/// Records commands.
pub struct CommandEncoder {
  inner: Arc<Mutex<EncoderInner>>,
}

fn lock_encoder(inner: &Mutex<EncoderInner>) -> MutexGuard<'_, EncoderInner> {
  inner
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl CommandEncoder {
  fn record(&self, command: Command, keep: impl IntoIterator<Item = Keep>) {
    let mut inner = lock_encoder(&self.inner);
    inner.commands.push(command);
    inner.keep.extend(keep);
  }

  pub fn begin_render_pass(&mut self, desc: &RenderPassDescriptor<'_>) -> RenderPass<'_> {
    let mut keep: Vec<Keep> = Vec::new();
    let colour = desc
      .color_attachments
      .iter()
      .map(|attachment| {
        attachment.as_ref().map(|attachment| {
          keep.push(attachment.view.inner.clone());
          ColourTarget {
            view: attachment.view.id(),
            clear: match attachment.ops.load {
              LoadOp::Clear(colour) => Some(colour),
              LoadOp::Load => None,
            },
            store: attachment.ops.store == StoreOp::Store,
          }
        })
      })
      .collect();
    let depth = desc.depth_stencil_attachment.as_ref().map(|attachment| {
      keep.push(attachment.view.inner.clone());
      let ops = attachment.depth_ops.unwrap_or(Operations {
        load: LoadOp::Load,
        store: StoreOp::Store,
      });
      DepthTarget {
        view: attachment.view.id(),
        clear: match ops.load {
          LoadOp::Clear(depth) => Some(depth),
          LoadOp::Load => None,
        },
        store: ops.store == StoreOp::Store,
      }
    });
    self.record(Command::BeginRenderPass { colour, depth }, keep);
    RenderPass {
      encoder: Arc::clone(&self.inner),
      _encoder: std::marker::PhantomData,
    }
  }

  pub fn begin_compute_pass(&mut self, _desc: &ComputePassDescriptor<'_>) -> ComputePass<'_> {
    self.record(Command::BeginComputePass, []);
    ComputePass {
      encoder: Arc::clone(&self.inner),
      _encoder: std::marker::PhantomData,
    }
  }

  pub fn copy_buffer_to_buffer(
    &mut self,
    source: &Buffer,
    source_offset: u64,
    destination: &Buffer,
    destination_offset: u64,
    size: u64,
  ) {
    self.record(
      Command::CopyBufferToBuffer {
        source: source.id(),
        source_offset,
        destination: destination.id(),
        destination_offset,
        size,
      },
      [
        source.inner.clone() as Keep,
        destination.inner.clone() as Keep,
      ],
    );
  }

  pub fn copy_texture_to_texture(
    &mut self,
    source: TexelCopyTextureInfo<'_>,
    destination: TexelCopyTextureInfo<'_>,
    size: Extent3d,
  ) {
    self.record(
      Command::CopyTextureToTexture {
        source: TextureCopy {
          texture: source.texture.id(),
          mip_level: source.mip_level,
          origin: source.origin,
        },
        destination: TextureCopy {
          texture: destination.texture.id(),
          mip_level: destination.mip_level,
          origin: destination.origin,
        },
        size,
      },
      [
        source.texture.inner.clone() as Keep,
        destination.texture.inner.clone() as Keep,
      ],
    );
  }

  /// Timestamp queries do not exist natively.
  pub fn resolve_query_set(
    &mut self,
    _set: &QuerySet,
    _queries: Range<u32>,
    _destination: &Buffer,
    _offset: u64,
  ) {
  }

  pub fn finish(self) -> CommandBuffer {
    let mut inner = lock_encoder(&self.inner);
    CommandBuffer {
      commands: std::mem::take(&mut inner.commands),
      keep: std::mem::take(&mut inner.keep),
    }
  }
}

/// Finished commands, waiting to be submitted.
pub struct CommandBuffer {
  commands: Vec<Command>,
  keep: Vec<Keep>,
}

/// A render pass. It ends when it drops.
pub struct RenderPass<'a> {
  encoder: Arc<Mutex<EncoderInner>>,
  _encoder: std::marker::PhantomData<&'a mut ()>,
}

impl RenderPass<'_> {
  fn record(&mut self, command: Command, keep: impl IntoIterator<Item = Keep>) {
    let mut inner = lock_encoder(&self.encoder);
    inner.commands.push(command);
    inner.keep.extend(keep);
  }

  /// The pass, no longer borrowing its encoder.
  pub fn forget_lifetime(self) -> RenderPass<'static> {
    let pass = RenderPass {
      encoder: Arc::clone(&self.encoder),
      _encoder: std::marker::PhantomData,
    };
    // Ending is the returned pass's job.
    std::mem::forget(self);
    pass
  }

  pub fn set_pipeline(&mut self, pipeline: &RenderPipeline) {
    self.record(
      Command::SetRenderPipeline(pipeline.id()),
      [pipeline.inner.clone() as Keep],
    );
  }

  pub fn set_bind_group<'b>(
    &mut self,
    index: u32,
    group: impl Into<Option<&'b BindGroup>>,
    _offsets: &[u32],
  ) {
    let group = group.into();
    self.record(
      Command::SetBindGroup {
        index,
        group: group.map(BindGroup::id),
      },
      group.map(|group| group.inner.clone() as Keep),
    );
  }

  pub fn set_vertex_buffer(&mut self, slot: u32, slice: BufferSlice<'_>) {
    self.record(
      Command::SetVertexBuffer {
        slot,
        buffer: slice.buffer.id(),
        offset: slice.offset,
        size: slice.size,
      },
      [slice.buffer.inner.clone() as Keep],
    );
  }

  pub fn set_index_buffer(&mut self, slice: BufferSlice<'_>, format: IndexFormat) {
    self.record(
      Command::SetIndexBuffer {
        buffer: slice.buffer.id(),
        format,
        offset: slice.offset,
      },
      [slice.buffer.inner.clone() as Keep],
    );
  }

  pub fn set_viewport(
    &mut self,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    min_depth: f32,
    max_depth: f32,
  ) {
    self.record(
      Command::SetViewport {
        x,
        y,
        width,
        height,
        min_depth,
        max_depth,
      },
      [],
    );
  }

  pub fn draw(&mut self, vertices: Range<u32>, instances: Range<u32>) {
    self.record(
      Command::Draw {
        vertices,
        instances,
      },
      [],
    );
  }

  pub fn draw_indexed(&mut self, indices: Range<u32>, base_vertex: i32, instances: Range<u32>) {
    self.record(
      Command::DrawIndexed {
        indices,
        base_vertex,
        instances,
      },
      [],
    );
  }

  pub fn draw_indirect(&mut self, buffer: &Buffer, offset: u64) {
    self.record(
      Command::DrawIndirect {
        buffer: buffer.id(),
        offset,
      },
      [buffer.inner.clone() as Keep],
    );
  }

  pub fn draw_indexed_indirect(&mut self, buffer: &Buffer, offset: u64) {
    self.record(
      Command::DrawIndexedIndirect {
        buffer: buffer.id(),
        offset,
      },
      [buffer.inner.clone() as Keep],
    );
  }
}

impl Drop for RenderPass<'_> {
  fn drop(&mut self) {
    lock_encoder(&self.encoder)
      .commands
      .push(Command::EndRenderPass);
  }
}

/// A compute pass. It ends when it drops.
pub struct ComputePass<'a> {
  encoder: Arc<Mutex<EncoderInner>>,
  _encoder: std::marker::PhantomData<&'a mut ()>,
}

impl ComputePass<'_> {
  fn record(&mut self, command: Command, keep: impl IntoIterator<Item = Keep>) {
    let mut inner = lock_encoder(&self.encoder);
    inner.commands.push(command);
    inner.keep.extend(keep);
  }

  pub fn set_pipeline(&mut self, pipeline: &ComputePipeline) {
    self.record(
      Command::SetComputePipeline(pipeline.id()),
      [pipeline.inner.clone() as Keep],
    );
  }

  pub fn set_bind_group<'b>(
    &mut self,
    index: u32,
    group: impl Into<Option<&'b BindGroup>>,
    _offsets: &[u32],
  ) {
    let group = group.into();
    self.record(
      Command::SetBindGroup {
        index,
        group: group.map(BindGroup::id),
      },
      group.map(|group| group.inner.clone() as Keep),
    );
  }

  pub fn dispatch_workgroups(&mut self, x: u32, y: u32, z: u32) {
    self.record(Command::Dispatch { x, y, z }, []);
  }
}

impl Drop for ComputePass<'_> {
  fn drop(&mut self) {
    lock_encoder(&self.encoder)
      .commands
      .push(Command::EndComputePass);
  }
}

// --- the surface -----------------------------------------------------------------------

/// The surface's size and format.
#[derive(Clone, Debug)]
pub struct SurfaceConfiguration {
  pub format: TextureFormat,
  pub width: u32,
  pub height: u32,
}

/// The host's render target. The lifetime only mirrors wgpu's.
pub struct Surface<'window> {
  shared: Arc<Shared>,
  config: SurfaceConfiguration,
  _window: std::marker::PhantomData<&'window ()>,
}

/// A frame's render target.
pub struct SurfaceTexture {
  pub texture: Texture,
}

/// What [`Surface::get_current_texture`] returns.
pub enum CurrentSurfaceTexture {
  Success(SurfaceTexture),
  Suboptimal(SurfaceTexture),
  Timeout,
  Occluded,
  Outdated,
  Lost,
  Validation,
}

impl Surface<'_> {
  /// The size and format given, for [`Self::configure`].
  pub fn config(&self) -> SurfaceConfiguration {
    self.config.clone()
  }

  /// Resize the host's render target.
  pub fn configure(&mut self, _device: &Device, config: &SurfaceConfiguration) {
    self.config = config.clone();
    self.shared.push(Op::ConfigureSurface {
      width: config.width,
      height: config.height,
      format: config.format,
    });
  }

  /// This frame's render target: always [`OUTPUT_TEXTURE`].
  pub fn get_current_texture(&self) -> CurrentSurfaceTexture {
    CurrentSurfaceTexture::Success(SurfaceTexture {
      texture: Texture {
        inner: Arc::new(TextureInner {
          res: Res {
            id: OUTPUT_TEXTURE,
            shared: Arc::clone(&self.shared),
          },
          size: Extent3d {
            width: self.config.width,
            height: self.config.height,
            depth_or_array_layers: 1,
          },
          mip_level_count: 1,
          format: self.config.format,
        }),
      },
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn attributes_are_packed_in_order() {
    const ATTRIBUTES: [VertexAttribute; 3] =
      vertex_attr_array![0 => Float32x3, 1 => Snorm16x2, 4 => Uint8x4];
    assert_eq!(
      ATTRIBUTES.map(|attribute| (attribute.shader_location, attribute.offset)),
      [(0, 0), (1, 12), (4, 16)]
    );
  }

  #[test]
  fn a_release_follows_the_submit_that_used_it() {
    let recorder = Recorder::new();
    let (device, queue) = recorder.device();
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: None });
    {
      let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("scratch"),
        size: 16,
        usage: BufferUsages::STORAGE,
        mapped_at_creation: false,
      });
      let group = device.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: &BindGroupLayout,
        entries: &[BindGroupEntry {
          binding: 3,
          resource: buffer.as_entire_binding(),
        }],
      });
      let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
      pass.set_bind_group(0, &group, &[]);
      pass.dispatch_workgroups(1, 1, 1);
    }
    // Both handles have dropped, but the pass still holds them.
    assert!(!recorder
      .take_ops()
      .iter()
      .any(|op| matches!(op, Op::Release(_))));
    queue.submit([encoder.finish()]);
    let ops = recorder.take_ops();
    assert!(matches!(&ops[0], Op::Submit(commands) if commands.len() == 4));
    assert_eq!(
      ops[1..]
        .iter()
        .filter(|op| matches!(op, Op::Release(_)))
        .count(),
      2
    );
  }

  #[test]
  fn mapped_buffers_write_on_unmap_and_read_back() {
    let recorder = Recorder::new();
    let (device, _queue) = recorder.device();
    let buffer = device.create_buffer(&BufferDescriptor {
      label: None,
      size: 8,
      usage: BufferUsages::UNIFORM,
      mapped_at_creation: true,
    });

    if let Ok(mut range) = buffer.slice(..).get_mapped_range_mut() {
      range.copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    buffer.unmap();
    let ops = recorder.take_ops();
    assert!(
      matches!(&ops[1], Op::WriteBuffer { offset: 0, data, .. } if data == &[1, 2, 3, 4, 5, 6, 7, 8])
    );

    let read = Arc::new(Mutex::new(Vec::new()));
    let reader = buffer.clone();
    let out = Arc::clone(&read);
    buffer.slice(..).map_async(MapMode::Read, move |result| {
      assert!(result.is_ok());

      if let (Ok(view), Ok(mut out)) = (reader.slice(4..).get_mapped_range(), out.lock()) {
        out.extend_from_slice(&view);
      }

      reader.unmap();
    });
    assert!(recorder.complete_map(buffer.id(), &[9, 9, 9, 9, 4, 3, 2, 1]));
    assert_eq!(*read.lock().unwrap(), vec![4, 3, 2, 1]);
    // An unmapped read is not written back.
    assert!(recorder
      .take_ops()
      .iter()
      .all(|op| !matches!(op, Op::WriteBuffer { .. })));
  }
}
