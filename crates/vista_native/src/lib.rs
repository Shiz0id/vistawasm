//! A C API for VistaWASM's terrain and world generation.
//!
//! Native engines link this crate as a static or dynamic library and call
//! the functions declared in `include/vista_native.h`. It runs the same
//! engine core as the browser build: terrain generation, CPU erosion,
//! rivers, lakes, glaciers, biomes, materials, and tree and grass
//! placement. The host renders the results itself, or attaches a renderer
//! ([`renderer`]): the browser build's own, lowered to Direct3D 11
//! commands ([`d3d11`], `include/vista_d3d11.h`) for the host's device.
//!
//! Options cross the boundary as JSON in the same shape as the
//! JavaScript API's options (camelCase keys), so every option the engine
//! has is available without a C struct for each.
//!
//! Every function returns a [`VistaStatus`]. On failure,
//! `vista_last_error()` explains why. Results the library allocates are
//! freed with their matching `vista_*_free` function.

#![cfg(not(target_arch = "wasm32"))]

pub mod d3d11;
mod ffi;
pub mod renderer;

use std::ffi::{c_char, c_void, CString};

use vista_types::{
  AtmosphereOptions, BiomeOptions, CameraOptions, CloudsOptions, DebugView, DemLoadOptions,
  FloraOptions, FractalTerrainOptions, GrassOptions, MistOptions, RawHeightmapOptions,
  RenderQualityOptions, ShadowOptions, SunOptions, SurfaceOptions, TimeOfDayOptions,
  VistaEngineOptions, WaterOptions, WeatherOptions,
};
use vista_wasm::engine::EngineCore;
use vista_wasm::export::{MapData, MapKind, TREE_RECORD_FLOATS};

pub use ffi::VistaStatus;
use ffi::{
  bytes_arg, call, mut_arg, optional_str_arg, parse_json, ref_arg, require_out, str_arg, write_out,
  Failure, Outcome,
};

/// An engine: one terrain and everything built from it. Not thread-safe:
/// use each engine from one thread at a time.
pub struct VistaEngine {
  core: EngineCore,
  /// The renderer, once `vista_renderer_attach()` adds one.
  renderer: Option<renderer::NativeRenderer>,
}

/// The active terrain's size and heights.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VistaTerrainInfo {
  /// Samples per row.
  pub width: u32,
  /// Rows.
  pub height: u32,
  /// Metres between neighbouring samples.
  pub metres_per_sample: f32,
  /// Sea level in metres.
  pub sea_level_metres: f32,
  /// Lowest height in metres.
  pub min_height_metres: f32,
  /// Highest height in metres.
  pub max_height_metres: f32,
  /// Mean height in metres.
  pub mean_height_metres: f32,
}

/// An exported map. Free it with `vista_map_free()`.
#[repr(C)]
#[derive(Debug)]
pub struct VistaMap {
  /// Pixels per row.
  pub width: u32,
  /// Rows.
  pub height: u32,
  /// Values per pixel, interleaved.
  pub channels: u32,
  /// 1 when `data` holds `float`s, 0 when it holds `uint8_t`s.
  pub is_float: u32,
  /// Row-major values, `width * height * channels` of them.
  pub data: *const c_void,
  /// `data`'s length in bytes.
  pub data_bytes: usize,
  /// How to read the values: units, scale, range, legend, metres per
  /// pixel, sea level and generator, as JSON.
  pub encoding_json: *const c_char,
}

/// `VistaMap` with the storage its pointers point into. `map` comes
/// first, so a pointer to this is a pointer to it.
#[repr(C)]
struct OwnedMap {
  map: VistaMap,
  _data: MapData,
  _encoding: CString,
}

/// Exported trees. Free them with `vista_trees_free()`.
#[repr(C)]
#[derive(Debug)]
pub struct VistaTrees {
  /// `count` records of `floats_per_record` floats: x, y, z (metres),
  /// species, variant, scale, rotation (radians), tint, dryness, and 1 for
  /// a hand-placed tree or 0 for a procedural one.
  pub records: *const f32,
  /// Number of trees.
  pub count: u32,
  /// Floats per tree (10).
  pub floats_per_record: u32,
}

#[repr(C)]
struct OwnedTrees {
  trees: VistaTrees,
  _records: Vec<f32>,
}

