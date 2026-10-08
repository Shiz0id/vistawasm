//! The renderer's CPU-side planning, shared by every backend.
//!
//! The browser renderer (`render::gpu`) and native hosts (through
//! `render::native_gpu` and `vista_native`) draw the same frame from the
//! same numbers: the uniform blocks laid out as the shaders read them, the
//! culling and generator parameters, and the sizes and limits of the
//! buffers they fill. Everything here is plain data and arithmetic; no GPU
//! API appears.

use bytemuck::{Pod, Zeroable};

use crate::render::flora::FloraInstance;
use crate::render::frame::FrameParams;
use crate::render::tree_growth::{Age, AGES, VARIANTS};
use crate::render::tree_models::{mesh_slot, TreeSpecies, MESH_SLOTS, SPECIES_COUNT};
use crate::render::water::OCEAN_SNAP_METRES;
use vista_types::CloudsOptions;

/// Per-frame uniforms shared by every render shader.
///
/// The layout must stay in sync with `FrameUniforms` in `common.wgsl`, which
/// is prepended to every render shader, so there is exactly one declaration
/// to keep in step.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct FrameUniforms {
  pub view_proj: [f32; 16],
  pub camera_position: [f32; 4],
  pub camera_forward: [f32; 4],
  pub camera_right: [f32; 4],
  pub camera_up: [f32; 4],
  pub sun_direction: [f32; 4],
  pub atmosphere: [f32; 4],
  pub sky_tint: [f32; 4],
  pub mist_params: [f32; 4],
  pub mist_colour: [f32; 4],
  pub mist_wind: [f32; 4],
  pub cloud_params: [f32; 4],
  pub cloud_motion: [f32; 4],
  pub cloud_colour: [f32; 4],
  pub water_params: [f32; 4],
  pub water_shallow: [f32; 4],
  pub water_deep: [f32; 4],
  pub water_current: [f32; 4],
  pub wave_params: [f32; 4],
  pub wave_params2: [f32; 4],
  pub water_origin: [f32; 4],
  pub vegetation: [f32; 4],
  pub vegetation2: [f32; 4],
  pub viewport: [f32; 4],
  pub shadow_view_proj: [f32; 16],
  pub shadow_params: [f32; 4],
  pub weather: [f32; 4],
  pub weather2: [f32; 4],
  pub surface: [f32; 4],
  pub clouds2: [f32; 4],
  pub clouds3: [f32; 4],
  pub clouds4: [f32; 4],
  pub weather3: [f32; 4],
  pub previous_view_proj: [f32; 16],
  pub temporal: [f32; 4],
  pub distances: [f32; 4],
  pub fades: [f32; 4],
  pub output: [f32; 4],
  pub cold: [f32; 4],
  pub sea_ice: [f32; 4],
  pub rivers: [f32; 4],
  pub ground: [f32; 4],
  pub vegetation3: [f32; 4],
  pub rock: [f32; 4],
  pub regional: [f32; 4],
  pub air: [f32; 4],
  pub light: [f32; 4],
  pub gust: [f32; 4],
  pub weather4: [f32; 4],
  pub clouds5: [f32; 4],
  pub alto: [f32; 4],
  pub rivers2: [f32; 4],
  pub waterside: [f32; 4],
  pub mouths: [[f32; 4]; 16],
}

const _: () = assert!(std::mem::size_of::<FrameUniforms>() == 1248);

/// Static world data: species bounds, wind and tints, terrain mapping,
/// material tints, and the canopy layer's colour per species. Mirrors
/// `WorldInfo` in `common.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct WorldInfo {
  pub species: [[f32; 4]; 8],
  pub species_tint: [[f32; 4]; 8],
  pub terrain: [f32; 4],
  pub terrain2: [f32; 4],
  pub material_tints: [[f32; 4]; vista_types::MATERIAL_COUNT],
  /// x: variants per species in the impostor atlas.
  pub trees: [f32; 4],
  /// Per species, the canopy layer's colour, averaged from the impostors
  /// on the GPU after each bake (see `GpuContext::bake_impostors`), so it
  /// is written only up to here from the CPU.
  pub species_canopy: [[f32; 4]; 8],
}

const _: () = assert!(std::mem::size_of::<WorldInfo>() == 624);

/// Bytes of [`WorldInfo`] the CPU writes: all but `species_canopy`.
pub const WORLD_INFO_CPU_BYTES: u64 = 496;

/// Tree culling parameters. Mirrors `CullParams` in `tree_cull.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CullParams {
  pub planes: [[f32; 4]; 6],
  pub camera: [f32; 4],
  pub params: [f32; 4],
  pub bounds: [[f32; 4]; 8],
  pub lod: [f32; 4],
  pub shadow: [f32; 4],
  pub stream: [u32; 4],
  pub thin: [f32; 4],
  pub budget: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<CullParams>() == 336);

/// Terrain shadow bake parameters. Mirrors `BakeParams` in
/// `terrain_shadow.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct TerrainShadowParams {
  pub sun: [f32; 4],
  pub grid: [f32; 4],
}

/// Mirrors `TreeParams` in `tree_generate.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct TreeGenerateParams {
  pub terrain: [f32; 4],
  pub terrain2: [f32; 4],
  pub mesh: [f32; 4],
  pub rules: [f32; 4],
  pub shape: [u32; 4],
  pub max_slopes: [[f32; 4]; 2],
  pub roots: [[f32; 4]; 2],
}

