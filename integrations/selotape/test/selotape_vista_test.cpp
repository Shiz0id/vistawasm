// SelotapeVista against vista_native: generate terrains the size the map
// editor makes, and check what lands on the Terrain_s.
//
// Build and run it with integrations/selotape/test/run.sh.

#include "SelotapeVista.h"

#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <thread>
#include <vector>

namespace {

int g_failures = 0;

void Check(bool ok, const std::string& what) {
  if (!ok) {
    std::fprintf(stderr, "FAIL: %s\n", what.c_str());
    g_failures += 1;
  }
}

uint32_t SplatSum(uint64_t packed) {
  uint32_t sum = 0;

  for (uint32_t i = 0; i < SelotapeTerrain::k_layers; ++i) {
    sum += uint32_t((packed >> (8 * i)) & 0xff);
  }

  return sum;
}

struct LayerShares {
  double share[SelotapeTerrain::k_layers] = {};
};

LayerShares GenerateAndCheck(SelotapeVista::Settings_s settings, const SelotapeVista::LayerMap_s& layers,
                              const char* label) {
  LayerShares shares;
  SelotapeVista::Result_s result;
  std::string error;
  const auto start = std::chrono::steady_clock::now();
  const bool ok = SelotapeVista::Generate(settings, "proc:desert", layers, &result, &error);
  const double seconds =
    std::chrono::duration<double>(std::chrono::steady_clock::now() - start).count();

  if (!ok) {
    Check(false, std::string(label) + ": " + error);
    return shares;
  }

  const SelotapeTerrain::Terrain_s& t = result.terrain;
  const uint32_t samples = uint32_t(settings.sizeMetres / settings.spacing) + 1;
  Check(t.samplesX == samples && t.samplesZ == samples, std::string(label) + ": grid size");
  Check(t.heights.size() == size_t(samples) * samples, std::string(label) + ": heights");
  Check(t.splat.size() == t.heights.size(), std::string(label) + ": splat");
  Check(result.waterDepth.size() == t.heights.size(), std::string(label) + ": water depth");
  Check(result.biome.size() == t.heights.size(), std::string(label) + ": biomes");
  Check(std::fabs(t.SizeX() - settings.sizeMetres) < 0.01f, std::string(label) + ": extent");
  Check(std::fabs(t.originX + settings.sizeMetres * 0.5f) < 0.01f, std::string(label) + ": centred");

  float low = 1e30f, high = -1e30f;
  uint32_t badSplat = 0;
  uint64_t layerTotals[SelotapeTerrain::k_layers] = {};

  for (size_t i = 0; i < t.heights.size(); ++i) {
    low = std::min(low, t.heights[i]);
    high = std::max(high, t.heights[i]);
    badSplat += SplatSum(t.splat[i]) == 255 ? 0 : 1;

    for (uint32_t k = 0; k < SelotapeTerrain::k_layers; ++k) {
      layerTotals[k] += (t.splat[i] >> (8 * k)) & 0xff;
    }
  }

  Check(std::isfinite(low) && std::isfinite(high) && high > low, std::string(label) + ": relief");
  Check(badSplat == 0, std::string(label) + ": " + std::to_string(badSplat) + " splat words do not sum to 255");

  for (const SelotapeVista::Tree_s& tree : result.trees) {
    if (std::fabs(tree.x) > settings.sizeMetres * 0.5f + 1.0f || tree.species > 7) {
      Check(false, std::string(label) + ": a tree off the map or of no species");
      break;
    }
  }

  for (uint32_t index : result.waterIndices) {
    if (index >= result.waterVertices.size()) {
      Check(false, std::string(label) + ": a water index past its vertices");
      break;
    }
  }

  std::printf("%s: %ux%u from Vista's %u, %.2f s, heights %.1f..%.1f m, %zu trees, %zu water triangles\n",
              label, samples, samples, SelotapeVista::VistaSamples(settings), seconds, low, high,
              result.trees.size(), result.waterIndices.size() / 3);
  std::printf("  layer shares:");

  for (uint32_t k = 0; k < SelotapeTerrain::k_layers; ++k) {
    shares.share[k] = double(layerTotals[k]) / (255.0 * double(t.heights.size()));
    std::printf(" %.1f%%", 100.0 * shares.share[k]);
  }

  std::printf("\n");
  return shares;
}

// SelotapeVista resamples heights itself, because Vista's exportMap stops
// at 2048 a side. Where both can, the two must agree.
void CheckResamplingMatchesVista(const SelotapeVista::Settings_s& settings) {
  SelotapeVista::Result_s result;
  std::string error;

  if (!SelotapeVista::Generate(settings, "", SelotapeVista::FourLayerMap(), &result, &error)) {
    Check(false, "resampling check: " + error);
    return;
  }

  VistaEngine* engine = nullptr;
  vista_engine_create(nullptr, &engine);
  const std::string json = SelotapeVista::FractalJson(settings, SelotapeVista::VistaSamples(settings));
  VistaMap* map = nullptr;
  const uint32_t samples = result.terrain.samplesX;
  const bool ok = vista_engine_generate_fractal(engine, json.c_str(), nullptr, nullptr) == VISTA_OK &&
    vista_engine_export_map(engine, VISTA_MAP_HEIGHT, samples, samples, &map) == VISTA_OK;
  Check(ok, std::string("resampling check: ") + vista_last_error());

  if (ok) {
    const auto* vista = static_cast<const float*>(map->data);
    float worst = 0.0f;

    for (size_t i = 0; i < result.terrain.heights.size(); ++i) {
      worst = std::max(worst, std::fabs(vista[i] - result.terrain.heights[i]));
    }

    Check(worst < 0.01f, "heights differ from Vista's own resampling by " + std::to_string(worst) + " m");
    std::printf("resampling matches Vista's exportMap to %.5f m over %ux%u\n", worst, samples, samples);
  }

  vista_map_free(map);
  vista_engine_destroy(engine);
}

uint32_t Layer(uint64_t packed, uint32_t k) {
  return uint32_t((packed >> (8 * k)) & 0xff);
}

// How long each stage takes, as shares of the whole, to check the weights
// OverallProgress uses.
void MeasureStages(const SelotapeVista::Settings_s& settings) {
  using Clock = std::chrono::steady_clock;
  std::vector<std::pair<std::string, double>> marks;
  const auto start = Clock::now();
  SelotapeVista::Result_s result;
  std::string error;
  float last = -1.0f;
  bool monotonic = true;
  const SelotapeVista::Progress_t progress = [&](const char* phase, float value) {
    const double at = std::chrono::duration<double>(Clock::now() - start).count();

    if (marks.empty() || marks.back().first != phase) {
      marks.emplace_back(phase, at);
    }

    const float overall = SelotapeVista::OverallProgress(phase, value);
    monotonic = monotonic && overall + 1e-4f >= last;
    last = overall;
    return true;
  };
  Check(SelotapeVista::Generate(settings, "", SelotapeVista::FourLayerMap(), &result, &error, progress),
        "stage timing: " + error);
  const double total = std::chrono::duration<double>(Clock::now() - start).count();
  Check(monotonic, "OverallProgress never goes backwards over a real generation");
  std::printf("stage shares at Vista %u:", SelotapeVista::VistaSamples(settings));

  for (size_t i = 0; i < marks.size(); ++i) {
    const double end = i + 1 < marks.size() ? marks[i + 1].second : total;
    std::printf(" %s %.0f%%", marks[i].first.c_str(), 100.0 * (end - marks[i].second) / total);
  }

  std::printf(" (%.1f s)\n", total);
}

void CheckEditorHelpers() {
  std::string error;
  SelotapeVista::Settings_s quick;
  quick.sizeMetres = 512.0f;
  quick.spacing = 2.0f;
  quick.erosionQuality = "preview";

  // Job_c: runs, reports progress that only rises, and hands the result over.
  {
    SelotapeVista::Job_c job;
    Check(job.Start(quick, "proc:desert", SelotapeVista::FourLayerMap()), "Job_c starts");
    Check(!job.Start(quick, "proc:desert", SelotapeVista::FourLayerMap()), "Job_c refuses a second start");
    float last = 0.0f;
    bool rising = true;

    while (!job.Done()) {
      const float now = job.Progress();
      rising = rising && now >= last;
      last = now;
      std::this_thread::sleep_for(std::chrono::milliseconds(5));
    }

    SelotapeVista::Result_s result;
    bool cancelled = true;
    Check(job.Take(&result, &error, &cancelled), "Job_c result: " + error);
    Check(!cancelled && rising && job.Progress() == 1.0f, "Job_c progress rises to 1");
    Check(result.terrain.samplesX == 257, "Job_c terrain");
    Check(!job.Running() && !job.Done(), "Job_c is idle after Take");
  }

  // Job_c: a cancel during erosion stops it, and the job starts again.
  {
    SelotapeVista::Settings_s slow = quick;
    slow.sizeMetres = 1024.0f;
    slow.spacing = 1.0f;
    slow.erosionQuality = "high";
    SelotapeVista::Job_c job;
    job.Start(slow, "", SelotapeVista::FourLayerMap());

    while (!job.Done() && job.Phase() != "erosion") {
      std::this_thread::sleep_for(std::chrono::milliseconds(2));
    }

    const auto cancelledAt = std::chrono::steady_clock::now();
    job.Cancel();

    while (!job.Done()) {
      std::this_thread::sleep_for(std::chrono::milliseconds(2));
    }

    const double latency =
      std::chrono::duration<double>(std::chrono::steady_clock::now() - cancelledAt).count();
    bool cancelled = false;
    Check(!job.Take(nullptr, &error, &cancelled) && cancelled, "Job_c cancels during erosion");
    Check(error.find("cancelled") != std::string::npos, "the cancel says so: " + error);
    std::printf("cancel landed %.2f s after it was asked for, during erosion\n", latency);
    Check(job.Start(quick, "", SelotapeVista::FourLayerMap()), "Job_c starts again after a cancel");
  }

  // A destroyed running job cancels and waits, without crashing.
  {
    SelotapeVista::Job_c job;
    job.Start(quick, "", SelotapeVista::FourLayerMap());
  }

  SelotapeVista::Result_s base;
  Check(SelotapeVista::Generate(quick, "proc:desert", SelotapeVista::FourLayerMap(), &base, &error),
        "helpers base: " + error);
  SelotapeTerrain::Terrain_s t = base.terrain;

  // RegenerateInPlace: new ground, the same terrain otherwise.
  {
    SelotapeTerrain::Terrain_s again = t;
    again.materials = "proc:snow";
    again.originX = 100.0f;
    SelotapeVista::Settings_s other = quick;
    other.seed = 2;
    other.sizeMetres = 1.0f;   // taken from the terrain, so ignored
    SelotapeVista::Result_s rest;
    Check(SelotapeVista::RegenerateInPlace(&again, other, SelotapeVista::FourLayerMap(), &error,
                                            SelotapeVista::Progress_t(), nullptr, &rest),
          "RegenerateInPlace: " + error);
    Check(again.samplesX == t.samplesX && again.materials == "proc:snow" && again.originX == 100.0f,
          "RegenerateInPlace keeps the terrain's own fields");
    Check(again.heights != t.heights && again.gen.seed == 2, "RegenerateInPlace makes new ground");
    Check(rest.terrain.heights.empty() && rest.biome.size() == t.heights.size(),
          "RegenerateInPlace hands back the rest");
    SelotapeTerrain::Terrain_s oblong = t;
    oblong.samplesZ = 129;
    Check(!SelotapeVista::RegenerateInPlace(&oblong, other, SelotapeVista::FourLayerMap(), &error) &&
            oblong.heights == t.heights,
          "RegenerateInPlace refuses a terrain that is not square, unchanged");
  }

  // AutoSplatVista: a sculpted peak turns to rock and snow; outside the
  // region nothing changes; every word still sums to 255.
  {
    SelotapeTerrain::Terrain_s sculpted = t;
    SelotapeTerrain::Rect_s region;
    region.x0 = 100;
    region.z0 = 100;
    region.x1 = 156;
    region.z1 = 156;

    for (int32_t z = region.z0; z <= region.z1; ++z) {
      for (int32_t x = region.x0; x <= region.x1; ++x) {
        const float dx = float(x - 128), dz = float(z - 128);
        // A 700 m spike, far above anything the base map has.
        sculpted.heights[sculpted.Index(uint32_t(x), uint32_t(z))] += 700.0f * std::exp(-(dx * dx + dz * dz) / 120.0f);
      }
    }

    const std::vector<uint64_t> before = sculpted.splat;
    Check(SelotapeVista::AutoSplatVista(&sculpted, quick, SelotapeVista::FourLayerMap(), region, &error),
          "AutoSplatVista: " + error);
    uint32_t outsideChanged = 0, bad = 0;

    for (uint32_t z = 0; z < sculpted.samplesZ; ++z) {
      for (uint32_t x = 0; x < sculpted.samplesX; ++x) {
        const size_t i = sculpted.Index(x, z);
        const bool inside = int32_t(x) >= region.x0 && int32_t(x) <= region.x1 && int32_t(z) >= region.z0 &&
          int32_t(z) <= region.z1;
        outsideChanged += !inside && sculpted.splat[i] != before[i];
        bad += SplatSum(sculpted.splat[i]) != 255;
      }
    }

    Check(outsideChanged == 0, "AutoSplatVista leaves the rest alone");
    Check(bad == 0, "AutoSplatVista keeps every word summing to 255");
    // The summit: base ground (snow in a four-layer set) or rock, not grass.
    const uint64_t summit = sculpted.splat[sculpted.Index(128, 128)];
    Check(Layer(summit, 1) < 64, "the sculpted summit is not grass: " + std::to_string(Layer(summit, 1)));

    // The whole map, unsculpted, comes out close to what Generate painted.
    SelotapeTerrain::Terrain_s repainted = t;
    SelotapeTerrain::Rect_s all;
    Check(SelotapeVista::AutoSplatVista(&repainted, quick, SelotapeVista::FourLayerMap(), all, &error),
          "AutoSplatVista over everything: " + error);
    uint32_t sameTop = 0;

    for (size_t i = 0; i < t.splat.size(); ++i) {
      uint32_t topA = 0, topB = 0;

      for (uint32_t k = 1; k < 4; ++k) {
        topA = Layer(t.splat[i], k) > Layer(t.splat[i], topA) ? k : topA;
        topB = Layer(repainted.splat[i], k) > Layer(repainted.splat[i], topB) ? k : topB;
      }

      sameTop += topA == topB;
    }

    const double agreement = double(sameTop) / double(t.splat.size());
    std::printf("AutoSplatVista on unsculpted ground agrees with Generate on %.1f%% of samples\n", 100.0 * agreement);
    Check(agreement > 0.8, "AutoSplatVista agrees with Generate's own painting on the same ground");
  }

  // PrepareApron: the rim meets a flat generator at the edge's height.
  {
    SelotapeTerrain::Terrain_s apron = t;
    const float height = SelotapeVista::PrepareApron(&apron, 40.0f);
    float worst = 0.0f;

    for (uint32_t x = 0; x < apron.samplesX; ++x) {
      worst = std::max(worst, std::fabs(apron.heights[apron.Index(x, 0)] - height));
      worst = std::max(worst, std::fabs(apron.heights[apron.Index(x, apron.samplesZ - 1)] - height));
    }

    Check(worst < 0.01f, "the faded rim sits at the apron's height");
    Check(apron.gen.amplitude == 0.0f && apron.gen.baseHeight == height, "the apron's generator is a flat plain");
    Check(apron.heights[apron.Index(128, 128)] == t.heights[t.Index(128, 128)], "the middle is untouched");
  }

  // TreePlacements: mapped, thinned, spaced, deterministic.
  {
    SelotapeVista::SpeciesMap_s species;

    for (uint32_t s = 0; s < 8; ++s) {
      species.species[s].templateName = s == 7 ? "" : "tree_" + std::to_string(s);
      species.species[s].baseScale = 2.0f;
    }

    SelotapeVista::TreeFilter_s filter;
    filter.maxCount = 150;
    filter.minSpacing = 6.0f;
    const auto placed = SelotapeVista::TreePlacements(base.trees, species, filter);
    const auto again = SelotapeVista::TreePlacements(base.trees, species, filter);
    Check(!placed.empty() && placed.size() <= 150, "TreePlacements keeps to maxCount");
    Check(placed.size() == again.size(), "TreePlacements is deterministic");
    bool same = placed.size() == again.size(), spaced = true, mapped = true;

    for (size_t i = 0; i < placed.size(); ++i) {
      same = same && placed[i].pos[0] == again[i].pos[0] && placed[i].pos[2] == again[i].pos[2];
      mapped = mapped && placed[i].species != 7 && placed[i].templateName == "tree_" + std::to_string(placed[i].species);

      for (size_t j = 0; j < i; ++j) {
        const float dx = placed[i].pos[0] - placed[j].pos[0], dz = placed[i].pos[2] - placed[j].pos[2];
        spaced = spaced && dx * dx + dz * dz >= 36.0f;
      }
    }

    Check(same, "TreePlacements places the same props twice");
    Check(spaced, "TreePlacements keeps minSpacing");
    Check(mapped, "TreePlacements maps species and skips unmapped ones");

    // Spread over the map: each quarter holds some of them.
    uint32_t quarters[4] = {};

    for (const auto& p : placed) {
      quarters[(p.pos[0] >= 0.0f ? 1 : 0) + (p.pos[2] >= 0.0f ? 2 : 0)] += 1;
    }

    Check(quarters[0] && quarters[1] && quarters[2] && quarters[3], "a thinned forest still covers the map");

    SelotapeVista::TreeFilter_s corner;
    corner.region[0] = 0.0f;
    corner.region[1] = 0.0f;
    corner.region[2] = 256.0f;
    corner.region[3] = 256.0f;
    bool inside = true;

    for (const auto& p : SelotapeVista::TreePlacements(base.trees, species, corner)) {
      inside = inside && p.pos[0] >= 0.0f && p.pos[2] >= 0.0f;
    }

    Check(inside, "TreePlacements keeps to its region");
  }
}

}  // namespace