/// Which part of the water's geometry `vista_engine_water_mesh()` returns.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VistaWaterMeshPart {
  /// River ribbons, lakes, oxbows and plunge pools (`VistaWaterVertex`).
  Surfaces = 0,
  /// Waterfall sheets and mist (`VistaWaterVertex`).
  Falls = 1,
  /// Bank strips beside streams narrower than a sample
  /// (`VistaBankVertex`).
  Banks = 2,
}

/// An indexed triangle list. Free it with `vista_mesh_free()`.
#[repr(C)]
#[derive(Debug)]
pub struct VistaMesh {
  /// `vertex_count` vertices of `vertex_stride` bytes.
  pub vertices: *const c_void,
  /// Number of vertices.
  pub vertex_count: u32,
  /// Bytes per vertex.
  pub vertex_stride: u32,
  /// Triangle list indices into `vertices`.
  pub indices: *const u32,
  /// Number of indices, three per triangle.
  pub index_count: u32,
}

#[repr(C)]
struct OwnedMesh {
  mesh: VistaMesh,
  _vertices: Vec<u8>,
  _indices: Vec<u32>,
}

/// Called as generation advances, with the phase's name and its progress
/// from 0 to 1. `phase` is valid only during the call. Returns 0 to go on,
/// or anything else to cancel.
pub type VistaProgressFn = Option<unsafe extern "C" fn(*const c_char, f32, *mut c_void) -> i32>;

/// The library's version, such as `"2.0.0"`.
#[no_mangle]
pub extern "C" fn vista_version() -> *const c_char {
  concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// The last error on this thread. Valid until the next failed call on
/// this thread; never null.
#[no_mangle]
pub extern "C" fn vista_last_error() -> *const c_char {
  ffi::last_error()
}

/// Create an engine from engine options as JSON (null or `""` for the
/// defaults). Render, camera and sky options are accepted and checked
/// but have no effect without a renderer.
///
/// # Safety
///
/// `options_json` is null or a NUL-terminated string. `out` points to
/// writable memory for a pointer.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_create(
  options_json: *const c_char,
  out: *mut *mut VistaEngine,
) -> VistaStatus {
  call(|| {
    require_out(out, "out")?;
    // SAFETY: as the caller promises.
    let json = unsafe { optional_str_arg(options_json, "options_json") }?;
    let options: VistaEngineOptions = parse_json(json, "options_json")?;
    let core = EngineCore::new_for_tests(options)?;
    let engine = Box::into_raw(Box::new(VistaEngine {
      core,
      renderer: None,
    }));
    // SAFETY: checked above.
    unsafe { write_out(out, engine, "out") }
  })
}

/// Destroy an engine. Null is ignored.
///
/// # Safety
///
/// `engine` is null or came from `vista_engine_create()` and has not been
/// destroyed.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_destroy(engine: *mut VistaEngine) {
  if !engine.is_null() {
    // SAFETY: as the caller promises.
    drop(unsafe { Box::from_raw(engine) });
  }
}

/// Generate a seeded terrain from fractal terrain options as JSON (null
/// or `""` for the defaults), and build its world. Erosion, when the
/// options ask for it, runs on the CPU. A failure leaves the previous
/// terrain in place. `progress` returning non-zero cancels: generation
/// stops at its next check with `VISTA_CANCELLED`, the previous terrain
/// unchanged. Once the new terrain starts to replace the old ("rivers" and
/// the end of "finishing"), the return value is not read.
///
/// # Safety
///
/// `engine` is a live engine; `options_json` is null or a NUL-terminated
/// string; `progress` is null or safe to call with `user`.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_generate_fractal(
  engine: *mut VistaEngine,
  options_json: *const c_char,
  progress: VistaProgressFn,
  user: *mut c_void,
) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    // SAFETY: as the caller promises.
    let json = unsafe { optional_str_arg(options_json, "options_json") }?;
    let options: FractalTerrainOptions = parse_json(json, "options_json")?;
    let mut report = |phase: &str, fraction: f32| match (progress, CString::new(phase)) {
      // SAFETY: the caller promises the callback is safe with `user`.
      (Some(callback), Ok(phase)) => (unsafe { callback(phase.as_ptr(), fraction, user) }) == 0,
      _ => true,
    };
    ffi::block_on(
      engine
        .core
        .generate_fractal_with_progress(options, &mut report),
    )?;
    Ok(())
  })
}