/// Mirrors `Job` in `generate_common.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuJob {
  pub tile: [i32; 2],
  pub first: u32,
  pub capacity: u32,
  pub slot: u32,
  pub keep: f32,
  pub unused: [u32; 2],
}

/// Most tiles of one kind generated in a frame.
pub const MAX_JOBS: usize = crate::render::lattice::TILES_PER_FRAME * 2;

/// Mirrors `GrassParams` in `grass_generate.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GrassGenerateParams {
  pub terrain: [f32; 4],
  pub terrain2: [f32; 4],
  pub mesh: [f32; 4],
  pub rules: [f32; 4],
  pub shape: [u32; 4],
  pub camera: [f32; 4],
  pub view: [f32; 4],
  pub planes: [[f32; 4]; 6],
  pub classes: [[u32; 4]; 8],
}

/// Mirrors `BoulderParams` in `boulder_generate.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct BoulderGenerateParams {
  pub terrain: [f32; 4],
  pub terrain2: [f32; 4],
  pub mesh: [f32; 4],
  pub rules: [f32; 4],
  pub shape: [u32; 4],
  pub camera: [f32; 4],
  pub tall: [[f32; 4]; 2],
  pub planes: [[f32; 4]; 6],
}

/// Boulder draw lists: finest, middle and coarsest level per variant,
/// then one shadow list per variant (drawn at the middle level). Their
/// capacities mirror `boulder_generate.wgsl`.
pub const BOULDER_LISTS: usize = crate::render::boulders::VARIANTS * 4;

pub const BOULDER_LIST_CAPACITY: [u32; 3] = [1024, 2048, 4096];

pub const BOULDER_VARIANT_ENTRIES: u32 = 7168;

pub const BOULDER_SHADOW_CAPACITY: u32 = 2048;

const _: () = assert!(
  BOULDER_LIST_CAPACITY[0] + BOULDER_LIST_CAPACITY[1] + BOULDER_LIST_CAPACITY[2]
    == BOULDER_VARIANT_ENTRIES
);

/// Words in the boulders' indirect arguments: an indexed draw per list,
/// then the count of every boulder drawn.
pub const BOULDER_ARGS_WORDS: usize = BOULDER_LISTS * 5 + 1;

/// Bytes per boulder: `Boulder`'s eight floats.
pub const BOULDER_BYTES: u64 = 32;

const _: () =
  assert!(std::mem::size_of::<crate::render::boulders::Boulder>() as u64 == BOULDER_BYTES);

/// Grounding parameters for one instance buffer. Mirrors `GroundParams`
/// in `grounding.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GroundParams {
  pub terrain: [f32; 4],
  pub terrain2: [f32; 4],
  pub mesh: [f32; 4],
  pub shape: [u32; 4],
  pub roots: [[f32; 4]; 2],
}

const _: () = assert!(std::mem::size_of::<GroundParams>() == 96);

/// The mesh slot a mesh list draws: its own, or for an understorey list
/// its species' and variant's young, full mesh.
pub fn list_mesh(list: usize) -> usize {
  if list < MESH_SLOTS {
    return list;
  }

  let understorey = list - MESH_SLOTS;
  mesh_slot(
    understorey / VARIANTS,
    understorey % VARIANTS,
    Age::Young as usize,
    0,
  )
}

/// Each mesh list's first slot and slots: for each species present,
/// the full and light meshes of every variant grown and age class, and
/// with streamed tiles the understorey's young meshes. Each list has
/// twice its share of the species' trees, so an uneven mix of ages still
/// fits; a full list passes trees to the lighter level, and on to
/// impostors.
pub fn mesh_list_layout(
  present: u32,
  species_slots: &[u32; SPECIES_COUNT],
  pool: u32,
  streamed: bool,
  variants: usize,
) -> (Vec<u32>, Vec<u32>) {
  let mut capacities = vec![0u32; MESH_LISTS];

  for species in 0..SPECIES_COUNT {
    if present & 1 << species == 0 {
      continue;
    }

    // Shrubs are never stunted.
    let ages = if species == TreeSpecies::Shrub as usize {
      AGES - 1
    } else {
      AGES
    };
    let lists = (variants * ages) as u32;

    for (lod, most) in [(0, LOD0_MESH_SLOTS), (1, LOD1_MESH_SLOTS)] {
      let total = species_slots[species].min(most);
      let share = (total * 2).div_ceil(lists).min(total);

      for variant in 0..variants {
        for age in 0..ages {
          capacities[mesh_slot(species, variant, age, lod)] = share;
        }
      }
    }

    if streamed {
      let total = pool.min(UNDERSTOREY_MESH_SLOTS);

      for variant in 0..variants {
        capacities[MESH_SLOTS + species * VARIANTS + variant] =
          (total * 3 / 2).div_ceil(variants as u32).min(total);
      }
    }
  }

  let mut running = 0;
  let offsets = capacities
    .iter()
    .map(|capacity| {
      let offset = running;
      running += capacity;
      offset
    })
    .collect();
  (offsets, capacities)
}

