# VistaD3D11: VistaWASM's renderer on a Direct3D 11 device

`VistaD3D11.h` and `VistaD3D11.cpp` draw a VistaWASM engine's scene on a
host's own `ID3D11Device`, into the host's own texture. Add the two files
to the host's build, link `vista_native` (see
[crates/vista_native](../../../crates/vista_native)), and include
`crates/vista_native/include`.

The engine draws with the browser build's own renderer: the same passes,
shaders, streaming and level of detail. Natively, `vista_native` records
that work and lowers it to Direct3D 11 calls (`vista_d3d11.h`).
`Executor_c` runs them. Nothing about the scene is reimplemented in C++,
so the picture follows the browser build as VistaWASM changes.

## Use

```cpp
#include "VistaD3D11.h"

VistaEngine* engine = nullptr;
vista_engine_create(nullptr, &engine);

// Draw into a 1280 x 720 B8G8R8A8_UNORM texture with a render target
// bind flag, such as a swap chain's back buffer.
VistaD3D11::Renderer_c renderer(engine, device, context, 1280, 720, VISTA_OUTPUT_BGRA8);

vista_engine_generate_fractal(engine, R"({ "seed": 7, "size": 1024 })", nullptr, nullptr);
vista_engine_set(engine, "camera", R"({ "position": [0, 400, 900], "target": [0, 300, 0] })");

// Each frame, on the thread that owns the immediate context:
std::string error;

if (!renderer.Frame(nowMilliseconds, backBuffer, &error)) {
  log(error);
}

// The context is left cleared: set your own state again before drawing.
```

- `Frame()` runs everything recorded since the last frame (a new terrain,
  changed options), then the frame. Call `Flush()` after a long change,
  such as generating a terrain, to run its uploads before the next frame.
- `Resize()` when the target changes size; the next frame's target must be
  the new size.
- Statistics and errors: `vista_engine_stats_json()` and
  `vista_engine_events_json()`. Errors running a stream are reported there
  too.
- Destroy the `Renderer_c` before the engine. It detaches the renderer
  and releases every object it made.

The output format decides the gamma: `VISTA_OUTPUT_RGBA8` and
`VISTA_OUTPUT_BGRA8` targets get gamma-encoded colour from the renderer;
the `_SRGB` formats encode it themselves. The frame is tone mapped either
way.

## Requirements

- Feature level 11.0. The streams use compute shaders, raw buffers,
  indirect draws and typed UAV stores of `R8G8B8A8_UNORM`, which every
  11.0 device has.
- The immediate context, on one thread. `Run()` and `Frame()` leave it
  cleared with `ClearState()`.
- Memory: the streams create what the browser build does. A 4 km map at
  2 m with trees, grass and boulders is a few hundred megabytes.

## Running streams yourself

`Executor_c` runs any stream from `vista_renderer_frame()` or
`vista_renderer_commands()`. The record format is in
`crates/vista_native/include/vista_d3d11.h`; every enum value is Direct3D
11's own. Run every stream, in order. Hand read-backs back with
`vista_renderer_complete_read()`: `Renderer_c` shows how.

## Testing

`test/run.sh OUT.png` builds the executor and `test/render_test.cpp` with
MinGW, links `vista_native` built for Windows, and draws the browser
visual check's default scene under Wine. Compare the PNG with
`scripts/visual-check/capture.mjs`'s capture of the same scene. Licence:
AGPL-3.0-only, as VistaWASM.
