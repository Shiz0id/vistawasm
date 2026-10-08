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
