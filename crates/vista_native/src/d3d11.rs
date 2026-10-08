//! Lowers the renderer's recording (`vista_wasm::render::recorder`) to the
//! Direct3D 11 command stream described in `include/vista_d3d11.h`.
//!
//! The recording is WebGPU's model: bind groups matched to a pipeline's
//! layout, passes that begin and end, views of any kind. This turns it
//! into Direct3D 11's: registers per stage from the translated shaders'
//! manifest, views created for how each resource is bound, render
//! targets set and cleared, and what Direct3D 11.0 needs that WebGPU does
//! not:
//!
//! - Constant buffers are written whole: partial writes go to a copy
//!   kept here, and bytes the GPU copied in are copied in again after.
//! - Shaders that read `vertex_index` or `instance_index` get the draw's
//!   first vertex and instance in a constant buffer.
//! - Depth textures are typeless, with depth and float views.
//! - A 2D view of one layer of a layered texture is read from a 2D copy
//!   of that layer, refreshed when the layer changes.
//! - Every slot a draw or dispatch does not use is unbound, and passes
//!   start from nothing bound, so a resource is never an input and an
//!   output at once.

use std::collections::{HashMap, HashSet};

use vista_wasm::render::recorder::{
  self as rec, BlendFactor, BlendOperation, BufferUsages, Command, CompareFunction, Id, Op,
  Resource, TextureDimension, TextureFormat, TextureUsages, TextureViewDimension, VertexFormat,
  OUTPUT_TEXTURE,
};

mod table {
  /// A pipeline stage.
  #[derive(Clone, Copy, Debug, Eq, PartialEq)]
  pub enum Stage {
    Vertex,
    Fragment,
    Compute,
  }

  /// One WGSL binding's register.
  #[derive(Clone, Copy, Debug)]
  pub struct Binding {
    pub group: u32,
    pub binding: u32,
    /// `b`, `t`, `s` or `u`.
    pub class: u8,
    pub register: u32,
  }

  /// One translated, compiled entry point.
  #[derive(Debug)]
  pub struct ShaderFile {
    pub file: &'static str,
    pub module: &'static str,
    pub entry: &'static str,
    pub stage: Stage,
    pub overrides: &'static [(&'static str, bool)],
    pub bindings: &'static [Binding],
    pub special_constants: Option<u32>,
    pub cso: &'static [u8],
  }

  include!(concat!(env!("OUT_DIR"), "/d3d11_shaders.rs"));
}

pub use table::{ShaderFile, Stage, COMPILED, FILES, SOURCES};

// --- Direct3D 11 values ------------------------------------------------------------

pub mod op {
  pub const CREATE_BUFFER: u32 = 1;
  pub const CREATE_TEXTURE: u32 = 2;
  pub const CREATE_SRV: u32 = 3;
  pub const CREATE_UAV: u32 = 4;
  pub const CREATE_RTV: u32 = 5;
  pub const CREATE_DSV: u32 = 6;
  pub const CREATE_SAMPLER: u32 = 7;
  pub const CREATE_SHADER: u32 = 8;
  pub const CREATE_INPUT_LAYOUT: u32 = 9;
  pub const CREATE_BLEND: u32 = 10;
  pub const CREATE_RASTERIZER: u32 = 11;
  pub const CREATE_DEPTH: u32 = 12;
  pub const RELEASE: u32 = 13;
  pub const UPDATE_BUFFER: u32 = 20;
  pub const UPDATE_TEXTURE: u32 = 21;
  pub const COPY_BUFFER: u32 = 22;
  pub const COPY_TEXTURE: u32 = 23;
  pub const READBACK: u32 = 24;
  pub const SET_TARGETS: u32 = 30;
  pub const CLEAR_RTV: u32 = 31;
  pub const CLEAR_DSV: u32 = 32;
  pub const SET_VIEWPORT: u32 = 33;
  pub const SET_GRAPHICS: u32 = 34;
  pub const SET_COMPUTE: u32 = 35;
  pub const BIND: u32 = 36;
  pub const UNBIND_ALL: u32 = 37;
  pub const SET_VERTEX_BUFFER: u32 = 38;
  pub const SET_INDEX_BUFFER: u32 = 39;
  pub const DRAW: u32 = 40;
  pub const DRAW_INDEXED: u32 = 41;
  pub const DRAW_INDIRECT: u32 = 42;
  pub const DRAW_INDEXED_INDIRECT: u32 = 43;
  pub const DISPATCH: u32 = 44;
  pub const PRESENT: u32 = 50;
  pub const OUTPUT_SIZE: u32 = 51;
}

const BIND_VERTEX_BUFFER: u32 = 0x1;
const BIND_INDEX_BUFFER: u32 = 0x2;
const BIND_CONSTANT_BUFFER: u32 = 0x4;
const BIND_SHADER_RESOURCE: u32 = 0x8;
const BIND_RENDER_TARGET: u32 = 0x20;
const BIND_DEPTH_STENCIL: u32 = 0x40;
const BIND_UNORDERED_ACCESS: u32 = 0x80;
const MISC_DRAWINDIRECT_ARGS: u32 = 0x10;
const MISC_BUFFER_ALLOW_RAW_VIEWS: u32 = 0x20;

const SRV_BUFFEREX: u32 = 11;
const SRV_TEXTURE2D: u32 = 4;
const SRV_TEXTURE2DARRAY: u32 = 5;
const SRV_TEXTURE3D: u32 = 8;
const UAV_BUFFER: u32 = 1;
const UAV_TEXTURE2D: u32 = 4;
const UAV_TEXTURE2DARRAY: u32 = 5;
const UAV_TEXTURE3D: u32 = 8;
const RTV_TEXTURE2D: u32 = 4;
const RTV_TEXTURE2DARRAY: u32 = 5;
const DSV_TEXTURE2D: u32 = 3;
const DSV_TEXTURE2DARRAY: u32 = 4;
const RAW_FLAG: u32 = 1;

const DXGI_R32_TYPELESS: u32 = 39;
const DXGI_D32_FLOAT: u32 = 40;
const DXGI_R32_FLOAT: u32 = 41;
const DXGI_R16_UINT: u32 = 57;
const DXGI_R32_UINT: u32 = 42;

/// Slots per stage that are ever cleared or tracked.
pub const CONSTANT_SLOTS: usize = 14;
pub const RESOURCE_SLOTS: usize = 128;
pub const SAMPLER_SLOTS: usize = 16;
pub const UNORDERED_SLOTS: usize = 8;

/// Ids the lowering makes for itself: views, aliases, shaders and states.
const FIRST_OWN_ID: u32 = 0x8000_0000;

/// A texture format as DXGI's.
pub fn dxgi_format(format: TextureFormat) -> u32 {
  match format {
    TextureFormat::R8Unorm => 61,
    TextureFormat::Rg8Unorm => 49,
    TextureFormat::Rgba8Unorm => 28,
    TextureFormat::Rgba8UnormSrgb => 29,
    TextureFormat::Bgra8Unorm => 87,
    TextureFormat::Bgra8UnormSrgb => 91,
    TextureFormat::Rgba16Float => 10,
    TextureFormat::R32Float => DXGI_R32_FLOAT,
    TextureFormat::Depth32Float => DXGI_D32_FLOAT,
  }
}

