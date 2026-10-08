//! What the renderer needs for one frame, resolved by the engine.
//!
//! These are plain data, compiled on every target: the browser renderer
//! (`render::gpu`) reads them directly, and native builds hand them to a
//! host renderer through `render::native_gpu`.

use vista_types::{
  AtmosphereOptions, CloudsOptions, FloraOptions, MistOptions, ShadowOptions, SurfaceOptions,
  WaterOptions,
};

use crate::render::pipelines::Needs;

/// Weather values the shaders need for one frame.
#[derive(Clone, Debug, Default)]
pub struct FrameWeather {
  /// Rain intensity.
  pub rain: f32,
  /// Snowfall intensity.
  pub snow: f32,
  /// Ground wetness.
  pub wetness: f32,
  /// Settled snow.
  pub snow_cover: f32,
  /// Lightning flash brightness.
  pub lightning: f32,
  /// How grey and flat the sky is.
  pub overcast: f32,
  /// Wind vector (x, z) in metres per second.
  pub wind: [f32; 2],
  /// World position (x, z) of the latest lightning strike.
  pub lightning_position: [f32; 2],
  /// How far precipitation exceeds full intensity (1 or more).
  pub heaviness: f32,
  /// Raindrops on the lens (see [`crate::lens_drops::LensDrops::packed`]).
  pub lens_drops: Vec<[f32; 4]>,
  /// Low drifting snow, 0 to 1.
  pub blowing_snow: f32,
  /// The regional weather map's corner (xy), 1 / its size (z), and 1 when
  /// it is read (w).
  pub regional: [f32; 4],
  /// Mie colour (rgb) and phase asymmetry (w).
  pub air: [f32; 4],
  /// Direct sun, shadow strength, indirect light, and how flat the sky
  /// light is under a deck.
  pub light: [f32; 4],
  /// The gust front: distance travelled, gustiness, mean wind, and 1 when
  /// on.
  pub gust: [f32; 4],
  /// 1 when the surface weather map is read, whitecaps, blown spray.
  pub sea: [f32; 4],
  /// Added to the terrain shadows' softness under cloud.
  pub shadow_softening: f32,
}

/// Where the sea may freeze.
#[derive(Clone, Copy, Debug, Default)]
pub struct SeaIce {
  /// Whether any sea, on the terrain or beyond it, is cold enough to
  /// freeze. When `false` the water shader skips sea ice entirely.
  pub possible: bool,
  /// Temperature unit ((°C + 30) / 65) of the open sea beyond the terrain.
  pub open_sea_unit: f32,
}

/// Rivers, lakes and waterfalls this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct RiverFrame {
  /// How full snowmelt makes the rivers, 0.4 to 1.4: it scales speed and
  /// foam, and raises the surface inside the channel above 1.
  pub melt: f32,
  /// Whether any lake, river or waterfall is below 0 °C. When `false` the
  /// water shader skips all frozen-water code.
  pub freezing: bool,
  /// Whether there are waterfalls. When `false` the water shader skips
  /// all waterfall code.
  pub falls: bool,
  /// Whether there is a wet-bank field. When `false` the terrain shader
  /// skips wet banks.
  pub wet_banks: bool,
  /// Eddies and vortices in rivers, 0 to 1 (`WaterOptions::eddies`). At 0
  /// the water shader skips all eddy code.
  pub eddies: f32,
  /// Refraction and caustics in shallow water, 0 to 1
  /// (`WaterOptions::refraction`).
  pub refraction: f32,
  /// The river mouths whose plumes tint the sea (`water::plume_mouths`).
  pub mouths: [[f32; 4]; 16],
  /// The stream stones' lattice seed (`BoulderRules::stones`), which the
  /// water shader breaks its foam on.
  pub stones: u32,
}

/// The canopy layer this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct CanopyFrame {
  /// Crown area per square metre per unit of cover share, at the tree
  /// density (`lattice::canopy_density`); 0 without trees.
  pub density: f32,
  /// Where individual trees give way to the canopy layer, in metres.
  pub distance: f32,
  /// Whether the layer is drawn.
  pub drawn: bool,
}

