# Porting VistaWASM to a native Direct3D 11 engine

This guide is for engines that want VistaWASM's worlds without a browser.
It covers what is already ported, what is left, and the traps between
WebGPU and Direct3D 11.

VistaWASM is AGPL-3.0-only. A port, and the engine it is linked into, is
covered by the same licence. Keep the `LICENSE`, `NOTICE` and the
licence lines in generated files.

## The plan in one table

| Part | Size | How it is ported | State |
| --- | --- | --- | --- |
| World generation: terrain, erosion, rivers, lakes, glaciers, biomes, materials, tree and grass placement | about 22,000 lines of Rust | Not rewritten. `crates/vista_native` builds it as a C library. | Done |
| Shaders | 23 WGSL files, 10,000 lines | Translated to Shader Model 5.0 HLSL by `crates/vista_hlsl`, and compiled with Microsoft's compiler. | Done: 74 of 74 compile |
| Render orchestration: resources, passes, frame order | `render/gpu.rs` and friends, about 8,500 lines of Rust over wgpu | Not rewritten. Natively the same code records its work, `vista_native` lowers it to Direct3D 11 calls, and `ports/d3d11/executor` (C++) runs them on the host's device. | Done: matches the browser's frames |
| Host glue: camera, input, frame loop | `js/src`, small | Use the engine's own. | The host's |

The split keeps both hard parts, world generation and rendering, the
browser build's own code. The same seed and options make the same map in
both, bit for bit, and fixes made upstream arrive by rebuilding the
library.

## 1. The C library: `crates/vista_native`

Build it:

```sh
cargo build -p vista_native --release
```

This produces `target/release/vista_native.lib` and `vista_native.dll` on
Windows (`libvista_native.a` and `.so` elsewhere). Include
`crates/vista_native/include/vista_native.h`. With the static library, also
link the system libraries that
`cargo rustc -p vista_native --release --crate-type staticlib -- --print native-static-libs`
prints. On Windows these are usually `ws2_32`, `userenv`, `ntdll`,
`bcrypt` and `advapi32`.

Build for the engine's architecture and C runtime. For a 32-bit engine,
add the target first: `rustup target add i686-pc-windows-msvc`, then pass
`--target i686-pc-windows-msvc`.

### A minimal host

```cpp
#include "vista_native.h"

VistaEngine *engine = nullptr;

if (vista_engine_create(nullptr, &engine) != VISTA_OK) {
  log_error(vista_last_error());
  return;
}

const char *options = R"({ "seed": 42, "size": 1024, "erosion": {} })";

if (vista_engine_generate_fractal(engine, options, nullptr, nullptr) != VISTA_OK) {
  log_error(vista_last_error());
}

VistaMap *heights = nullptr;

if (vista_engine_export_map(engine, VISTA_MAP_HEIGHT, 0, 0, &heights) == VISTA_OK) {
  // heights->data: width * height floats, row-major, metres.
  upload_heightfield(heights->width, heights->height, static_cast<const float *>(heights->data));
  vista_map_free(heights);
}

vista_engine_destroy(engine);
```

`crates/vista_native/examples/smoke.cpp` is a complete example. Run it with
`crates/vista_native/examples/run-smoke.sh` (Linux or macOS, any C++17
compiler).

### What it gives you

- **Options as JSON**, in the shape of the JavaScript API
  ([options reference](options-reference.md)). Keys you leave out take the
  defaults, at every depth. Unknown keys are errors, with the list of valid
  keys in the message.
- **Generation:** `vista_engine_generate_fractal()`, with an optional
  progress callback that can cancel (return non-zero; the call returns
  `VISTA_CANCELLED` and the previous terrain stays). `vista_engine_load_raw_heightmap()` and
  `vista_engine_load_geotiff()` load real elevation data instead.
- **Fifteen maps** through `vista_engine_export_map()`: heights, biomes,
  water and its depth, drainage, discharge, twelve material weights, slope,
  normals, occlusion, temperature, moisture, and tree and grass density.
  Each comes with JSON that gives its units and legend. Pass a width and
  height to resample.
- **Trees** through `vista_engine_export_trees()`: position on the ground,
  species, variant, scale, rotation, tint and dryness.
- **Water geometry** through `vista_engine_water_mesh()`: river ribbons,
  lakes and pools; waterfall sheets and mist; and bank strips. The vertex
  layouts are `VistaWaterVertex` (56 bytes) and `VistaBankVertex` (44
  bytes), which are what `water.wgsl` reads, so the buffers can be uploaded
  as they are.
- **Live edits:** `vista_engine_set()` replaces any option group (biomes,
  flora, grass, water, surface, camera, sun, atmosphere, clouds, mist,
  quality, weather, shadows, time of day, debug view) and rebuilds what
  depends on it.
- **Rendering:** `vista_renderer_attach()` and `vista_renderer_frame()`.
  See section 3.

### Rules