/// A vertex attribute format as DXGI's.
fn dxgi_vertex(format: VertexFormat) -> u32 {
  match format {
    VertexFormat::Float32 => 41,
    VertexFormat::Float32x2 => 16,
    VertexFormat::Float32x3 => 6,
    VertexFormat::Float32x4 => 2,
    VertexFormat::Snorm16x2 => 37,
    VertexFormat::Snorm16x4 => 13,
    VertexFormat::Uint32x3 => 7,
    VertexFormat::Unorm8x4 => 28,
    VertexFormat::Uint8x4 => 30,
  }
}

fn comparison(function: CompareFunction) -> u32 {
  match function {
    CompareFunction::Never => 1,
    CompareFunction::Less => 2,
    CompareFunction::Equal => 3,
    CompareFunction::LessEqual => 4,
    CompareFunction::Greater => 5,
    CompareFunction::NotEqual => 6,
    CompareFunction::GreaterEqual => 7,
    CompareFunction::Always => 8,
  }
}

/// A blend factor as D3D11_BLEND, for the colour or the alpha channel,
/// which may not use colour factors.
fn blend_factor(factor: BlendFactor, alpha: bool) -> u32 {
  match (factor, alpha) {
    (BlendFactor::Zero, _) => 1,
    (BlendFactor::One, _) => 2,
    (BlendFactor::Src, false) => 3,
    (BlendFactor::OneMinusSrc, false) => 4,
    (BlendFactor::SrcAlpha, _) | (BlendFactor::Src, true) => 5,
    (BlendFactor::OneMinusSrcAlpha, _) | (BlendFactor::OneMinusSrc, true) => 6,
    (BlendFactor::DstAlpha, _) | (BlendFactor::Dst, true) => 7,
    (BlendFactor::OneMinusDstAlpha, _) | (BlendFactor::OneMinusDst, true) => 8,
    (BlendFactor::Dst, false) => 9,
    (BlendFactor::OneMinusDst, false) => 10,
  }
}

fn blend_operation(operation: BlendOperation) -> u32 {
  match operation {
    BlendOperation::Add => 1,
    BlendOperation::Subtract => 2,
    BlendOperation::ReverseSubtract => 3,
    BlendOperation::Min => 4,
    BlendOperation::Max => 5,
  }
}

/// D3D11_FILTER for a sampler.
fn filter(desc: &rec::SamplerDesc) -> u32 {
  let linear = |on: bool, bit: u32| if on { bit } else { 0 };
  let mip = desc.mipmap_filter == rec::MipmapFilterMode::Linear;
  let mag = desc.mag_filter == rec::FilterMode::Linear;
  let min = desc.min_filter == rec::FilterMode::Linear;
  let base = if desc.anisotropy_clamp > 1 && mip && mag && min {
    0x55
  } else {
    linear(mip, 0x1) | linear(mag, 0x4) | linear(min, 0x10)
  };
  base | if desc.compare.is_some() { 0x80 } else { 0 }
}

fn address(mode: rec::AddressMode) -> u32 {
  match mode {
    rec::AddressMode::Repeat => 1,
    rec::AddressMode::MirrorRepeat => 2,
    rec::AddressMode::ClampToEdge => 3,
  }
}

// --- what the lowering tracks -------------------------------------------------------

struct BufferInfo {
  size: u32,
  constant: bool,
  /// A constant buffer's contents, so writes can be whole.
  shadow: Vec<u8>,
  /// Bytes the GPU copied into a constant buffer: destination offset,
  /// source buffer, source offset and size. They are copied again after
  /// each whole write.
  gpu_copies: Vec<(u32, Id, u32, u32)>,
  /// Raw views made of it: (is UAV, offset, size) to view id.
  views: HashMap<(bool, u32, u32), u32>,
}

struct Alias {
  id: u32,
  srv: HashMap<(u32, u32), u32>,
  /// The texture's generation it was copied at.
  generation: u64,
}

struct TextureInfo {
  dimension: TextureDimension,
  width: u32,
  height: u32,
  layers: u32,
  mips: u32,
  format: TextureFormat,
  /// Bumped at every write, so aliases know when to copy again.
  generation: u64,
  /// 2D copies of single layers, for 2D views of a layered texture.
  aliases: HashMap<u32, Alias>,
}

impl TextureInfo {
  fn depth(&self) -> bool {
    self.format == TextureFormat::Depth32Float
  }

  fn subresource(&self, mip: u32, layer: u32) -> u32 {
    mip + layer * self.mips
  }
}

/// The kinds of view one recorded view may need.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum ViewKind {
  Srv,
  Uav,
  Rtv,
  Dsv,
}

struct ViewInfo {
  texture: Id,
  dimension: TextureViewDimension,
  base_mip: u32,
  mips: u32,
  base_layer: u32,
  layers: u32,
  made: HashMap<ViewKind, u32>,
}

struct RenderPipe {
  vertex: &'static ShaderFile,
  fragment: Option<&'static ShaderFile>,
  vertex_shader: u32,
  pixel_shader: u32,
  input_layout: u32,
  blend: u32,
  rasterizer: u32,
  depth: u32,
  topology: u32,
  strides: Vec<u32>,
}

struct ComputePipe {
  shader: &'static ShaderFile,
  id: u32,
}

enum Pipeline {
  Render(RenderPipe),
  Compute(ComputePipe),
  /// A pipeline whose shaders the port lacks: its draws are skipped.
  Missing,
}

/// One stage's bound slots, as the executor has them.
#[derive(Clone)]
struct Slots {
  constant: [u32; CONSTANT_SLOTS],
  resource: Vec<u32>,
  sampler: [u32; SAMPLER_SLOTS],
  unordered: [u32; UNORDERED_SLOTS],
}

impl Default for Slots {
  fn default() -> Self {
    Self {
      constant: [0; CONSTANT_SLOTS],
      resource: vec![0; RESOURCE_SLOTS],
      sampler: [0; SAMPLER_SLOTS],
      unordered: [0; UNORDERED_SLOTS],
    }
  }
}

impl Slots {
  fn slot(&mut self, kind: u32, register: usize) -> Option<&mut u32> {
    match kind {
      0 => self.constant.get_mut(register),
      1 => self.resource.get_mut(register),
      2 => self.sampler.get_mut(register),
      _ => self.unordered.get_mut(register),
    }
  }
}

/// Pass state, as WebGPU defines it: reset at each pass.
#[derive(Default)]
struct PassState {
  pipeline: Option<Id>,
  groups: [Option<Id>; 4],
  vertex_buffers: [Option<(Id, u64)>; 8],
  index_buffer: Option<(Id, rec::IndexFormat, u64)>,
  /// What the executor has now.
  set_pipeline: Option<Id>,
  set_vertex_buffers: [Option<(Id, u64, u32)>; 8],
  set_index_buffer: Option<(Id, rec::IndexFormat, u64)>,
  slots: [Slots; 3],
  special: Option<(i32, u32)>,
}