/// Load a raw heightmap (`"uint16"`, `"int16"` or `"float32"` samples) with raw heightmap
/// options as JSON, which must give at least its width, height and sample
/// format.
///
/// # Safety
///
/// `engine` is a live engine; `bytes` points to `length` readable bytes;
/// `options_json` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_load_raw_heightmap(
  engine: *mut VistaEngine,
  bytes: *const u8,
  length: usize,
  options_json: *const c_char,
) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    // SAFETY: as the caller promises.
    let bytes = unsafe { bytes_arg(bytes, length, "bytes") }?;
    // SAFETY: as the caller promises.
    let json = unsafe { str_arg(options_json, "options_json") }?;
    let options: RawHeightmapOptions = serde_json::from_str(json)
      .map_err(|error| Failure::options(format!("options_json is not valid: {error}.")))?;
    ffi::block_on(engine.core.load_raw_heightmap(bytes, options))?;
    Ok(())
  })
}

/// Load an uncompressed GeoTIFF elevation file, with DEM load options as
/// JSON (null or `""` for the defaults).
///
/// # Safety
///
/// `engine` is a live engine; `bytes` points to `length` readable bytes;
/// `options_json` is null or a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_load_geotiff(
  engine: *mut VistaEngine,
  bytes: *const u8,
  length: usize,
  options_json: *const c_char,
) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    // SAFETY: as the caller promises.
    let bytes = unsafe { bytes_arg(bytes, length, "bytes") }?;
    // SAFETY: as the caller promises.
    let json = unsafe { optional_str_arg(options_json, "options_json") }?;
    let options: DemLoadOptions = parse_json(json, "options_json")?;
    ffi::block_on(engine.core.load_dem_from_array_buffer(bytes, options))?;
    Ok(())
  })
}

/// Replace one group of options, and rebuild what depends on it.
/// `section` names the group, as the engine options name it: `"biomes"`,
/// `"flora"`, `"grass"`, `"water"` and `"surface"` shape the world;
/// `"camera"`, `"sun"`, `"atmosphere"`, `"clouds"`, `"mist"`,
/// `"quality"`, `"weather"`, `"shadows"`, `"timeOfDay"` and `"debugView"`
/// only what a renderer draws. `options_json` is that group as JSON (null
/// or `""` for its defaults; for `"debugView"`, a string such as
/// `"\"slope\""`).
///
/// # Safety
///
/// `engine` is a live engine; both strings are NUL-terminated, and
/// `options_json` may be null.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_set(
  engine: *mut VistaEngine,
  section: *const c_char,
  options_json: *const c_char,
) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    // SAFETY: as the caller promises.
    let section = unsafe { str_arg(section, "section") }?;
    // SAFETY: as the caller promises.
    let json = unsafe { optional_str_arg(options_json, "options_json") }?;
    let core = &mut engine.core;

    match section {
      "biomes" => core.set_biomes(parse_json::<BiomeOptions>(json, "options_json")?)?,
      "flora" => core.set_flora(parse_json::<FloraOptions>(json, "options_json")?)?,
      "grass" => core.set_grass(parse_json::<GrassOptions>(json, "options_json")?)?,
      "water" => core.set_water(parse_json::<WaterOptions>(json, "options_json")?)?,
      "surface" => core.set_surface(parse_json::<SurfaceOptions>(json, "options_json")?)?,
      "camera" => core.set_camera(parse_json::<CameraOptions>(json, "options_json")?)?,
      "sun" => core.set_sun(parse_json::<SunOptions>(json, "options_json")?)?,
      "atmosphere" => core.set_atmosphere(parse_json::<AtmosphereOptions>(json, "options_json")?)?,
      "clouds" => core.set_clouds(parse_json::<CloudsOptions>(json, "options_json")?)?,
      "mist" => core.set_mist(parse_json::<MistOptions>(json, "options_json")?)?,
      "quality" => {
        core.set_render_quality(parse_json::<RenderQualityOptions>(json, "options_json")?)?
      }
      "weather" => core.set_weather(parse_json::<WeatherOptions>(json, "options_json")?)?,
      "shadows" => core.set_shadows(parse_json::<ShadowOptions>(json, "options_json")?)?,
      "timeOfDay" => core.set_time_of_day(parse_json::<TimeOfDayOptions>(json, "options_json")?)?,
      "debugView" => core.set_debug_view(parse_json::<DebugView>(json, "options_json")?)?,
      other => {
        return Err(Failure::argument(format!(
          "section must be \"biomes\", \"flora\", \"grass\", \"water\", \"surface\", \"camera\", \"sun\", \"atmosphere\", \"clouds\", \"mist\", \"quality\", \"weather\", \"shadows\", \"timeOfDay\" or \"debugView\", but it is \"{other}\"."
        )))
      }
    }

    Ok(())
  })
}