/// Mesh lists: canopy trees per mesh slot (species, variant, age class
/// and level of detail, `tree_models::mesh_slot`), then understorey
/// saplings per species and variant.
pub const MESH_LISTS: usize = MESH_SLOTS + SPECIES_COUNT * VARIANTS;

/// Words in the indirect argument buffer: an indexed mesh draw per list
/// (5 words each), then one impostor draw and one shadow draw (4 each).
/// The triangle budget reads them all back.
pub const INDIRECT_WORDS: usize = MESH_LISTS * 5 + 4 * 2;

pub const IMPOSTOR_ARGS_BASE: usize = MESH_LISTS * 5;

pub const SHADOW_ARGS_BASE: usize = IMPOSTOR_ARGS_BASE + 4;

/// Mesh slots per species for understorey saplings: they are meshes only
/// within 15 m.
pub const UNDERSTOREY_MESH_SLOTS: u32 = 4_096;

/// Full meshes are drawn within this distance, lighter ones beyond.
pub const LOD0_METRES: f32 = 50.0;

/// Trees are impostors beyond this distance, or the mesh distance when
/// less.
pub const IMPOSTOR_METRES: f32 = 150.0;

/// Most full meshes per species: more than stand within 50 m.
pub const LOD0_MESH_SLOTS: u32 = 2_048;

/// Most light meshes per species: more than stand within 150 m, less
/// than the triangle budget allows.
pub const LOD1_MESH_SLOTS: u32 = 12_288;

/// Beyond this distance, understorey saplings are impostor cards.
pub const UNDERSTOREY_CARD_METRES: f32 = 15.0;

/// Drawn tufts within 15 m of the camera, which are two crossed quads:
/// every lattice point within 15 m, with room to spare. The single cards
/// beyond follow them in the drawn list.
pub const GRASS_NEAR_SLOTS: u32 = 8_192;

/// Bytes per drawn tuft: `FloraInstance`'s seven floats.
pub const DRAWN_TUFT_BYTES: u64 = 28;

const _: () = assert!(std::mem::size_of::<FloraInstance>() as u64 == DRAWN_TUFT_BYTES);

/// Floats per drawn tree: `TreeInstance`'s eight, the species word
/// holding species plus fade / 2, and the extra crown width.
pub const DRAWN_TREE_FLOATS: u64 = 9;

/// Most mesh slots per species: more would already cost far more than
/// the triangle budget allows, and the rest draw as impostors.
pub const STREAMED_MESH_SLOTS: u32 = 24_576;

pub const TERRAIN_SHADOW_MAX: u32 = 1024;

pub const OCEAN_GRID_SAMPLES: u32 = 193;

/// How much taller than the ordinary cloud layer storm towers grow, at
/// full `towering`.
pub const TOWER_STRETCH: f32 = 1.6;

pub const OCEAN_FAR_REACH_METRES: f32 = 60_000.0;

/// Foliage tint multipliers per species, in `TreeSpecies` order.
pub const SPECIES_TINTS: [[f32; 4]; 8] = [
  [1.0, 1.0, 0.95, 0.0],
  [0.78, 0.95, 0.85, 0.0],
  [0.6, 0.8, 0.76, 0.0],
  [1.12, 1.05, 0.78, 0.0],
  [0.95, 1.12, 0.85, 0.0],
  [1.0, 1.02, 0.8, 0.0],
  [1.08, 1.02, 0.72, 0.0],
  [0.95, 1.05, 0.9, 0.0],
];

pub fn direction_from_degrees(degrees: f32) -> [f32; 2] {
  let radians = degrees.to_radians();
  [radians.sin(), radians.cos()]
}

/// The mid-level layer's drift direction: the cloud wind veered 20
/// degrees, as winds veer with height. `alto_wind` in `atmosphere.wgsl`
/// turns the cloud wind the same way.
pub fn alto_wind_direction(cloud_wind_degrees: f32) -> [f32; 2] {
  direction_from_degrees(cloud_wind_degrees + 20.0)
}

/// Height of the mid-level layer: the requested height, kept 300 m above
/// the top of the ordinary cloud layer and, where there is room, 500 m
/// below the cirrus. Storm towers may rise through it.
pub fn alto_height(clouds: &CloudsOptions) -> f32 {
  let low = clouds.height_metres + clouds.thickness_metres.max(1.0) + 300.0;
  let high = clouds
    .cirrus_height_metres
    .max(clouds.height_metres + clouds.thickness_metres)
    - 500.0;
  clouds.alto_height_metres.clamp(low, high.max(low))
}

pub fn flag(on: bool) -> f32 {
  if on {
    1.0
  } else {
    0.0
  }
}

/// What the uniforms read from the renderer besides the frame's
/// parameters: its sizes and what it holds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UniformView {
  /// Internal render size in pixels.
  pub width: u32,
  /// See `width`.
  pub height: u32,
  /// Output (canvas) size in pixels.
  pub canvas_width: u32,
  /// See `canvas_width`.
  pub canvas_height: u32,
  /// Fraction of the output resolution rendered.
  pub render_scale: f32,
  /// Whether water is drawn.
  pub water_visible: bool,
  /// Whether there are trees.
  pub trees: bool,
  /// Whether boulders are streamed.
  pub boulders_streamed: bool,
  /// The streamed trees' far keep, or 1 without streamed trees.
  pub tree_far_keep: f32,
}

