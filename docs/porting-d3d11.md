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
| Render orchestration: resources, passes, frame order | `render/gpu.rs` and friends, about 8,500 lines of Rust over wgpu | Rewritten by hand in C++ against Direct3D 11. | To do |
| Host glue: camera, input, frame loop | `js/src`, small | Use the engine's own. | To do |

The split keeps the hard part (world generation) identical to the browser
build. The same seed and options make the same map in both, and fixes made
upstream arrive by rebuilding the library.

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
- **Live edits:** `vista_engine_set()` replaces the biome, flora, grass,
  water or surface options and rebuilds what depends on them.

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

## 3. The renderer: what is left

`crates/vista_wasm/src/render/gpu.rs` is the reference. It creates the
resources, records the passes and holds the frame order. Port it to C++
pass by pass. Each pass has its WGSL entry point, so it maps directly to
one of the HLSL files above.

A frame runs these passes in this order (from `PipelineKind::ALL` in
`render/pipelines.rs`). The scene decides which ones exist (`Needs`).

| Pass | Kind | HLSL |
| --- | --- | --- |
| Terrain shadow bake | compute | `terrain_shadow/bake` |
| Wet ground, puddles, snow | compute | `surface_weather/step_main` |
| Grounding trees and grass | compute | `grounding/ground_main` |
| Tree tiles | compute | `tree_generate/generate_main` |
| Tree culling and LOD | compute | `tree_cull/cull_main` |
| Grass tiles, then culling | compute | `grass_generate/generate_main`, `cull_main` |
| Boulder tiles, then culling | compute | `boulder_generate/generate_main`, `cull_main` |
| Tree shadow map | render | `trees/vertex_shadow`, `fragment_shadow` |
| Boulder shadows | render | `boulders/shadow_main`, `shadow_fragment` |
| Terrain | render | `clipmap_render/vertex_main`, `fragment_main` |
| Distant canopy | render | `clipmap_render/canopy_vertex_main`, `canopy_fragment_main` |
| Bank strips | render | `clipmap_render/vertex_bank`, `fragment_bank` |
| Tree meshes (near, then light) | render | `trees/vertex_mesh`, `fragment_mesh`, then the `LIGHT-1` variants |
| Tree impostors | render | `trees/vertex_impostor`, `fragment_impostor` |
| Grass | render | `grass_instances/vertex_main`, `fragment_main` |
| Boulders | render | `boulders/vertex_main`, `fragment_main` |
| Clouds (quarter, then full) | render | `atmosphere/cloud_quarter_main`, `cloud_main` |
| Sky, fog and tone mapping | render | `atmosphere/fragment_main` |
| Scene copy for reflections | render | `atmosphere/scene_copy_main` |
| Ocean (with or without sea ice) | render | `water/vertex_main`, `fragment_main` (and `SEA_ICE-0`) |
| Rivers, lakes and pools | render | `water/*.INLAND-1` |
| Waterfalls | render | `water/fragment_fall` |
| Present and lens drops | render | `atmosphere/present_main` |

The full-screen passes use `atmosphere/vertex_main`.

Passes that run once or when something changes:

- **Procedural textures** (`texture_gen/*`, then `mipgen/*`). These bake
  the terrain, flora and water textures and the cloud noise at start-up.
  See `render/textures.rs`.
- **Impostor baking** (`trees/vertex_bake`, `fragment_bake`). This renders
  each tree species into its impostor atlas.
- **GPU erosion** (`hydraulic_erosion/*`, `thermal_erosion/*`). This is
  optional. `vista_native` erodes on the CPU, and the result is the same
  model. Port it only if CPU erosion is too slow for your largest maps.
  See `render/erosion_compute.rs`.

Direct3D 11 tracks hazards between passes itself. Unbind a resource as a
UAV before binding it as a shader resource, or the runtime unbinds it and
warns.

### Suggested order

1. Draw the heightfield from `VISTA_MAP_HEIGHT` with your own simple
    shader, to check scale, coordinates and orientation.
2. Bake the procedural textures and draw the terrain with
    `clipmap_render`. The terrain's mesh and streaming are in
    `render/terrain_mesh.rs` and `terrain/clipmap.rs`, both plain Rust.
3. Add the sky, fog and tone mapping (`atmosphere`).
4. Add the water from `vista_engine_water_mesh()`.
5. Add the trees, grass and boulders: the compute generators and culling,
    then their draws.
6. Add the clouds, shadows and weather.

## Keeping in step with upstream

- After pulling VistaWASM, rebuild `vista_native` and run
  `cargo run -p vista_hlsl`. Then diff `ports/d3d11/hlsl/manifest.json`:
  any changed binding shows where the C++ side must change too.
- `cargo test -p vista_native` checks the C API.
- `crates/vista_hlsl/src/main.rs` composes the shader modules the same way
  as `render/shaders.rs`. If upstream adds a shader or changes how one is
  assembled, update `MODULES` there.