/// Lowers recordings to Direct3D 11 command streams. One per renderer:
/// it remembers what the earlier streams created.
pub struct Lowering {
  out: Vec<u8>,
  next_own: u32,
  buffers: HashMap<Id, BufferInfo>,
  textures: HashMap<Id, TextureInfo>,
  views: HashMap<Id, ViewInfo>,
  groups: HashMap<Id, Vec<(u32, Resource)>>,
  modules: HashMap<Id, String>,
  pipelines: HashMap<Id, Pipeline>,
  shaders: HashMap<&'static str, u32>,
  pass: PassState,
  special: u32,
  /// Releases waiting until no constant buffer copies from them.
  pinned: HashMap<Id, u32>,
  pending_release: HashSet<Id>,
  warned: HashSet<String>,
  /// Problems found since the last [`Self::take_warnings`].
  warnings: Vec<String>,
}

impl Lowering {
  /// A lowering for a renderer whose output is `width` x `height` in
  /// `format`.
  pub fn new(width: u32, height: u32, format: TextureFormat) -> Self {
    let mut lowering = Self {
      out: Vec::new(),
      next_own: FIRST_OWN_ID,
      buffers: HashMap::new(),
      textures: HashMap::new(),
      views: HashMap::new(),
      groups: HashMap::new(),
      modules: HashMap::new(),
      pipelines: HashMap::new(),
      shaders: HashMap::new(),
      pass: PassState::default(),
      special: 0,
      pinned: HashMap::new(),
      pending_release: HashSet::new(),
      warned: HashSet::new(),
      warnings: Vec::new(),
    };
    lowering.set_output(width, height, format);
    lowering.special = lowering.own_id();
    lowering.emit(
      op::CREATE_BUFFER,
      &[lowering.special, 16, BIND_CONSTANT_BUFFER, 0],
    );
    lowering
  }

  fn set_output(&mut self, width: u32, height: u32, format: TextureFormat) {
    self.textures.insert(
      OUTPUT_TEXTURE,
      TextureInfo {
        dimension: TextureDimension::D2,
        width,
        height,
        layers: 1,
        mips: 1,
        format,
        generation: 0,
        aliases: HashMap::new(),
      },
    );
    self.emit(op::OUTPUT_SIZE, &[width, height, dxgi_format(format)]);
  }

  /// Lower `ops`, appending to the stream.
  pub fn lower(&mut self, ops: Vec<Op>) {
    for op in ops {
      self.lower_op(op);
    }
  }

  /// The stream lowered since the last call.
  pub fn take_stream(&mut self) -> Vec<u8> {
    std::mem::take(&mut self.out)
  }

  /// What went wrong since the last call: shaders the port lacks and
  /// bindings that could not be made. Each is reported once.
  pub fn take_warnings(&mut self) -> Vec<String> {
    std::mem::take(&mut self.warnings)
  }

  fn warn(&mut self, message: String) {
    if self.warned.insert(message.clone()) {
      self.warnings.push(message);
    }
  }

  fn own_id(&mut self) -> u32 {
    let id = self.next_own;
    self.next_own = self.next_own.wrapping_add(1).max(FIRST_OWN_ID);
    id
  }

  fn emit(&mut self, op: u32, words: &[u32]) {
    self.emit_with(op, words, &[]);
  }

  fn emit_with(&mut self, op: u32, words: &[u32], bytes: &[u8]) {
    let padded = bytes.len().div_ceil(4) * 4;
    let size = words.len() * 4 + padded;
    self.out.extend_from_slice(&op.to_le_bytes());
    self.out.extend_from_slice(&(size as u32).to_le_bytes());

    for word in words {
      self.out.extend_from_slice(&word.to_le_bytes());
    }

    self.out.extend_from_slice(bytes);
    self.out.resize(self.out.len() + padded - bytes.len(), 0);
  }

  fn lower_op(&mut self, op: Op) {
    match op {
      Op::CreateBuffer {
        id, size, usage, ..
      } => self.create_buffer(id, size, usage),
      Op::CreateTexture {
        id,
        size,
        mip_level_count,
        dimension,
        format,
        usage,
        ..
      } => self.create_texture(id, size, mip_level_count, dimension, format, usage),
      Op::CreateView {
        id,
        texture,
        dimension,
        base_mip_level,
        mip_level_count,
        base_array_layer,
        array_layer_count,
      } => {
        let Some(info) = self.textures.get(&texture) else {
          self.warn(format!(
            "a view of texture #{texture}, which does not exist"
          ));
          return;
        };
        let dimension = dimension.unwrap_or(match info.dimension {
          TextureDimension::D3 => TextureViewDimension::D3,
          _ if info.layers > 1 => TextureViewDimension::D2Array,
          _ => TextureViewDimension::D2,
        });
        let layers = if dimension == TextureViewDimension::D3 {
          1
        } else {
          array_layer_count.unwrap_or(info.layers.saturating_sub(base_array_layer))
        };
        self.views.insert(
          id,
          ViewInfo {
            texture,
            dimension,
            base_mip: base_mip_level,
            mips: mip_level_count.unwrap_or(info.mips.saturating_sub(base_mip_level)),
            base_layer: base_array_layer,
            layers,
            made: HashMap::new(),
          },
        );
      }
      Op::CreateSampler { id, desc } => {
        let words = [
          id,
          filter(&desc),
          address(desc.address_mode_u),
          address(desc.address_mode_v),
          address(desc.address_mode_w),
          u32::from(desc.anisotropy_clamp.clamp(1, 16)),
          desc.compare.map_or(1, comparison),
          desc.lod_min_clamp.to_bits(),
          desc.lod_max_clamp.to_bits(),
        ];
        self.emit(op::CREATE_SAMPLER, &words);
      }
      Op::CreateBindGroup { id, entries, .. } => {
        self.groups.insert(id, entries);
      }
      Op::CreateShaderModule { id, module } => {
        self.modules.insert(id, module);
      }
      Op::CreateRenderPipeline { id, desc } => {
        let pipeline = self.render_pipeline(&desc);
        self.pipelines.insert(id, pipeline);
      }
      Op::CreateComputePipeline {
        id,
        module,
        entry_point,
        constants,
        ..
      } => {
        let pipeline = match self.find_shader(module, Stage::Compute, &entry_point, &constants) {
          Some(shader) => Pipeline::Compute(ComputePipe {
            shader,
            id: self.shader(shader),
          }),
          None => Pipeline::Missing,
        };
        self.pipelines.insert(id, pipeline);
      }
      Op::WriteBuffer {
        buffer,
        offset,
        data,
      } => self.write_buffer(buffer, offset, &data),
      Op::WriteTexture {
        texture,
        mip_level,
        origin,
        data,
        bytes_per_row,
        rows_per_image,
        size,
      } => self.write_texture(
        texture,
        mip_level,
        origin,
        &data,
        bytes_per_row,
        rows_per_image,
        size,
      ),
      Op::Submit(commands) => {
        self.pass = PassState::default();

        for command in commands {
          self.lower_command(command);
        }

        self.pass = PassState::default();
      }
      Op::MapRead { buffer } => {
        let size = self.buffers.get(&buffer).map_or(0, |info| info.size);
        self.emit(op::READBACK, &[buffer, size]);
      }
      Op::Present => self.emit(op::PRESENT, &[]),
      Op::ConfigureSurface {
        width,
        height,
        format,
      } => self.set_output(width, height, format),
      Op::Release(id) => {
        if self.pinned.contains_key(&id) {
          self.pending_release.insert(id);
        } else {
          self.release(id);
        }
      }
    }
  }

