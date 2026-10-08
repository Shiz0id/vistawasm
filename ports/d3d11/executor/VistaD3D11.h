// VistaD3D11.h: runs VistaWASM's Direct3D 11 command streams on a host's
// device, and drives a VistaWASM engine's renderer with them.
//
// vista_native lowers the browser build's renderer to Direct3D 11 calls,
// one record each (vista_d3d11.h). Executor_c runs those records on the
// host's ID3D11Device and immediate context, into the host's render
// target. Renderer_c ties it to an engine: one call per frame draws the
// engine's scene into a texture of the host's.
//
// Feature level 11.0. Everything runs on the thread that owns the
// immediate context.
//
// Licence: AGPL-3.0-only, as VistaWASM.

#pragma once

#include <d3d11.h>

#include <cstddef>
#include <cstdint>
#include <functional>
#include <string>
#include <unordered_map>
#include <vector>

#include "vista_native.h"

namespace VistaD3D11 {

// A COM pointer: releases what it holds.
template <typename T>
class Com_c {
public:
  Com_c() = default;
  explicit Com_c(T* pointer) : m_pointer(pointer) {}
  Com_c(const Com_c& other) : m_pointer(other.m_pointer) {
    if (m_pointer) {
      m_pointer->AddRef();
    }
  }
  Com_c(Com_c&& other) noexcept : m_pointer(other.m_pointer) { other.m_pointer = nullptr; }
  Com_c& operator=(Com_c other) {
    std::swap(m_pointer, other.m_pointer);
    return *this;
  }
  ~Com_c() {
    if (m_pointer) {
      m_pointer->Release();
    }
  }
  T* Get() const { return m_pointer; }
  T* operator->() const { return m_pointer; }
  // For Create* out-parameters: releases what was held first.
  T** Put() {
    *this = Com_c();
    return &m_pointer;
  }
  explicit operator bool() const { return m_pointer != nullptr; }

private:
  T* m_pointer = nullptr;
};

// A buffer's bytes a stream asked to read back, ready.
struct Read_s {
  uint32_t buffer = 0;
  std::vector<uint8_t> bytes;
};

// Runs command streams. Not copyable. Destroying it releases every object
// the streams made.
class Executor_c {
public:
  Executor_c(ID3D11Device* device, ID3D11DeviceContext* context);
  Executor_c(const Executor_c&) = delete;
  Executor_c& operator=(const Executor_c&) = delete;

  // The texture the streams draw into (VISTA_D3D_OUTPUT), for the streams
  // that follow: a render target in the format the renderer was attached
  // with, as big as the renderer was told. A swap chain's back buffer
  // will do.
  void SetOutput(ID3D11Texture2D* target);

  // Run one stream. False if any record failed; Errors() says why. The
  // context is left cleared (ClearState): set the host's own state after.
  bool Run(const uint8_t* data, size_t bytes);

  // Read-backs that have arrived since the last call. The GPU is never
  // waited for: a read arrives a frame or two after its stream ran.
  std::vector<Read_s> TakeReads();

  // What failed since the last call.
  std::vector<std::string> TakeErrors();

  // Frames the streams finished (VISTA_D3D_PRESENT records), in all.
  uint64_t Presents() const { return m_presents; }

  // Whether the device was removed: a new device and a new renderer are
  // needed.
  bool DeviceLost() const { return m_lost; }

private:
  struct Object_s {
    Com_c<IUnknown> object;
    // A vertex shader's bytecode, for input layouts.
    std::vector<uint8_t> bytecode;
    // A buffer's size and whether it is a constant buffer.
    uint32_t size = 0;
    bool constant = false;
  };

  struct PendingRead_s {
    uint32_t buffer = 0;
    Com_c<ID3D11Buffer> staging;
  };

  bool Record(uint32_t op, const uint32_t* words, size_t count, const uint8_t* payload, size_t bytes);
  void Fail(const std::string& message);
  void Create(uint32_t id, IUnknown* object);
  void Create(uint32_t id, IUnknown* object, Object_s&& extra);
  ID3D11Resource* Resource(uint32_t id);
  template <typename T> T* As(uint32_t id);
  void UnbindAll();

  Com_c<ID3D11Device> m_device;
  Com_c<ID3D11DeviceContext> m_context;
  Com_c<ID3D11Texture2D> m_output;
  std::unordered_map<uint32_t, Object_s> m_objects;
  std::vector<PendingRead_s> m_reads;
  std::vector<std::string> m_errors;
  // Zeros, for initial contents: WebGPU starts every resource at zero.
  std::vector<uint8_t> m_zeros;
  uint64_t m_presents = 0;
  bool m_lost = false;
};

// An engine's renderer on a host's device: attaches it, and each Frame()
// draws the engine's scene into the host's texture. The engine stays the
// host's: generate terrain, set the camera and options through
// vista_native.h as usual.
class Renderer_c {
public:
  // Attach a renderer to `engine` drawing `width` x `height` in `format`.
  // Check Ok() after.
  Renderer_c(VistaEngine* engine, ID3D11Device* device, ID3D11DeviceContext* context, uint32_t width,
              uint32_t height, VistaOutputFormat format);
  // Detaches the renderer.
  ~Renderer_c();
  Renderer_c(const Renderer_c&) = delete;
  Renderer_c& operator=(const Renderer_c&) = delete;

  bool Ok() const { return m_attached; }

  // Draw the scene at `now_ms` (the host's clock in milliseconds) into
  // `target`. False with the reason in *error. Uploads recorded since the
  // last frame (a new terrain, changed options) run first.
  bool Frame(double now_ms, ID3D11Texture2D* target, std::string* error);

  // Run what was recorded since the last frame now, without drawing:
  // after generating a terrain, say, so the next frame is not slow.
  bool Flush(ID3D11Texture2D* target, std::string* error);

  // Resize: `target` must be this size from the next frame.
  bool Resize(uint32_t width, uint32_t height, std::string* error);

  Executor_c& Executor() { return m_executor; }

private:
  bool Run(const VistaCommands& commands, ID3D11Texture2D* target, std::string* error);

  VistaEngine* m_engine = nullptr;
  Executor_c m_executor;
  bool m_attached = false;
};

}  // namespace VistaD3D11
