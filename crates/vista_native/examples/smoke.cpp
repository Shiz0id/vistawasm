// A C++ smoke test for vista_native: generate an eroded island, read
// back every kind of result, and write the heightmap as a 16-bit PGM a
// person can open.
//
// Build and run it with crates/vista_native/examples/run-smoke.sh.

#include "vista_native.h"

#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

static_assert(sizeof(VistaWaterVertex) == 56, "VistaWaterVertex must match water.wgsl");
static_assert(sizeof(VistaBankVertex) == 44, "VistaBankVertex must match water.wgsl");

namespace {

// Report a failed call with the library's own explanation, and stop.
void check(VistaStatus status, const char *what) {
  if (status != VISTA_OK) {
    std::fprintf(stderr, "%s failed (%d): %s\n", what, static_cast<int>(status), vista_last_error());
    std::exit(1);
  }
}

struct EngineDeleter {
  void operator()(VistaEngine *engine) const { vista_engine_destroy(engine); }
};

struct MapDeleter {
  void operator()(VistaMap *map) const { vista_map_free(map); }
};

using EnginePtr = std::unique_ptr<VistaEngine, EngineDeleter>;
using MapPtr = std::unique_ptr<VistaMap, MapDeleter>;

MapPtr export_map(const VistaEngine *engine, VistaMapKind kind) {
  VistaMap *map = nullptr;
  check(vista_engine_export_map(engine, kind, 0, 0, &map), "vista_engine_export_map");
  return MapPtr(map);
}

int on_progress(const char *phase, float progress, void *user) {
  auto *last = static_cast<std::string *>(user);

  if (*last != phase) {
    std::printf("  %s\n", phase);
    *last = phase;
  }

  (void)progress;
  return 0;  // go on
}

}  // namespace

int main(int argc, char **argv) {
  const char *pgm_path = argc > 1 ? argv[1] : "vista-height.pgm";
  std::printf("vista_native %s\n", vista_version());

  VistaEngine *raw = nullptr;
  check(vista_engine_create(nullptr, &raw), "vista_engine_create");
  EnginePtr engine(raw);

  // A bad option must fail cleanly and leave the engine usable.
  VistaStatus bad = vista_engine_generate_fractal(engine.get(), "{\"seed\": 1, \"notAnOption\": 2}", nullptr, nullptr);

  if (bad != VISTA_ERROR_OPTIONS) {
    std::fprintf(stderr, "expected VISTA_ERROR_OPTIONS for an unknown key, got %d\n", static_cast<int>(bad));
    return 1;
  }

  std::printf("unknown option rejected: %s\n", vista_last_error());

  const char *options =
    "{"
    "  \"seed\": 4242,"
    "  \"size\": 256,"
    "  \"horizontalScaleMetres\": 30,"
    "  \"shape\": { \"island\": 0.5 },"
    "  \"erosion\": {}"
    "}";
  std::string last_phase;
  check(vista_engine_generate_fractal(engine.get(), options, on_progress, &last_phase), "vista_engine_generate_fractal");

  VistaTerrainInfo info{};
  check(vista_engine_terrain_info(engine.get(), &info), "vista_engine_terrain_info");
  std::printf(
    "terrain %ux%u, %.1f m per sample, heights %.1f to %.1f m, sea level %.1f m\n",
    info.width, info.height, info.metres_per_sample, info.min_height_metres,
    info.max_height_metres, info.sea_level_metres
  );

  MapPtr height = export_map(engine.get(), VISTA_MAP_HEIGHT);
  MapPtr biome = export_map(engine.get(), VISTA_MAP_BIOME);
  MapPtr materials = export_map(engine.get(), VISTA_MAP_MATERIALS);

  if (!height->is_float || height->channels != 1 || height->width != info.width) {
    std::fprintf(stderr, "the height map has the wrong shape\n");
    return 1;
  }

  if (biome->is_float || materials->channels != 12) {
    std::fprintf(stderr, "the biome or material map has the wrong shape\n");
    return 1;
  }

  // Count each biome, to show the classification ran.
  unsigned counts[19] = {};
  auto *biomes = static_cast<const uint8_t *>(biome->data);

  for (size_t i = 0; i < biome->data_bytes; i += 1) {
    if (biomes[i] < 19) {
      counts[biomes[i]] += 1;
    }
  }

  std::printf("biomes present:");

  for (int kind = 0; kind < 19; kind += 1) {
    if (counts[kind] > 0) {
      std::printf(" %d", kind);
    }
  }

  std::printf("\nmaterial encoding: %.120s...\n", materials->encoding_json);

  VistaTrees *trees = nullptr;
  check(vista_engine_export_trees(engine.get(), nullptr, 1000000, &trees), "vista_engine_export_trees");
  std::printf("trees: %u\n", trees->count);
  vista_trees_free(trees);

  for (uint32_t part = VISTA_WATER_SURFACES; part <= VISTA_WATER_BANKS; part += 1) {
    VistaMesh *mesh = nullptr;
    check(vista_engine_water_mesh(engine.get(), part, &mesh), "vista_engine_water_mesh");
    std::printf(
      "water mesh %u: %u vertices of %u bytes, %u triangles\n",
      part, mesh->vertex_count, mesh->vertex_stride, mesh->index_count / 3
    );
    vista_mesh_free(mesh);
  }

  char *waterfalls = nullptr;
  check(vista_engine_query_json(engine.get(), "waterfalls", &waterfalls), "vista_engine_query_json");
  std::printf("waterfalls JSON: %zu bytes\n", std::strlen(waterfalls));
  vista_string_free(waterfalls);

  uint8_t centre_biome = 0;
  check(vista_engine_biome_at(engine.get(), 0.0f, 0.0f, &centre_biome), "vista_engine_biome_at");
  std::printf("biome at the centre: %u\n", centre_biome);

  // A 16-bit PGM, scaled from the lowest to the highest height.
  auto *heights = static_cast<const float *>(height->data);
  size_t count = static_cast<size_t>(height->width) * height->height;
  auto [low, high] = std::minmax_element(heights, heights + count);
  float span = std::max(*high - *low, 0.001f);
  std::vector<unsigned char> pixels(count * 2);

  for (size_t i = 0; i < count; i += 1) {
    auto value = static_cast<uint16_t>((heights[i] - *low) / span * 65535.0f);
    pixels[i * 2] = static_cast<unsigned char>(value >> 8);
    pixels[i * 2 + 1] = static_cast<unsigned char>(value & 0xff);
  }

  FILE *file = std::fopen(pgm_path, "wb");

  if (!file) {
    std::fprintf(stderr, "could not write %s\n", pgm_path);
    return 1;
  }

  std::fprintf(file, "P5\n%u %u\n65535\n", height->width, height->height);
  std::fwrite(pixels.data(), 1, pixels.size(), file);
  std::fclose(file);
  std::printf("wrote %s\nok\n", pgm_path);
  return 0;
}
