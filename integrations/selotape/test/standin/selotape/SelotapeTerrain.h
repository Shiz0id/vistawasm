// Test stand-in for Selotape's SelotapeTerrain.h, written for this test
// from the interface SelotapeVista uses: the Terrain_s fields it fills,
// Rect_s, PackLayerWeights and FadeEdges. It is not Selotape's header. Pass
// Selotape's include directory to run.sh to build against the real one.
#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace SelotapeTerrain {

constexpr uint32_t k_maxQuadsPerSide = 8192;
constexpr float k_maxSpacing = 64.0f;
constexpr uint32_t k_layers = 8;
constexpr uint32_t k_shapeNoise = 0;

struct GenSettings_s {
  uint32_t seed = 1;
  float sizeMetres = 1024.0f;
  float spacing = 2.0f;
  float baseHeight = 0.0f;
  float amplitude = 60.0f;
  float warp = 0.0f;
  uint32_t thermalIterations = 0;
  uint32_t droplets = 0;
  float flatRadius = 0.0f;
  uint32_t shape = 0;
};

struct SplatRule_s {
  bool enabled = false;
};

struct Terrain_s {
  uint32_t samplesX = 0;
  uint32_t samplesZ = 0;
  float spacing = 2.0f;
  float originX = 0.0f;
  float originZ = 0.0f;
  std::vector<float> heights;
  std::vector<uint64_t> splat;
  std::string materials;
  GenSettings_s gen;
  SplatRule_s rules[k_layers];

  size_t Index(uint32_t xi, uint32_t zi) const { return static_cast<size_t>(zi) * samplesX + xi; }
  float SizeX() const { return (samplesX - 1) * spacing; }
  float SizeZ() const { return (samplesZ - 1) * spacing; }
};

struct Rect_s {
  int32_t x0 = 0, z0 = 0, x1 = -1, z1 = -1;
  bool Empty() const { return x1 < x0 || z1 < z0; }
};

uint64_t PackLayerWeights(const float w[k_layers]);
void FadeEdges(Terrain_s* t, float width, float height);

}  // namespace SelotapeTerrain
