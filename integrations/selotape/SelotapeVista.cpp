// SelotapeVista.cpp: see SelotapeVista.h.
//
// Licence: AGPL-3.0-only, as VistaWASM.

#include "SelotapeVista.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <sstream>
#include <unordered_map>

namespace SelotapeVista {

namespace {

constexpr uint32_t k_materials = 12;
// The largest grid Vista generates, exports or loads: the browser build's
// memory limit, kept by the native build too.
constexpr uint32_t k_maxVistaSamples = 2048;

struct EngineDeleter {
  void operator()(VistaEngine* engine) const { vista_engine_destroy(engine); }
};
struct MapDeleter {
  void operator()(VistaMap* map) const { vista_map_free(map); }
};
struct TreesDeleter {
  void operator()(VistaTrees* trees) const { vista_trees_free(trees); }
};
struct MeshDeleter {
  void operator()(VistaMesh* mesh) const { vista_mesh_free(mesh); }
};
struct StringDeleter {
  void operator()(char* text) const { vista_string_free(text); }
};

using EnginePtr = std::unique_ptr<VistaEngine, EngineDeleter>;
using MapPtr = std::unique_ptr<VistaMap, MapDeleter>;

// A failed vista_* call as an error message naming what was being done.
bool Fail(const char* what, std::string* error) {
  if (error != nullptr) {
    *error = std::string(what) + ": " + vista_last_error();
  }

  return false;
}

bool Invalid(const std::string& why, std::string* error) {
  if (error != nullptr) {
    *error = why;
  }

  return false;
}

// Enum names go into JSON as they are, so only plain identifiers are let
// through.
bool IsIdentifier(const std::string& text) {
  return !text.empty() && std::all_of(text.begin(), text.end(), [](char c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '-';
  });
}

// A JSON object's text without its braces, for splicing into another, or
// empty. Only the outer braces are checked: Vista parses the rest and
// reports any mistake with its position.
bool ObjectBody(const std::string& json, const char* name, std::string* body, std::string* error) {
  const size_t first = json.find_first_not_of(" \t\r\n");

  if (first == std::string::npos) {
    body->clear();
    return true;
  }

  const size_t last = json.find_last_not_of(" \t\r\n");

  if (json[first] != '{' || json[last] != '}') {
    return Invalid(std::string(name) + " must be a JSON object, such as { \"key\": 1 }.", error);
  }

  *body = json.substr(first + 1, last - first - 1);

  if (body->find_first_not_of(" \t\r\n") == std::string::npos) {
    body->clear();
  }

  return true;
}

std::string Number(float value) {
  char text[32];
  std::snprintf(text, sizeof(text), "%.9g", static_cast<double>(value));
  return text;
}

// What vista_native calls: 0 to go on, non-zero to cancel.
int OnProgress(const char* phase, float progress, void* user) {
  const auto* callback = static_cast<const Progress_t*>(user);
  return (*callback)(phase, progress) ? 0 : 1;
}

MapPtr Export(VistaEngine* engine, uint32_t kind, std::string* error, const char* what) {
  VistaMap* map = nullptr;

  if (vista_engine_export_map(engine, kind, 0, 0, &map) != VISTA_OK) {
    Fail(what, error);
    return MapPtr();
  }

  return MapPtr(map);
}

// Run `rows(from, to)` over [0, count) in bands, one thread a band. Every
// row is computed the same way on any thread count.
template <typename Rows>
void ForRows(uint32_t count, const Rows& rows) {
  const uint32_t threads = std::max(1u, std::min(16u, std::thread::hardware_concurrency()));
  const uint32_t band = (count + threads - 1) / threads;
  std::vector<std::thread> workers;

  for (uint32_t i = 0; i < threads; ++i) {
    const uint32_t from = i * band;
    const uint32_t to = std::min(count, from + band);

    if (from < to) {
      workers.emplace_back(rows, from, to);
    }
  }

  for (std::thread& worker : workers) {
    worker.join();
  }
}

// Where output sample `i` of `out` falls on a grid of `size`: corner to
// corner, as Vista's exportMap resamples.
double SourceCoordinate(uint32_t i, uint32_t size, uint32_t out) {
  return out < 2 ? 0.0 : double(i) * double(size - 1) / double(out - 1);
}

// Fold Vista's material weights (`source`, sourceX x sourceZ, 12 bytes a
// sample, spanning the terrain corner to corner) onto the splat of
// `region`, bilinearly at each terrain sample.
void FoldMaterials(const uint8_t* source, uint32_t sourceX, uint32_t sourceZ, const LayerMap_s& layers,
                   const SelotapeTerrain::Rect_s& region, SelotapeTerrain::Terrain_s* t) {
  const uint32_t x0r = uint32_t(std::max(region.x0, 0));
  const uint32_t z0r = uint32_t(std::max(region.z0, 0));
  const uint32_t x1r = std::min(uint32_t(std::max(region.x1, 0)), t->samplesX - 1);
  const uint32_t z1r = std::min(uint32_t(std::max(region.z1, 0)), t->samplesZ - 1);

  if (x1r < x0r || z1r < z0r) {
    return;
  }

  ForRows(z1r - z0r + 1, [&](uint32_t from, uint32_t to) {
    for (uint32_t row = from; row < to; ++row) {
      const uint32_t zi = z0r + row;
      const double sz = SourceCoordinate(zi, sourceZ, t->samplesZ);
      const uint32_t z0 = std::min(uint32_t(sz), sourceZ - 1);
      const uint32_t z1 = std::min(z0 + 1, sourceZ - 1);
      const float fz = float(sz - double(z0));

      for (uint32_t xi = x0r; xi <= x1r; ++xi) {
        const double sx = SourceCoordinate(xi, sourceX, t->samplesX);
        const uint32_t x0 = std::min(uint32_t(sx), sourceX - 1);
        const uint32_t x1 = std::min(x0 + 1, sourceX - 1);
        const float fx = float(sx - double(x0));
        const size_t taps[4] = { size_t(z0) * sourceX + x0, size_t(z0) * sourceX + x1,
                                 size_t(z1) * sourceX + x0, size_t(z1) * sourceX + x1 };
        const float weight[4] = { (1 - fx) * (1 - fz), fx * (1 - fz), (1 - fx) * fz, fx * fz };
        float w[SelotapeTerrain::k_layers] = {};

        for (int tap = 0; tap < 4; ++tap) {
          const uint8_t* m = source + taps[tap] * k_materials;

          for (uint32_t material = 0; material < k_materials; ++material) {
            const int layer = layers.layerOf[material];

            if (layer >= 0 && layer < int(SelotapeTerrain::k_layers)) {
              w[layer] += weight[tap] * m[material];
            }
          }
        }

        t->splat[t->Index(xi, zi)] = SelotapeTerrain::PackLayerWeights(w);
      }
    }
  });
}

// The four Catmull-Rom taps and weights at `at` on a grid of `size`, the
// edge samples repeated past the ends.
void CubicTaps(double at, uint32_t size, uint32_t index[4], float weight[4]) {
  const double base = std::floor(at);
  const double t = at - base;
  const double t2 = t * t, t3 = t2 * t;
  const double w[4] = { -0.5 * t3 + t2 - 0.5 * t, 1.5 * t3 - 2.5 * t2 + 1.0, -1.5 * t3 + 2.0 * t2 + 0.5 * t,
                        0.5 * t3 - 0.5 * t2 };

  for (int k = 0; k < 4; ++k) {
    const int64_t i = int64_t(base) - 1 + k;
    index[k] = uint32_t(std::clamp<int64_t>(i, 0, int64_t(size) - 1));
    weight[k] = float(w[k]);
  }
}

// An n x n float map to m x m, bicubic (Catmull-Rom) and corner to
// corner, as Vista resamples heights. Separable: across, then down.
std::vector<float> ResampleCubic(const float* source, uint32_t n, uint32_t m) {
  if (n == m) {
    return std::vector<float>(source, source + size_t(n) * n);
  }

  std::vector<uint32_t> index(size_t(m) * 4);
  std::vector<float> weight(size_t(m) * 4);

  for (uint32_t i = 0; i < m; ++i) {
    CubicTaps(SourceCoordinate(i, n, m), n, &index[size_t(i) * 4], &weight[size_t(i) * 4]);
  }

  std::vector<float> across(size_t(n) * m);
  ForRows(n, [&](uint32_t from, uint32_t to) {
    for (uint32_t z = from; z < to; ++z) {
      const float* row = source + size_t(z) * n;

      for (uint32_t x = 0; x < m; ++x) {
        const uint32_t* at = &index[size_t(x) * 4];
        const float* w = &weight[size_t(x) * 4];
        across[size_t(z) * m + x] = w[0] * row[at[0]] + w[1] * row[at[1]] + w[2] * row[at[2]] + w[3] * row[at[3]];
      }
    }
  });

  std::vector<float> out(size_t(m) * m);
  ForRows(m, [&](uint32_t from, uint32_t to) {
    for (uint32_t z = from; z < to; ++z) {
      const uint32_t* at = &index[size_t(z) * 4];
      const float* w = &weight[size_t(z) * 4];
      const float* r[4] = { &across[size_t(at[0]) * m], &across[size_t(at[1]) * m], &across[size_t(at[2]) * m],
                            &across[size_t(at[3]) * m] };

      for (uint32_t x = 0; x < m; ++x) {
        out[size_t(z) * m + x] = w[0] * r[0][x] + w[1] * r[1][x] + w[2] * r[2][x] + w[3] * r[3][x];
      }
    }
  });

  return out;
}

// An n x n byte map to m x m by the nearest sample, as Vista resamples
// categories such as biomes.
std::vector<uint8_t> ResampleNearest(const uint8_t* source, uint32_t n, uint32_t m) {
  std::vector<uint8_t> out(size_t(m) * m);

  for (uint32_t z = 0; z < m; ++z) {
    const uint32_t sz = uint32_t(std::lround(SourceCoordinate(z, n, m)));

    for (uint32_t x = 0; x < m; ++x) {
      const uint32_t sx = uint32_t(std::lround(SourceCoordinate(x, n, m)));
      out[size_t(z) * m + x] = source[size_t(sz) * n + sx];
    }
  }

  return out;
}

// An engine whose world follows `s`' biome and flora options.
EnginePtr CreateEngine(const Settings_s& s, std::string* error) {
  std::string biomes, flora;

  if (!ObjectBody(s.biomesJson, "biomesJson", &biomes, error) || !ObjectBody(s.floraJson, "floraJson", &flora, error)) {
    return EnginePtr();
  }

  // Render options are irrelevant without a renderer; the biome and flora
  // options shape the world.
  std::string json = "{";

  if (!biomes.empty()) {
    json += " \"biomes\": {" + biomes + "}";
  }

  if (!flora.empty()) {
    json += std::string(biomes.empty() ? "" : ",") + " \"flora\": {" + flora + "}";
  }

  json += " }";
  VistaEngine* raw = nullptr;

  if (vista_engine_create(json.c_str(), &raw) != VISTA_OK) {
    Fail("Vista engine options", error);
    return EnginePtr();
  }

  return EnginePtr(raw);
}

// The checks every entry point makes on `s` before any work.
bool CheckSettings(const Settings_s& s, std::string* error) {
  if (!(s.spacing >= 1.0f && s.spacing <= SelotapeTerrain::k_maxSpacing) || std::floor(s.spacing) != s.spacing) {
    return Invalid("spacing must be a whole number of metres from 1 to 64, but it is " + Number(s.spacing) + ".",
                   error);
  }

  const float quads = s.sizeMetres / s.spacing;

  if (!(quads >= 16.0f) || std::floor(quads) != quads || quads > float(SelotapeTerrain::k_maxQuadsPerSide)) {
    return Invalid("sizeMetres must be a whole number of spacings, 16 to " +
                     std::to_string(SelotapeTerrain::k_maxQuadsPerSide) + " of them, but it is " +
                     Number(s.sizeMetres) + " m at " + Number(s.spacing) + " m.",
                   error);
  }

  for (const std::string* name : { &s.landform, &s.edges, &s.erosionQuality }) {
    if (!IsIdentifier(*name)) {
      return Invalid("landform, edges and erosionQuality must be plain names, such as \"alpine\", but one is \"" +
                       *name + "\".",
                     error);
    }
  }

  std::string body;
  return ObjectBody(s.extraJson, "extraJson", &body, error) && ObjectBody(s.biomesJson, "biomesJson", &body, error) &&
    ObjectBody(s.floraJson, "floraJson", &body, error);
}

// A stable 64-bit mix of a tree's position, for an order that does not
// depend on how Vista listed the trees.
uint64_t PositionHash(float x, float z) {
  uint32_t ix = 0, iz = 0;
  std::memcpy(&ix, &x, 4);
  std::memcpy(&iz, &z, 4);
  uint64_t h = (uint64_t(ix) << 32) ^ iz;
  h ^= h >> 33;
  h *= 0xff51afd7ed558ccdull;
  h ^= h >> 33;
  h *= 0xc4ceb9fe1a85ec53ull;
  h ^= h >> 33;
  return h;
}

}  // namespace

LayerMap_s FourLayerMap() {
  // VistaMaterial order: lush grass, dry grass, forest floor, sand, rock,
  // snow, mud, volcanic, ice, tundra, gravel, scree.
  return LayerMap_s { { 1, 0, 1, 0, 2, 0, 3, 2, 0, 0, 3, 2 } };
}

LayerMap_s EightLayerMap() {
  return LayerMap_s { { 1, 0, 4, 5, 2, 6, 3, 2, 6, 0, 3, 7 } };
}

LayerMap_s LayerMapFor(uint32_t layers) {
  return layers >= 8 ? EightLayerMap() : FourLayerMap();
}

uint32_t VistaSamples(const Settings_s& s) {
  if (s.vistaSamples != 0) {
    return s.vistaSamples;
  }

  const float samples = s.spacing > 0.0f ? s.sizeMetres / s.spacing + 1.0f : 0.0f;
  uint32_t n = 16;

  while (n * 2 <= k_maxVistaSamples && float(n * 2) <= samples) {
    n *= 2;
  }

  return n;
}

std::string FractalJson(const Settings_s& s, uint32_t n) {
  std::ostringstream json;
  // Vista's grid spans the same metres as the terrain's: resampling maps
  // its corner samples onto the terrain's corner samples.
  const float metresPerSample = n > 1 ? s.sizeMetres / float(n - 1) : s.sizeMetres;
  json << "{ \"seed\": " << s.seed << ", \"size\": " << n << ", \"horizontalScaleMetres\": "
       << Number(metresPerSample) << ", \"verticalScale\": " << Number(s.verticalScale)
       << ", \"seaLevelMetres\": " << Number(s.seaLevelMetres) << ", \"landform\": \"" << s.landform
       << "\", \"edges\": \"" << s.edges << "\"";

  if (s.erosion) {
    json << ", \"erosion\": { \"quality\": \"" << s.erosionQuality << "\" }";
  }

  if (s.island > 0.0f) {
    json << ", \"shape\": { \"island\": " << Number(s.island) << " }";
  }

  std::string extra;

  if (ObjectBody(s.extraJson, "extraJson", &extra, nullptr) && !extra.empty()) {
    json << ", " << extra;
  }

  json << " }";
  return json.str();
}

bool Generate(const Settings_s& s, const std::string& materials, const LayerMap_s& layers, Result_s* out,
              std::string* error, const Progress_t& progress, bool* cancelled) {
  if (cancelled != nullptr) {
    *cancelled = false;
  }

  if (out == nullptr) {
    return Invalid("SelotapeVista::Generate needs somewhere to put the result.", error);
  }

  if (!CheckSettings(s, error)) {
    return false;
  }

  const uint32_t n = VistaSamples(s);
  const uint32_t samples = uint32_t(s.sizeMetres / s.spacing) + 1;
  EnginePtr engine = CreateEngine(s, error);

  if (!engine) {
    return false;
  }

  const std::string fractal = FractalJson(s, n);
  const VistaStatus generated = progress
    ? vista_engine_generate_fractal(engine.get(), fractal.c_str(), OnProgress, const_cast<Progress_t*>(&progress))
    : vista_engine_generate_fractal(engine.get(), fractal.c_str(), nullptr, nullptr);

  if (generated == VISTA_CANCELLED && cancelled != nullptr) {
    *cancelled = true;
  }

  if (generated != VISTA_OK) {
    return Fail("Vista generation", error);
  }

  Result_s result;
  SelotapeTerrain::Terrain_s& t = result.terrain;
  t.samplesX = samples;
  t.samplesZ = samples;
  t.spacing = s.spacing;
  t.originX = -0.5f * s.sizeMetres;
  t.originZ = -0.5f * s.sizeMetres;
  t.materials = materials;
  t.gen.seed = s.seed;
  t.gen.sizeMetres = s.sizeMetres;
  t.gen.spacing = s.spacing;
  t.gen.baseHeight = s.seaLevelMetres;

  // Vista painted the ground; Selotape's own rules would paint over it.
  for (SelotapeTerrain::SplatRule_s& rule : t.rules) {
    rule.enabled = false;
  }

  // Every map is read at Vista's own size and resampled here: Vista's
  // exportMap stops at 2048 a side (the browser build's memory limit), and
  // a 4 km terrain at 1 m has 4097. The filters are exportMap's own.
  MapPtr heights = Export(engine.get(), VISTA_MAP_HEIGHT, error, "Vista heights");

  if (!heights) {
    return false;
  }

  t.heights = ResampleCubic(static_cast<const float*>(heights->data), heights->width, samples);
  heights.reset();

  // Materials at Vista's own size, folded per sample: resampling twelve
  // channels to a 4097-sample grid first would take 200 MB.
  MapPtr materialMap = Export(engine.get(), VISTA_MAP_MATERIALS, error, "Vista materials");

  if (!materialMap) {
    return false;
  }

  t.splat.assign(t.heights.size(), 0);
  SelotapeTerrain::Rect_s all;
  all.x0 = 0;
  all.z0 = 0;
  all.x1 = int32_t(samples) - 1;
  all.z1 = int32_t(samples) - 1;
  FoldMaterials(static_cast<const uint8_t*>(materialMap->data), materialMap->width, materialMap->height, layers, all,
                &t);
  materialMap.reset();

  MapPtr depth = Export(engine.get(), VISTA_MAP_WATER_DEPTH, error, "Vista water depth");

  if (!depth) {
    return false;
  }

  result.waterDepth = ResampleCubic(static_cast<const float*>(depth->data), depth->width, samples);
  depth.reset();

  // A cubic filter overshoots at a shore; dry ground is 0 deep.
  for (float& metres : result.waterDepth) {
    metres = std::max(metres, 0.0f);
  }

  MapPtr biome = Export(engine.get(), VISTA_MAP_BIOME, error, "Vista biomes");

  if (!biome) {
    return false;
  }

  result.biome = ResampleNearest(static_cast<const uint8_t*>(biome->data), biome->width, samples);
  biome.reset();

  VistaTrees* rawTrees = nullptr;

  // The engine's own cap would refuse a large forest; this asks for up to
  // the engine's maximum.
  if (vista_engine_export_trees(engine.get(), nullptr, 4000000, &rawTrees) != VISTA_OK) {
    return Fail("Vista trees", error);
  }

  std::unique_ptr<VistaTrees, TreesDeleter> trees(rawTrees);
  result.trees.reserve(trees->count);

  for (uint32_t i = 0; i < trees->count; ++i) {
    const float* r = trees->records + size_t(i) * trees->floats_per_record;
    Tree_s tree;
    tree.x = r[0];
    tree.y = r[1];
    tree.z = r[2];
    tree.species = uint32_t(r[3]);
    tree.variant = uint32_t(r[4]);
    tree.scale = r[5];
    tree.yawDegrees = r[6] * (180.0f / 3.14159265358979f);
    result.trees.push_back(tree);
  }

  VistaMesh* rawMesh = nullptr;

  if (vista_engine_water_mesh(engine.get(), VISTA_WATER_SURFACES, &rawMesh) != VISTA_OK) {
    return Fail("Vista water mesh", error);
  }

  std::unique_ptr<VistaMesh, MeshDeleter> mesh(rawMesh);
  const auto* vertices = static_cast<const VistaWaterVertex*>(mesh->vertices);
  result.waterVertices.assign(vertices, vertices + mesh->vertex_count);
  result.waterIndices.assign(mesh->indices, mesh->indices + mesh->index_count);

  char* metadata = nullptr;

  if (vista_engine_query_json(engine.get(), "metadata", &metadata) != VISTA_OK) {
    return Fail("Vista metadata", error);
  }

  result.metadataJson = std::unique_ptr<char, StringDeleter>(metadata).get();
  *out = std::move(result);
  return true;
}

// --- off the UI thread ------------------------------------------------------

float OverallProgress(const std::string& phase, float progress) {
  // Each stage's share of a generation with balanced erosion, as the test
  // measures it at Vista's 1024 (integrations/selotape/test): tectonics
  // and drainage 1% each, detail 5%, erosion 67%, then "finishing" (0 to
  // 0.5) 5%, rivers 5%, and "finishing" (0.5 to 1) 16%.
  struct Stage_s {
    const char* name;
    float start, share;
  };
  static const Stage_s k_stages[] = {
    { "tectonics", 0.00f, 0.01f }, { "drainage", 0.01f, 0.01f }, { "detail", 0.02f, 0.05f },
    { "erosion", 0.07f, 0.67f },   { "rivers", 0.79f, 0.05f },
  };
  const float p = std::clamp(progress, 0.0f, 1.0f);

  if (phase == "finishing") {
    // Before the rivers it is the map's conditioning; after them, the
    // vegetation and the end.
    return p < 0.5f ? 0.74f + p * 0.1f : 0.84f + (p - 0.5f) * 0.32f;
  }

  for (const Stage_s& stage : k_stages) {
    if (phase == stage.name) {
      return stage.start + stage.share * p;
    }
  }

  return 0.0f;
}

Job_c::~Job_c() {
  Cancel();
  Join();
}

void Job_c::Join() {
  if (m_worker.joinable()) {
    m_worker.join();
  }
}

bool Job_c::Start(const Settings_s& settings, const std::string& materials, const LayerMap_s& layers) {
  if (m_running.load() || m_done.load()) {
    return false;
  }

  Join();
  {
    std::lock_guard<std::mutex> lock(m_lock);
    m_phase.clear();
    m_progress = 0.0f;
    m_ok = false;
    m_cancelled = false;
    m_error.clear();
    m_result = Result_s();
  }
  m_cancel.store(false);
  m_running.store(true);
  m_worker = std::thread([this, settings, materials, layers]() {
    Result_s result;
    std::string error;
    bool cancelled = false;
    const Progress_t progress = [this](const char* phase, float value) {
      std::lock_guard<std::mutex> lock(m_lock);
      m_phase = phase;
      m_progress = std::max(m_progress, OverallProgress(phase, value));
      return !m_cancel.load();
    };
    const bool ok = Generate(settings, materials, layers, &result, &error, progress, &cancelled);
    {
      std::lock_guard<std::mutex> lock(m_lock);
      m_ok = ok;
      m_cancelled = cancelled;
      m_error = error;
      m_result = std::move(result);
      m_progress = ok ? 1.0f : m_progress;
    }
    m_done.store(true);
    m_running.store(false);
  });
  return true;
}

void Job_c::Cancel() {
  m_cancel.store(true);
}

bool Job_c::Running() const {
  return m_running.load();
}

bool Job_c::Done() const {
  return m_done.load();
}

std::string Job_c::Phase() const {
  std::lock_guard<std::mutex> lock(m_lock);
  return m_phase;
}

float Job_c::Progress() const {
  std::lock_guard<std::mutex> lock(m_lock);
  return m_progress;
}

bool Job_c::Take(Result_s* out, std::string* error, bool* cancelled) {
  if (!m_done.load()) {
    return Invalid("The Vista job has not finished.", error);
  }

  Join();
  std::lock_guard<std::mutex> lock(m_lock);
  m_done.store(false);

  if (cancelled != nullptr) {
    *cancelled = m_cancelled;
  }

  if (!m_ok) {
    return Invalid(m_error, error);
  }

  if (out != nullptr) {
    *out = std::move(m_result);
  }

  m_result = Result_s();
  return true;
}

// --- the editor's operations ------------------------------------------------

bool RegenerateInPlace(SelotapeTerrain::Terrain_s* t, Settings_s s, const LayerMap_s& layers, std::string* error,
                       const Progress_t& progress, bool* cancelled, Result_s* rest) {
  if (t == nullptr || t->samplesX < 2 || t->samplesX != t->samplesZ) {
    return Invalid("RegenerateInPlace needs a square terrain; Vista generates only square maps.", error);
  }

  s.sizeMetres = t->SizeX();
  s.spacing = t->spacing;
  Result_s result;

  if (!Generate(s, t->materials, layers, &result, error, progress, cancelled)) {
    return false;
  }

  // Only what Vista made changes: the size is the same, and the origin,
  // materials, haze, stock layers and lights stay as the author left them.
  t->heights = std::move(result.terrain.heights);
  t->splat = std::move(result.terrain.splat);
  t->gen.seed = s.seed;

  for (SelotapeTerrain::SplatRule_s& rule : t->rules) {
    rule.enabled = false;
  }

  if (rest != nullptr) {
    result.terrain = SelotapeTerrain::Terrain_s();
    *rest = std::move(result);
  }

  return true;
}

bool AutoSplatVista(SelotapeTerrain::Terrain_s* t, const Settings_s& s, const LayerMap_s& layers,
                    const SelotapeTerrain::Rect_s& region, std::string* error) {
  if (t == nullptr || t->samplesX < 2 || t->samplesZ < 2 || t->heights.size() != size_t(t->samplesX) * t->samplesZ) {
    return Invalid("AutoSplatVista needs a terrain with heights.", error);
  }

  if (!IsIdentifier(s.landform)) {
    return Invalid("landform must be a plain name, such as \"alpine\", but it is \"" + s.landform + "\".", error);
  }

  // Vista loads at most 2048 a side. Every `stride`-th sample is taken, a
  // stride that divides both sides so the grid still spans the terrain
  // exactly; a 4097-sample terrain is read at 2049 -> 1025.
  uint32_t stride = 1;

  while (std::max((t->samplesX - 1) / stride + 1, (t->samplesZ - 1) / stride + 1) > k_maxVistaSamples) {
    if ((t->samplesX - 1) % (stride * 2) != 0 || (t->samplesZ - 1) % (stride * 2) != 0) {
      return Invalid("The terrain is too large for Vista and its sides (" + std::to_string(t->samplesX) + " x " +
                       std::to_string(t->samplesZ) + " samples) share no stride that brings it to 2048 or fewer.",
                     error);
    }

    stride *= 2;
  }

  const uint32_t nx = (t->samplesX - 1) / stride + 1;
  const uint32_t nz = (t->samplesZ - 1) / stride + 1;
  std::vector<float> heights(size_t(nx) * nz);

  for (uint32_t z = 0; z < nz; ++z) {
    for (uint32_t x = 0; x < nx; ++x) {
      heights[size_t(z) * nx + x] = t->heights[t->Index(x * stride, z * stride)];
    }
  }

  EnginePtr engine = CreateEngine(s, error);

  if (!engine) {
    return false;
  }

  std::ostringstream options;
  options << "{ \"width\": " << nx << ", \"height\": " << nz
          << ", \"sampleFormat\": \"float32\", \"byteOrder\": \"little-endian\", \"metresPerSample\": "
          << Number(t->spacing * float(stride)) << ", \"heightScaleMetres\": 1, \"seaLevelMetres\": "
          << Number(s.seaLevelMetres) << ", \"landform\": \"" << s.landform << "\" }";
  std::vector<uint8_t> bytes(heights.size() * 4);

  for (size_t i = 0; i < heights.size(); ++i) {
    uint32_t bits = 0;
    std::memcpy(&bits, &heights[i], 4);

    for (int b = 0; b < 4; ++b) {
      bytes[i * 4 + b] = uint8_t(bits >> (8 * b));
    }
  }

  if (vista_engine_load_raw_heightmap(engine.get(), bytes.data(), bytes.size(), options.str().c_str()) != VISTA_OK) {
    return Fail("Vista could not read the terrain's heights", error);
  }

  MapPtr materialMap = Export(engine.get(), VISTA_MAP_MATERIALS, error, "Vista materials");

  if (!materialMap) {
    return false;
  }

  SelotapeTerrain::Rect_s all;
  all.x0 = 0;
  all.z0 = 0;
  all.x1 = int32_t(t->samplesX) - 1;
  all.z1 = int32_t(t->samplesZ) - 1;

  if (t->splat.size() != t->heights.size()) {
    t->splat.assign(t->heights.size(), 0xff);
  }

  FoldMaterials(static_cast<const uint8_t*>(materialMap->data), materialMap->width, materialMap->height, layers,
                region.Empty() ? all : region, t);
  return true;
}

float PrepareApron(SelotapeTerrain::Terrain_s* t, float fadeMetres) {
  if (t == nullptr || t->samplesX < 2 || t->samplesZ < 2 || t->heights.empty()) {
    return 0.0f;
  }

  double sum = 0.0;
  uint32_t count = 0;

  for (uint32_t x = 0; x < t->samplesX; ++x) {
    sum += t->heights[t->Index(x, 0)] + t->heights[t->Index(x, t->samplesZ - 1)];
    count += 2;
  }

  for (uint32_t z = 1; z + 1 < t->samplesZ; ++z) {
    sum += t->heights[t->Index(0, z)] + t->heights[t->Index(t->samplesX - 1, z)];
    count += 2;
  }

  const float edge = float(sum / count);
  // A flat plain at the edge's height: no noise, no shape, no erosion.
  t->gen.baseHeight = edge;
  t->gen.amplitude = 0.0f;
  t->gen.warp = 0.0f;
  t->gen.shape = SelotapeTerrain::k_shapeNoise;
  t->gen.thermalIterations = 0;
  t->gen.droplets = 0;
  t->gen.flatRadius = 0.0f;

  if (fadeMetres > 0.0f) {
    SelotapeTerrain::FadeEdges(t, fadeMetres, edge);
  }

  return edge;
}

// --- the trees as props -------------------------------------------------------

std::vector<Placement_s> TreePlacements(const std::vector<Tree_s>& trees, const SpeciesMap_s& species,
                                        const TreeFilter_s& filter) {
  const bool anywhere = filter.region[0] > filter.region[2] || filter.region[1] > filter.region[3];
  std::vector<std::pair<uint64_t, const Tree_s*>> order;
  order.reserve(trees.size());

  for (const Tree_s& tree : trees) {
    if (tree.species >= 8 || species.species[tree.species].templateName.empty()) {
      continue;
    }

    if (!anywhere && (tree.x < filter.region[0] || tree.x > filter.region[2] || tree.z < filter.region[1] ||
                      tree.z > filter.region[3])) {
      continue;
    }

    order.emplace_back(PositionHash(tree.x, tree.z), &tree);
  }

  // A hash of position orders them, so any prefix is spread over the whole
  // map rather than being the first rows Vista listed.
  std::sort(order.begin(), order.end(), [](const auto& a, const auto& b) {
    return a.first != b.first ? a.first < b.first
                              : (a.second->x != b.second->x ? a.second->x < b.second->x : a.second->z < b.second->z);
  });

  std::vector<Placement_s> out;
  out.reserve(std::min<size_t>(order.size(), filter.maxCount));
  // Kept trunks by grid cell of minSpacing, to test spacing in constant time.
  std::unordered_map<uint64_t, std::vector<std::pair<float, float>>> cells;
  const float cell = filter.minSpacing;
  const auto key = [](int64_t cx, int64_t cz) { return (uint64_t(cx) << 32) ^ uint64_t(uint32_t(cz)); };

  for (const auto& entry : order) {
    if (out.size() >= filter.maxCount) {
      break;
    }

    const Tree_s& tree = *entry.second;

    if (cell > 0.0f) {
      const int64_t cx = int64_t(std::floor(tree.x / cell));
      const int64_t cz = int64_t(std::floor(tree.z / cell));
      bool crowded = false;

      for (int64_t dz = -1; dz <= 1 && !crowded; ++dz) {
        for (int64_t dx = -1; dx <= 1 && !crowded; ++dx) {
          const auto found = cells.find(key(cx + dx, cz + dz));

          if (found == cells.end()) {
            continue;
          }

          for (const auto& at : found->second) {
            const float ex = at.first - tree.x, ez = at.second - tree.z;

            if (ex * ex + ez * ez < cell * cell) {
              crowded = true;
              break;
            }
          }
        }
      }

      if (crowded) {
        continue;
      }

      cells[key(cx, cz)].emplace_back(tree.x, tree.z);
    }

    const SpeciesProp_s& prop = species.species[tree.species];
    Placement_s placement;
    placement.templateName = prop.templateName;
    placement.model = prop.model;
    placement.pos[0] = tree.x;
    placement.pos[1] = tree.y - filter.sink;
    placement.pos[2] = tree.z;
    placement.rotDeg[1] = tree.yawDegrees;
    placement.scale = tree.scale * prop.baseScale;
    placement.species = tree.species;
    out.push_back(placement);
  }

  return out;
}

// --- the meta block -----------------------------------------------------------

std::string MetaLines(const Settings_s& s) {
  // JSON is whitespace-insensitive, so line breaks inside it become spaces
  // and each value stays on its own line.
  auto line = [](std::string text) {
    std::replace(text.begin(), text.end(), '\n', ' ');
    std::replace(text.begin(), text.end(), '\r', ' ');
    return text;
  };
  std::ostringstream meta;
  meta << "vista.seed = " << s.seed << "\n"
       << "vista.sizeMetres = " << Number(s.sizeMetres) << "\n"
       << "vista.spacing = " << Number(s.spacing) << "\n"
       << "vista.landform = " << s.landform << "\n"
       << "vista.edges = " << s.edges << "\n"
       << "vista.verticalScale = " << Number(s.verticalScale) << "\n"
       << "vista.seaLevelMetres = " << Number(s.seaLevelMetres) << "\n"
       << "vista.erosion = " << (s.erosion ? 1 : 0) << "\n"
       << "vista.erosionQuality = " << s.erosionQuality << "\n"
       << "vista.island = " << Number(s.island) << "\n"
       << "vista.vistaSamples = " << s.vistaSamples << "\n"
       << "vista.extraJson = " << line(s.extraJson) << "\n"
       << "vista.biomesJson = " << line(s.biomesJson) << "\n"
       << "vista.floraJson = " << line(s.floraJson) << "\n";
  return meta.str();
}

bool IsVistaMeta(const std::string& meta) {
  return meta.compare(0, 11, "vista.seed ") == 0 || meta.find("\nvista.seed ") != std::string::npos;
}

bool ParseMetaLines(const std::string& meta, Settings_s* out, std::string* error) {
  Settings_s s = *out;
  std::istringstream lines(meta);
  std::string text;

  while (std::getline(lines, text)) {
    const size_t equals = text.find('=');

    if (text.compare(0, 6, "vista.") != 0 || equals == std::string::npos) {
      continue;
    }

    auto trim = [](const std::string& v) {
      const size_t a = v.find_first_not_of(" \t\r");
      const size_t b = v.find_last_not_of(" \t\r");
      return a == std::string::npos ? std::string() : v.substr(a, b - a + 1);
    };
    const std::string key = trim(text.substr(6, equals - 6));
    const std::string value = trim(text.substr(equals + 1));
    char* end = nullptr;
    bool ok = true;
    auto number = [&]() {
      const float v = std::strtof(value.c_str(), &end);
      ok = !value.empty() && end != nullptr && *end == '\0';
      return v;
    };
    auto whole = [&]() {
      const unsigned long v = std::strtoul(value.c_str(), &end, 10);
      ok = !value.empty() && end != nullptr && *end == '\0';
      return uint32_t(v);
    };

    if (key == "seed") s.seed = whole();
    else if (key == "sizeMetres") s.sizeMetres = number();
    else if (key == "spacing") s.spacing = number();
    else if (key == "landform") s.landform = value;
    else if (key == "edges") s.edges = value;
    else if (key == "verticalScale") s.verticalScale = number();
    else if (key == "seaLevelMetres") s.seaLevelMetres = number();
    else if (key == "erosion") s.erosion = whole() != 0;
    else if (key == "erosionQuality") s.erosionQuality = value;
    else if (key == "island") s.island = number();
    else if (key == "vistaSamples") s.vistaSamples = whole();
    else if (key == "extraJson") s.extraJson = value;
    else if (key == "biomesJson") s.biomesJson = value;
    else if (key == "floraJson") s.floraJson = value;

    if (!ok) {
      return Invalid("vista." + key + " is not a number: \"" + value + "\".", error);
    }
  }

  *out = s;
  return true;
}

}  // namespace SelotapeVista
