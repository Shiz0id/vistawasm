// SelotapeVista.h: VistaWASM worlds as Selotape map editor terrains.
//
// Generates a world with vista_native (crates/vista_native) and lays it on
// a SelotapeTerrain::Terrain_s: the heights on the terrain's own grid, and
// Vista's twelve ground materials folded into the terrain's splat layers.
// The trees, the water depth and the river meshes come back beside it for
// the editor to place or draw.
//
// The editor's own operations that assume Selotape's generator each have a
// Vista counterpart here: Job_c for generating off the UI thread,
// RegenerateInPlace for Regenerate, AutoSplatVista for AutoSplat,
// PrepareApron for the apron, and TreePlacements for scattering the trees
// as props. README.md says where each one plugs in.
//
// Licence: AGPL-3.0-only, as VistaWASM.

#pragma once

#include "selotape/SelotapeTerrain.h"
#include "vista_native.h"

#include <atomic>
#include <cstdint>
#include <functional>
#include <mutex>
#include <string>
#include <thread>
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
  // such as { "temperatureBias": 0.8, "moistureBias": -0.8 } for a hot,
  // dry climate. Empty for the defaults.
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
  float    yawDegrees = 0.0f;              // about +y, from Vista's frame
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
// `progress` runs 0..1 within it. Return false to cancel.
using Progress_t = std::function<bool(const char* phase, float progress)>;

// The world for `settings`, laid on a terrain of `settings.sizeMetres` at
// `settings.spacing`, with `materials` stored as given and its splat from
// `layers`. False with the reason in *error, and *out untouched; *cancelled
// (when given) says whether `progress` cancelled it.
//
// It blocks: 3 s for 1 km at 2 m, 13 s for 1 km at 1 m, about 45 s for 4 km
// at 1 m. Use Job_c from the editor.
bool Generate(const Settings_s& settings, const std::string& materials, const LayerMap_s& layers,
              Result_s* out, std::string* error, const Progress_t& progress = Progress_t(),
              bool* cancelled = nullptr);

// The FractalTerrainOptions JSON Generate passes Vista, for logs and tests.
std::string FractalJson(const Settings_s& settings, uint32_t vistaSamples);
// The Vista grid Generate uses for `settings`.
uint32_t VistaSamples(const Settings_s& settings);

// --- off the UI thread ------------------------------------------------------

// One Generate on a worker thread, for a dialog with a progress bar and a
// Cancel button. Every member may be called from the UI thread while it
// runs. Not copyable; destroying a running job cancels it and waits.
class Job_c {
public:
  Job_c() = default;
  Job_c(const Job_c&) = delete;
  Job_c& operator=(const Job_c&) = delete;
  ~Job_c();

  // Starts Generate on a worker. False (and nothing started) while an
  // earlier job runs or holds an untaken result.
  bool Start(const Settings_s& settings, const std::string& materials, const LayerMap_s& layers);
  // Asks the worker to stop at Vista's next report. Done() then turns true,
  // and Take() returns false with *cancelled set.
  void Cancel();
  bool Running() const;
  bool Done() const;
  // The stage now running, and the whole job's progress from 0 to 1
  // (weighted by how long each stage usually takes, so a bar moves evenly).
  std::string Phase() const;
  float Progress() const;
  // Once Done(): the result, or false with *error (and *cancelled). Joins
  // the worker, so the job can Start again.
  bool Take(Result_s* out, std::string* error, bool* cancelled = nullptr);

private:
  void Join();

  std::thread       m_worker;
  std::atomic<bool> m_running { false };
  std::atomic<bool> m_done { false };
  std::atomic<bool> m_cancel { false };
  mutable std::mutex m_lock;   // guards everything below
  std::string m_phase;
  float       m_progress = 0.0f;
  bool        m_ok = false;
  bool        m_cancelled = false;
  std::string m_error;
  Result_s    m_result;
};

// The whole generation's progress from 0 to 1, from a stage and the
// progress within it, by how long each stage usually takes.
float OverallProgress(const std::string& phase, float progress);

