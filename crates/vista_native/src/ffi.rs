//! The checked helpers every `unsafe` read of a host pointer goes through,
//! the thread's last error, and the status codes.

use std::cell::RefCell;
use std::ffi::{c_char, CStr, CString};
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::task::{Context, Poll, Waker};

use vista_wasm::VistaError;

/// What a call returns. Every status but `Ok` sets the thread's last error.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VistaStatus {
  /// The call succeeded.
  Ok = 0,
  /// A pointer was null, a string was not UTF-8, or a value was out of range.
  InvalidArgument = 1,
  /// The options JSON did not parse or failed validation.
  Options = 2,
  /// The engine refused or failed the request.
  Engine = 3,
  /// The engine panicked. Destroy it: its state is unknown.
  Panic = 4,
}

/// A failed call: its status and the message `vista_last_error()` returns.
pub struct Failure {
  pub status: VistaStatus,
  pub message: String,
}

impl Failure {
  pub fn argument(message: impl Into<String>) -> Self {
    Self {
      status: VistaStatus::InvalidArgument,
      message: message.into(),
    }
  }

  pub fn options(message: impl Into<String>) -> Self {
    Self {
      status: VistaStatus::Options,
      message: message.into(),
    }
  }
}

impl From<VistaError> for Failure {
  fn from(error: VistaError) -> Self {
    let status = match error {
      VistaError::OptionsInvalid(_) => VistaStatus::Options,
      _ => VistaStatus::Engine,
    };

    Self {
      status,
      message: error.to_string(),
    }
  }
}

pub type Outcome<T = ()> = Result<T, Failure>;

thread_local! {
  static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_last_error(message: &str) {
  // An interior NUL would cut the message short; a space reads the same.
  let message = CString::new(message.replace('\0', " ")).unwrap_or_default();
  LAST_ERROR.with(|last| *last.borrow_mut() = message);
}

/// The thread's last error message, kept until the next failed call on
/// this thread.
pub fn last_error() -> *const c_char {
  LAST_ERROR.with(|last| last.borrow().as_ptr())
}

/// Run one API call: catch a panic, record a failure's message, and
/// return its status.
pub fn call(body: impl FnOnce() -> Outcome) -> VistaStatus {
  match catch_unwind(AssertUnwindSafe(body)) {
    Ok(Ok(())) => VistaStatus::Ok,
    Ok(Err(failure)) => {
      set_last_error(&failure.message);
      failure.status
    }
    Err(_) => {
      set_last_error("VistaWASM panicked. Destroy this engine and create a new one.");
      VistaStatus::Panic
    }
  }
}

/// Drive `future` to completion on this thread. Native engine futures
/// never wait on anything (there is no GPU to wait for), so the first
/// poll finishes them; the loop is only a guard.
pub fn block_on<F: Future>(future: F) -> F::Output {
  let mut future = std::pin::pin!(future);
  let mut context = Context::from_waker(Waker::noop());

  loop {
    if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
      return output;
    }

    std::thread::yield_now();
  }
}

/// A host string argument as UTF-8.
///
/// # Safety
///
/// `pointer` is null or points to a NUL-terminated string that stays
/// valid for the call.
pub unsafe fn str_arg<'a>(pointer: *const c_char, name: &str) -> Outcome<&'a str> {
  if pointer.is_null() {
    return Err(Failure::argument(format!("{name} must not be null.")));
  }

  // SAFETY: not null, and the caller promises a NUL-terminated string.
  let text = unsafe { CStr::from_ptr(pointer) };
  text
    .to_str()
    .map_err(|_| Failure::argument(format!("{name} must be UTF-8.")))
}

/// An optional host string argument: null means "not given".
///
/// # Safety
///
/// As [`str_arg`].
pub unsafe fn optional_str_arg<'a>(pointer: *const c_char, name: &str) -> Outcome<Option<&'a str>> {
  if pointer.is_null() {
    Ok(None)
  } else {
    // SAFETY: as the caller promises.
    unsafe { str_arg(pointer, name) }.map(Some)
  }
}

/// A host byte buffer.
///
/// # Safety
///
/// `pointer` is null or points to `length` readable bytes that stay
/// valid for the call.
pub unsafe fn bytes_arg<'a>(pointer: *const u8, length: usize, name: &str) -> Outcome<&'a [u8]> {
  if pointer.is_null() {
    return Err(Failure::argument(format!("{name} must not be null.")));
  }

  if length > isize::MAX as usize {
    return Err(Failure::argument(format!("{name} is too long.")));
  }

  // SAFETY: not null, within `isize::MAX`, and the caller promises
  // `length` readable bytes.
  Ok(unsafe { std::slice::from_raw_parts(pointer, length) })
}