/// Wind-driven offsets, integrated over time rather than computed as
/// speed x time, so changing the wind (for example when the weather
/// changes) never makes clouds, mist, or currents jump.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UniformMotion {
  /// Low clouds.
  pub cloud_offset: [f32; 2],
  /// Cirrus drifts with its own speed, not the low clouds'.
  pub cirrus_offset: [f32; 2],
  /// So does the mid-level layer, along its own, veered wind.
  pub alto_offset: [f32; 2],
  /// Mist.
  pub mist_offset: [f32; 2],
  /// Water currents.
  pub current_offset: [f32; 2],
  /// Sea ice floes drift with the wind.
  pub sea_ice_offset: [f32; 2],
  /// How far the clouds have evolved.
  pub cloud_evolution: f32,
}

impl UniformMotion {
  /// Advance the motion by `dt` and write the frame's uniforms into `u`,
  /// all but the shadow, temporal and previous-frame fields, which the
  /// frame sets after.
  pub fn update(
    &mut self,
    u: &mut FrameUniforms,
    params: &FrameParams,
    view: &UniformView,
    time: f32,
    dt: f32,
  ) {
    // Integrate wind-driven motion. The wind carries clouds and currents
    // downwind, so their noise lookups move upwind.
    let clouds = &params.clouds;
    let cloud_wind = direction_from_degrees(clouds.wind_direction_degrees);
    let cloud_speed = clouds.speed.max(0.0) * 15.0 * dt;
    self.cloud_offset[0] -= cloud_wind[0] * cloud_speed;
    self.cloud_offset[1] -= cloud_wind[1] * cloud_speed;
    let cirrus_speed = clouds.cirrus_speed.max(0.0) * 15.0 * dt;
    self.cirrus_offset[0] -= cloud_wind[0] * cirrus_speed;
    self.cirrus_offset[1] -= cloud_wind[1] * cirrus_speed;
    let alto_wind = alto_wind_direction(clouds.wind_direction_degrees);
    let alto_speed = clouds.alto_speed.clamp(0.0, 4.0) * 15.0 * dt;
    self.alto_offset[0] -= alto_wind[0] * alto_speed;
    self.alto_offset[1] -= alto_wind[1] * alto_speed;
    self.cloud_evolution += clouds.evolution.clamp(0.0, 1.0) * 14.0 * dt;
    let mist = &params.mist;
    let mist_wind = direction_from_degrees(mist.wind_direction_degrees);
    let mist_speed = mist.wind_speed_metres_per_second.max(0.0) * dt;
    self.mist_offset[0] += mist_wind[0] * mist_speed;
    self.mist_offset[1] += mist_wind[1] * mist_speed;
    let water = &params.water;
    let current = direction_from_degrees(water.current_direction_degrees);
    let current_speed = water.current_speed.max(0.0) * dt;
    self.current_offset[0] -= current[0] * current_speed;
    self.current_offset[1] -= current[1] * current_speed;
    // Pack ice drifts at about 2 % of the wind speed.
    self.sea_ice_offset[0] -= params.weather.wind[0] * 0.02 * dt;
    self.sea_ice_offset[1] -= params.weather.wind[1] * 0.02 * dt;

    let apply_gamma = u.camera_up[3];
    let p = params.camera_position;
    let tan_half_fov_y = (params.field_of_view_degrees.to_radians() * 0.5).tan();

    u.view_proj = params.view_proj;
    u.camera_position = [p[0], p[1], p[2], time];
    u.camera_forward = [
      params.camera_forward[0],
      params.camera_forward[1],
      params.camera_forward[2],
      tan_half_fov_y,
    ];
    u.camera_right = [
      params.camera_right[0],
      params.camera_right[1],
      params.camera_right[2],
      params.aspect_ratio.max(0.001),
    ];
    u.camera_up = [
      params.camera_up[0],
      params.camera_up[1],
      params.camera_up[2],
      apply_gamma,
    ];
    u.sun_direction = [
      params.sun_direction[0],
      params.sun_direction[1],
      params.sun_direction[2],
      params.sun_intensity.max(0.0),
    ];

    let atmosphere = &params.atmosphere;
    u.atmosphere = [
      atmosphere.rayleigh_strength.max(0.0),
      atmosphere.mie_strength.max(0.0),
      atmosphere.haze_distance_metres.max(1.0),
      atmosphere.exposure.max(0.0),
    ];
    u.sky_tint = [
      atmosphere.sky_tint[0],
      atmosphere.sky_tint[1],
      atmosphere.sky_tint[2],
      params.debug_view as f32,
    ];
    u.mist_params = [
      params.mist_density.clamp(0.0, 1.0),
      mist.base_height_metres,
      mist.height_falloff_metres.max(1.0),
      params.mist_noise_strength.max(0.0),
    ];
    u.mist_colour = [
      mist.colour[0],
      mist.colour[1],
      mist.colour[2],
      params.mist_water_level_metres,
    ];
    u.mist_wind = [
      self.mist_offset[0],
      self.mist_offset[1],
      mist.sun_scattering.clamp(0.0, 1.0),
      ((mist.seed_offset % 997) as f32 * 0.618_034).fract(),
    ];

    let seed = (clouds.seed_offset % 10_007) as f32;
    u.clouds2 = [
      if params.cloud_coverage > 0.0 {
        clouds.cirrus.clamp(0.0, 1.0)
      } else {
        0.0
      },
      clouds
        .cirrus_height_metres
        .max(clouds.height_metres + clouds.thickness_metres),
      cloud_wind[0],
      cloud_wind[1],
    ];
    // Storm towers rise well above the ordinary cloud layer, so the slab is
    // stretched to hold them; the shader keeps ordinary clouds at their
    // own height inside it.
    let towering = clouds.towering.clamp(0.0, 1.0);
    u.cloud_params = [
      params.cloud_coverage.clamp(0.0, 1.0),
      clouds.height_metres,
      clouds.thickness_metres.max(1.0) * (1.0 + towering * TOWER_STRETCH),
      params.cloud_raymarch_steps as f32,
    ];
    u.clouds3 = [
      clouds.stratiform.clamp(0.0, 1.0),
      towering,
      clouds.base_darkness.clamp(0.0, 1.0),
      clouds.ragged_base.clamp(0.0, 1.0),
    ];
    u.clouds4 = [
      if params.cloud_coverage > 0.0 {
        clouds.rain_shafts.clamp(0.0, 1.0)
      } else {
        0.0
      },
      params.weather.lightning_position[0],
      params.weather.lightning_position[1],
      1.0 + towering * TOWER_STRETCH,
    ];
    u.cloud_motion = [
      self.cloud_offset[0] + seed * 173.0,
      self.cloud_offset[1] + seed * 311.0,
      self.cloud_evolution,
      clouds.density.clamp(0.0, 1.0),
    ];
    u.clouds5 = [
      clouds.base_variation.clamp(0.0, 0.2),
      clouds.base_lumpiness.clamp(0.0, 1.0),
      params.alto_amounts[0].clamp(0.0, 1.0),
      params.alto_amounts[1].clamp(0.0, 1.0),
    ];
    let alto_height = alto_height(clouds);
    u.alto = [self.alto_offset[0], self.alto_offset[1], alto_height, 0.0];
    u.cloud_colour = [
      clouds.colour[0],
      clouds.colour[1],
      clouds.colour[2],
      flag(clouds.cast_shadows && params.shadows.clouds.enabled),
    ];
    u.water_params = [
      water.wave_scale.max(0.0),
      water.reflectivity.clamp(0.0, 1.0),
      water.clarity_metres.max(0.1),
      water.foam.clamp(0.0, 1.0),
    ];
    u.water_shallow = [
      water.shallow_colour[0],
      water.shallow_colour[1],
      water.shallow_colour[2],
      water.sea_level_metres,
    ];
    u.water_deep = [
      water.deep_colour[0],
      water.deep_colour[1],
      water.deep_colour[2],
      params.near_metres.max(0.001),
    ];
    u.water_current = [
      self.current_offset[0],
      self.current_offset[1],
      water.current_speed.max(0.0),
      params.far_metres.max(1.0),
    ];
    let waves = &water.waves;
    u.wave_params = [
      waves.amplitude_metres.clamp(0.0, 30.0),
      waves.wavelength_metres.clamp(0.5, 2_000.0),
      waves.direction_degrees.to_radians(),
      waves.steepness.clamp(0.0, 1.0),
    ];
    u.wave_params2 = [
      waves.speed.max(0.0),
      waves.directional_spread.clamp(0.0, 1.0),
      flag(waves.enabled),
      0.0,
    ];
    u.water_origin = [
      (p[0] / OCEAN_SNAP_METRES).round() * OCEAN_SNAP_METRES,
      (p[2] / OCEAN_SNAP_METRES).round() * OCEAN_SNAP_METRES,
      flag(view.water_visible),
      flag(water.reflections == vista_types::WaterReflections::Screen),
    ];

    let flora = &params.flora;
    u.vegetation = [
      flora.wind_strength.clamp(0.0, 1.0),
      flora.species_variation.clamp(0.0, 1.0),
      params.tree_style as f32,
      params.grass_view_distance_metres.max(0.0),
    ];
    u.vegetation2 = [
      flora.mesh_distance_metres.max(1.0),
      params.grass_cover,
      params.vegetation.grass_radius,
      params.grass_height,
    ];
    let width = view.width.max(1) as f32;
    let height = view.height.max(1) as f32;
    u.viewport = [width, height, 1.0 / width, 1.0 / height];
    u.output = [
      view.canvas_width.max(1) as f32,
      view.canvas_height.max(1) as f32,
      view.render_scale,
      0.0,
    ];

    let shadows = &params.shadows;
    // Boulders in view cast into the same map as the trees.
    let tree_shadows = shadows.trees.enabled
      && (view.trees || (params.needs.boulders && view.boulders_streamed))
      && params.sun_direction[1] > 0.0;
    u.shadow_params = [
      if tree_shadows {
        shadows.trees.strength.clamp(0.0, 1.0)
      } else {
        0.0
      },
      0.5 + shadows.trees.softness.clamp(0.0, 1.0) * 2.5,
      if shadows.terrain.enabled {
        shadows.terrain.strength.clamp(0.0, 1.0)
      } else {
        0.0
      },
      shadows.clouds.strength.clamp(0.0, 1.0),
    ];
    let weather = &params.weather;
    u.weather = [
      weather.rain,
      weather.snow,
      weather.wetness,
      weather.snow_cover,
    ];
    u.weather2 = [
      weather.lightning,
      weather.overcast,
      weather.wind[0],
      weather.wind[1],
    ];
    u.distances = [
      params.distances.render_metres,
      params.distances.detail_metres,
      params.distances.cloud_metres,
      0.0,
    ];
    u.fades = [
      params.distances.render_fade_metres,
      params.distances.cloud_fade_metres,
      0.0,
      0.0,
    ];
    u.weather3 = [
      self.cirrus_offset[0],
      self.cirrus_offset[1],
      flag(!weather.lens_drops.is_empty()),
      weather.heaviness.max(1.0),
    ];
    u.cold = [weather.blowing_snow.clamp(0.0, 1.0), 0.0, 0.0, 0.0];
    u.sea_ice = [
      flag(params.sea_ice.possible),
      params.sea_ice.open_sea_unit.clamp(0.0, 1.0),
      self.sea_ice_offset[0],
      self.sea_ice_offset[1],
    ];
    let rivers = &params.rivers;
    u.rivers = [
      rivers.melt.clamp(0.4, 1.4),
      flag(rivers.freezing),
      flag(rivers.falls),
      flag(rivers.wet_banks),
    ];
    // The stones' seed in two halves, whole numbers a float holds
    // exactly, and whether boulder meshes draw them.
    u.rivers2 = [
      rivers.eddies.clamp(0.0, 1.0),
      flag(view.boulders_streamed),
      (rivers.stones & 0xffff) as f32,
      (rivers.stones >> 16) as f32,
    ];
    // Where riparian scrub thins out with distance, as the tree cull
    // thins it, so the terrain's far tint takes over exactly there.
    let far_keep = view.tree_far_keep;
    u.waterside = [
      if view.trees {
        params.vegetation.tree_radius.max(1.0)
      } else {
        0.0
      },
      far_keep,
      if params.canopy.drawn {
        params.canopy.distance.max(1.0)
      } else {
        1.0e30
      },
      params.rivers.refraction.clamp(0.0, 1.0),
    ];
    u.mouths = params.rivers.mouths;
    u.ground = params.mesh_ground;
    u.vegetation3 = [
      params.canopy.density,
      params.canopy.distance.max(1.0),
      params.shadows.trees.distance_metres,
      flag(params.canopy.drawn),
    ];
    u.rock = params.rock;
    u.regional = weather.regional;
    u.air = weather.air;
    u.light = weather.light;
    u.gust = weather.gust;
    u.weather4 = weather.sea;
    // Under cloud, shadows soften towards nothing.
    u.shadow_params[0] *= weather.light[1];
    u.shadow_params[2] *= weather.light[1];
    let surface = &params.surface;
    u.surface = [
      flag(surface.textures),
      flag(surface.detail_normals),
      surface.texture_scale.clamp(0.05, 20.0),
      0.0,
    ];
  }
}