- Generation runs on the calling thread and erosion runs on the CPU. A
  1024 by 1024 map with erosion takes seconds, not milliseconds, so run it
  on a worker thread in an editor.
- An engine is not thread-safe. Use one engine per thread, or lock.
- Everything the library returns is freed with its `vista_*_free()`
  function, never with `free` or `delete`.
- A failed call changes nothing: the previous terrain stays.
- World space is metres, y up, with the map's centre at the origin.

## 2. The shaders: `crates/vista_hlsl`

```sh
cargo run -p vista_hlsl
```

This rewrites `ports/d3d11/hlsl`. Do not edit those files by hand. Change
the WGSL in `crates/vista_wasm/src/shaders` and run the tool again.

- **One file per entry point**, such as `trees/vertex_mesh.hlsl`. Each
  file's header gives the `fxc` profile and entry point and lists its
  resources.
- **Registers are numbered per stage and per class** (`b`, `t`, `s`, `u`)
  from 0, over only the resources that entry point uses. This keeps
  compute shaders within the 8 UAVs of feature level 11.0.
  `manifest.json` maps each WGSL `@group/@binding` to its register for
  every file.
- **`override` constants become variants.** `trees` has a `LIGHT-1`
  variant for the lighter far meshes. `water` has `INLAND-1` (rivers and
  lakes) and `SEA_ICE-0` (open ocean) variants. A file without a suffix
  uses the defaults.
- **Samplers are plain `register(sN)` declarations.** Naga writes a
  Direct3D 12 sampler heap; the tool rewrites it for Direct3D 11.
