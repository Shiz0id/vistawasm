/*
 * vista_native.h: the C API for VistaWASM's terrain and world generation.
 *
 * Link `vista_native.lib` (static) or `vista_native.dll` (dynamic), built
 * with `cargo build -p vista_native --release`. The static library also
 * needs the system libraries `cargo rustc -p vista_native --release
 * --crate-type staticlib -- --print native-static-libs` lists; on Windows
 * these are usually ws2_32, userenv, ntdll, bcrypt and advapi32.
 *
 * The library runs the browser engine's core without a GPU: terrain
 * generation, CPU erosion, rivers, lakes, glaciers, biomes, materials, and
 * tree and grass placement. The host draws the results.
 *
 * Coordinates: world metres, y up, with the terrain's centre at x = 0,
 * z = 0. Sample (column, row) sits at
 *   x = (column - (width - 1) / 2) * metres_per_sample
 *   z = (row - (height - 1) / 2) * metres_per_sample
 * so rows run towards +z.
 *
 * Options: JSON in the JavaScript API's shape (camelCase keys, see
 * docs/options-reference.md). Unknown keys are errors. Null or "" means
 * the defaults wherever a function allows it.
 *
 * Errors: every function returns a VistaStatus. On failure,
 * vista_last_error() explains why, in English, until the next failure on
 * the same thread.
 *
 * Memory: everything the library allocates has a matching vista_*_free().
 * Strings and buffers the host passes are only read during the call.
 *
 * Threads: an engine is not thread-safe. Use each engine from one thread
 * at a time; separate engines may run on separate threads.
 *
 * Licence: AGPL-3.0-only, as VistaWASM.
 */

#ifndef VISTA_NATIVE_H
#define VISTA_NATIVE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef enum VistaStatus {
  VISTA_OK = 0,
  /* A pointer was null, a string was not UTF-8, or a value was out of range. */
  VISTA_ERROR_INVALID_ARGUMENT = 1,
  /* The options JSON did not parse or failed validation. */
  VISTA_ERROR_OPTIONS = 2,
  /* The engine refused or failed the request. */
  VISTA_ERROR_ENGINE = 3,
  /* The engine panicked. Destroy it: its state is unknown. */
  VISTA_ERROR_PANIC = 4
} VistaStatus;

/* Map kinds for vista_engine_export_map(). Float maps hold `float`s; the
 * others hold `uint8_t`s. The map's encoding JSON gives its units, scale
 * and legend. */
typedef enum VistaMapKind {
  VISTA_MAP_HEIGHT = 0,         /* float, metres above the datum */
  VISTA_MAP_BIOME = 1,          /* uint8, VistaBiomeKind */
  VISTA_MAP_WATER = 2,          /* uint8, which water covers a sample (see legend) */
  VISTA_MAP_WATER_DEPTH = 3,    /* float, metres */
  VISTA_MAP_FLOW = 4,           /* float, upstream drainage area in km^2 */
  VISTA_MAP_DISCHARGE = 5,      /* float, mean discharge in m^3/s */
  VISTA_MAP_MATERIALS = 6,      /* uint8 x 12, weights in VistaMaterial order */
  VISTA_MAP_SLOPE = 7,          /* float, degrees */
  VISTA_MAP_NORMALS = 8,        /* float x 3, world-space unit normals */
  VISTA_MAP_OCCLUSION = 9,      /* uint8, 0 occluded to 255 open */
  VISTA_MAP_TEMPERATURE = 10,   /* float, mean annual degrees C */
  VISTA_MAP_MOISTURE = 11,      /* uint8, 0 arid to 255 saturated */
  VISTA_MAP_TREE_DENSITY = 12,  /* uint8, trees per hectare / 4 */
  VISTA_MAP_GRASS_DENSITY = 13, /* uint8, ground covered, 0 to 255 */
  VISTA_MAP_SOURCE_HEIGHT = 14  /* float, heights before rivers, lakes and ice */
} VistaMapKind;

typedef enum VistaBiomeKind {
  VISTA_BIOME_GRASSY_MEADOWS = 0,
  VISTA_BIOME_OUTER_THICKET = 1,
  VISTA_BIOME_OUTER_FOREST = 2,
  VISTA_BIOME_INNER_FOREST = 3,
  VISTA_BIOME_MOUNTAIN_FOOTHILLS = 4,
  VISTA_BIOME_MOUNTAIN_PROPER = 5,
  VISTA_BIOME_OUTER_VOLCANIC = 6,
  VISTA_BIOME_CALDERA_VOLCANIC = 7,
  VISTA_BIOME_SAVANNAH_EXPANSE = 8,
  VISTA_BIOME_COASTAL_BEACH = 9,
  VISTA_BIOME_COASTAL_ROCKY = 10,
  VISTA_BIOME_OUTER_JUNGLE = 11,
  VISTA_BIOME_INNER_JUNGLE = 12,
  VISTA_BIOME_SWAMP_WETLANDS = 13,
  VISTA_BIOME_OCEAN = 14,
  VISTA_BIOME_ALPINE_TRANSITION = 15,
  VISTA_BIOME_LOWER_SNOWY_PEAKS = 16,
  VISTA_BIOME_UPPER_SNOWY_PEAKS = 17,
  VISTA_BIOME_ICE_ARCTIC = 18
} VistaBiomeKind;