/// Reusing distant clouds: only for volumetric clouds seen from well
/// outside their layer (from inside it clouds are close and shift too
/// much between frames), and only when the other cloud image holds the
/// previous frame's clouds (`history_valid`).
pub fn cloud_reuse(params: &FrameParams, history_valid: bool) -> bool {
  let clouds_drawn = params.clouds_drawn();
  let camera_y = params.camera_position[1];
  let base = params.clouds.height_metres;
  let top = base
    + params.clouds.thickness_metres.max(1.0)
      * (1.0 + params.clouds.towering.clamp(0.0, 1.0) * TOWER_STRETCH);
  let outside_layer = camera_y < base - 300.0 || camera_y > top + 300.0;
  params.clouds.temporal
    && clouds_drawn
    && params.cloud_raymarch_steps > 0
    && outside_layer
    && history_valid
}

/// The streamed trees' shape, for culling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeStreamShape {
  /// Entries per tile of the nearest class.
  pub first_capacity: u32,
  /// `TreeRules::far_keep`.
  pub far_keep: f32,
  /// `TreeRules::understorey`.
  pub understorey: f32,
}

/// What the tree cull reads besides the frame's parameters.
pub struct TreeCullInputs<'a> {
  /// The tree shadow map's frame.
  pub shadow: &'a crate::render::shadow_math::ShadowFrame,
  /// Whether trees cast shadows this frame.
  pub tree_shadows: bool,
  /// Internal render height in pixels.
  pub height: u32,
  /// Height and radius per species.
  pub tree_bounds: &'a [(f32, f32); SPECIES_COUNT],
  /// Static trees plus every entry of the tile pool.
  pub instance_count: u32,
  /// The static trees.
  pub static_count: u32,
  /// The streamed trees, if any.
  pub stream: Option<TreeStreamShape>,
  /// Variants per species grown.
  pub ready_variants: usize,
  /// Variants whose impostors are baked for every species present.
  pub impostor_variants: usize,
}