/// Everything the renderer needs to shade one frame. The engine resolves
/// weather into these values, so the renderer never needs to know whether
/// a setting came from the host or from the weather system.
pub struct FrameParams {
  /// Combined view-projection matrix.
  pub view_proj: [f32; 16],
  /// Camera position in metres.
  pub camera_position: [f32; 3],
  /// Unit camera forward vector.
  pub camera_forward: [f32; 3],
  /// Unit camera right vector.
  pub camera_right: [f32; 3],
  /// Unit camera up vector.
  pub camera_up: [f32; 3],
  /// Vertical field of view.
  pub field_of_view_degrees: f32,
  /// Viewport aspect ratio.
  pub aspect_ratio: f32,
  /// Near plane distance.
  pub near_metres: f32,
  /// Far plane distance.
  pub far_metres: f32,
  /// Unit vector towards the sun.
  pub sun_direction: [f32; 3],
  /// Sun intensity.
  pub sun_intensity: f32,
  /// Atmosphere controls.
  pub atmosphere: AtmosphereOptions,
  /// Water controls.
  pub water: WaterOptions,
  /// Effective mist density (0 when mist is off).
  pub mist_density: f32,
  /// Mist noise strength (0 for flat mist).
  pub mist_noise_strength: f32,
  /// Water level for the rise-above-water term, or a far-away sentinel.
  pub mist_water_level_metres: f32,
  /// Mist controls.
  pub mist: MistOptions,
  /// Effective cloud coverage (0 when clouds are off).
  pub cloud_coverage: f32,
  /// Cloud raymarch steps (0 for painted clouds).
  pub cloud_raymarch_steps: u32,
  /// Effective altocumulus and altostratus amounts (0 when clouds are
  /// off).
  pub alto_amounts: [f32; 2],
  /// Cloud controls.
  pub clouds: CloudsOptions,
  /// Tree style: 0 billboard, 1 cross-quad, 2 mesh.
  pub tree_style: u32,
  /// Time canopy meshes, understorey meshes and impostors as separate
  /// passes.
  pub split_tree_timing: bool,
  /// Flora controls.
  pub flora: FloraOptions,
  /// Grass fade-out distance.
  pub grass_view_distance_metres: f32,
  /// The share of an ideal meadow's ground its tufts cover near the
  /// camera (0 while grass is off), for the ground's grass sheen.
  pub grass_cover: f32,
  /// How tall meadow grass grows (see `lattice::grass_height`).
  pub grass_height: f32,
  /// Tiles to stream this frame, and the near radii.
  pub vegetation: crate::render::vegetation::StreamFrame,
  /// The canopy layer.
  pub canopy: CanopyFrame,
  /// Bare rock: the beds' rise per metre along x and z, their spacing in
  /// metres, and the angle of the sunward side in radians (see
  /// `materials.wgsl`).
  pub rock: [f32; 4],
  /// Debug view index.
  pub debug_view: u32,
  /// Shadow controls.
  pub shadows: ShadowOptions,
  /// Terrain surface controls.
  pub surface: SurfaceOptions,
  /// Resolved weather.
  pub weather: FrameWeather,
  /// Sea ice conditions.
  pub sea_ice: SeaIce,
  /// River, lake and waterfall conditions.
  pub rivers: RiverFrame,
  /// The terrain mesh being drawn, for grounding trees and grass (see
  /// `terrain_mesh::mesh_ground_uniform`).
  pub mesh_ground: [f32; 4],
  /// Lowest and highest terrain heights, for fitting the shadow map.
  pub height_range: (f32, f32),
  /// Render, detail, and cloud distances.
  pub distances: vista_types::RenderDistances,
  /// Fraction of the canvas resolution to render the scene at (0.25 to 1).
  pub render_scale: f32,
  /// Smoothed time step to animate by, in seconds.
  pub frame_seconds: f32,
  /// What the scene draws this frame.
  pub needs: Needs,
  /// What it is likely to draw soon.
  pub likely: Needs,
}

impl FrameParams {
  /// Whether the cloud pass runs: low clouds, or a mid-level layer.
  pub fn clouds_drawn(&self) -> bool {
    self.cloud_coverage > 0.001 || self.alto_amounts[0].max(self.alto_amounts[1]) > 0.001
  }
}

/// One step of the surface weather, as the engine asks for it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SurfaceWeatherStep {
  /// Weather seconds to advance.
  pub dt: f32,
  /// Settle every texel to this state first, instead of stepping.
  pub settle: Option<crate::weather::surface::SurfaceCell>,
  /// Sunlight reaching the ground, 0 to 1.
  pub sun: f32,
  /// Mean wind in m/s.
  pub wind: f32,
  /// The weather's temperature offset in °C.
  pub celsius_offset: f32,
  /// Precipitation everywhere, when there is no regional map.
  pub precipitation: f32,
  /// The preset's mean precipitation: settling wets each place in
  /// proportion to its own share of it.
  pub mean_precipitation: f32,
  /// The regional map's size in metres, when it is read.
  pub regional: Option<f32>,
  /// Crown area per square metre per unit of cover share.
  pub canopy: f32,
}