/* Channel order of VISTA_MAP_MATERIALS. */
typedef enum VistaMaterial {
  VISTA_MATERIAL_LUSH_GRASS = 0,
  VISTA_MATERIAL_DRY_GRASS = 1,
  VISTA_MATERIAL_FOREST_FLOOR = 2,
  VISTA_MATERIAL_SAND = 3,
  VISTA_MATERIAL_ROCK = 4,
  VISTA_MATERIAL_SNOW = 5,
  VISTA_MATERIAL_MUD = 6,
  VISTA_MATERIAL_VOLCANIC = 7,
  VISTA_MATERIAL_ICE = 8,
  VISTA_MATERIAL_TUNDRA = 9,
  VISTA_MATERIAL_GRAVEL = 10,
  VISTA_MATERIAL_SCREE = 11
} VistaMaterial;

/* Tree species, as stored in a tree record's species field. */
typedef enum VistaTreeSpecies {
  VISTA_TREE_OAK = 0,
  VISTA_TREE_PINE = 1,
  VISTA_TREE_SPRUCE = 2,
  VISTA_TREE_PALM = 3,
  VISTA_TREE_JUNGLE = 4,
  VISTA_TREE_CYPRESS = 5,
  VISTA_TREE_ACACIA = 6,
  VISTA_TREE_SHRUB = 7
} VistaTreeSpecies;

typedef enum VistaWaterMeshPart {
  /* River ribbons, lakes, oxbows and plunge pools (VistaWaterVertex). */
  VISTA_WATER_SURFACES = 0,
  /* Waterfall sheets and mist (VistaWaterVertex). */
  VISTA_WATER_FALLS = 1,
  /* Bank strips beside streams narrower than a sample (VistaBankVertex). */
  VISTA_WATER_BANKS = 2
} VistaWaterMeshPart;

/* Values of VistaWaterVertex.params[0], before any fraction (see below). */
#define VISTA_WATER_KIND_OCEAN 0.0f
#define VISTA_WATER_KIND_RIVER 1.0f
#define VISTA_WATER_KIND_LAKE 2.0f
#define VISTA_WATER_KIND_FALL 3.0f
#define VISTA_WATER_KIND_SPRAY 4.0f
#define VISTA_WATER_KIND_POOL 5.0f

/* One water vertex, 56 bytes, as water.wgsl reads it.
 *
 * position: world metres.
 * flow: surface current in m/s (x, z).
 * params: x kind (VISTA_WATER_KIND_*; rivers add how far into a step-pool
 *   the vertex lies, 0 to 0.98, so take floor() for the kind); y across
 *   the channel, -1 to 1, for rivers and falls; z half width on this side
 *   for rivers, impact speed for falls, sprite size for mist; w metres
 *   along the centreline for rivers.
 * extra: rivers: slope, curvature (-1 to 1), depth (m), degrees C. Lakes:
 *   unused, unused, depth, degrees C. Falls: metres down the sheet, sheet
 *   length, degrees C, height. Pools: bowl depth, fall height x discharge,
 *   unused, degrees C. Mist: fall height, discharge, degrees C, pool radius.
 * swirl: rivers' eddy strength, -1 to 1; the sign gives the bank. */
typedef struct VistaWaterVertex {
  float position[3];
  float flow[2];
  float params[4];
  float extra[4];
  float swirl;
} VistaWaterVertex;

/* One bank strip vertex, 44 bytes.
 *
 * outward: unit direction away from the water (x, z).
 * params: x where across the bank's profile (0 water's edge, 1 face's
 *   foot, 2 face's top, 3 lip's edge, 4 back of the turf); y metres along
 *   the centreline; z flow speed in m/s; w side (-1 or 1).
 * shape: x how much of a cut bank (0 shelf, 1 cut face); y stream width. */
typedef struct VistaBankVertex {
  float position[3];
  float outward[2];
  float params[4];
  float shape[2];
} VistaBankVertex;

typedef struct VistaEngine VistaEngine;

typedef struct VistaTerrainInfo {
  uint32_t width;
  uint32_t height;
  float metres_per_sample;
  float sea_level_metres;
  float min_height_metres;
  float max_height_metres;
  float mean_height_metres;
} VistaTerrainInfo;

/* An exported map. Read-only; free with vista_map_free(). */
typedef struct VistaMap {
  uint32_t width;
  uint32_t height;
  /* Values per pixel, interleaved. */
  uint32_t channels;
  /* 1: data holds floats. 0: data holds uint8_t. */
  uint32_t is_float;
  /* Row-major, width * height * channels values. */
  const void *data;
  size_t data_bytes;
  /* Units, scale, range, legend, metresPerPixel, seaLevelMetres and
   * generator, as JSON. */
  const char *encoding_json;
} VistaMap;

