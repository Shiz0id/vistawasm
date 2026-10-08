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

Every editor operation that assumes Selotape's own generator has a Vista
counterpart in `SelotapeVista.h`. The snippets use CMapEditor's own
declarations (`FinishTerrainMap`, `TerrainWholeOp`, `NewPropText`,
`UniqueName`). Field names like `m_vistaJob` are suggestions. Your
`CMapEditor_terrain.cpp` was not uploaded, so match the surrounding code.

### 1. New terrain map: generate off the UI thread

`Job_c` runs `Generate` on a worker, with progress and Cancel for the
dialog:

```cpp
// CMapEditor members.
SelotapeVista::Job_c      m_vistaJob;
SelotapeVista::Settings_s m_vistaSettings;
std::string               m_vistaMapName;

// The dialog's Create button.
m_vistaSettings.sizeMetres = float(m_newTerrainSize) * 1024.0f;
m_vistaSettings.spacing = float(m_newTerrainSpacing);
m_vistaJob.Start(m_vistaSettings, materials, SelotapeVista::LayerMapFor(PaintableLayers()));

// Each frame while the dialog is open.
DrawProgressBar(m_vistaJob.Phase(), m_vistaJob.Progress());   // your UI
if (cancelPressed) m_vistaJob.Cancel();

if (m_vistaJob.Done()) {
  SelotapeVista::Result_s result;
  std::string error;
  bool cancelled = false;

  if (m_vistaJob.Take(&result, &error, &cancelled)) {
    FinishTerrainMap(m_vistaMapName, &result.terrain, materials, setupPath, terrainPath, "");
    PlaceVistaTrees(result.trees);   // step 6
  } else if (!cancelled) {
    m_status = "Vista: " + error;
  }
}
```

`Progress()` is weighted by how long each stage really takes, so the bar
moves evenly. A cancel lands within about half a second, and the
previous terrain is untouched. Destroying a running job cancels it and
waits.

### 2. Saving and loading the settings

Write `SelotapeVista::MetaLines(settings)` into the `.ter`'s meta block
in `SelotapeTerrain::Serialise`, after the existing lines. On load,
`IsVistaMeta(meta)` says the terrain came from Vista, and
`ParseMetaLines(meta, &settings, &error)` reads the settings back.
`Parse` must keep meta lines it does not recognise. If it drops them,
keep the raw meta text on `Terrain_s` beside `gen`.

### 3. Regenerate

For a Vista terrain, `TerrainRegenerate` calls `RegenerateInPlace`
instead of Selotape's generator. Size, origin, materials, haze and stock
fields stay; heights and splat are replaced. Run it through the job (or
`TerrainWholeOp` for small maps) so it is one undo step:

```cpp
TerrainWholeOp("Regenerate (Vista)", SelotapeTerrain::Rect_s::All(*m_terrain), [&](SelotapeTerrain::Terrain_s& t) {
  std::string error;
  SelotapeVista::Result_s rest;

  if (!SelotapeVista::RegenerateInPlace(&t, settings, layers, &error, {}, nullptr, &rest)) {
    m_status = "Vista: " + error;
  }
});
```

### 4. Auto-splat

Vista terrains have every Selotape rule disabled, so `TerrainAutoSplat`
would paint nothing useful. For a Vista terrain, call `AutoSplatVista`.
It hands the current heights, sculpting included, back to Vista, which
classifies the ground with the saved landform and climate:

```cpp
TerrainWholeOp("Auto-splat (Vista)", region, [&](SelotapeTerrain::Terrain_s& t) {
  std::string error;

  if (!SelotapeVista::AutoSplatVista(&t, settings, SelotapeVista::LayerMapFor(PaintableLayers()), region, &error)) {
    m_status = "Vista: " + error;
  }
});
```

