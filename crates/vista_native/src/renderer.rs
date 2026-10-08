//! Rendering: the engine draws with the browser build's own renderer,
//! recorded and lowered to Direct3D 11 commands for the host to run (see
//! `include/vista_d3d11.h` and `crate::d3d11`).

use std::ffi::{c_char, c_void, CString};

use vista_wasm::render::recorder::{Error, Recorder, TextureFormat};

use crate::d3d11::Lowering;
use crate::ffi::{
  bytes_arg, call, mut_arg, ref_arg, require_out, str_arg, write_out, Failure, Outcome,
};
use crate::{VistaEngine, VistaStatus};

/// The host's render target format.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VistaOutputFormat {
  /// `DXGI_FORMAT_R8G8B8A8_UNORM`: the renderer applies the display gamma.
  Rgba8 = 0,
  /// `DXGI_FORMAT_R8G8B8A8_UNORM_SRGB`: the target applies it.
  Rgba8Srgb = 1,
  /// `DXGI_FORMAT_B8G8R8A8_UNORM`, as most swap chains.
  Bgra8 = 2,
  /// `DXGI_FORMAT_B8G8R8A8_UNORM_SRGB`.
  Bgra8Srgb = 3,
  /// `DXGI_FORMAT_R16G16B16A16_FLOAT`, tone mapped and gamma encoded.
  Rgba16Float = 4,
}

impl VistaOutputFormat {
  fn texture_format(self) -> TextureFormat {
    match self {
      Self::Rgba8 => TextureFormat::Rgba8Unorm,
      Self::Rgba8Srgb => TextureFormat::Rgba8UnormSrgb,
      Self::Bgra8 => TextureFormat::Bgra8Unorm,
      Self::Bgra8Srgb => TextureFormat::Bgra8UnormSrgb,
      Self::Rgba16Float => TextureFormat::Rgba16Float,
    }
  }
}

/// A command stream to run, valid until the next renderer call on its
/// engine.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VistaCommands {
  /// The records (see `vista_d3d11.h`).
  pub data: *const u8,
  /// Their length in bytes.
  pub bytes: usize,
}

/// An attached renderer: the recording, its lowering and the last
/// stream handed out.
pub struct NativeRenderer {
  recorder: Recorder,
  lowering: Lowering,
  stream: Vec<u8>,
}

impl NativeRenderer {
  /// Lower everything recorded since the last stream into a new one.
  fn drain(&mut self) -> VistaCommands {
    self.lowering.lower(self.recorder.take_ops());

    // What the lowering could not do reaches the host as GPU errors.
    for warning in self.lowering.take_warnings() {
      self.recorder.report_error(Error::Validation {
        source: (),
        description: format!("Direct3D 11 lowering: {warning}."),
      });
    }

    self.stream = self.lowering.take_stream();
    VistaCommands {
      data: self.stream.as_ptr(),
      bytes: self.stream.len(),
    }
  }
}

fn renderer(engine: &mut VistaEngine) -> Outcome<&mut NativeRenderer> {
  engine.renderer.as_mut().ok_or_else(|| Failure {
    status: VistaStatus::Engine,
    message: "The engine has no renderer: call vista_renderer_attach() first.".to_string(),
  })
}

/// Attach a renderer drawing into the host's `width` x `height` target
/// in `format`. An active terrain is uploaded: the commands come with the
/// next stream. A renderer already attached is replaced; destroy its
/// executor's objects.
///
/// # Safety
///
/// `engine` is a live engine.
#[no_mangle]
pub unsafe extern "C" fn vista_renderer_attach(
  engine: *mut VistaEngine,
  width: u32,
  height: u32,
  format: VistaOutputFormat,
) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    let recorder = Recorder::new();
    let format = format.texture_format();
    engine
      .core
      .attach_renderer(recorder.clone(), width, height, format)?;
    engine.renderer = Some(NativeRenderer {
      recorder,
      lowering: Lowering::new(width, height, format),
      stream: Vec::new(),
    });
    Ok(())
  })
}

/// Detach the renderer. The engine carries on without drawing. Destroy
/// the executor that ran its streams: it holds every object they made.
///
/// # Safety
///
/// `engine` is a live engine.
#[no_mangle]
pub unsafe extern "C" fn vista_renderer_detach(engine: *mut VistaEngine) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    engine.core.detach_renderer();
    engine.renderer = None;
    Ok(())
  })
}

/// Resize the host's render target. The change comes with the next
/// stream, as a `VISTA_D3D_OUTPUT_SIZE` record.
///
/// # Safety
///
/// `engine` is a live engine with a renderer.
#[no_mangle]
pub unsafe extern "C" fn vista_renderer_resize(
  engine: *mut VistaEngine,
  width: u32,
  height: u32,
) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    renderer(engine)?;
    engine.core.resize(width, height, Some(1.0))?;
    Ok(())
  })
}