/* Number of floats in one tree record. */
#define VISTA_TREE_RECORD_FLOATS 10

/* Exported trees. Read-only; free with vista_trees_free().
 *
 * Each record: x, y, z (world metres, y on the ground), species
 * (VistaTreeSpecies), variant, scale, rotation (radians about y), tint,
 * dryness, and 1 for a hand-placed tree or 0 for a procedural one. */
typedef struct VistaTrees {
  const float *records;
  uint32_t count;
  uint32_t floats_per_record;
} VistaTrees;

/* An indexed triangle list. Read-only; free with vista_mesh_free(). */
typedef struct VistaMesh {
  const void *vertices;
  uint32_t vertex_count;
  uint32_t vertex_stride;
  const uint32_t *indices;
  uint32_t index_count;
} VistaMesh;

/* Called as generation advances. `phase` is "tectonics", "drainage",
 * "detail", "erosion", "finishing" or "rivers", valid only during the
 * call; `progress` runs from 0 to 1 within each phase. */
typedef void (*VistaProgressFn)(const char *phase, float progress, void *user);

/* The library's version, such as "2.0.0". */
const char *vista_version(void);

/* The last error on this thread. Never null. */
const char *vista_last_error(void);

/* Create an engine from engine options as JSON (null for the defaults).
 * Render, camera and sky options are checked but unused without a
 * renderer. */
VistaStatus vista_engine_create(const char *options_json, VistaEngine **out);

/* Destroy an engine. Null is ignored. */
void vista_engine_destroy(VistaEngine *engine);

/* Generate a seeded terrain from fractal terrain options as JSON (null for
 * the defaults), and build its world: rivers, lakes, glaciers, biomes,
 * materials, trees and grass. Erosion, when the options ask for it, runs
 * on the CPU. A failure leaves the previous terrain in place.
 * `progress` may be null. */
VistaStatus vista_engine_generate_fractal(
  VistaEngine *engine,
  const char *options_json,
  VistaProgressFn progress,
  void *user
);

/* Load a raw heightmap of "uint16", "int16" or "float32" samples.
 * options_json must give every RawHeightmapOptions field except the
 * optional byteOrder, noDataValue, seaLevelMetres and landform: width,
 * height, sampleFormat, metresPerSample and heightScaleMetres. */
VistaStatus vista_engine_load_raw_heightmap(
  VistaEngine *engine,
  const uint8_t *bytes,
  size_t length,
  const char *options_json
);

/* Load an uncompressed GeoTIFF elevation file. options_json may be null. */
VistaStatus vista_engine_load_geotiff(
  VistaEngine *engine,
  const uint8_t *bytes,
  size_t length,
  const char *options_json
);

/* Replace one group of world options and rebuild what depends on it.
 * section: "biomes", "flora", "grass", "water" or "surface". */
VistaStatus vista_engine_set(
  VistaEngine *engine,
  const char *section,
  const char *options_json
);

/* Describe the active terrain. Fails when there is none. */
VistaStatus vista_engine_terrain_info(
  const VistaEngine *engine,
  VistaTerrainInfo *out
);

/* Export a map at the terrain's size (width and height 0) or resampled to
 * width x height. kind is a VistaMapKind. */
VistaStatus vista_engine_export_map(
  const VistaEngine *engine,
  uint32_t kind,
  uint32_t width,
  uint32_t height,
  VistaMap **out
);

void vista_map_free(VistaMap *map);

/* Export trees inside region (min x, min z, max x, max z in world metres;
 * null for everywhere), at most max_count (0 for the engine's default).
 * More trees than max_count is an error: export a smaller region. */
VistaStatus vista_engine_export_trees(
  const VistaEngine *engine,
  const float *region,
  uint32_t max_count,
  VistaTrees **out
);

void vista_trees_free(VistaTrees *trees);

/* Copy one part of the water's geometry. part is a VistaWaterMeshPart.
 * The mesh may be empty. The ocean is not a mesh: draw it as a grid at
 * sea level. */
VistaStatus vista_engine_water_mesh(
  const VistaEngine *engine,
  uint32_t part,
  VistaMesh **out
);

void vista_mesh_free(VistaMesh *mesh);

/* The biome at world x, z. Fails outside the terrain. */
VistaStatus vista_engine_biome_at(
  const VistaEngine *engine,
  float x,
  float z,
  uint8_t *out
);

/* Part of the world as JSON. what: "metadata", "waterfalls" or "inflows".
 * Free the string with vista_string_free(). */
VistaStatus vista_engine_query_json(
  const VistaEngine *engine,
  const char *what,
  char **out
);

void vista_string_free(char *text);

#ifdef __cplusplus
}
#endif

#endif /* VISTA_NATIVE_H */
