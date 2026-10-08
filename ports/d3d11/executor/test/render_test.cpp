// render_test: draws a VistaWASM scene with VistaD3D11 on a real Direct3D
// 11 device and saves it as a PNG, to compare with the browser build's
// frame of the same scene (scripts/visual-check/capture.mjs).
//
//   render_test.exe OUT.png [WIDTH HEIGHT FRAMES [TERRAIN_JSON [CAMERA_JSON
//                   [SECTION=JSON ...]]]]
//
// The defaults are the browser visual check's default scene. Each
// SECTION=JSON sets one option group (vista_engine_set) after the terrain
// is generated, such as 'surface={"boulders":false}'. A TERRAIN_JSON of
// raw:FILE:OPTIONS_JSON loads FILE (uncompressed) with
// vista_engine_load_raw_heightmap instead of generating a terrain.
// RENDER_TEST_FRAME_MS sets the clock's step between frames (default a
// sixtieth of a second): the browser check's software frames take about
// a second each, which weather transitions notice. It prints the
// frame's statistics and exits non-zero if the executor or the engine
// reported an error. ports/d3d11/executor/test/run.sh builds it with MinGW
// and runs it under Wine.
//
// Licence: AGPL-3.0-only, as VistaWASM.

#include <d3d11.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

#include "VistaD3D11.h"

namespace {

uint32_t Crc(const uint8_t* data, size_t size, uint32_t crc = 0) {
  crc = ~crc;

  for (size_t i = 0; i < size; ++i) {
    crc ^= data[i];

    for (int bit = 0; bit < 8; ++bit) {
      crc = (crc >> 1) ^ (0xedb88320u & (0u - (crc & 1u)));
    }
  }

  return ~crc;
}

void Chunk(FILE* file, const char* type, const std::vector<uint8_t>& data) {
  uint8_t header[8] = {
    uint8_t(data.size() >> 24), uint8_t(data.size() >> 16), uint8_t(data.size() >> 8), uint8_t(data.size()),
    uint8_t(type[0]), uint8_t(type[1]), uint8_t(type[2]), uint8_t(type[3]),
  };
  std::vector<uint8_t> crcData(header + 4, header + 8);
  crcData.insert(crcData.end(), data.begin(), data.end());
  uint32_t crc = Crc(crcData.data(), crcData.size());
  uint8_t tail[4] = { uint8_t(crc >> 24), uint8_t(crc >> 16), uint8_t(crc >> 8), uint8_t(crc) };
  std::fwrite(header, 1, 8, file);
  std::fwrite(data.data(), 1, data.size(), file);
  std::fwrite(tail, 1, 4, file);
}

// An RGB PNG with stored (uncompressed) deflate blocks: no zlib needed.
bool WritePng(const char* path, const uint8_t* rgba, uint32_t width, uint32_t height, uint32_t pitch) {
  FILE* file = std::fopen(path, "wb");

  if (!file) {
    return false;
  }

  std::vector<uint8_t> raw;

  for (uint32_t y = 0; y < height; ++y) {
    raw.push_back(0);

    for (uint32_t x = 0; x < width; ++x) {
      const uint8_t* texel = rgba + size_t(y) * pitch + x * 4;
      raw.insert(raw.end(), texel, texel + 3);
    }
  }

  std::vector<uint8_t> zlib = { 0x78, 0x01 };
  uint32_t a = 1, b = 0;

  for (uint8_t byte : raw) {
    a = (a + byte) % 65521;
    b = (b + a) % 65521;
  }

  for (size_t at = 0; at < raw.size() || at == 0; at += 65535) {
    size_t length = std::min<size_t>(65535, raw.size() - at);
    bool last = at + length >= raw.size();
    zlib.push_back(last ? 1 : 0);
    zlib.push_back(uint8_t(length));
    zlib.push_back(uint8_t(length >> 8));
    zlib.push_back(uint8_t(~length));
    zlib.push_back(uint8_t(~length >> 8));
    zlib.insert(zlib.end(), raw.begin() + at, raw.begin() + at + length);

    if (last) {
      break;
    }
  }

  uint32_t adler = (b << 16) | a;
  zlib.insert(zlib.end(), { uint8_t(adler >> 24), uint8_t(adler >> 16), uint8_t(adler >> 8), uint8_t(adler) });
  static const uint8_t signature[8] = { 0x89, 'P', 'N', 'G', 0x0d, 0x0a, 0x1a, 0x0a };
  std::fwrite(signature, 1, 8, file);
  Chunk(file, "IHDR", {
    uint8_t(width >> 24), uint8_t(width >> 16), uint8_t(width >> 8), uint8_t(width),
    uint8_t(height >> 24), uint8_t(height >> 16), uint8_t(height >> 8), uint8_t(height),
    8, 2, 0, 0, 0,
  });
  Chunk(file, "IDAT", zlib);
  Chunk(file, "IEND", {});
  return std::fclose(file) == 0;
}

int Fail(const std::string& message) {
  std::fprintf(stderr, "render_test: %s\n", message.c_str());
  return 1;
}

std::string LastError() {
  return vista_last_error();
}

}  // namespace