  // --- resources ---------------------------------------------------------------------

  fn create_buffer(&mut self, id: Id, size: u64, usage: BufferUsages) {
    let constant = usage.contains(BufferUsages::UNIFORM);
    let size = u32::try_from(size).unwrap_or(u32::MAX);
    let (byte_width, bind, misc) = if constant {
      (size.max(16).div_ceil(16) * 16, BIND_CONSTANT_BUFFER, 0)
    } else {
      let mut bind = 0;
      let mut misc = 0;

      if usage.contains(BufferUsages::VERTEX) {
        bind |= BIND_VERTEX_BUFFER;
      }

      if usage.contains(BufferUsages::INDEX) {
        bind |= BIND_INDEX_BUFFER;
      }

      if usage.contains(BufferUsages::INDIRECT) {
        misc |= MISC_DRAWINDIRECT_ARGS;
      }

      // Storage buffers are raw views. A buffer only copied to or from
      // gets a raw view's flags too, so it has a bind flag.
      if usage.contains(BufferUsages::STORAGE) || bind == 0 {
        bind |= BIND_SHADER_RESOURCE;
        misc |= MISC_BUFFER_ALLOW_RAW_VIEWS;
      }

      if usage.contains(BufferUsages::STORAGE) {
        bind |= BIND_UNORDERED_ACCESS;
      }

      (size.max(4).div_ceil(4) * 4, bind, misc)
    };
    self.buffers.insert(
      id,
      BufferInfo {
        size: byte_width,
        constant,
        shadow: if constant {
          vec![0; byte_width as usize]
        } else {
          Vec::new()
        },
        gpu_copies: Vec::new(),
        views: HashMap::new(),
      },
    );
    self.emit(op::CREATE_BUFFER, &[id, byte_width, bind, misc]);
  }

  fn create_texture(
    &mut self,
    id: Id,
    size: rec::Extent3d,
    mips: u32,
    dimension: TextureDimension,
    format: TextureFormat,
    usage: TextureUsages,
  ) {
    let depth = format == TextureFormat::Depth32Float;
    let mut bind = 0;

    if usage.contains(TextureUsages::TEXTURE_BINDING) {
      bind |= BIND_SHADER_RESOURCE;
    }

    if usage.contains(TextureUsages::STORAGE_BINDING) {
      bind |= BIND_UNORDERED_ACCESS;
    }

    if usage.contains(TextureUsages::RENDER_ATTACHMENT) {
      bind |= if depth {
        BIND_DEPTH_STENCIL
      } else {
        BIND_RENDER_TARGET
      };
    }

    // A texture only copied to or from still needs a bind flag.
    if bind == 0 {
      bind = BIND_SHADER_RESOURCE;
    }

    let format_code = if depth {
      DXGI_R32_TYPELESS
    } else {
      dxgi_format(format)
    };
    let dimension_code = match dimension {
      TextureDimension::D1 => 1,
      TextureDimension::D2 => 2,
      TextureDimension::D3 => 3,
    };
    self.textures.insert(
      id,
      TextureInfo {
        dimension,
        width: size.width,
        height: size.height,
        layers: size.depth_or_array_layers,
        mips,
        format,
        generation: 0,
        aliases: HashMap::new(),
      },
    );
    self.emit(
      op::CREATE_TEXTURE,
      &[
        id,
        dimension_code,
        size.width,
        size.height,
        size.depth_or_array_layers,
        mips,
        format_code,
        bind,
      ],
    );
  }

  fn release(&mut self, id: Id) {
    if let Some(buffer) = self.buffers.remove(&id) {
      for view in buffer.views.into_values() {
        self.emit(op::RELEASE, &[view]);
      }

      for (_, source, _, _) in buffer.gpu_copies {
        self.unpin(source);
      }

      self.emit(op::RELEASE, &[id]);
    } else if let Some(texture) = self.textures.remove(&id) {
      for alias in texture.aliases.into_values() {
        for view in alias.srv.into_values() {
          self.emit(op::RELEASE, &[view]);
        }

        self.emit(op::RELEASE, &[alias.id]);
      }

      self.emit(op::RELEASE, &[id]);
    } else if let Some(view) = self.views.remove(&id) {
      for made in view.made.into_values() {
        self.emit(op::RELEASE, &[made]);
      }
    } else if let Some(pipeline) = self.pipelines.remove(&id) {
      if let Pipeline::Render(pipe) = pipeline {
        for state in [pipe.input_layout, pipe.blend, pipe.rasterizer, pipe.depth] {
          if state != 0 {
            self.emit(op::RELEASE, &[state]);
          }
        }
      }
    } else if self.groups.remove(&id).is_none() && self.modules.remove(&id).is_none() {
      // A sampler.
      self.emit(op::RELEASE, &[id]);
    }
  }

  fn pin(&mut self, id: Id) {
    *self.pinned.entry(id).or_insert(0) += 1;
  }

  fn unpin(&mut self, id: Id) {
    let Some(count) = self.pinned.get_mut(&id) else {
      return;
    };
    *count -= 1;

    if *count == 0 {
      self.pinned.remove(&id);

      if self.pending_release.remove(&id) {
        self.release(id);
      }
    }
  }

  // --- data ----------------------------------------------------------------------------

  fn write_buffer(&mut self, buffer: Id, offset: u64, data: &[u8]) {
    let Some(info) = self.buffers.get_mut(&buffer) else {
      self.warn(format!("a write to buffer #{buffer}, which does not exist"));
      return;
    };
    let offset = offset as usize;

    if !info.constant {
      let end = (offset + data.len()).min(info.size as usize);
      let data = &data[..end.saturating_sub(offset)];
      self.emit_with(
        op::UPDATE_BUFFER,
        &[buffer, offset as u32, data.len() as u32],
        data,
      );
      return;
    }

    let end = (offset + data.len()).min(info.shadow.len());

    if offset < end {
      info.shadow[offset..end].copy_from_slice(&data[..end - offset]);
    }

    let shadow = info.shadow.clone();
    let copies = info.gpu_copies.clone();
    self.emit_with(
      op::UPDATE_BUFFER,
      &[buffer, 0, shadow.len() as u32],
      &shadow,
    );

    // The whole write covered what the GPU copied in, which was not in
    // the copy kept here.
    for (destination_offset, source, source_offset, size) in copies {
      self.emit(
        op::COPY_BUFFER,
        &[buffer, destination_offset, source, source_offset, size],
      );
    }
  }

