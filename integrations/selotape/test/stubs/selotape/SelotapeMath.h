// Test stand-in for Selotape's SelotapeMath.h: only what SelotapeTerrain.h
// names. The editor build uses the real header.
#pragma once

struct vec3_u {
  float x = 0.0f, y = 0.0f, z = 0.0f;
  vec3_u() = default;
  vec3_u(float x_, float y_, float z_) : x(x_), y(y_), z(z_) {}
};