/// Run the weather `seconds` ahead at once (up to a day), as if that much
/// time had passed: rain falls, puddles fill and snow settles.
///
/// # Safety
///
/// `engine` is a live engine.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_advance_weather(
  engine: *mut VistaEngine,
  seconds: f32,
) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    engine.core.advance_weather(seconds)?;
    Ok(())
  })
}

fn active_terrain(engine: &VistaEngine) -> Outcome<&vista_wasm::terrain::HeightMap> {
  engine.core.terrain().ok_or_else(|| Failure {
    status: VistaStatus::Engine,
    message: "The engine has no terrain: generate or load one first.".to_string(),
  })
}

/// Describe the active terrain.
///
/// # Safety
///
/// `engine` is a live engine; `out` points to a writable
/// `VistaTerrainInfo`.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_terrain_info(
  engine: *const VistaEngine,
  out: *mut VistaTerrainInfo,
) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { ref_arg(engine, "engine") }?;
    let metadata = &active_terrain(engine)?.metadata;
    let info = VistaTerrainInfo {
      width: metadata.width,
      height: metadata.height,
      metres_per_sample: metadata.metres_per_sample,
      sea_level_metres: metadata.sea_level_metres,
      min_height_metres: metadata.min_height_metres,
      max_height_metres: metadata.max_height_metres,
      mean_height_metres: metadata.mean_height_metres,
    };
    // SAFETY: as the caller promises.
    unsafe { write_out(out, info, "out") }
  })
}

/// Export one of the world's maps (a `VistaMapKind`) at the terrain's own
/// size (`width` and `height` 0) or resampled to `width` by `height`.
///
/// # Safety
///
/// `engine` is a live engine; `out` points to writable memory for a
/// pointer.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_export_map(
  engine: *const VistaEngine,
  kind: u32,
  width: u32,
  height: u32,
  out: *mut *mut VistaMap,
) -> VistaStatus {
  call(|| {
    require_out(out, "out")?;
    // SAFETY: as the caller promises.
    let engine = unsafe { ref_arg(engine, "engine") }?;
    let kind = *MapKind::ALL.get(kind as usize).ok_or_else(|| {
      Failure::argument(format!(
        "kind must be a VistaMapKind from 0 to {}, but it is {kind}.",
        MapKind::ALL.len() - 1
      ))
    })?;
    let size = match (width, height) {
      (0, 0) => None,
      (0, _) | (_, 0) => {
        return Err(Failure::argument(
          "width and height must both be 0 (the terrain's size) or both be at least 1.",
        ))
      }
      size => Some([size.0, size.1]),
    };
    let exported = engine.core.export_map(kind, size)?;
    let encoding = encoding_json(&exported.encoding);
    let (data, data_bytes, is_float) = match &exported.data {
      MapData::F32(values) => (values.as_ptr().cast::<c_void>(), values.len() * 4, 1),
      MapData::U8(values) => (values.as_ptr().cast::<c_void>(), values.len(), 0),
    };
    let owned = Box::new(OwnedMap {
      map: VistaMap {
        width: exported.width,
        height: exported.height,
        channels: exported.channels,
        is_float,
        data,
        data_bytes,
        encoding_json: encoding.as_ptr(),
      },
      _data: exported.data,
      _encoding: encoding,
    });
    // SAFETY: checked above. `map` is the first field of a `repr(C)`
    // struct, so the pointers are the same.
    unsafe { write_out(out, Box::into_raw(owned).cast::<VistaMap>(), "out") }
  })
}