/// Parse an optional JSON argument over `T`'s defaults, as the
/// JavaScript wrapper does: keys the host leaves out take the engine
/// core's defaults, at every depth, and null or `""` means all defaults.
/// Unknown keys are still errors.
pub fn parse_json<T>(json: Option<&str>, name: &str) -> Outcome<T>
where
  T: serde::de::DeserializeOwned + serde::Serialize + Default,
{
  let invalid =
    |error: serde_json::Error| Failure::options(format!("{name} is not valid: {error}."));
  let mut value = serde_json::to_value(T::default()).map_err(invalid)?;

  if let Some(text) = json.map(str::trim).filter(|text| !text.is_empty()) {
    let given: Value = serde_json::from_str(text).map_err(invalid)?;
    merge(&mut value, given);
  }

  serde_json::from_value(value).map_err(invalid)
}

use serde_json::Value;

/// The defaults of option groups that are absent (null) by default, so a
/// host that gives part of one gets the rest from its defaults.
fn group_default(key: &str) -> Option<Value> {
  use vista_types::*;

  fn of<T: serde::Serialize + Default>() -> Option<Value> {
    serde_json::to_value(T::default()).ok()
  }

  match key {
    "shape" => of::<TerrainShapeOptions>(),
    "erosion" => of::<ErosionOptions>(),
    "climate" => of::<WeatherClimate>(),
    "render" => of::<RenderSizeOptions>(),
    "camera" => of::<CameraOptions>(),
    "sun" => of::<SunOptions>(),
    "atmosphere" => of::<AtmosphereOptions>(),
    "water" => of::<WaterOptions>(),
    "flora" => of::<FloraOptions>(),
    "grass" => of::<GrassOptions>(),
    "clouds" => of::<CloudsOptions>(),
    "mist" => of::<MistOptions>(),
    "quality" => of::<RenderQualityOptions>(),
    "biomes" => of::<BiomeOptions>(),
    "weather" => of::<WeatherOptions>(),
    "shadows" => of::<ShadowOptions>(),
    "surface" => of::<SurfaceOptions>(),
    _ => None,
  }
}

/// Lay `given` over `defaults`: objects merge key by key, and anything
/// else replaces the default.
fn merge(defaults: &mut Value, given: Value) {
  match (defaults, given) {
    (Value::Object(defaults), Value::Object(given)) => {
      for (key, value) in given {
        match defaults.get_mut(&key) {
          Some(slot @ Value::Null) if value.is_object() => {
            *slot = group_default(&key).unwrap_or(Value::Null);
            merge(slot, value);
          }
          Some(slot) => merge(slot, value),
          None => {
            defaults.insert(key, value);
          }
        }
      }
    }
    (slot, given) => *slot = given,
  }
}

/// Write `value` through a host out-pointer.
///
/// # Safety
///
/// `pointer` is null or points to writable, aligned memory for a `T`.
pub unsafe fn write_out<T>(pointer: *mut T, value: T, name: &str) -> Outcome {
  if pointer.is_null() {
    return Err(Failure::argument(format!("{name} must not be null.")));
  }

  // SAFETY: not null, and the caller promises a writable `T`. `write`
  // does not drop the old value, which may be uninitialised.
  unsafe { pointer.write(value) };
  Ok(())
}

/// Check an out-pointer before any work, so a null one fails before
/// the engine changes.
pub fn require_out<T>(pointer: *mut T, name: &str) -> Outcome {
  if pointer.is_null() {
    Err(Failure::argument(format!("{name} must not be null.")))
  } else {
    Ok(())
  }
}

/// A host handle as a shared reference.
///
/// # Safety
///
/// `pointer` is null or came from this library and has not been freed.
pub unsafe fn ref_arg<'a, T>(pointer: *const T, name: &str) -> Outcome<&'a T> {
  // SAFETY: as the caller promises.
  unsafe { pointer.as_ref() }.ok_or_else(|| Failure::argument(format!("{name} must not be null.")))
}

/// A host handle as an exclusive reference.
///
/// # Safety
///
/// As [`ref_arg`], and no other call uses the handle at the same time.
pub unsafe fn mut_arg<'a, T>(pointer: *mut T, name: &str) -> Outcome<&'a mut T> {
  // SAFETY: as the caller promises.
  unsafe { pointer.as_mut() }.ok_or_else(|| Failure::argument(format!("{name} must not be null.")))
}