  #[allow(clippy::too_many_arguments)]
  fn write_texture(
    &mut self,
    texture: Id,
    mip: u32,
    origin: rec::Origin3d,
    data: &[u8],
    bytes_per_row: u32,
    rows_per_image: u32,
    size: rec::Extent3d,
  ) {
    let Some(info) = self.textures.get_mut(&texture) else {
      self.warn(format!(
        "a write to texture #{texture}, which does not exist"
      ));
      return;
    };
    info.generation += 1;
    let image = bytes_per_row as usize * rows_per_image as usize;

    if info.dimension == TextureDimension::D3 {
      let subresource = mip;
      let bytes = &data[..data.len().min(image * size.depth_or_array_layers as usize)];
      self.emit_with(
        op::UPDATE_TEXTURE,
        &[
          texture,
          subresource,
          origin.x,
          origin.y,
          origin.z,
          origin.x + size.width,
          origin.y + size.height,
          origin.z + size.depth_or_array_layers,
          bytes_per_row,
          image as u32,
          bytes.len() as u32,
        ],
        bytes,
      );
      return;
    }

    let subresources: Vec<u32> = (0..size.depth_or_array_layers)
      .map(|layer| info.subresource(mip, origin.z + layer))
      .collect();

    for (layer, subresource) in subresources.into_iter().enumerate() {
      let start = (layer * image).min(data.len());
      let bytes = &data[start..(start + image).min(data.len())];
      self.emit_with(
        op::UPDATE_TEXTURE,
        &[
          texture,
          subresource,
          origin.x,
          origin.y,
          0,
          origin.x + size.width,
          origin.y + size.height,
          1,
          bytes_per_row,
          image as u32,
          bytes.len() as u32,
        ],
        bytes,
      );
    }
  }

  // --- shaders and pipelines ----------------------------------------------------------

  /// The translated entry point of `module` for `stage`, with override
  /// `constants` (any not given take the module's defaults).
  fn find_shader(
    &mut self,
    module: Id,
    stage: Stage,
    entry: &str,
    constants: &[(String, f64)],
  ) -> Option<&'static ShaderFile> {
    let name = self.modules.get(&module).cloned().unwrap_or_default();

    if name.is_empty() {
      self.warn(format!(
        "a shader module the Direct3D 11 port does not have (entry point {entry}): run `cargo run -p vista_hlsl` and ports/d3d11/tools/check-hlsl.sh"
      ));
      return None;
    }

    let default_file = format!("{name}/{entry}.hlsl");
    let defaults = FILES
      .iter()
      .find(|file| file.file == default_file)
      .map(|file| file.overrides);
    let wanted: Vec<(&str, bool)> = defaults
      .unwrap_or_default()
      .iter()
      .map(|(key, default)| {
        let value = constants
          .iter()
          .find(|(name, _)| name == key)
          .map_or(*default, |(_, value)| *value != 0.0);
        (*key, value)
      })
      .collect();
    let found = FILES.iter().find(|file| {
      file.module == name
        && file.stage == stage
        && file.entry == entry
        && wanted
          .iter()
          .all(|wanted| file.overrides.iter().any(|value| value == wanted))
    });

    if found.is_none() {
      self.warn(format!(
        "{name}::{entry} with {constants:?} is not in the Direct3D 11 port: run `cargo run -p vista_hlsl` and ports/d3d11/tools/check-hlsl.sh"
      ));
    }