fn encoding_json(encoding: &vista_wasm::export::MapEncoding) -> CString {
  let legend = encoding.legend.map(|legend| {
    legend
      .iter()
      .map(|(name, colour)| serde_json::json!({ "name": name, "colour": colour }))
      .collect::<Vec<_>>()
  });
  let json = serde_json::json!({
    "units": encoding.units,
    "scale": encoding.scale,
    "range": encoding.range,
    "legend": legend,
    "metresPerPixel": encoding.metres_per_pixel,
    "seaLevelMetres": encoding.sea_level_metres,
    "generator": encoding.generator,
  });
  CString::new(json.to_string()).unwrap_or_default()
}

/// Free a map from `vista_engine_export_map()`. Null is ignored.
///
/// # Safety
///
/// `map` is null or came from `vista_engine_export_map()` and has not
/// been freed.
#[no_mangle]
pub unsafe extern "C" fn vista_map_free(map: *mut VistaMap) {
  if !map.is_null() {
    // SAFETY: as the caller promises, it is the first field of an
    // `OwnedMap` this library boxed.
    drop(unsafe { Box::from_raw(map.cast::<OwnedMap>()) });
  }
}

/// Export the trees inside `region` (min x, min z, max x, max z in world
/// metres; null for the whole terrain), at most `max_count` of them (0
/// for the engine's default). More trees than that is an error.
///
/// # Safety
///
/// `engine` is a live engine; `region` is null or points to 4 floats;
/// `out` points to writable memory for a pointer.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_export_trees(
  engine: *const VistaEngine,
  region: *const f32,
  max_count: u32,
  out: *mut *mut VistaTrees,
) -> VistaStatus {
  call(|| {
    require_out(out, "out")?;
    // SAFETY: as the caller promises.
    let engine = unsafe { ref_arg(engine, "engine") }?;
    let region = if region.is_null() {
      None
    } else {
      // SAFETY: not null, and the caller promises 4 floats.
      let values = unsafe { std::slice::from_raw_parts(region, 4) };
      Some([values[0], values[1], values[2], values[3]])
    };
    let max_count = (max_count != 0).then_some(max_count);
    let records = engine.core.export_trees(region, max_count)?;
    let count = u32::try_from(records.len() / TREE_RECORD_FLOATS)
      .map_err(|_| Failure::argument("Too many trees to export at once."))?;
    let owned = Box::new(OwnedTrees {
      trees: VistaTrees {
        records: records.as_ptr(),
        count,
        floats_per_record: TREE_RECORD_FLOATS as u32,
      },
      _records: records,
    });
    // SAFETY: checked above; `trees` is the first field.
    unsafe { write_out(out, Box::into_raw(owned).cast::<VistaTrees>(), "out") }
  })
}

/// Free trees from `vista_engine_export_trees()`. Null is ignored.
///
/// # Safety
///
/// `trees` is null or came from `vista_engine_export_trees()` and has not
/// been freed.
#[no_mangle]
pub unsafe extern "C" fn vista_trees_free(trees: *mut VistaTrees) {
  if !trees.is_null() {
    // SAFETY: as the caller promises.
    drop(unsafe { Box::from_raw(trees.cast::<OwnedTrees>()) });
  }
}