/// The tree cull pass's parameters.
pub fn tree_cull_params(params: &FrameParams, inputs: &TreeCullInputs<'_>) -> CullParams {
  let tan_half_fov_y = (params.field_of_view_degrees.to_radians() * 0.5)
    .tan()
    .max(0.0001);
  let mut cull = CullParams::zeroed();
  cull.planes = crate::maths::frustum_planes(&params.view_proj);
  // Not `clamp`: `min` then `max` keeps a NaN reach finite.
  #[allow(clippy::manual_clamp)]
  let mesh_reach = params
    .vegetation
    .mesh_metres
    .map_or(params.flora.mesh_distance_metres, |reach| {
      reach.min(params.flora.mesh_distance_metres)
    })
    .min(IMPOSTOR_METRES)
    .max(1.0);
  cull.camera = [
    params.camera_position[0],
    params.camera_position[1],
    params.camera_position[2],
    mesh_reach,
  ];
  cull.params = [
    params
      .far_metres
      .min(40_000.0)
      .min(params.distances.render_metres),
    params.tree_style as f32,
    inputs.height as f32 / (2.0 * tan_half_fov_y),
    inputs.instance_count as f32,
  ];
  // The square shadow map covers a circle of radius x sqrt(2) at its
  // corners; casters just outside it still reach into it.
  cull.shadow = [
    inputs.shadow.centre[0],
    inputs.shadow.centre[1],
    inputs.shadow.radius * 1.42,
    flag(inputs.tree_shadows),
  ];

  for (slot, (height, radius)) in inputs.tree_bounds.iter().enumerate() {
    cull.bounds[slot] = [*height, *radius, 0.0, 0.0];
  }

  // Full meshes near, lighter ones to the mesh distance (a third of
  // it at least), then impostors.
  cull.lod = [
    LOD0_METRES.min(cull.camera[3] / 3.0),
    0.0,
    inputs.ready_variants as f32,
    inputs.impostor_variants as f32,
  ];

  cull.budget = [
    params.vegetation.shadow_metres,
    0.0,
    UNDERSTOREY_CARD_METRES,
    params.vegetation.max_shadow_casters as f32,
  ];

  cull.stream = [
    inputs.static_count,
    inputs.stream.map_or(1, |stream| stream.first_capacity),
    0,
    0,
  ];
  // Without streamed tiles the far set is the whole forest: nothing
  // thins, and with no canopy layer drawn no tree gives way to it.
  cull.thin = [
    params.vegetation.tree_radius.max(1.0),
    inputs.stream.map_or(1.0, |stream| stream.far_keep),
    if params.canopy.drawn {
      params.canopy.distance.max(1.0)
    } else {
      f32::MAX
    },
    inputs.stream.map_or(1.0, |stream| stream.understorey),
  ];
  cull
}

