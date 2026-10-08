//! The C API as a host calls it: through raw pointers and status codes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;

use vista_native::*;

fn c(text: &str) -> CString {
  CString::new(text).unwrap()
}

fn last_error() -> String {
  unsafe { CStr::from_ptr(vista_last_error()) }
    .to_string_lossy()
    .into_owned()
}

/// An engine with a small eroded island, destroyed on drop.
struct Engine(*mut VistaEngine);

impl Engine {
  fn new() -> Self {
    let mut engine = ptr::null_mut();
    let status = unsafe { vista_engine_create(ptr::null(), &mut engine) };
    assert_eq!(status, VistaStatus::Ok, "{}", last_error());
    Self(engine)
  }

  fn island() -> Self {
    let engine = Self::new();
    let options = c(r#"{ "seed": 7, "size": 64, "shape": { "island": 0.5 }, "erosion": {} }"#);
    let status =
      unsafe { vista_engine_generate_fractal(engine.0, options.as_ptr(), None, ptr::null_mut()) };
    assert_eq!(status, VistaStatus::Ok, "{}", last_error());
    engine
  }

  fn map(&self, kind: u32, width: u32, height: u32) -> Result<*mut VistaMap, VistaStatus> {
    let mut map = ptr::null_mut();
    match unsafe { vista_engine_export_map(self.0, kind, width, height, &mut map) } {
      VistaStatus::Ok => Ok(map),
      status => Err(status),
    }
  }
}

impl Drop for Engine {
  fn drop(&mut self) {
    unsafe { vista_engine_destroy(self.0) };
  }
}

#[test]
fn the_version_is_the_crate_version() {
  let version = unsafe { CStr::from_ptr(vista_version()) };
  assert_eq!(version.to_str().unwrap(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn null_arguments_fail_with_a_message_instead_of_crashing() {
  let status = unsafe { vista_engine_create(ptr::null(), ptr::null_mut()) };
  assert_eq!(status, VistaStatus::InvalidArgument);
  assert!(last_error().contains("out"), "{}", last_error());

  let status =
    unsafe { vista_engine_generate_fractal(ptr::null_mut(), ptr::null(), None, ptr::null_mut()) };
  assert_eq!(status, VistaStatus::InvalidArgument);
  assert!(last_error().contains("engine"), "{}", last_error());

  // Freeing null is allowed, as with `free`.
  unsafe {
    vista_engine_destroy(ptr::null_mut());
    vista_map_free(ptr::null_mut());
    vista_trees_free(ptr::null_mut());
    vista_mesh_free(ptr::null_mut());
    vista_string_free(ptr::null_mut());
  }
}

#[test]
fn bad_options_fail_and_leave_the_terrain_in_place() {
  let engine = Engine::island();

  for json in [
    r#"{ "seed": 1, "typo": 2 }"#,
    "{ not json",
    r#"{ "size": 100 }"#,
  ] {
    let json = c(json);
    let status =
      unsafe { vista_engine_generate_fractal(engine.0, json.as_ptr(), None, ptr::null_mut()) };
    assert_eq!(status, VistaStatus::Options, "{json:?}");
    assert!(!last_error().is_empty());
  }

  let mut info = VistaTerrainInfo::default();
  let status = unsafe { vista_engine_terrain_info(engine.0, &mut info) };
  assert_eq!(status, VistaStatus::Ok);
  assert_eq!((info.width, info.height), (64, 64));
}

#[test]
fn partial_options_take_the_defaults_for_everything_left_out() {
  let engine = Engine::new();
  // `erosion` is absent by default; an empty object asks for erosion
  // with every erosion default.
  let options = c(r#"{ "size": 32, "erosion": {} }"#);
  let status =
    unsafe { vista_engine_generate_fractal(engine.0, options.as_ptr(), None, ptr::null_mut()) };
  assert_eq!(status, VistaStatus::Ok, "{}", last_error());
}

#[test]
fn a_terrain_must_exist_before_anything_is_read_from_it() {
  let engine = Engine::new();
  let mut info = VistaTerrainInfo::default();
  let status = unsafe { vista_engine_terrain_info(engine.0, &mut info) };
  assert_eq!(status, VistaStatus::Engine);
  assert!(
    last_error().contains("generate or load"),
    "{}",
    last_error()
  );
  assert!(engine.map(0, 0, 0).is_err());
}

#[test]
fn every_map_has_the_size_and_layout_it_describes() {
  let engine = Engine::island();

  for kind in 0..15 {
    for (width, height) in [(0, 0), (17, 9)] {
      let map = engine
        .map(kind, width, height)
        .unwrap_or_else(|status| panic!("kind {kind}: {status:?}: {}", last_error()));
      let view = unsafe { &*map };
      let (expected_width, expected_height) = if width == 0 {
        (64, 64)
      } else {
        (width, height)
      };
      let value_bytes = if view.is_float == 1 { 4 } else { 1 };
      assert_eq!((view.width, view.height), (expected_width, expected_height));
      assert_eq!(
        view.data_bytes,
        (view.width * view.height * view.channels) as usize * value_bytes,
        "kind {kind}"
      );
      let encoding = unsafe { CStr::from_ptr(view.encoding_json) }
        .to_str()
        .unwrap();
      let encoding: serde_json::Value = serde_json::from_str(encoding).unwrap();
      assert!(encoding["metresPerPixel"].is_array(), "kind {kind}");
      unsafe { vista_map_free(map) };
    }
  }

  assert_eq!(
    engine.map(15, 0, 0).unwrap_err(),
    VistaStatus::InvalidArgument
  );
  assert_eq!(
    engine.map(0, 8, 0).unwrap_err(),
    VistaStatus::InvalidArgument
  );
}

#[test]
fn the_heights_match_the_terrain_info() {
  let engine = Engine::island();
  let mut info = VistaTerrainInfo::default();
  unsafe { vista_engine_terrain_info(engine.0, &mut info) };
  let map = engine.map(0, 0, 0).unwrap();
  let view = unsafe { &*map };
  let heights = unsafe {
    std::slice::from_raw_parts(view.data.cast::<f32>(), (view.width * view.height) as usize)
  };
  let lowest = heights.iter().copied().fold(f32::MAX, f32::min);
  let highest = heights.iter().copied().fold(f32::MIN, f32::max);
  assert!((lowest - info.min_height_metres).abs() < 0.01);
  assert!((highest - info.max_height_metres).abs() < 0.01);
  unsafe { vista_map_free(map) };
}

#[test]
fn trees_meshes_and_queries_come_back_whole() {
  let engine = Engine::island();

  let mut trees = ptr::null_mut();
  let status = unsafe { vista_engine_export_trees(engine.0, ptr::null(), 1_000_000, &mut trees) };
  assert_eq!(status, VistaStatus::Ok, "{}", last_error());
  let view = unsafe { &*trees };
  assert_eq!(view.floats_per_record, 10);
  assert!(view.count > 0);
  unsafe { vista_trees_free(trees) };

  // A region with no trees in it exports none.
  let nowhere = [1.0e6_f32, 1.0e6, 1.0e6 + 1.0, 1.0e6 + 1.0];
  let mut trees = ptr::null_mut();
  let status = unsafe { vista_engine_export_trees(engine.0, nowhere.as_ptr(), 0, &mut trees) };
  assert_eq!(status, VistaStatus::Ok, "{}", last_error());
  assert_eq!(unsafe { (*trees).count }, 0);
  unsafe { vista_trees_free(trees) };

  for (part, stride) in [(0, 56), (1, 56), (2, 44)] {
    let mut mesh = ptr::null_mut();
    let status = unsafe { vista_engine_water_mesh(engine.0, part, &mut mesh) };
    assert_eq!(status, VistaStatus::Ok, "{}", last_error());
    let view = unsafe { &*mesh };
    assert_eq!(view.vertex_stride, stride);
    assert_eq!(view.index_count % 3, 0);
    let indices = unsafe { std::slice::from_raw_parts(view.indices, view.index_count as usize) };
    assert!(indices.iter().all(|&index| index < view.vertex_count));
    unsafe { vista_mesh_free(mesh) };
  }

  for what in ["metadata", "waterfalls", "inflows"] {
    let what = c(what);
    let mut json: *mut c_char = ptr::null_mut();
    let status = unsafe { vista_engine_query_json(engine.0, what.as_ptr(), &mut json) };
    assert_eq!(status, VistaStatus::Ok, "{}", last_error());
    let text = unsafe { CStr::from_ptr(json) }.to_str().unwrap();
    serde_json::from_str::<serde_json::Value>(text).unwrap();
    unsafe { vista_string_free(json) };
  }

  let mut biome = 255;
  let status = unsafe { vista_engine_biome_at(engine.0, 0.0, 0.0, &mut biome) };
  assert_eq!(status, VistaStatus::Ok);
  assert!(biome < 19);
  let status = unsafe { vista_engine_biome_at(engine.0, 1.0e7, 0.0, &mut biome) };
  assert_eq!(status, VistaStatus::InvalidArgument);
}

#[test]
fn option_groups_can_be_replaced_after_generation() {
  let engine = Engine::island();
  let flora = c("flora");
  let none = c(r#"{ "density": 0 }"#);
  let status = unsafe { vista_engine_set(engine.0, flora.as_ptr(), none.as_ptr()) };
  assert_eq!(status, VistaStatus::Ok, "{}", last_error());

  let mut trees = ptr::null_mut();
  unsafe { vista_engine_export_trees(engine.0, ptr::null(), 0, &mut trees) };
  assert_eq!(unsafe { (*trees).count }, 0);
  unsafe { vista_trees_free(trees) };

  let unknown = c("sky");
  let status = unsafe { vista_engine_set(engine.0, unknown.as_ptr(), ptr::null()) };
  assert_eq!(status, VistaStatus::InvalidArgument);
}

#[test]
fn a_raw_heightmap_loads() {
  let engine = Engine::new();
  let bytes: Vec<u8> = (0..32 * 32)
    .flat_map(|i: u32| ((i % 32) as f32 * 10.0).to_le_bytes())
    .collect();
  let options = c(
    r#"{ "width": 32, "height": 32, "sampleFormat": "float32", "metresPerSample": 10, "heightScaleMetres": 1 }"#,
  );
  let status = unsafe {
    vista_engine_load_raw_heightmap(engine.0, bytes.as_ptr(), bytes.len(), options.as_ptr())
  };
  assert_eq!(status, VistaStatus::Ok, "{}", last_error());
  let mut info = VistaTerrainInfo::default();
  unsafe { vista_engine_terrain_info(engine.0, &mut info) };
  assert_eq!((info.width, info.height), (32, 32));
}

#[test]
fn progress_reports_each_phase_in_order() {
  unsafe extern "C" fn record(phase: *const c_char, _: f32, user: *mut c_void) -> i32 {
    let phases = unsafe { &mut *user.cast::<Vec<String>>() };
    let phase = unsafe { CStr::from_ptr(phase) }
      .to_string_lossy()
      .into_owned();

    if phases.last() != Some(&phase) {
      phases.push(phase);
    }

    0
  }

  let engine = Engine::new();
  let mut phases: Vec<String> = Vec::new();
  let options = c(r#"{ "size": 32 }"#);
  let status = unsafe {
    vista_engine_generate_fractal(
      engine.0,
      options.as_ptr(),
      Some(record),
      (&mut phases as *mut Vec<String>).cast(),
    )
  };
  assert_eq!(status, VistaStatus::Ok, "{}", last_error());
  assert_eq!(phases.first().map(String::as_str), Some("tectonics"));
  assert!(phases.iter().any(|phase| phase == "finishing"));
}

#[test]
fn cancelling_stops_generation_and_keeps_the_previous_terrain() {
  /// Cancels at the first report of `user`'s phase, and counts the reports
  /// after it.
  struct Stop {
    phase: &'static str,
    stopped: bool,
    later: u32,
  }

  unsafe extern "C" fn stop_at(phase: *const c_char, _: f32, user: *mut c_void) -> i32 {
    let stop = unsafe { &mut *user.cast::<Stop>() };

    if stop.stopped {
      stop.later += 1;
    }

    let phase = unsafe { CStr::from_ptr(phase) }.to_string_lossy();
    stop.stopped |= phase == stop.phase;
    i32::from(stop.stopped)
  }

  let engine = Engine::island();
  let before = engine.map(0, 0, 0).unwrap();
  let heights_before =
    unsafe { std::slice::from_raw_parts((*before).data.cast::<f32>(), 64 * 64) }.to_vec();
  unsafe { vista_map_free(before) };

  for phase in ["tectonics", "drainage", "erosion"] {
    let mut stop = Stop {
      phase,
      stopped: false,
      later: 0,
    };
    let options = c(r#"{ "seed": 99, "size": 128, "erosion": {} }"#);
    let status = unsafe {
      vista_engine_generate_fractal(
        engine.0,
        options.as_ptr(),
        Some(stop_at),
        (&mut stop as *mut Stop).cast(),
      )
    };
    assert_eq!(status, VistaStatus::Cancelled, "{phase}");
    assert!(stop.stopped, "{phase}");
    assert_eq!(stop.later, 0, "{phase}: no report after the cancel");
    assert!(last_error().contains("cancelled"), "{}", last_error());

    // The island is still there, sample for sample.
    let mut info = VistaTerrainInfo::default();
    unsafe { vista_engine_terrain_info(engine.0, &mut info) };
    assert_eq!((info.width, info.height), (64, 64), "{phase}");
    let after = engine.map(0, 0, 0).unwrap();
    let heights_after = unsafe { std::slice::from_raw_parts((*after).data.cast::<f32>(), 64 * 64) };
    assert_eq!(heights_after, &heights_before[..], "{phase}");
    unsafe { vista_map_free(after) };
  }

  // And the engine still generates afterwards.
  let options = c(r#"{ "seed": 99, "size": 32 }"#);
  let status =
    unsafe { vista_engine_generate_fractal(engine.0, options.as_ptr(), None, ptr::null_mut()) };
  assert_eq!(status, VistaStatus::Ok, "{}", last_error());
}