// --- the editor's operations, for a Vista terrain ------------------------------

// Regenerate: new heights and splat for `t` from `settings`, keeping its
// size, origin, materials and everything else on it. `settings.sizeMetres`
// and `spacing` are taken from `t`, which must be square. `rest`, when
// given, receives the new trees, water and biomes (its terrain is left
// empty). False with *error, and `t` unchanged. The editor records the old
// terrain for undo first, as for Regenerate.
bool RegenerateInPlace(SelotapeTerrain::Terrain_s* t, Settings_s settings, const LayerMap_s& layers,
                        std::string* error, const Progress_t& progress = Progress_t(),
                        bool* cancelled = nullptr, Result_s* rest = nullptr);

// AutoSplat: the splat of `region` (all of `t` when empty) from Vista's own
// ground classification of `t`'s heights as they now are, sculpting
// included, with `settings`' landform, climate and sea level. Painted
// weights inside the region are replaced, as AutoSplat replaces them.
// False with *error, and `t` unchanged.
bool AutoSplatVista(SelotapeTerrain::Terrain_s* t, const Settings_s& settings, const LayerMap_s& layers,
                    const SelotapeTerrain::Rect_s& region, std::string* error);

// The apron (BuildApronMesh) continues the ground past the edge with the
// terrain's own generator settings, which for a Vista terrain are not what
// made it. This makes them a flat plain at the height the edge averages,
// and fades the outer `fadeMetres` of the terrain down to it with
// SelotapeTerrain::FadeEdges, so the two meet. Returns that height.
// Needs GeneratedHeight to give baseHeight for amplitude 0 and shape
// noise, as Selotape's generator does.
float PrepareApron(SelotapeTerrain::Terrain_s* t, float fadeMetres);

// --- the trees as props -------------------------------------------------------

// The palette entry each tree species is placed as, in VistaTreeSpecies
// order (oak, pine, spruce, palm, jungle, cypress, acacia, shrub). An entry
// with an empty templateName is not placed.
struct SpeciesProp_s {
  std::string templateName;   // as Scatter_s::templateName
  std::string model;          // as Scatter_s::model; may be empty
  float       baseScale = 1.0f;   // the model's size for a tree of Vista scale 1
};
struct SpeciesMap_s {
  SpeciesProp_s species[8];
};

// One prop to place: what NewPropText takes.
struct Placement_s {
  std::string templateName;
  std::string model;
  float pos[3] = {};      // world metres, on the ground less `sink`
  float rotDeg[3] = {};   // (0, yaw, 0): check the yaw sign against PropTransform
  float scale = 1.0f;     // for a template with a scale field
  uint32_t species = 0;
};

// Which trees to place.
struct TreeFilter_s {
  uint32_t maxCount = 2000;     // props are heavier than foliage
  float    minSpacing = 0.0f;   // metres between trunks; 0 keeps Vista's own spacing
  float    sink = 0.2f;         // metres pressed into the ground, as Scatter_s::sink
  // World rectangle (min x, min z, max x, max z); all of it when min > max.
  float    region[4] = { 1.0f, 1.0f, 0.0f, 0.0f };
};

// The trees of `result` as props, mapped by `species`, thinned to
// `filter`. Deterministic: the same inputs place the same props. When
// there are more than maxCount, the ones kept are spread evenly over the
// map rather than the first in Vista's order.
std::vector<Placement_s> TreePlacements(const std::vector<Tree_s>& trees, const SpeciesMap_s& species,
                                        const TreeFilter_s& filter);

// --- the .ter's meta block ------------------------------------------------------

// The settings as "vista.<field> = <value>" lines for the .ter's meta
// block, and back. Parse ignores lines it does not know; it is false only
// for a vista.* line whose value does not read.
std::string MetaLines(const Settings_s& settings);
bool        ParseMetaLines(const std::string& meta, Settings_s* out, std::string* error);
// Whether a meta block came from Generate here.
bool        IsVistaMeta(const std::string& meta);

}  // namespace SelotapeVista