On ground nobody sculpted it agrees with `Generate`'s painting on about
nine samples in ten (88% on the test's 1 km mesa desert). The rest differs
where Vista's rivers would carve their beds again.

### 5. The apron

`BuildApronMesh` continues the ground past the edge with
`GeneratedHeight(gen)`, Selotape's own noise. Call
`PrepareApron(&terrain, 64.0f)` once after generating. It sets `gen` to a
flat plain at the edge's average height and fades the outer 64 m down to
it, so the two meet. It relies on `GeneratedHeight` giving `baseHeight`
when `amplitude` is 0, which is how the header describes the generator.
Check this against `SelotapeTerrain.cpp`.

### 6. Trees as props

`TreePlacements` maps each Vista species to a palette entry, thins the
forest evenly to `maxCount`, keeps `minSpacing`, and presses trunks
`sink` metres into the ground. Every result is ready for `NewPropText`:

```cpp
void CMapEditor::PlaceVistaTrees(const std::vector<SelotapeVista::Tree_s>& trees) {
  SelotapeVista::SpeciesMap_s species;
  species.species[0] = { "staticprop", "haze:foliage/oak_a", 1.0f };    // your models
  species.species[1] = { "staticprop", "haze:foliage/pine_a", 1.0f };
  // ... spruce, palm, jungle, cypress, acacia, shrub; an empty templateName skips a species.

  SelotapeVista::TreeFilter_s filter;
  filter.maxCount = 2000;
  filter.minSpacing = 4.0f;
  std::set<std::string> taken = /* the setup's declaration names */;

  for (const SelotapeVista::Placement_s& p : SelotapeVista::TreePlacements(trees, species, filter)) {
    const std::string name = UniqueName("vistaTree", taken);
    taken.insert(name);
    const vec3_u pos(p.pos[0], p.pos[1], p.pos[2]);
    const vec3_u rot(p.rotDeg[0], p.rotDeg[1], p.rotDeg[2]);
    const std::string text = NewPropText(p.templateName, name, pos, rot, bg, p.model);
    // Append `text` as TerrainScatter appends its props: one undo step for the lot.
  }
}
```

Two things to check against the engine: whether `rotDeg.y` turns the same
way as Vista's yaw (flip its sign if trees face the wrong way), and
whether the prop template has a scale field for `p.scale`.

### 7. Water

Selotape draws no water on a `.ter` yet. `waterDepth` and `biome` are one
value per terrain sample. `waterVertices` and `waterIndices` are in the
layout `ports/d3d11/hlsl/water/*.INLAND-1.hlsl` reads, so the D3D11 port
draws them as they are (docs/porting-d3d11.md).

## Timings

One thread of a cloud machine, erosion at `"balanced"`. An editor PC will
likely be faster.

| Terrain | Vista grid | Time |
| --- | --- | --- |
| 512 m at 2 m | 256 | 0.4 s |
| 1 km at 2 m | 512 | 2.4 s |
| 1 km at 1 m | 1024 | 11 s |
| 4 km at 1 m | 2048 | about 45 s |

Erosion is two-thirds of the time. For a quick preview, set
`vistaSamples = 256` and `erosionQuality = "preview"`, then generate at
full size once the author is happy. The same seed gives the same land at
any grid size, at different detail.

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
- that the heights match Vista's own resampling, to 0.08 mm;
- `Job_c`: progress only rises, a cancel during erosion lands within a
  second, the job starts again afterwards, and destroying a running job
  is safe;
- `RegenerateInPlace` replaces only the ground, and refuses a terrain that
  is not square without touching it;
- `AutoSplatVista` turns a sculpted peak to rock or snow, leaves samples
  outside its region alone, and agrees with `Generate` on unsculpted
  ground;
- `PrepareApron` sets the rim to the apron's height and leaves the middle;
- `TreePlacements` keeps to `maxCount`, `minSpacing` and its region, covers
  the whole map when thinned, and places the same props every time;
- how long each stage takes, which the progress weights come from.

Add `--large` to include a 4 km map at 1 m. Without Selotape's include
directory, it builds against `test/standin`, which declares only what
`SelotapeVista` uses. `PackLayerWeights` and `FadeEdges` are stand-ins
written from their declarations' comments, so build against Selotape's own
to test the real ones.