- **Compiled and checked.** All 74 files compile with Microsoft's own
  HLSL compiler (`D3DCompiler_43.dll`, the DirectX SDK's, as `fxc /O3`).
  The compiled bytecode is committed in `ports/d3d11/cso/<module>/<entry>.cso`,
  so an engine loads it with `CreateVertexShader` and its kin and never
  compiles at run time. That matters: the texture bake's two shaders take
  3 minutes each to compile.
- **`ports/d3d11/tools/check-hlsl.sh`** compiles every file on Linux,
  through Wine, and rewrites `ports/d3d11/cso`. Run it after
  `cargo run -p vista_hlsl`. On Windows, `hlsl/compile-fxc.ps1` does the
  same with the SDK's `fxc`.
- **What the translation adjusts for fxc**, all in `vista_hlsl`: no
  64-bit loop guards (naga's, for Direct3D 12), `[allow_uav_condition]` on
  compute loops whose exit reads a UAV, and `[loop]` throughout the
  one-off texture bake so fxc does not spend 15 minutes unrolling it. One
  WGSL line changed for fxc: `leaf_cluster` in `texture_gen.wgsl` reads its
  loop count through a uniform that is always 0, because fxc folded the
  count to a constant and then failed to unroll the loop.
- **Warnings that remain** are WGSL's own arithmetic, which fxc flags but
  compiles as written: integer division, `pow` of a possibly negative
  base, negating an unsigned value, and a few gradients in branches.

### Buffers and layouts

The renderer's lowering (section 3) does all of this. It matters only to
an engine that draws with these shaders itself.

- WGSL storage buffers become `ByteAddressBuffer` (read-only) or
  `RWByteAddressBuffer`. Create them with
  `D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS`, and their views with
  `DXGI_FORMAT_R32_TYPELESS` and the raw flag
  (`D3D11_BUFFEREX_SRV_FLAG_RAW`, `D3D11_BUFFER_UAV_FLAG_RAW`).
- Buffers that hold indirect draw arguments also need
  `D3D11_RESOURCE_MISC_DRAWINDIRECT_ARGS`. The compute passes write the
  arguments, and `DrawInstancedIndirect` or `DrawIndexedInstancedIndirect`
  reads them. The argument layouts are the same as WebGPU's.
- Uniform buffers become `cbuffer`s. Naga pads the HLSL structs so their
  byte layout matches WGSL's, so the C++ side can fill them with the same
  bytes as the Rust structs in `render/gpu.rs`. Round each buffer's size up
  to a multiple of 16 bytes.
- Storage textures become `RWTexture2D`, `RWTexture2DArray` or
  `RWTexture3D`. VistaWASM only writes them (`rgba8unorm`), which every
  feature level 11.0 device supports. The tool flags any shader that
  starts reading one.

### Known differences, flagged in file headers

- `SV_VertexID` and `SV_InstanceID` do not include the base vertex or the
  start instance in Direct3D 11. WebGPU's do. Eight vertex shaders read one
  of them: the full-screen passes, grass, tree impostors and shadows, and
  impostor baking, which draws with start instances up to 127. Those
  shaders add the offsets from a constant buffer, as wgpu's own Direct3D
  12 backend does. The manifest names its register (`specialConstants`).
  Before each draw, fill it with `{ int first_vertex; int first_instance;
  uint other; }`: the draw's first vertex (its base vertex when indexed)
  and first instance. Indirect draws take 0 and 0; VistaWASM's indirect
  arguments start at 0 for these shaders.
- Clip space and texture coordinates already match. Both APIs use depth
  from 0 to 1, y up in clip space, and a top-left origin for textures and
  render targets.
- A pixel shader that writes UAVs shares the 8 output slots with render
  targets on feature level 11.0. None does today. The tool would flag one.

## 3. The renderer: `vista_native` and `ports/d3d11/executor`

The browser build's renderer (`render/gpu.rs`, with `gpu/weather.rs` and
`textures.rs`) also runs natively. It does not draw there. It records:

- **`render/recorder.rs`** is the part of wgpu the renderer uses, as
  recording. `gpu.rs` compiles against it natively, unchanged, so every
  pass, every streaming decision and every level of detail is the
  browser's.
- **`crates/vista_native/src/d3d11.rs`** lowers the recording to Direct3D
  11 calls, one record each. The record format is
  `crates/vista_native/include/vista_d3d11.h`. Every enum value is Direct3D
  11's own. The lowering takes care of the differences:
  - registers from `manifest.json`, and views made for how each resource
    is bound;
  - constant buffers written whole, as Direct3D 11.0 requires;
  - the draw offsets `SV_VertexID` and `SV_InstanceID` leave out;
  - typeless depth textures, with depth and float views;
  - a 2D copy of one layer of a layered texture, for shaders that read it
    as a 2D texture;
  - a copy of what a dispatch reads when it writes another mip of the same
    texture;
  - every slot unbound before it changes, so nothing is an input and an
    output at once.
  The compiled shaders are embedded, so a host ships no shader files.
- **`ports/d3d11/executor`** (`VistaD3D11.h`, `VistaD3D11.cpp`) runs the
  records on the host's `ID3D11Device` and immediate context, into the
  host's texture. `Renderer_c` drives an engine's renderer with one call a
  frame. Its [README](../ports/d3d11/executor/README.md) shows the code.

```cpp
VistaD3D11::Renderer_c renderer(engine, device, context, 1280, 720, VISTA_OUTPUT_BGRA8);
vista_engine_generate_fractal(engine, options, nullptr, nullptr);
vista_engine_set(engine, "camera", R"({ "position": [0, 400, 900], "target": [0, 300, 0] })");

// Each frame:
renderer.Frame(nowMilliseconds, backBuffer, &error);
```

- **Output.** The renderer draws the finished frame, tone mapped, into a
  texture of the host's: sky, terrain, water, trees, grass, boulders,
  clouds, weather and lens drops. Pick its format with
  `VistaOutputFormat`.
- **Drawing over it.** `vista_renderer_frame_info()` gives the scene's
  depth texture and the camera's matrices, so the host can draw its own
  geometry into the frame, depth-tested against the world. See the
  executor's README.
- **Requirements.** Feature level 11.0. The streams use compute shaders,
  raw buffers, indirect draws and typed UAV stores of `R8G8B8A8_UNORM`,
  which every 11.0 device has.

### How it was checked

- `crates/vista_native/tests/d3d11_stream.rs` replays a real scene's
  stream against a mock executor that enforces Direct3D 11's rules: every
  object exists before use, views fit their resources' bind flags,
  constant buffers are written whole, and no resource is an input and an
  output at once.
- `ports/d3d11/executor/test/run.sh` builds the executor with MinGW and
  draws a scene under Wine. On DXVK over Mesa's lavapipe, the frames match
  the browser build's frames of the same scenes:

  | Scene | Mean difference (0 to 255) | Pixels over 24 |
  | --- | --- | --- |
  | Generated default scene | 0.68 | 0.3% |
  | Fixed heightmap | 0.76 | 0.5% |
  | Ground-level river, grass and trees | 2.07 | 1.0% (swaying foliage) |
  | Rain | 0.25 | none |
  | Sea ice at -18 °C | 0.55 | 0.1% |

  Wine's own Direct3D 11 (wined3d) turns a few pixels near terrain
  triangle edges black on llvmpipe, which DXVK does not: a fault in its
  shader translation, not in the port.

## Keeping in step with upstream

- After pulling VistaWASM, rebuild `vista_native`. The renderer follows
  upstream by itself: nothing in the executor names a pass or a shader.
- After a shader change, run `cargo run -p vista_hlsl`, then
  `ports/d3d11/tools/check-hlsl.sh` to compile it (Wine, or
  `compile-fxc.ps1` on Windows). `cargo test -p vista_hlsl` fails while
  the translation is stale, and `cargo test -p vista_native` while the
  compiled shaders are.
- `cargo test -p vista_native` checks the C API and the stream.
- `crates/vista_hlsl/src/main.rs` composes the shader modules the same way
  as `render/shaders.rs`. If upstream adds a shader or changes how one is
  assembled, update `MODULES` there.