    found
  }

  /// The shader object for `file`, created the first time.
  fn shader(&mut self, file: &'static ShaderFile) -> u32 {
    if let Some(id) = self.shaders.get(file.file) {
      return *id;
    }

    let id = self.own_id();
    let stage = match file.stage {
      Stage::Vertex => 0,
      Stage::Fragment => 1,
      Stage::Compute => 2,
    };
    self.emit_with(
      op::CREATE_SHADER,
      &[id, stage, file.cso.len() as u32],
      file.cso,
    );
    self.shaders.insert(file.file, id);
    id
  }

  fn render_pipeline(&mut self, desc: &rec::RenderPipelineDesc) -> Pipeline {
    let Some(vertex) = self.find_shader(
      desc.module,
      Stage::Vertex,
      &desc.vertex_entry,
      &desc.constants,
    ) else {
      return Pipeline::Missing;
    };
    let fragment = match &desc.fragment_entry {
      Some(entry) => match self.find_shader(desc.module, Stage::Fragment, entry, &desc.constants) {
        Some(file) => Some(file),
        None => return Pipeline::Missing,
      },
      None => None,
    };
    let vertex_shader = self.shader(vertex);
    let pixel_shader = fragment.map_or(0, |file| self.shader(file));
    let mut elements = Vec::new();

    for (slot, layout) in desc.buffers.iter().enumerate() {
      let Some(layout) = layout else {
        continue;
      };
      let instance = layout.step_mode == rec::VertexStepMode::Instance;

      for attribute in &layout.attributes {
        elements.extend([
          attribute.shader_location,
          dxgi_vertex(attribute.format),
          slot as u32,
          attribute.offset as u32,
          u32::from(instance),
          u32::from(instance),
        ]);
      }
    }

    let input_layout = if elements.is_empty() {
      0
    } else {
      let id = self.own_id();
      let mut words = vec![id, vertex_shader, (elements.len() / 6) as u32];
      words.extend(elements);
      self.emit(op::CREATE_INPUT_LAYOUT, &words);
      id
    };
    let blend = self.own_id();
    let mut words = vec![blend];

    for index in 0..8 {
      let target = desc.targets.get(index);
      words.extend(match target {
        Some(Some(target)) => match target.blend {
          Some(state) => [
            1,
            blend_factor(state.color.src_factor, false),
            blend_factor(state.color.dst_factor, false),
            blend_operation(state.color.operation),
            blend_factor(state.alpha.src_factor, true),
            blend_factor(state.alpha.dst_factor, true),
            blend_operation(state.alpha.operation),
            target.write_mask.0,
          ],
          None => [0, 2, 1, 1, 2, 1, 1, target.write_mask.0],
        },
        // A gap in the targets writes nothing.
        Some(None) => [0, 2, 1, 1, 2, 1, 1, 0],
        None => [0, 2, 1, 1, 2, 1, 1, 15],
      });
    }

    self.emit(op::CREATE_BLEND, &words);
    let bias = desc.depth.map(|depth| depth.bias).unwrap_or_default();
    let rasterizer = self.own_id();
    self.emit(
      op::CREATE_RASTERIZER,
      &[
        rasterizer,
        match desc.cull_mode {
          None => 1,
          Some(rec::Face::Front) => 2,
          Some(rec::Face::Back) => 3,
        },
        u32::from(desc.front_face == rec::FrontFace::Ccw),
        bias.constant as u32,
        bias.clamp.to_bits(),
        bias.slope_scale.to_bits(),
        1,
      ],
    );
    let depth = self.own_id();
    let (enable, write, function) = match desc.depth {
      Some(state) => (
        1,
        u32::from(state.depth_write_enabled.unwrap_or(false)),
        comparison(state.depth_compare.unwrap_or(CompareFunction::Always)),
      ),
      None => (0, 0, comparison(CompareFunction::Always)),
    };
    self.emit(op::CREATE_DEPTH, &[depth, enable, write, function]);
    let topology = match desc.topology {
      rec::PrimitiveTopology::PointList => 1,
      rec::PrimitiveTopology::LineList => 2,
      rec::PrimitiveTopology::LineStrip => 3,
      rec::PrimitiveTopology::TriangleList => 4,
      rec::PrimitiveTopology::TriangleStrip => 5,
    };
    Pipeline::Render(RenderPipe {
      vertex,
      fragment,
      vertex_shader,
      pixel_shader,
      input_layout,
      blend,
      rasterizer,
      depth,
      topology,
      strides: desc
        .buffers
        .iter()
        .map(|layout| {
          layout
            .as_ref()
            .map_or(0, |layout| layout.array_stride as u32)
        })
        .collect(),
    })
  }

  // --- views --------------------------------------------------------------------------

  /// The Direct3D view of `kind` for recorded view `id`, made the first
  /// time it is needed.
  fn view(&mut self, id: Id, kind: ViewKind) -> Option<u32> {
    let Some(view) = self.views.get(&id) else {
      self.warn(format!("view #{id} does not exist"));
      return None;
    };
    let (texture_id, dimension) = (view.texture, view.dimension);
    let (base_mip, mips, base_layer, layers) =
      (view.base_mip, view.mips, view.base_layer, view.layers);
    let texture = self.textures.get(&texture_id)?;
    let layered = texture.layers > 1 && texture.dimension == TextureDimension::D2;

    // A 2D view of one layer of a layered texture is read from a 2D copy.
    if kind == ViewKind::Srv && dimension == TextureViewDimension::D2 && layered {
      return self.alias_view(texture_id, base_layer, base_mip, mips);
    }

    if let Some(made) = view.made.get(&kind) {
      return Some(*made);
    }

    let format = match (texture.depth(), kind) {
      (true, ViewKind::Dsv) => DXGI_D32_FLOAT,
      (true, _) => DXGI_R32_FLOAT,
      (false, _) => dxgi_format(texture.format),
    };
    let depth_slices = (texture.layers >> base_mip).max(1);
    let (record, words) = match (kind, dimension) {
      (ViewKind::Srv, TextureViewDimension::D2) => {
        (op::CREATE_SRV, [SRV_TEXTURE2D, base_mip, mips, 0, 0])
      }
      (ViewKind::Srv, TextureViewDimension::D3) => {
        (op::CREATE_SRV, [SRV_TEXTURE3D, base_mip, mips, 0, 0])
      }
      (ViewKind::Srv, _) => (
        op::CREATE_SRV,
        [SRV_TEXTURE2DARRAY, base_mip, mips, base_layer, layers],
      ),
      (ViewKind::Uav, TextureViewDimension::D2) if !layered => {
        (op::CREATE_UAV, [UAV_TEXTURE2D, base_mip, 0, 0, 0])
      }
      (ViewKind::Uav, TextureViewDimension::D3) => (
        op::CREATE_UAV,
        [UAV_TEXTURE3D, base_mip, 0, depth_slices, 0],
      ),
      (ViewKind::Uav, _) => (
        op::CREATE_UAV,
        [UAV_TEXTURE2DARRAY, base_mip, base_layer, layers, 0],
      ),
      (ViewKind::Rtv, _) if !layered && dimension == TextureViewDimension::D2 => {
        (op::CREATE_RTV, [RTV_TEXTURE2D, base_mip, 0, 0, 0])
      }
      (ViewKind::Rtv, _) => (
        op::CREATE_RTV,
        [RTV_TEXTURE2DARRAY, base_mip, base_layer, layers, 0],
      ),
      (ViewKind::Dsv, _) if !layered && dimension == TextureViewDimension::D2 => {
        (op::CREATE_DSV, [DSV_TEXTURE2D, base_mip, 0, 0, 0])
      }
      (ViewKind::Dsv, _) => (
        op::CREATE_DSV,
        [DSV_TEXTURE2DARRAY, base_mip, base_layer, layers, 0],
      ),
    };
    let made = self.own_id();
    self.emit(
      record,
      &[
        made, texture_id, format, words[0], words[1], words[2], words[3], words[4],
      ],
    );

    if let Some(view) = self.views.get_mut(&id) {
      view.made.insert(kind, made);
    }

    Some(made)
  }

  /// A shader resource view of layer `layer` of a layered texture, through
  /// a 2D copy of it brought up to date first.
  fn alias_view(&mut self, texture_id: Id, layer: u32, base_mip: u32, mips: u32) -> Option<u32> {
    let texture = self.textures.get(&texture_id)?;
    let (width, height, all_mips, format, generation) = (
      texture.width,
      texture.height,
      texture.mips,
      texture.format,
      texture.generation,
    );
    let existing = texture
      .aliases
      .get(&layer)
      .map(|alias| (alias.id, alias.generation));
    let alias = match existing {
      Some((alias, _)) => alias,
      None => {
        let alias = self.own_id();
        self.emit(
          op::CREATE_TEXTURE,
          &[
            alias,
            2,
            width,
            height,
            1,
            all_mips,
            dxgi_format(format),
            BIND_SHADER_RESOURCE,
          ],
        );
        alias
      }
    };

    if existing.is_none_or(|(_, copied)| copied != generation) {
      for mip in 0..all_mips {
        self.emit(
          op::COPY_TEXTURE,
          &[
            alias,
            mip,
            0,
            0,
            0,
            texture_id,
            mip + layer * all_mips,
            0,
            0,
            0,
            (width >> mip).max(1),
            (height >> mip).max(1),
            1,
          ],
        );
      }
    }

    let texture = self.textures.get_mut(&texture_id)?;
    let entry = texture.aliases.entry(layer).or_insert(Alias {
      id: alias,
      srv: HashMap::new(),
      generation,
    });
    entry.generation = generation;

    if let Some(view) = entry.srv.get(&(base_mip, mips)) {
      return Some(*view);
    }

    let view = self.next_own;
    self.next_own = self.next_own.wrapping_add(1).max(FIRST_OWN_ID);
    entry.srv.insert((base_mip, mips), view);
    self.emit(
      op::CREATE_SRV,
      &[
        view,
        alias,
        dxgi_format(format),
        SRV_TEXTURE2D,
        base_mip,
        mips,
        0,
        0,
      ],
    );
    Some(view)
  }

  /// A raw view of a buffer, made the first time it is needed.
  fn buffer_view(
    &mut self,
    buffer: Id,
    unordered: bool,
    offset: u64,
    size: Option<u64>,
  ) -> Option<u32> {
    let Some(info) = self.buffers.get(&buffer) else {
      self.warn(format!("buffer #{buffer} does not exist"));
      return None;
    };

    if info.constant {
      self.warn(format!(
        "constant buffer #{buffer} bound as a storage buffer"
      ));
      return None;
    }

    let offset = offset as u32;
    let size = size.map_or(info.size.saturating_sub(offset), |size| size as u32);
    let key = (unordered, offset, size);

    if let Some(view) = info.views.get(&key) {
      return Some(*view);
    }

    let view = self.own_id();
    let (record, dimension) = if unordered {
      (op::CREATE_UAV, UAV_BUFFER)
    } else {
      (op::CREATE_SRV, SRV_BUFFEREX)
    };
    self.emit(
      record,
      &[
        view,
        buffer,
        DXGI_R32_TYPELESS,
        dimension,
        offset / 4,
        size / 4,
        RAW_FLAG,
        0,
      ],
    );

    if let Some(info) = self.buffers.get_mut(&buffer) {
      info.views.insert(key, view);
    }

    Some(view)
  }

  /// What `resource` binds as for a register of `class`: the slot kind
  /// and the object.
  fn resolve(&mut self, class: u8, resource: Resource) -> Option<(u32, u32)> {
    match (class, resource) {
      (b'b', Resource::Buffer { buffer, offset, .. }) => {
        if offset != 0 {
          self.warn(format!(
            "constant buffer #{buffer} bound from byte {offset}, which Direct3D 11.0 cannot do"
          ));
        }

        Some((0, buffer))
      }
      (
        b't',
        Resource::Buffer {
          buffer,
          offset,
          size,
        },
      ) => self
        .buffer_view(buffer, false, offset, size)
        .map(|view| (1, view)),
      (b't', Resource::View(view)) => self.view(view, ViewKind::Srv).map(|view| (1, view)),
      (b's', Resource::Sampler(sampler)) => Some((2, sampler)),
      (
        b'u',
        Resource::Buffer {
          buffer,
          offset,
          size,
        },
      ) => self
        .buffer_view(buffer, true, offset, size)
        .map(|view| (3, view)),
      (b'u', Resource::View(view)) => {
        // Written by the dispatch that binds it.
        if let Some(texture) = self.views.get(&view).map(|view| view.texture) {
          if let Some(info) = self.textures.get_mut(&texture) {
            info.generation += 1;
          }
        }

        self.view(view, ViewKind::Uav).map(|view| (3, view))
      }
      (class, resource) => {
        self.warn(format!(
          "a {resource:?} bound to a `{}` register",
          class as char
        ));
        None
      }
    }
  }

  // --- commands -------------------------------------------------------------------------

  fn lower_command(&mut self, command: Command) {
    match command {
      Command::BeginRenderPass { colour, depth } => self.begin_render_pass(&colour, depth),
      Command::EndRenderPass => {
        self.emit(op::SET_TARGETS, &[0; 10]);
      }
      Command::BeginComputePass => {
        self.pass = PassState::default();
        self.emit(op::UNBIND_ALL, &[]);
      }
      Command::EndComputePass => {}
      Command::SetRenderPipeline(id) | Command::SetComputePipeline(id) => {
        self.pass.pipeline = Some(id);
      }
      Command::SetBindGroup { index, group } => {
        if let Some(slot) = self.pass.groups.get_mut(index as usize) {
          *slot = group;
        }
      }
      Command::SetVertexBuffer {
        slot,
        buffer,
        offset,
        ..
      } => {
        if let Some(slot) = self.pass.vertex_buffers.get_mut(slot as usize) {
          *slot = Some((buffer, offset));
        }
      }
      Command::SetIndexBuffer {
        buffer,
        format,
        offset,
      } => self.pass.index_buffer = Some((buffer, format, offset)),
      Command::SetViewport {
        x,
        y,
        width,
        height,
        min_depth,
        max_depth,
      } => self.emit(
        op::SET_VIEWPORT,
        &[x, y, width, height, min_depth, max_depth].map(f32::to_bits),
      ),
      Command::Draw {
        vertices,
        instances,
      } => {
        if self.prepare_draw(vertices.start as i32, instances.start, false) {
          self.emit(
            op::DRAW,
            &[
              vertices.len() as u32,
              instances.len() as u32,
              vertices.start,
              instances.start,
            ],
          );
        }
      }
      Command::DrawIndexed {
        indices,
        base_vertex,
        instances,
      } => {
        if self.prepare_draw(base_vertex, instances.start, true) {
          self.emit(
            op::DRAW_INDEXED,
            &[
              indices.len() as u32,
              instances.len() as u32,
              indices.start,
              base_vertex as u32,
              instances.start,
            ],
          );
        }
      }
      Command::DrawIndirect { buffer, offset } => {
        if self.prepare_draw(0, 0, false) {
          self.emit(op::DRAW_INDIRECT, &[buffer, offset as u32]);
        }
      }
      Command::DrawIndexedIndirect { buffer, offset } => {
        if self.prepare_draw(0, 0, true) {
          self.emit(op::DRAW_INDEXED_INDIRECT, &[buffer, offset as u32]);
        }
      }
      Command::Dispatch { x, y, z } => {
        if self.prepare_dispatch() {
          self.emit(op::DISPATCH, &[x, y, z]);
        }
      }
      Command::CopyBufferToBuffer {
        source,
        source_offset,
        destination,
        destination_offset,
        size,
      } => {
        let (source_offset, destination_offset, size) =
          (source_offset as u32, destination_offset as u32, size as u32);

        if let Some(info) = self.buffers.get_mut(&destination) {
          if info.constant {
            info.gpu_copies.retain(|(offset, _, _, length)| {
              *offset + *length <= destination_offset || *offset >= destination_offset + size
            });
            info
              .gpu_copies
              .push((destination_offset, source, source_offset, size));
            self.pin(source);
          }
        }

        self.emit(
          op::COPY_BUFFER,
          &[destination, destination_offset, source, source_offset, size],
        );
      }
      Command::CopyTextureToTexture {
        source,
        destination,
        size,
      } => self.copy_texture(source, destination, size),
    }
  }

  fn copy_texture(
    &mut self,
    source: rec::TextureCopy,
    destination: rec::TextureCopy,
    size: rec::Extent3d,
  ) {
    let (Some(from), Some(to)) = (
      self.textures.get(&source.texture),
      self.textures.get(&destination.texture),
    ) else {
      self.warn("a copy between textures that do not exist".to_string());
      return;
    };
    let three = to.dimension == TextureDimension::D3;
    let layers = if three { 1 } else { size.depth_or_array_layers };
    let depth = if three { size.depth_or_array_layers } else { 1 };
    let records: Vec<[u32; 13]> = (0..layers)
      .map(|layer| {
        let (from_layer, to_layer, from_z, to_z) = if three {
          (0, 0, source.origin.z, destination.origin.z)
        } else {
          (source.origin.z + layer, destination.origin.z + layer, 0, 0)
        };
        [
          destination.texture,
          to.subresource(destination.mip_level, to_layer),
          destination.origin.x,
          destination.origin.y,
          to_z,
          source.texture,
          from.subresource(source.mip_level, from_layer),
          source.origin.x,
          source.origin.y,
          from_z,
          source.origin.x + size.width,
          source.origin.y + size.height,
          from_z + depth,
        ]
      })
      .collect();

    for record in records {
      self.emit(op::COPY_TEXTURE, &record);
    }

    if let Some(info) = self.textures.get_mut(&destination.texture) {
      info.generation += 1;
    }
  }

  fn begin_render_pass(
    &mut self,
    colour: &[Option<rec::ColourTarget>],
    depth: Option<rec::DepthTarget>,
  ) {
    self.pass = PassState::default();
    self.emit(op::UNBIND_ALL, &[]);
    let mut targets = [0u32; 8];
    let mut size = None;

    for (slot, target) in colour.iter().enumerate().take(8) {
      let Some(target) = target else {
        continue;
      };
      size = size.or_else(|| self.view_size(target.view));
      self.touch(target.view);
      targets[slot] = self.view(target.view, ViewKind::Rtv).unwrap_or(0);
    }

    let depth_view = depth.and_then(|depth| {
      size = size.or_else(|| self.view_size(depth.view));
      self.touch(depth.view);
      self.view(depth.view, ViewKind::Dsv)
    });
    let mut words = vec![colour.len().min(8) as u32, depth_view.unwrap_or(0)];
    words.extend(targets);
    self.emit(op::SET_TARGETS, &words);

    for (slot, target) in colour.iter().enumerate().take(8) {
      if let (Some(target), true) = (target, targets[slot] != 0) {
        if let Some(clear) = target.clear {
          self.emit(
            op::CLEAR_RTV,
            &[
              targets[slot],
              (clear.r as f32).to_bits(),
              (clear.g as f32).to_bits(),
              (clear.b as f32).to_bits(),
              (clear.a as f32).to_bits(),
            ],
          );
        }
      }
    }

    if let (Some(depth), Some(view)) = (depth, depth_view) {
      if let Some(clear) = depth.clear {
        self.emit(op::CLEAR_DSV, &[view, clear.to_bits()]);
      }
    }

    // WebGPU's viewport starts as the whole target.
    let (width, height) = size.unwrap_or((1, 1));
    self.emit(
      op::SET_VIEWPORT,
      &[0.0, 0.0, width as f32, height as f32, 0.0, 1.0].map(f32::to_bits),
    );
  }

  fn view_size(&self, view: Id) -> Option<(u32, u32)> {
    let view = self.views.get(&view)?;
    let texture = self.textures.get(&view.texture)?;
    Some((
      (texture.width >> view.base_mip).max(1),
      (texture.height >> view.base_mip).max(1),
    ))
  }

  /// A view's texture is written.
  fn touch(&mut self, view: Id) {
    if let Some(texture) = self.views.get(&view).map(|view| view.texture) {
      if let Some(info) = self.textures.get_mut(&texture) {
        info.generation += 1;
      }
    }
  }

  /// The slots `files`' bindings want from the bound groups, by stage.
  fn wanted_slots(
    &mut self,
    files: &[(usize, &'static ShaderFile)],
  ) -> Vec<(usize, u32, usize, u32)> {
    let mut wanted = Vec::new();

    for (stage, file) in files {
      for binding in file.bindings {
        let resource = self
          .pass
          .groups
          .get(binding.group as usize)
          .copied()
          .flatten()
          .and_then(|group| self.groups.get(&group))
          .and_then(|entries| {
            entries
              .iter()
              .find(|(index, _)| *index == binding.binding)
              .map(|(_, resource)| *resource)
          });
        let Some(resource) = resource else {
          self.warn(format!(
            "{} needs @group({}) @binding({}), which nothing binds",
            file.file, binding.group, binding.binding
          ));
          continue;
        };

        if let Some((kind, object)) = self.resolve(binding.class, resource) {
          wanted.push((*stage, kind, binding.register as usize, object));
        }
      }
    }

    wanted
  }

  /// Bind exactly `wanted` in `stages`: first unbind every slot that
  /// changes, so nothing is bound as an input and an output at once, then
  /// bind.
  fn apply_slots(&mut self, stages: &[usize], wanted: &[(usize, u32, usize, u32)]) {
    let mut next: Vec<Slots> = stages.iter().map(|_| Slots::default()).collect();

    for (stage, kind, register, object) in wanted {
      if let Some(at) = stages.iter().position(|candidate| candidate == stage) {
        if let Some(slot) = next[at].slot(*kind, *register) {
          *slot = *object;
        }
      }
    }

    let mut unbind = Vec::new();
    let mut bind = Vec::new();

    for (at, stage) in stages.iter().enumerate() {
      let current = &self.pass.slots[*stage];
      let kinds: [(u32, &[u32], &[u32]); 4] = [
        (0, &current.constant, &next[at].constant),
        (1, &current.resource, &next[at].resource),
        (2, &current.sampler, &next[at].sampler),
        (3, &current.unordered, &next[at].unordered),
      ];

      for (kind, now, then) in kinds {
        for (register, (now, then)) in now.iter().zip(then).enumerate() {
          if now != then {
            if *now != 0 {
              unbind.push([*stage as u32, kind, register as u32, 0]);
            }

            if *then != 0 {
              bind.push([*stage as u32, kind, register as u32, *then]);
            }
          }
        }
      }
    }

    for record in unbind.iter().chain(&bind) {
      self.emit(op::BIND, record);
    }

    for (at, stage) in stages.iter().enumerate() {
      self.pass.slots[*stage] = next[at].clone();
    }
  }

  fn prepare_draw(&mut self, first_vertex: i32, first_instance: u32, indexed: bool) -> bool {
    let Some(id) = self.pass.pipeline else {
      return false;
    };
    let (vertex, fragment, strides, record) = match self.pipelines.get(&id) {
      Some(Pipeline::Render(pipe)) => (
        pipe.vertex,
        pipe.fragment,
        pipe.strides.clone(),
        [
          pipe.vertex_shader,
          pipe.pixel_shader,
          pipe.input_layout,
          pipe.blend,
          pipe.rasterizer,
          pipe.depth,
          pipe.topology,
        ],
      ),
      _ => return false,
    };

    if self.pass.set_pipeline != Some(id) {
      self.emit(op::SET_GRAPHICS, &record);
      self.pass.set_pipeline = Some(id);
    }

    for (slot, stride) in strides.iter().enumerate() {
      let Some((buffer, offset)) = self.pass.vertex_buffers.get(slot).copied().flatten() else {
        continue;
      };

      if self.pass.set_vertex_buffers[slot] != Some((buffer, offset, *stride)) {
        self.emit(
          op::SET_VERTEX_BUFFER,
          &[slot as u32, buffer, *stride, offset as u32],
        );
        self.pass.set_vertex_buffers[slot] = Some((buffer, offset, *stride));
      }
    }

    if indexed && self.pass.set_index_buffer != self.pass.index_buffer {
      if let Some((buffer, format, offset)) = self.pass.index_buffer {
        let format = match format {
          rec::IndexFormat::Uint16 => DXGI_R16_UINT,
          rec::IndexFormat::Uint32 => DXGI_R32_UINT,
        };
        self.emit(op::SET_INDEX_BUFFER, &[buffer, format, offset as u32]);
      }

      self.pass.set_index_buffer = self.pass.index_buffer;
    }

    let mut files = vec![(0, vertex)];
    files.extend(fragment.map(|file| (1, file)));
    let mut wanted = self.wanted_slots(&files);

    if let Some(register) = vertex.special_constants {
      if self.pass.special != Some((first_vertex, first_instance)) {
        let mut bytes = [0u8; 16];
        bytes[..4].copy_from_slice(&first_vertex.to_le_bytes());
        bytes[4..8].copy_from_slice(&first_instance.to_le_bytes());
        self.emit_with(op::UPDATE_BUFFER, &[self.special, 0, 16], &bytes);
        self.pass.special = Some((first_vertex, first_instance));
      }

      wanted.push((0, 0, register as usize, self.special));
    }

    self.apply_slots(&[0, 1], &wanted);
    true
  }

  fn prepare_dispatch(&mut self) -> bool {
    let Some(id) = self.pass.pipeline else {
      return false;
    };
    let (shader, object) = match self.pipelines.get(&id) {
      Some(Pipeline::Compute(pipe)) => (pipe.shader, pipe.id),
      _ => return false,
    };

    if self.pass.set_pipeline != Some(id) {
      self.emit(op::SET_COMPUTE, &[object]);
      self.pass.set_pipeline = Some(id);
    }

    let wanted = self.wanted_slots(&[(2, shader)]);
    self.apply_slots(&[2], &wanted);
    true
  }
}

/// 64-bit FNV-1a, as `vista_hlsl` and `check-hlsl.sh` hash sources with.
pub fn fnv1a(bytes: &[u8]) -> u64 {
  bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
    (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
  })
}