/// The trees' indirect draw arguments before culling: each mesh list's
/// indexed draw with no instances, then the impostor and shadow draws.
pub fn tree_indirect_args(
  tree_ranges: &[(u32, u32, i32)],
  tree_style: u32,
) -> [u32; INDIRECT_WORDS] {
  let impostor_vertices = if tree_style == 1 { 12 } else { 6 };
  let mut args = [0u32; INDIRECT_WORDS];

  for list in 0..MESH_LISTS {
    let (first_index, index_count, base_vertex) = tree_ranges[list_mesh(list)];
    args[list * 5] = index_count;
    args[list * 5 + 2] = first_index;
    args[list * 5 + 3] = base_vertex as u32;
  }

  args[IMPOSTOR_ARGS_BASE] = impostor_vertices;
  args[SHADOW_ARGS_BASE] = 6;
  args
}

/// Boulder mesh ranges: per variant and level, first index, index count
/// and base vertex (`BoulderMeshes::ranges`).
pub type BoulderRanges =
  [[(u32, u32, i32); crate::render::boulders::LODS]; crate::render::boulders::VARIANTS];

/// Each boulder list's indirect draw, with no instances yet, and the count
/// of boulders drawn at 0: what the cull pass starts from each frame.
pub fn boulder_cleared_args(ranges: &BoulderRanges) -> [u32; BOULDER_ARGS_WORDS] {
  let mut words = [0u32; BOULDER_ARGS_WORDS];

  for list in 0..BOULDER_LISTS {
    let (variant, lod) = if list < 18 {
      (list / 3, list % 3)
    } else {
      (list - 18, 1)
    };
    let (first, count, base) = ranges[variant][lod];
    words[list * 5..list * 5 + 5].copy_from_slice(&[count, 0, first, base as u32, 0]);
  }

  words
}

/// The first drawn entry of each boulder list.
pub fn boulder_list_start(list: usize) -> u32 {
  let variants = crate::render::boulders::VARIANTS as u32;

  if list < 18 {
    let starts = [
      0,
      BOULDER_LIST_CAPACITY[0],
      BOULDER_LIST_CAPACITY[0] + BOULDER_LIST_CAPACITY[1],
    ];
    (list / 3) as u32 * BOULDER_VARIANT_ENTRIES + starts[list % 3]
  } else {
    variants * BOULDER_VARIANT_ENTRIES + (list - 18) as u32 * BOULDER_SHADOW_CAPACITY
  }
}

