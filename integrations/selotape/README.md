# VistaWASM in the Selotape map editor

`SelotapeVista.h` and `SelotapeVista.cpp` make a VistaWASM world into a
`SelotapeTerrain::Terrain_s`, the same terrain the editor's own generator
makes. Everything downstream takes it unchanged: collision, sculpting,
painting, undo, `.ter` save and load, and the light bake.

```cpp
SelotapeVista::Settings_s settings;
settings.seed = 42;
settings.sizeMetres = 1024.0f;
settings.spacing = 1.0f;
settings.landform = "alpine";

SelotapeVista::Result_s result;
std::string error;

if (!SelotapeVista::Generate(settings, "proc:desert", SelotapeVista::LayerMapFor(8), &result, &error)) {
  m_status = "Vista: " + error;
  return false;
}

// result.terrain is a complete Terrain_s.
```

Licence: AGPL-3.0-only, as VistaWASM.

## Building

1. Build the library: `cargo build -p vista_native --release` in this
    repository. On Windows this makes `target/release/vista_native.lib`.
2. Add `SelotapeVista.cpp` to the editor's sources, and these include
    directories: this folder and `crates/vista_native/include`.
3. Link `vista_native.lib`, plus `ws2_32`, `userenv`, `ntdll`, `bcrypt`
    and `advapi32`.

Rust links the dynamic C runtime (`/MD`). If Selotape builds with `/MT`,
build the library with the static one to match:

```sh
RUSTFLAGS="-C target-feature=+crt-static" cargo build -p vista_native --release
```

The files use 2-space indentation, as this repository requires. Reformat
them to Selotape's style when you copy them in.

## What comes back

| Field | What it is |
| --- | --- |
| `terrain.heights` | Vista's heights on the terrain's grid, resampled bicubically, corner to corner |
| `terrain.splat` | Vista's twelve ground materials folded into the splat layers by a `LayerMap_s` |
| `terrain.gen` | Only `seed`, `sizeMetres` and `spacing` are meaningful (see below) |
| `terrain.rules` | All disabled: Vista painted the ground |
| `trees` | Every tree: position on the ground, species, variant, scale and yaw |
| `waterDepth`, `biome` | One value per terrain sample |
| `waterVertices`, `waterIndices` | River ribbons, lakes and pools in Vista's water vertex layout |
| `metadataJson` | Vista's own report, warnings included |

### Layers

`FourLayerMap()` keeps the four roles every built-in material set uses:

| Layer | Role | Vista materials |
| --- | --- | --- |
| 0 | base ground | dry grass, tundra, sand, snow, ice |
| 1 | second ground | lush grass, forest floor |
| 2 | steep faces | rock, volcanic, scree |
| 3 | low, wet or worn | mud, gravel |

`EightLayerMap()` gives forest floor (4), sand (5), snow and ice (6) and
scree (7) their own layers. Use `LayerMapFor(PaintableLayers())`. Any
other mapping is a `LayerMap_s` you fill yourself; -1 drops a material.

### Climate

A landform shapes the land; the biomes set the climate. A desert needs
both:

```cpp
settings.landform = "mesaDesert";
settings.biomesJson = R"({ "temperatureBias": 0.8, "moistureBias": -0.8 })";
```

Without the biomes line, a mesa desert comes out 72% grass. With it, 93%
of it is dry ground.

## Wiring it into CMapEditor

These are the places that need changing. All of them are in your
`CMapEditor_terrain.cpp` and `SelotapeTerrain.cpp`, which were not
uploaded, so check each one there.

1. **New terrain map.** Add Vista as a generator choice in the dialog
    (`DrawNewTerrainMap` and `DrawNewTerrainModal`). Run `Generate` on a
    worker, as the BEAST bake does: it blocks. When it finishes, pass
    `&result.terrain` to `FinishTerrainMap`.
2. **Saving the settings.** Append `MetaLines(settings)` to the `.ter`'s
    meta block in `Serialise`. On load, `IsVistaMeta(meta)` says whether
    the terrain came from Vista and `ParseMetaLines` reads the settings
    back. Check that `Parse` keeps meta lines it does not know.
3. **Regenerate.** `TerrainRegenerate` calls Selotape's own generator from
    `Terrain_s::gen`. For a Vista terrain, call `Generate` again with the
    saved settings instead, and keep the old terrain as the undo step.
4. **Auto-splat.** Vista's terrains disable every rule. `TerrainAutoSplat`
    would replace Vista's splat with the rules' result. Either hide it for
    Vista terrains or have it run the `LayerMap_s` fold again.
5. **The apron.** `BuildApronMesh` continues the ground past the edge
    with `GeneratedHeight(gen)`, which is Selotape's noise, not Vista's. It
    will not match a Vista terrain's edge. Two ways round it:
    `FadeEdges(&terrain, width, height)` so the edge meets a plain, or
    `settings.edges = "coast"` so the land ends in sea.
6. **Trees.** `result.trees` has every tree Vista placed. Map each
    `VistaTreeSpecies` to a palette entry and place them as `TerrainScatter`
    places props, or as foliage. `y` is already on the ground. Check that
    `yawDegrees` turns the same way as a prop's `rot`.
7. **Water.** Selotape draws no water on a `.ter` yet. `waterDepth` and
    `biome` are ready for a water pass. `waterVertices` is in the layout
    that `ports/d3d11/hlsl/water/*.INLAND-1.hlsl` reads.

## Timings

These are one thread of a cloud machine, erosion at `"balanced"`. Your
editor PC will likely be faster.

| Terrain | Vista grid | Time |
| --- | --- | --- |
| 512 m at 2 m | 256 | 0.5 s |
| 1 km at 2 m | 512 | 3 s |
| 1 km at 1 m | 1024 | 13 s |
| 4 km at 1 m | 2048 | about 45 s |

For previews in the dialog, set `vistaSamples = 256` and
`erosionQuality = "preview"`. Then generate at full size once the author
is happy. The same seed gives the same land at any grid size, at
different detail.

## Testing

```sh
integrations/selotape/test/run.sh                 # against a stand-in SelotapeTerrain.h
integrations/selotape/test/run.sh path/to/include # against Selotape's own
```

The test generates the editor's sizes and checks:

- the grid's size, extent and centring;
- that every splat word sums to 255;
- that trees and water stay on the map;
- that the `.ter` meta block reads back what was written;
- that bad settings are refused with a reason;
- that the heights match Vista's own resampling, to 0.08 mm.

Add `--large` to include a 4 km map at 1 m. Without Selotape's include
directory, it builds against `test/standin`, which declares only what
`SelotapeVista` uses. `PackLayerWeights` is a stand-in written from its
declaration's comment, so build against Selotape's own to test the real
one.