int main(int argc, char** argv) {
  if (argc < 2) {
    return Fail("usage: render_test.exe OUT.png [WIDTH HEIGHT FRAMES [TERRAIN_JSON [CAMERA_JSON [SECTION=JSON ...]]]]");
  }

  const uint32_t width = argc > 3 ? uint32_t(std::atoi(argv[2])) : 960;
  const uint32_t height = argc > 3 ? uint32_t(std::atoi(argv[3])) : 600;
  const int frames = argc > 4 ? std::atoi(argv[4]) : 3;
  // The browser visual check's defaults (scripts/visual-check/scenes.mjs).
  const char* terrain = argc > 5 ? argv[5]
    : R"({ "seed": 12345, "size": 512, "horizontalScaleMetres": 12, "verticalScale": 1,
          "seaLevelMetres": 0,
          "noise": { "kind": "ridged", "octaves": 7, "gain": 0.52, "lacunarity": 2.05, "warp": 0.15 },
          "shape": { "island": 0.35 } })";
  const char* camera = argc > 6 ? argv[6]
    : R"({ "position": [0, 420, 900], "target": [0, 300, 0], "fieldOfViewDegrees": 55,
          "nearMetres": 0.5, "farMetres": 120000 })";

  ID3D11Device* device = nullptr;
  ID3D11DeviceContext* context = nullptr;
  const D3D_FEATURE_LEVEL wanted = D3D_FEATURE_LEVEL_11_0;
  D3D_FEATURE_LEVEL level;
  HRESULT result = D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, 0, &wanted, 1,
                                      D3D11_SDK_VERSION, &device, &level, &context);

  if (FAILED(result)) {
    return Fail("no feature level 11.0 device");
  }

  VistaD3D11::Com_c<ID3D11Device> deviceHolder(device);
  VistaD3D11::Com_c<ID3D11DeviceContext> contextHolder(context);
  D3D11_TEXTURE2D_DESC desc = {};
  desc.Width = width;
  desc.Height = height;
  desc.MipLevels = 1;
  desc.ArraySize = 1;
  desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
  desc.SampleDesc.Count = 1;
  desc.Usage = D3D11_USAGE_DEFAULT;
  desc.BindFlags = D3D11_BIND_RENDER_TARGET;
  VistaD3D11::Com_c<ID3D11Texture2D> target;

  if (FAILED(device->CreateTexture2D(&desc, nullptr, target.Put()))) {
    return Fail("could not create the render target");
  }

  std::string engineJson = "{ \"render\": { \"width\": " + std::to_string(width) + ", \"height\": " +
    std::to_string(height) + ", \"devicePixelRatio\": 1 } }";
  VistaEngine* engine = nullptr;

  if (vista_engine_create(engineJson.c_str(), &engine) != VISTA_OK) {
    return Fail("vista_engine_create: " + LastError());
  }

  int status = 0;
  {
    VistaD3D11::Renderer_c renderer(engine, device, context, width, height, VISTA_OUTPUT_RGBA8);

    if (!renderer.Ok()) {
      return Fail("vista_renderer_attach: " + LastError());
    }

    std::string source = terrain;

    if (source.rfind("raw:", 0) == 0) {
      size_t colon = source.find(':', 4);
      std::string path = source.substr(4, colon - 4);
      FILE* file = std::fopen(path.c_str(), "rb");

      if (!file || colon == std::string::npos) {
        return Fail("could not open " + path);
      }

      std::vector<uint8_t> bytes;
      uint8_t chunk[65536];

      for (size_t read; (read = std::fread(chunk, 1, sizeof(chunk), file)) > 0;) {
        bytes.insert(bytes.end(), chunk, chunk + read);
      }

      std::fclose(file);

      if (vista_engine_load_raw_heightmap(engine, bytes.data(), bytes.size(), source.c_str() + colon + 1) != VISTA_OK) {
        return Fail("vista_engine_load_raw_heightmap: " + LastError());
      }
    } else if (vista_engine_generate_fractal(engine, terrain, nullptr, nullptr) != VISTA_OK) {
      return Fail("vista_engine_generate_fractal: " + LastError());
    }

    if (vista_engine_set(engine, "camera", camera) != VISTA_OK) {
      return Fail("camera: " + LastError());
    }

    for (int arg = 7; arg < argc; ++arg) {
      std::string setting = argv[arg];
      size_t equals = setting.find('=');

      if (equals == std::string::npos ||
          vista_engine_set(engine, setting.substr(0, equals).c_str(), setting.c_str() + equals + 1) != VISTA_OK) {
        return Fail(setting + ": " + LastError());
      }
    }

    std::string error;
    const char* step = std::getenv("RENDER_TEST_FRAME_MS");
    const double frameMs = step ? std::atof(step) : 1000.0 / 60.0;

    for (int frame = 0; frame < frames; ++frame) {
      // Let the GPU finish each frame, as the browser check waits for it.
      if (!renderer.Frame(frame * frameMs, target.Get(), &error)) {
        std::fprintf(stderr, "frame %d: %s\n", frame, error.c_str());
        status = 1;
        error.clear();
      }

      context->Flush();
    }

    std::printf("presents %llu\n", static_cast<unsigned long long>(renderer.Executor().Presents()));
    char* stats = nullptr;

    if (vista_engine_stats_json(engine, &stats) == VISTA_OK) {
      std::printf("stats %s\n", stats);
      vista_string_free(stats);
    }

    char* events = nullptr;

    if (vista_engine_events_json(engine, &events) == VISTA_OK) {
      std::printf("events %s\n", events);

      if (std::strcmp(events, "{\"errors\":[],\"lost\":null}") != 0) {
        status = 1;
      }

      vista_string_free(events);
    }
  }

  desc.Usage = D3D11_USAGE_STAGING;
  desc.BindFlags = 0;
  desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
  VistaD3D11::Com_c<ID3D11Texture2D> staging;

  if (FAILED(device->CreateTexture2D(&desc, nullptr, staging.Put()))) {
    return Fail("could not create the read-back texture");
  }

  context->CopyResource(staging.Get(), target.Get());
  D3D11_MAPPED_SUBRESOURCE mapped = {};

  if (FAILED(context->Map(staging.Get(), 0, D3D11_MAP_READ, 0, &mapped))) {
    return Fail("could not read the frame back");
  }

  bool written = WritePng(argv[1], static_cast<const uint8_t*>(mapped.pData), width, height, mapped.RowPitch);
  context->Unmap(staging.Get(), 0);
  vista_engine_destroy(engine);
  return written ? status : Fail("could not write the PNG");
}