/// Grounding parameters for one instance buffer: the terrain mapping
/// (`[terrain, terrain2, mesh]`), and its instance count, floats per
/// instance and kind (1 trees, 0 tufts and reeds, 2 boulders).
pub fn ground_params(
  mapping: [[f32; 4]; 3],
  [count, floats, kind]: [u32; 3],
  tree_roots: &[f32; SPECIES_COUNT],
) -> GroundParams {
  let mut roots = [[0.0; 4]; 2];

  for (slot, root) in tree_roots.iter().enumerate() {
    roots[slot / 4][slot % 4] = *root;
  }

  GroundParams {
    terrain: mapping[0],
    terrain2: mapping[1],
    mesh: mapping[2],
    shape: [count, floats, kind, 0],
    roots,
  }
}

/// The generator jobs for `changes` (at most [`MAX_JOBS`]), and the slots
/// whose counts must be zeroed first: every slot they empty or refill.
pub fn tile_jobs(
  layout: &crate::render::lattice::TileLayout,
  changes: &crate::render::lattice::TileChanges,
) -> (Vec<GpuJob>, Vec<u32>) {
  let emptied = changes
    .freed
    .iter()
    .chain(changes.jobs.iter().map(|job| &job.slot))
    .copied()
    .collect();
  let jobs = changes
    .jobs
    .iter()
    .take(MAX_JOBS)
    .map(|job| {
      let (first, capacity) = layout.slot_range(job.slot);
      GpuJob {
        tile: job.tile,
        first,
        capacity,
        slot: job.slot,
        keep: job.keep,
        unused: [0; 2],
      }
    })
    .collect();
  (jobs, emptied)
}

/// The boulder generator's and cull pass's parameters. `terrain` is the
/// world info's terrain mapping, `height` the internal render height.
pub fn boulder_generate_params(
  terrain: [[f32; 4]; 2],
  params: &FrameParams,
  texel_inverse: f32,
  height: u32,
  rules: &crate::render::boulders::BoulderRules,
  layout: &crate::render::lattice::TileLayout,
  planes: &[[f32; 4]; 6],
) -> BoulderGenerateParams {
  let frame = &params.vegetation;
  let camera = params.camera_position;
  let mut tall = [[0.0; 4]; 2];

  for (variant, tallness) in rules.heights.iter().enumerate() {
    tall[variant / 4][variant % 4] = *tallness;
  }

  BoulderGenerateParams {
    terrain: terrain[0],
    terrain2: terrain[1],
    mesh: params.mesh_ground,
    rules: [
      texel_inverse,
      frame.boulder_metres.max(1.0),
      crate::render::boulders::BOULDER_SHADOW_METRES,
      height as f32
        / (2.0
          * (params.field_of_view_degrees.to_radians() * 0.5)
            .tan()
            .max(1e-4)),
    ],
    shape: [
      rules.seed,
      layout.instance_count(),
      layout.classes.first().map_or(1, |class| class.capacity),
      rules.stones,
    ],
    camera: [camera[0], camera[1], camera[2], 0.0],
    tall,
    planes: *planes,
  }
}

/// The tree generator's parameters. `mapping` is the world info's terrain
/// mapping and the drawn mesh's ground.
pub fn tree_generate_params(
  mapping: [[f32; 4]; 3],
  texel_inverse: f32,
  rules: &crate::render::vegetation::TreeRules,
  tree_roots: &[f32; SPECIES_COUNT],
) -> TreeGenerateParams {
  let mut max_slopes = [[0.0; 4]; 2];
  let mut roots = [[0.0; 4]; 2];

  for slot in 0..SPECIES_COUNT {
    max_slopes[slot / 4][slot % 4] = rules.max_slopes[slot];
    roots[slot / 4][slot % 4] = tree_roots[slot];
  }

  TreeGenerateParams {
    terrain: mapping[0],
    terrain2: mapping[1],
    mesh: mapping[2],
    rules: [
      texel_inverse,
      rules.p_scale,
      rules.variation,
      rules.far_keep,
    ],
    shape: [
      rules.seed,
      rules.understorey.to_bits(),
      rules.boulders.unwrap_or(0),
      u32::from(rules.boulders.is_some()),
    ],
    max_slopes,
    roots,
  }
}

/// The grass generator's and cull pass's parameters.
pub fn grass_generate_params(
  mapping: [[f32; 4]; 3],
  params: &FrameParams,
  texel_inverse: f32,
  rules: &crate::render::grass::GrassRules,
  layout: &crate::render::lattice::TileLayout,
  grass_mask_neutral: bool,
  planes: &[[f32; 4]; 6],
) -> GrassGenerateParams {
  let frame = &params.vegetation;
  let mut classes = [[0u32; 4]; 8];

  for (slot, class) in layout.classes.iter().take(8).enumerate() {
    classes[slot] = [
      class.first_slot,
      class.first_instance,
      class.capacity,
      class.slots,
    ];
  }

  let camera = params.camera_position;
  GrassGenerateParams {
    terrain: mapping[0],
    terrain2: mapping[1],
    mesh: mapping[2],
    rules: [texel_inverse, rules.probability, rules.canopy, rules.height],
    shape: [
      rules.seed,
      layout.classes.len().min(8) as u32,
      layout.instance_count(),
      rules.boulders.unwrap_or(0),
    ],
    camera: [camera[0], camera[1], camera[2], frame.grass_radius],
    view: [
      params.grass_view_distance_metres,
      flag(rules.boulders.is_some()),
      flag(!grass_mask_neutral),
      frame.grass_handover,
    ],
    planes: *planes,
    classes,
  }
}