int main(int argc, char** argv) {
  const bool large = argc > 1 && std::string(argv[1]) == "--large";

  // The meta block round-trips, JSON included.
  SelotapeVista::Settings_s settings;
  settings.seed = 77;
  settings.landform = "alpine";
  settings.extraJson = "{ \"noise\": {\n \"octaves\": 8 } }";
  SelotapeVista::Settings_s back;
  std::string error;
  const std::string meta = "materials = proc:desert\n" + SelotapeVista::MetaLines(settings);
  Check(SelotapeVista::IsVistaMeta(meta), "IsVistaMeta");
  Check(!SelotapeVista::IsVistaMeta("materials = proc:desert\n"), "IsVistaMeta on a plain .ter");
  Check(SelotapeVista::ParseMetaLines(meta, &back, &error), "ParseMetaLines: " + error);
  Check(back.seed == 77 && back.landform == "alpine" && back.extraJson.find("octaves") != std::string::npos,
        "meta round trip");
  Check(!SelotapeVista::ParseMetaLines("vista.seed = many\n", &back, &error), "a bad number is refused");

  // Bad settings are refused with a reason, before any work.
  SelotapeVista::Result_s unused;
  SelotapeVista::Settings_s bad;
  bad.spacing = 1.5f;
  Check(!SelotapeVista::Generate(bad, "", SelotapeVista::FourLayerMap(), &unused, &error) &&
          error.find("spacing") != std::string::npos,
        "fractional spacing refused");
  bad = SelotapeVista::Settings_s();
  bad.landform = "alpine\", \"seed\": 2";
  Check(!SelotapeVista::Generate(bad, "", SelotapeVista::FourLayerMap(), &unused, &error), "a quoted name refused");
  bad = SelotapeVista::Settings_s();
  bad.landform = "swamp";
  Check(!SelotapeVista::Generate(bad, "", SelotapeVista::FourLayerMap(), &unused, &error) &&
          error.find("swamp") != std::string::npos,
        "an unknown landform refused by Vista: " + error);

  // The editor's sizes: 512 m at 2 m for a quick check, then 1 km at 1 m
  // and 2 m with each layer map.
  SelotapeVista::Settings_s quick;
  quick.sizeMetres = 512.0f;
  quick.spacing = 2.0f;
  quick.erosionQuality = "preview";
  GenerateAndCheck(quick, SelotapeVista::FourLayerMap(), "512 m at 2 m, 4 layers");

  // Resampled up: Vista's 256 onto 1025.
  SelotapeVista::Settings_s upsampled = quick;
  upsampled.vistaSamples = 256;
  upsampled.sizeMetres = 1024.0f;
  upsampled.spacing = 1.0f;
  CheckResamplingMatchesVista(upsampled);

  SelotapeVista::Settings_s km;
  km.seed = 4242;
  km.sizeMetres = 1024.0f;
  km.spacing = 1.0f;
  km.landform = "alpine";
  GenerateAndCheck(km, SelotapeVista::EightLayerMap(), "1 km at 1 m, alpine, 8 layers");

  // A landform shapes the land; the climate is the biomes'. A desert wants
  // both, as docs/world-design-guide.md's recipe gives them.
  km.spacing = 2.0f;
  km.landform = "mesaDesert";
  km.biomesJson = "{ \"temperatureBias\": 0.8, \"moistureBias\": -0.8 }";
  const LayerShares desert = GenerateAndCheck(km, SelotapeVista::FourLayerMap(), "1 km at 2 m, mesa desert, 4 layers");
  Check(desert.share[0] > desert.share[1], "a hot, dry desert is mostly dry ground (layer 0), not grass (layer 1)");

  CheckEditorHelpers();
  km.biomesJson.clear();
  km.landform = "continental";
  km.spacing = 1.0f;
  MeasureStages(km);

  if (large) {
    SelotapeVista::Settings_s big;
    big.sizeMetres = 4096.0f;
    big.spacing = 1.0f;
    GenerateAndCheck(big, SelotapeVista::EightLayerMap(), "4 km at 1 m, 8 layers");
  }

  if (g_failures != 0) {
    std::fprintf(stderr, "%d failures\n", g_failures);
    return 1;
  }

  std::printf("ok\n");
  return 0;
}
