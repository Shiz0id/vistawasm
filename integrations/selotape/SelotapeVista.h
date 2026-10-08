// SelotapeVista.h: VistaWASM worlds as Selotape map editor terrains.
//
// Generates a world with vista_native (crates/vista_native) and lays it on
// a SelotapeTerrain::Terrain_s: the heights on the terrain's own grid, and
// Vista's twelve ground materials folded into the terrain's splat layers.
// The trees, the water depth and the river meshes come back beside it for
// the editor to place or draw.
//
// Licence: AGPL-3.0-only, as VistaWASM.
//
// What it does not do, so the editor routes around it:
//   - Regenerate, AutoSplat and the apron read Terrain_s::gen and rules,
//     which are Selotape's own generator's. A Vista terrain keeps its
//     settings in Settings_s (MetaLines / ParseMetaLines) and is
//     regenerated through Generate here.
//   - It blocks. A 1 km map takes about a second and a 4 km one tens of
//     seconds, so call it from a worker, as the BEAST bake runs.

#pragma once

#include "selotape/SelotapeTerrain.h"
#include "vista_native.h"

#include <cstdint>
#include <functional>
#include <string>
#include <vector>

namespace SelotapeVista {

// Vista's generator settings, as the editor's panel holds them. Every
// field is deterministic: the same settings make the same ground.
struct Settings_s {
  uint32_t seed = 1;
  float    sizeMetres = 1024.0f;  // square, centred on the world origin
  float    spacing = 2.0f;        // metres between the terrain's samples
  // "continental", "alpine", "rollingHills", "archipelago", "mesaDesert",
  // "fjords" or "volcanicIsland".
  std::string landform = "continental";
  // "open" runs the land to the edge, as a battlefield wants; "coast" rings
  // it with sea, which drops to several hundred metres deep.
  std::string edges = "open";
  float    verticalScale = 1.0f;
  float    seaLevelMetres = 0.0f;
  bool     erosion = true;
  // "preview", "balanced", "high" or "offline".
  std::string erosionQuality = "balanced";
  float    island = 0.0f;         // 0 none .. 1 a single island
  // Vista's own grid, a power of two from 16 to 2048. 0 picks the largest
  // that does not exceed the terrain's samples, so nothing is invented;
  // a 4 km map at 1 m is generated at 2048 and resampled up.
  uint32_t vistaSamples = 0;
  // Any further FractalTerrainOptions keys, as a JSON object, such as
  // { "noise": { "octaves": 8 } }. It must not repeat a key the fields
  // above set (seed, size, horizontalScaleMetres, verticalScale,
  // seaLevelMetres, landform, edges, erosion, shape). Empty for none.
  std::string extraJson;
  // Vista's biome and flora options as JSON (docs/options-reference.md),
  // such as { "temperatureCelsius": 4 }. Empty for the defaults.
  std::string biomesJson;
  std::string floraJson;
};

// Which splat layer (0-based) each Vista material lands on, in
// VistaMaterial order. -1 drops it, and its share goes to the others.
struct LayerMap_s {
  int8_t layerOf[12] = {};
};

// For a four-layer set, in the roles every built-in set keeps: 0 base
// ground, 1 a second ground, 2 the steep faces, 3 the low, wet or worn
// places. Snow and ice fall on the base ground: a four-layer set has no
// room for them.
LayerMap_s FourLayerMap();
// For an eight-layer set: the four roles, then forest floor, sand, snow
// and ice, and scree.
LayerMap_s EightLayerMap();
// FourLayerMap or EightLayerMap, for a set of `layers` layers.
LayerMap_s LayerMapFor(uint32_t layers);

// A tree Vista placed, standing on the ground.
struct Tree_s {
  float    x = 0.0f, y = 0.0f, z = 0.0f;   // world metres
  uint32_t species = 0;                    // VistaTreeSpecies
  uint32_t variant = 0;
  float    scale = 1.0f;
  float    yawDegrees = 0.0f;
};

struct Result_s {
  SelotapeTerrain::Terrain_s terrain;
  std::vector<Tree_s> trees;
  // Per terrain sample, as Terrain_s::heights: water depth in metres (0 dry)
  // and the VistaBiomeKind.
  std::vector<float>   waterDepth;
  std::vector<uint8_t> biome;
  // River ribbons, lakes and pools, in the layout water.wgsl reads (see
  // vista_native.h); world metres in this engine's frame.
  std::vector<VistaWaterVertex> waterVertices;
  std::vector<uint32_t>         waterIndices;
  // The terrain's metadata as Vista reports it, as JSON: height range,
  // sea level, generator version and any warnings.
  std::string metadataJson;
};

// `phase` names the stage ("tectonics", "erosion", "rivers", ...);
// `progress` runs 0..1 within it.
using Progress_t = std::function<void(const char* phase, float progress)>;

// The world for `settings`, laid on a terrain of `settings.sizeMetres` at
// `settings.spacing`, with `materials` stored as given and its splat from
// `layers`. False with the reason in *error; *out is then untouched.
bool Generate(const Settings_s& settings, const std::string& materials, const LayerMap_s& layers,
              Result_s* out, std::string* error, const Progress_t& progress = Progress_t());

// The FractalTerrainOptions JSON Generate passes Vista, for logs and tests.
std::string FractalJson(const Settings_s& settings, uint32_t vistaSamples);
// The Vista grid Generate uses for `settings`.
uint32_t VistaSamples(const Settings_s& settings);

// The settings as "vista.<field> = <value>" lines for the .ter's meta
// block, and back. Parse ignores lines it does not know; it is false only
// for a vista.* line whose value does not read.
std::string MetaLines(const Settings_s& settings);
bool        ParseMetaLines(const std::string& meta, Settings_s* out, std::string* error);
// Whether a meta block came from Generate here.
bool        IsVistaMeta(const std::string& meta);

}  // namespace SelotapeVista