/// Draw one frame at the host's clock `now_ms` (milliseconds, any origin,
/// never going back; a negative value steps a sixtieth of a second), and
/// return the commands to run: everything recorded since the last stream,
/// then the frame. The frame ends with `VISTA_D3D_PRESENT` unless the
/// engine skipped it. Calling this also tells the engine the streams
/// handed out before were run.
///
/// # Safety
///
/// `engine` is a live engine with a renderer; `out` points to writable
/// memory for a `VistaCommands`.
#[no_mangle]
pub unsafe extern "C" fn vista_renderer_frame(
  engine: *mut VistaEngine,
  now_ms: f64,
  out: *mut VistaCommands,
) -> VistaStatus {
  call(|| {
    require_out(out, "out")?;
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    renderer(engine)?.recorder.work_done();
    engine
      .core
      .set_host_clock_ms((now_ms >= 0.0).then_some(now_ms));
    let rendered = engine.core.render_once().map(|_| ());
    let commands = renderer(engine)?.drain();
    // SAFETY: checked above.
    unsafe { write_out(out, commands, "out") }?;
    rendered.map_err(Failure::from)
  })
}

/// Return the commands recorded since the last stream without drawing:
/// uploads after generating or loading a terrain, or changing options.
/// Run them before the next frame's.
///
/// # Safety
///
/// As `vista_renderer_frame()`.
#[no_mangle]
pub unsafe extern "C" fn vista_renderer_commands(
  engine: *mut VistaEngine,
  out: *mut VistaCommands,
) -> VistaStatus {
  call(|| {
    require_out(out, "out")?;
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    let commands = renderer(engine)?.drain();
    // SAFETY: checked above.
    unsafe { write_out(out, commands, "out") }
  })
}

/// Hand over the bytes of a buffer a `VISTA_D3D_READBACK` record asked
/// for: `size` bytes, the buffer's whole contents.
///
/// # Safety
///
/// `engine` is a live engine with a renderer; `bytes` points to `size`
/// readable bytes.
#[no_mangle]
pub unsafe extern "C" fn vista_renderer_complete_read(
  engine: *mut VistaEngine,
  buffer: u32,
  bytes: *const c_void,
  size: usize,
) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    // SAFETY: as the caller promises.
    let bytes = unsafe { bytes_arg(bytes.cast(), size, "bytes") }?;

    if renderer(engine)?.recorder.complete_map(buffer, bytes) {
      Ok(())
    } else {
      Err(Failure::argument(format!(
        "buffer {buffer} has no read waiting, or {size} bytes is not its size."
      )))
    }
  })
}

/// A `VISTA_D3D_READBACK` the host could not make.
///
/// # Safety
///
/// `engine` is a live engine with a renderer.
#[no_mangle]
pub unsafe extern "C" fn vista_renderer_fail_read(
  engine: *mut VistaEngine,
  buffer: u32,
) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    renderer(engine)?.recorder.fail_map(buffer);
    Ok(())
  })
}

/// Report an error running a stream (a failed `Create*` call, say). It
/// joins the engine's GPU errors (`vista_engine_events_json()`).
///
/// # Safety
///
/// `engine` is a live engine with a renderer; `message` is a
/// NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn vista_renderer_report_error(
  engine: *mut VistaEngine,
  message: *const c_char,
) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    // SAFETY: as the caller promises.
    let message = unsafe { str_arg(message, "message") }?;
    renderer(engine)?.recorder.report_error(Error::Validation {
      source: (),
      description: message.to_string(),
    });
    Ok(())
  })
}

/// Report the device lost (`DXGI_ERROR_DEVICE_REMOVED`, say). Frames then
/// fail; detach the renderer and attach a new one on a new device.
///
/// # Safety
///
/// As `vista_renderer_report_error()`.
#[no_mangle]
pub unsafe extern "C" fn vista_renderer_report_lost(
  engine: *mut VistaEngine,
  message: *const c_char,
) -> VistaStatus {
  call(|| {
    // SAFETY: as the caller promises.
    let engine = unsafe { mut_arg(engine, "engine") }?;
    // SAFETY: as the caller promises.
    let message = unsafe { str_arg(message, "message") }?;
    renderer(engine)?.recorder.report_lost(message.to_string());
    Ok(())
  })
}

fn json_out(value: &serde_json::Value, out: *mut *mut c_char) -> Outcome {
  let text = CString::new(value.to_string()).map_err(|_| Failure::argument("unreadable JSON"))?;
  // SAFETY: the caller checked `out`.
  unsafe { write_out(out, text.into_raw(), "out") }
}

/// The last frame's statistics as JSON (`RenderStats` in
/// docs/options-reference.md). Free it with `vista_string_free()`.
///
/// # Safety
///
/// `engine` is a live engine; `out` points to writable memory for a
/// pointer.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_stats_json(
  engine: *const VistaEngine,
  out: *mut *mut c_char,
) -> VistaStatus {
  call(|| {
    require_out(out, "out")?;
    // SAFETY: as the caller promises.
    let engine = unsafe { ref_arg(engine, "engine") }?;
    let value = serde_json::to_value(engine.core.stats())
      .map_err(|error| Failure::argument(error.to_string()))?;
    json_out(&value, out)
  })
}

/// GPU errors since the last call, and why the device was lost if it was,
/// as `{ "errors": [...], "lost": null or "..." }`. Free it with
/// `vista_string_free()`.
///
/// # Safety
///
/// As `vista_engine_stats_json()`.
#[no_mangle]
pub unsafe extern "C" fn vista_engine_events_json(
  engine: *const VistaEngine,
  out: *mut *mut c_char,
) -> VistaStatus {
  call(|| {
    require_out(out, "out")?;
    // SAFETY: as the caller promises.
    let engine = unsafe { ref_arg(engine, "engine") }?;
    let (errors, lost) = engine.core.take_gpu_events();
    json_out(&serde_json::json!({ "errors": errors, "lost": lost }), out)
  })
}
