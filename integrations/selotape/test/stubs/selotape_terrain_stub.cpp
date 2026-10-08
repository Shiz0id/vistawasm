// Test stand-in for the one SelotapeTerrain.cpp function SelotapeVista
// calls, written from its declaration's comment: eight weights of any
// scale as shares rounded to sum to 255, layer 0 in the low byte; all zero
// gives the first layer alone. The editor build links the real one.
#include "selotape/SelotapeTerrain.h"

#include <cmath>

namespace SelotapeTerrain {

uint64_t PackLayerWeights(const float w[k_layers]) {
  float total = 0.0f;

  for (uint32_t i = 0; i < k_layers; ++i) {
    total += w[i] > 0.0f ? w[i] : 0.0f;
  }

  if (total <= 0.0f) {
    return 255u;
  }

  uint32_t bytes[k_layers] = {};
  uint32_t sum = 0, largest = 0;

  for (uint32_t i = 0; i < k_layers; ++i) {
    bytes[i] = static_cast<uint32_t>(std::lround((w[i] > 0.0f ? w[i] : 0.0f) / total * 255.0f));
    sum += bytes[i];
    largest = bytes[i] > bytes[largest] ? i : largest;
  }

  bytes[largest] = bytes[largest] + 255u - sum;
  uint64_t packed = 0;

  for (uint32_t i = 0; i < k_layers; ++i) {
    packed |= static_cast<uint64_t>(bytes[i] & 0xffu) << (8 * i);
  }

  return packed;
}

// From its declaration's comment: "Blends the outer `width` metres down to
// `height`". Smoothstep from the edge inwards.
void FadeEdges(Terrain_s* t, float width, float height) {
  for (uint32_t z = 0; z < t->samplesZ; ++z) {
    for (uint32_t x = 0; x < t->samplesX; ++x) {
      const float fromEdge = std::fmin(std::fmin(float(x), float(t->samplesX - 1 - x)),
                                       std::fmin(float(z), float(t->samplesZ - 1 - z))) * t->spacing;

      if (fromEdge < width) {
        const float k = fromEdge / width;
        const float blend = k * k * (3.0f - 2.0f * k);
        float& h = t->heights[t->Index(x, z)];
        h = height + (h - height) * blend;
      }
    }
  }
}

}  // namespace SelotapeTerrain