/// Copy one part of the water's geometry: river and lake surfaces,
/// waterfalls, or bank strips, in world metres. The mesh may be empty.
///
/// # Safety
///
/// `engine` is a live engine; `out` points to writable memory for a
/// pointer.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_water_mesh(
  engine: *const VistaEngine,
  part: u32,
  out: *mut *mut VistaMesh,
) -> VistaStatus {
  call(|| {
    require_out(out, "out")?;
    // SAFETY: as the caller promises.
    let engine = unsafe { ref_arg(engine, "engine") }?;
    active_terrain(engine)?;
    let rivers = engine.core.river_network();
    let (vertices, stride, indices): (Vec<u8>, usize, &[u32]) = match part {
      0 => (
        bytemuck::cast_slice(&rivers.vertices).to_vec(),
        std::mem::size_of::<vista_wasm::render::water::WaterVertex>(),
        &rivers.indices,
      ),
      1 => (
        bytemuck::cast_slice(&rivers.fall_vertices).to_vec(),
        std::mem::size_of::<vista_wasm::render::water::WaterVertex>(),
        &rivers.fall_indices,
      ),
      2 => (
        bytemuck::cast_slice(&rivers.bank_vertices).to_vec(),
        std::mem::size_of::<vista_wasm::render::water::BankVertex>(),
        &rivers.bank_indices,
      ),
      other => {
        return Err(Failure::argument(format!(
          "part must be a VistaWaterMeshPart from 0 to 2, but it is {other}."
        )))
      }
    };
    let indices = indices.to_vec();
    let too_big = || Failure::argument("The mesh is too large to describe with 32-bit counts.");
    let owned = Box::new(OwnedMesh {
      mesh: VistaMesh {
        vertices: vertices.as_ptr().cast(),
        vertex_count: u32::try_from(vertices.len() / stride).map_err(|_| too_big())?,
        vertex_stride: stride as u32,
        indices: indices.as_ptr(),
        index_count: u32::try_from(indices.len()).map_err(|_| too_big())?,
      },
      _vertices: vertices,
      _indices: indices,
    });
    // SAFETY: checked above; `mesh` is the first field.
    unsafe { write_out(out, Box::into_raw(owned).cast::<VistaMesh>(), "out") }
  })
}

/// Free a mesh from `vista_engine_water_mesh()`. Null is ignored.
///
/// # Safety
///
/// `mesh` is null or came from `vista_engine_water_mesh()` and has not
/// been freed.
#[no_mangle]
pub unsafe extern "C" fn vista_mesh_free(mesh: *mut VistaMesh) {
  if !mesh.is_null() {
    // SAFETY: as the caller promises.
    drop(unsafe { Box::from_raw(mesh.cast::<OwnedMesh>()) });
  }
}

/// The biome (a `VistaBiomeKind`) at world `x`, `z` in metres. Fails when
/// the point is outside the terrain.
///
/// # Safety
///
/// `engine` is a live engine; `out` points to a writable byte.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_biome_at(
  engine: *const VistaEngine,
  x: f32,
  z: f32,
  out: *mut u8,
) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { ref_arg(engine, "engine") }?;
    active_terrain(engine)?;
    let biome = engine
      .core
      .biome_at(x, z)
      .ok_or_else(|| Failure::argument(format!("({x}, {z}) is outside the terrain.")))?;
    // SAFETY: as the caller promises.
    unsafe { write_out(out, biome as u8, "out") }
  })
}

/// Describe part of the world as JSON: `"metadata"` (the terrain's
/// metadata, warnings included), `"waterfalls"` or `"inflows"`. Free the
/// string with `vista_string_free()`.
///
/// # Safety
///
/// `engine` is a live engine; `what` is a NUL-terminated string; `out`
/// points to writable memory for a pointer.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_query_json(
  engine: *const VistaEngine,
  what: *const c_char,
  out: *mut *mut c_char,
) -> VistaStatus {
  call(|| {
    require_out(out, "out")?;
    // SAFETY: as the caller promises.
    let engine = unsafe { ref_arg(engine, "engine") }?;
    // SAFETY: as the caller promises.
    let what = unsafe { str_arg(what, "what") }?;
    let json = match what {
      "metadata" => serde_json::to_string(&active_terrain(engine)?.metadata),
      "waterfalls" => serde_json::to_string(&engine.core.waterfalls()),
      "inflows" => serde_json::to_string(&engine.core.inflows()),
      other => {
        return Err(Failure::argument(format!(
          "what must be \"metadata\", \"waterfalls\" or \"inflows\", but it is \"{other}\"."
        )))
      }
    }
    .map_err(|error| Failure {
      status: VistaStatus::Engine,
      message: format!("Could not write {what} as JSON: {error}."),
    })?;
    let json = CString::new(json).unwrap_or_default();
    // SAFETY: checked above.
    unsafe { write_out(out, json.into_raw(), "out") }
  })
}

/// Free a string from `vista_engine_query_json()`. Null is ignored.
///
/// # Safety
///
/// `text` is null or came from `vista_engine_query_json()` and has not
/// been freed.
#[no_mangle]
pub unsafe extern "C" fn vista_string_free(text: *mut c_char) {
  if !text.is_null() {
    // SAFETY: as the caller promises, it came from `CString::into_raw`.
    drop(unsafe { CString::from_raw(text) });
  }
}
