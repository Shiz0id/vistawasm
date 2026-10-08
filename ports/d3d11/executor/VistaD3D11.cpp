// VistaD3D11.cpp: see VistaD3D11.h.
//
// Licence: AGPL-3.0-only, as VistaWASM.

#include "VistaD3D11.h"

#include <algorithm>
#include <cstdio>
#include <cstring>

#include "vista_d3d11.h"

namespace VistaD3D11 {

namespace {

// Slots the stream may bind, per stage (see vista_d3d11.h).
constexpr UINT k_constantSlots = 14;
constexpr UINT k_resourceSlots = 128;
constexpr UINT k_samplerSlots = 16;
constexpr UINT k_unorderedSlots = 8;
constexpr UINT k_vertexSlots = 16;

std::string Hex(uint32_t value) {
  char text[16];
  std::snprintf(text, sizeof(text), "%08x", value);
  return text;
}

// Fixed payload sizes, in 32-bit words, of the records that have one.
size_t Words(uint32_t op) {
  switch (op) {
    case VISTA_D3D_CREATE_BUFFER: return 4;
    case VISTA_D3D_CREATE_TEXTURE: return 8;
    case VISTA_D3D_CREATE_SRV:
    case VISTA_D3D_CREATE_UAV:
    case VISTA_D3D_CREATE_RTV:
    case VISTA_D3D_CREATE_DSV: return 8;
    case VISTA_D3D_CREATE_SAMPLER: return 9;
    case VISTA_D3D_CREATE_SHADER: return 3;
    case VISTA_D3D_CREATE_INPUT_LAYOUT: return 3;
    case VISTA_D3D_CREATE_BLEND: return 1 + 8 * 8;
    case VISTA_D3D_CREATE_RASTERIZER: return 7;
    case VISTA_D3D_CREATE_DEPTH: return 4;
    case VISTA_D3D_RELEASE: return 1;
    case VISTA_D3D_UPDATE_BUFFER: return 3;
    case VISTA_D3D_UPDATE_TEXTURE: return 11;
    case VISTA_D3D_COPY_BUFFER: return 5;
    case VISTA_D3D_COPY_TEXTURE: return 13;
    case VISTA_D3D_READBACK: return 2;
    case VISTA_D3D_SET_TARGETS: return 10;
    case VISTA_D3D_CLEAR_RTV: return 5;
    case VISTA_D3D_CLEAR_DSV: return 2;
    case VISTA_D3D_SET_VIEWPORT: return 6;
    case VISTA_D3D_SET_GRAPHICS: return 7;
    case VISTA_D3D_SET_COMPUTE: return 1;
    case VISTA_D3D_BIND: return 4;
    case VISTA_D3D_UNBIND_ALL: return 0;
    case VISTA_D3D_SET_VERTEX_BUFFER: return 4;
    case VISTA_D3D_SET_INDEX_BUFFER: return 3;
    case VISTA_D3D_DRAW: return 4;
    case VISTA_D3D_DRAW_INDEXED: return 5;
    case VISTA_D3D_DRAW_INDIRECT:
    case VISTA_D3D_DRAW_INDEXED_INDIRECT: return 2;
    case VISTA_D3D_DISPATCH: return 3;
    case VISTA_D3D_PRESENT: return 0;
    case VISTA_D3D_OUTPUT_SIZE: return 3;
    default: return SIZE_MAX;
  }
}

float Float(uint32_t bits) {
  float value;
  std::memcpy(&value, &bits, sizeof(value));
  return value;
}

}  // namespace

Executor_c::Executor_c(ID3D11Device* device, ID3D11DeviceContext* context) {
  device->AddRef();
  context->AddRef();
  m_device = Com_c<ID3D11Device>(device);
  m_context = Com_c<ID3D11DeviceContext>(context);
}

void Executor_c::SetOutput(ID3D11Texture2D* target) {
  if (target) {
    target->AddRef();
  }

  m_output = Com_c<ID3D11Texture2D>(target);
}

void Executor_c::Fail(const std::string& message) {
  // A broken stream can fail the same way every frame; a few say enough.
  if (m_errors.size() < 16) {
    m_errors.push_back(message);
  }
}

void Executor_c::Create(uint32_t id, IUnknown* object) {
  Create(id, object, Object_s());
}

void Executor_c::Create(uint32_t id, IUnknown* object, Object_s&& extra) {
  if (!object) {
    return;
  }

  extra.object = Com_c<IUnknown>(object);
  m_objects[id] = std::move(extra);
}

template <typename T>
T* Executor_c::As(uint32_t id) {
  if (id == 0) {
    return nullptr;
  }

  auto found = m_objects.find(id);

  if (found == m_objects.end()) {
    Fail("record uses #" + Hex(id) + ", which does not exist");
    return nullptr;
  }

  // Objects are stored as the interface they were created as.
  return static_cast<T*>(found->second.object.Get());
}

ID3D11Resource* Executor_c::Resource(uint32_t id) {
  if (id == VISTA_D3D_OUTPUT) {
    if (!m_output) {
      Fail("a stream draws to the output, but SetOutput() gave none");
    }

    return m_output.Get();
  }

  return As<ID3D11Resource>(id);
}

void Executor_c::UnbindAll() {
  ID3D11Buffer* buffers[k_vertexSlots > k_constantSlots ? k_vertexSlots : k_constantSlots] = {};
  ID3D11ShaderResourceView* views[k_resourceSlots] = {};
  ID3D11SamplerState* samplers[k_samplerSlots] = {};
  ID3D11UnorderedAccessView* unordered[k_unorderedSlots] = {};
  UINT zeros[k_vertexSlots] = {};
  m_context->VSSetConstantBuffers(0, k_constantSlots, buffers);
  m_context->PSSetConstantBuffers(0, k_constantSlots, buffers);
  m_context->CSSetConstantBuffers(0, k_constantSlots, buffers);
  m_context->VSSetShaderResources(0, k_resourceSlots, views);
  m_context->PSSetShaderResources(0, k_resourceSlots, views);
  m_context->CSSetShaderResources(0, k_resourceSlots, views);
  m_context->VSSetSamplers(0, k_samplerSlots, samplers);
  m_context->PSSetSamplers(0, k_samplerSlots, samplers);
  m_context->CSSetSamplers(0, k_samplerSlots, samplers);
  m_context->CSSetUnorderedAccessViews(0, k_unorderedSlots, unordered, nullptr);
  m_context->IASetVertexBuffers(0, k_vertexSlots, buffers, zeros, zeros);
  m_context->IASetIndexBuffer(nullptr, DXGI_FORMAT_R32_UINT, 0);
}

bool Executor_c::Run(const uint8_t* data, size_t bytes) {
  size_t errors = m_errors.size();
  size_t at = 0;

  while (at + sizeof(VistaD3DRecord) <= bytes) {
    VistaD3DRecord header;
    std::memcpy(&header, data + at, sizeof(header));
    at += sizeof(header);

    if (header.bytes % 4 != 0 || at + header.bytes > bytes) {
      Fail("a record runs past the end of the stream");
      break;
    }

    // Records are 32-bit words; copy them out so alignment never matters.
    std::vector<uint32_t> words(header.bytes / 4);
    std::memcpy(words.data(), data + at, header.bytes);
    size_t fixed = Words(header.op);

    if (fixed == SIZE_MAX || words.size() < fixed) {
      Fail("unknown or short record " + std::to_string(header.op));
    } else {
      Record(header.op, words.data(), words.size(), data + at + fixed * 4, header.bytes - fixed * 4);
    }

    at += header.bytes;
  }

  m_context->ClearState();
  HRESULT removed = m_device->GetDeviceRemovedReason();

  if (FAILED(removed)) {
    m_lost = true;
    Fail("the device was removed (" + Hex(static_cast<uint32_t>(removed)) + ")");
  }

  return m_errors.size() == errors;
}

bool Executor_c::Record(uint32_t op, const uint32_t* w, size_t count, const uint8_t* payload, size_t bytes) {
  HRESULT result = S_OK;
  auto check = [&](const char* what, uint32_t id) {
    if (FAILED(result)) {
      Fail(std::string(what) + " #" + Hex(id) + " failed (" + Hex(static_cast<uint32_t>(result)) + ")");
      return false;
    }

    return true;
  };

  switch (op) {
    case VISTA_D3D_CREATE_BUFFER: {
      D3D11_BUFFER_DESC desc = {};
      desc.ByteWidth = w[1];
      desc.Usage = D3D11_USAGE_DEFAULT;
      desc.BindFlags = w[2];
      desc.MiscFlags = w[3];

      if (m_zeros.size() < desc.ByteWidth) {
        m_zeros.resize(desc.ByteWidth);
      }

      D3D11_SUBRESOURCE_DATA zeros = { m_zeros.data(), desc.ByteWidth, 0 };
      ID3D11Buffer* buffer = nullptr;
      result = m_device->CreateBuffer(&desc, &zeros, &buffer);

      if (check("CreateBuffer", w[0])) {
        Object_s extra;
        extra.size = w[1];
        extra.constant = (w[2] & D3D11_BIND_CONSTANT_BUFFER) != 0;
        Create(w[0], buffer, std::move(extra));
      }

      return true;
    }
    case VISTA_D3D_CREATE_TEXTURE: {
      uint32_t id = w[0], dimension = w[1], width = w[2], height = w[3], layers = w[4], mips = w[5];
      DXGI_FORMAT format = static_cast<DXGI_FORMAT>(w[6]);
      UINT bind = w[7];
      // Every texel starts at zero, as in WebGPU: render targets too, as
      // layers of the impostor atlas are read before they are drawn. Depth
      // textures are always cleared before they are read, and not every
      // driver takes initial data for them.
      bool zero = (bind & D3D11_BIND_DEPTH_STENCIL) == 0;
      UINT texel = format == DXGI_FORMAT_R16G16B16A16_FLOAT ? 8 : format == DXGI_FORMAT_R8_UNORM ? 1
        : format == DXGI_FORMAT_R8G8_UNORM ? 2 : 4;
      size_t largest = size_t(width) * height * texel * (dimension == 3 ? layers : 1);

      if (zero && m_zeros.size() < largest) {
        m_zeros.resize(largest);
      }

      std::vector<D3D11_SUBRESOURCE_DATA> initial;

      for (uint32_t layer = 0; zero && layer < (dimension == 3 ? 1 : layers); ++layer) {
        for (uint32_t mip = 0; mip < mips; ++mip) {
          UINT row = std::max(width >> mip, 1u) * texel;
          initial.push_back({ m_zeros.data(), row, row * std::max(height >> mip, 1u) });
        }
      }

      ID3D11Resource* texture = nullptr;

      if (dimension == 3) {
        D3D11_TEXTURE3D_DESC desc = { width, height, layers, mips, format, D3D11_USAGE_DEFAULT, bind, 0, 0 };
        result = m_device->CreateTexture3D(&desc, zero ? initial.data() : nullptr,
                                            reinterpret_cast<ID3D11Texture3D**>(&texture));
      } else if (dimension == 2) {
        D3D11_TEXTURE2D_DESC desc = {
          width, height, mips, layers, format, { 1, 0 }, D3D11_USAGE_DEFAULT, bind, 0, 0,
        };
        result = m_device->CreateTexture2D(&desc, zero ? initial.data() : nullptr,
                                            reinterpret_cast<ID3D11Texture2D**>(&texture));
      } else {
        D3D11_TEXTURE1D_DESC desc = { width, mips, layers, format, D3D11_USAGE_DEFAULT, bind, 0, 0 };
        result = m_device->CreateTexture1D(&desc, zero ? initial.data() : nullptr,
                                            reinterpret_cast<ID3D11Texture1D**>(&texture));
      }

      if (check("CreateTexture", id)) {
        Create(id, texture);
      }

      return true;
    }
    case VISTA_D3D_CREATE_SRV: {
      D3D11_SHADER_RESOURCE_VIEW_DESC desc = {};
      desc.Format = static_cast<DXGI_FORMAT>(w[2]);
      desc.ViewDimension = static_cast<D3D11_SRV_DIMENSION>(w[3]);

      switch (desc.ViewDimension) {
        case D3D11_SRV_DIMENSION_BUFFEREX: desc.BufferEx = { w[4], w[5], w[6] }; break;
        case D3D11_SRV_DIMENSION_TEXTURE2D: desc.Texture2D = { w[4], w[5] }; break;
        case D3D11_SRV_DIMENSION_TEXTURE2DARRAY: desc.Texture2DArray = { w[4], w[5], w[6], w[7] }; break;
        case D3D11_SRV_DIMENSION_TEXTURE3D: desc.Texture3D = { w[4], w[5] }; break;
        default: Fail("unknown view dimension for #" + Hex(w[0])); return true;
      }

      ID3D11Resource* resource = Resource(w[1]);
      ID3D11ShaderResourceView* view = nullptr;

      if (resource) {
        result = m_device->CreateShaderResourceView(resource, &desc, &view);

        if (check("CreateShaderResourceView", w[0])) {
          Create(w[0], view);
        }
      }

      return true;
    }
    case VISTA_D3D_CREATE_UAV: {
      D3D11_UNORDERED_ACCESS_VIEW_DESC desc = {};
      desc.Format = static_cast<DXGI_FORMAT>(w[2]);
      desc.ViewDimension = static_cast<D3D11_UAV_DIMENSION>(w[3]);

      switch (desc.ViewDimension) {
        case D3D11_UAV_DIMENSION_BUFFER: desc.Buffer = { w[4], w[5], w[6] }; break;
        case D3D11_UAV_DIMENSION_TEXTURE2D: desc.Texture2D = { w[4] }; break;
        case D3D11_UAV_DIMENSION_TEXTURE2DARRAY: desc.Texture2DArray = { w[4], w[5], w[6] }; break;
        case D3D11_UAV_DIMENSION_TEXTURE3D: desc.Texture3D = { w[4], w[5], w[6] }; break;
        default: Fail("unknown view dimension for #" + Hex(w[0])); return true;
      }

      ID3D11Resource* resource = Resource(w[1]);
      ID3D11UnorderedAccessView* view = nullptr;

      if (resource) {
        result = m_device->CreateUnorderedAccessView(resource, &desc, &view);

        if (check("CreateUnorderedAccessView", w[0])) {
          Create(w[0], view);
        }
      }

      return true;
    }
    case VISTA_D3D_CREATE_RTV: {
      D3D11_RENDER_TARGET_VIEW_DESC desc = {};
      desc.Format = static_cast<DXGI_FORMAT>(w[2]);
      desc.ViewDimension = static_cast<D3D11_RTV_DIMENSION>(w[3]);

      if (desc.ViewDimension == D3D11_RTV_DIMENSION_TEXTURE2D) {
        desc.Texture2D = { w[4] };
      } else {
        desc.Texture2DArray = { w[4], w[5], w[6] };
      }

      ID3D11Resource* resource = Resource(w[1]);
      ID3D11RenderTargetView* view = nullptr;

      if (resource) {
        result = m_device->CreateRenderTargetView(resource, &desc, &view);

        if (check("CreateRenderTargetView", w[0])) {
          Create(w[0], view);
        }
      }

      return true;
    }
    case VISTA_D3D_CREATE_DSV: {
      D3D11_DEPTH_STENCIL_VIEW_DESC desc = {};
      desc.Format = static_cast<DXGI_FORMAT>(w[2]);
      desc.ViewDimension = static_cast<D3D11_DSV_DIMENSION>(w[3]);

      if (desc.ViewDimension == D3D11_DSV_DIMENSION_TEXTURE2D) {
        desc.Texture2D = { w[4] };
      } else {
        desc.Texture2DArray = { w[4], w[5], w[6] };
      }

      ID3D11Resource* resource = Resource(w[1]);
      ID3D11DepthStencilView* view = nullptr;

      if (resource) {
        result = m_device->CreateDepthStencilView(resource, &desc, &view);

        if (check("CreateDepthStencilView", w[0])) {
          Create(w[0], view);
        }
      }

      return true;
    }
    case VISTA_D3D_CREATE_SAMPLER: {
      D3D11_SAMPLER_DESC desc = {};
      desc.Filter = static_cast<D3D11_FILTER>(w[1]);
      desc.AddressU = static_cast<D3D11_TEXTURE_ADDRESS_MODE>(w[2]);
      desc.AddressV = static_cast<D3D11_TEXTURE_ADDRESS_MODE>(w[3]);
      desc.AddressW = static_cast<D3D11_TEXTURE_ADDRESS_MODE>(w[4]);
      desc.MaxAnisotropy = w[5];
      desc.ComparisonFunc = static_cast<D3D11_COMPARISON_FUNC>(w[6]);
      desc.MinLOD = Float(w[7]);
      desc.MaxLOD = Float(w[8]);
      ID3D11SamplerState* sampler = nullptr;
      result = m_device->CreateSamplerState(&desc, &sampler);

      if (check("CreateSamplerState", w[0])) {
        Create(w[0], sampler);
      }

      return true;
    }
    case VISTA_D3D_CREATE_SHADER: {
      uint32_t id = w[0], stage = w[1], size = w[2];

      if (size > bytes) {
        Fail("shader #" + Hex(id) + " is cut short");
        return true;
      }

      Object_s extra;
      IUnknown* shader = nullptr;

      if (stage == 0) {
        result = m_device->CreateVertexShader(payload, size, nullptr,
                                              reinterpret_cast<ID3D11VertexShader**>(&shader));
        extra.bytecode.assign(payload, payload + size);
      } else if (stage == 1) {
        result = m_device->CreatePixelShader(payload, size, nullptr,
                                              reinterpret_cast<ID3D11PixelShader**>(&shader));
      } else {
        result = m_device->CreateComputeShader(payload, size, nullptr,
                                                reinterpret_cast<ID3D11ComputeShader**>(&shader));
      }

      if (check("Create*Shader", id)) {
        Create(id, shader, std::move(extra));
      }

      return true;
    }
    case VISTA_D3D_CREATE_INPUT_LAYOUT: {
      uint32_t id = w[0], elements = w[2];

      if (count < 3 + size_t(elements) * 6) {
        Fail("input layout #" + Hex(id) + " is cut short");
        return true;
      }

      auto shader = m_objects.find(w[1]);

      if (shader == m_objects.end() || shader->second.bytecode.empty()) {
        Fail("input layout #" + Hex(id) + " names no vertex shader");
        return true;
      }

      std::vector<D3D11_INPUT_ELEMENT_DESC> descs;

      for (uint32_t i = 0; i < elements; ++i) {
        const uint32_t* e = w + 3 + i * 6;
        descs.push_back({ "LOC", e[0], static_cast<DXGI_FORMAT>(e[1]), e[2], e[3],
                          e[4] ? D3D11_INPUT_PER_INSTANCE_DATA : D3D11_INPUT_PER_VERTEX_DATA, e[5] });
      }

      ID3D11InputLayout* layout = nullptr;
      result = m_device->CreateInputLayout(descs.data(), UINT(descs.size()), shader->second.bytecode.data(),
                                            shader->second.bytecode.size(), &layout);

      if (check("CreateInputLayout", id)) {
        Create(id, layout);
      }

      return true;
    }
    case VISTA_D3D_CREATE_BLEND: {
      D3D11_BLEND_DESC desc = {};
      desc.IndependentBlendEnable = TRUE;

      for (int i = 0; i < 8; ++i) {
        const uint32_t* t = w + 1 + i * 8;
        desc.RenderTarget[i] = {
          t[0] != 0,
          static_cast<D3D11_BLEND>(t[1]),
          static_cast<D3D11_BLEND>(t[2]),
          static_cast<D3D11_BLEND_OP>(t[3]),
          static_cast<D3D11_BLEND>(t[4]),
          static_cast<D3D11_BLEND>(t[5]),
          static_cast<D3D11_BLEND_OP>(t[6]),
          static_cast<UINT8>(t[7]),
        };
      }

      ID3D11BlendState* state = nullptr;
      result = m_device->CreateBlendState(&desc, &state);

      if (check("CreateBlendState", w[0])) {
        Create(w[0], state);
      }

      return true;
    }
    case VISTA_D3D_CREATE_RASTERIZER: {
      D3D11_RASTERIZER_DESC desc = {};
      desc.FillMode = D3D11_FILL_SOLID;
      desc.CullMode = static_cast<D3D11_CULL_MODE>(w[1]);
      desc.FrontCounterClockwise = w[2] != 0;
      desc.DepthBias = static_cast<INT>(w[3]);
      desc.DepthBiasClamp = Float(w[4]);
      desc.SlopeScaledDepthBias = Float(w[5]);
      desc.DepthClipEnable = w[6] != 0;
      ID3D11RasterizerState* state = nullptr;
      result = m_device->CreateRasterizerState(&desc, &state);

      if (check("CreateRasterizerState", w[0])) {
        Create(w[0], state);
      }

      return true;
    }
    case VISTA_D3D_CREATE_DEPTH: {
      D3D11_DEPTH_STENCIL_DESC desc = {};
      desc.DepthEnable = w[1] != 0;
      desc.DepthWriteMask = static_cast<D3D11_DEPTH_WRITE_MASK>(w[2]);
      desc.DepthFunc = static_cast<D3D11_COMPARISON_FUNC>(w[3]);
      ID3D11DepthStencilState* state = nullptr;
      result = m_device->CreateDepthStencilState(&desc, &state);

      if (check("CreateDepthStencilState", w[0])) {
        Create(w[0], state);
      }

      return true;
    }
    case VISTA_D3D_RELEASE:
      m_objects.erase(w[0]);
      return true;
    case VISTA_D3D_UPDATE_BUFFER: {
      auto found = m_objects.find(w[0]);

      if (found == m_objects.end() || w[2] > bytes) {
        Fail("a write to buffer #" + Hex(w[0]) + ", which does not exist");
        return true;
      }

      auto* buffer = static_cast<ID3D11Buffer*>(found->second.object.Get());

      // Direct3D 11.0 writes constant buffers whole, with no box.
      if (found->second.constant) {
        m_context->UpdateSubresource(buffer, 0, nullptr, payload, 0, 0);
      } else {
        D3D11_BOX box = { w[1], 0, 0, w[1] + w[2], 1, 1 };
        m_context->UpdateSubresource(buffer, 0, &box, payload, 0, 0);
      }

      return true;
    }
    case VISTA_D3D_UPDATE_TEXTURE: {
      ID3D11Resource* texture = Resource(w[0]);

      if (texture && w[10] <= bytes) {
        D3D11_BOX box = { w[2], w[3], w[4], w[5], w[6], w[7] };
        m_context->UpdateSubresource(texture, w[1], &box, payload, w[8], w[9]);
      }

      return true;
    }
    case VISTA_D3D_COPY_BUFFER: {
      ID3D11Resource* destination = Resource(w[0]);
      ID3D11Resource* source = Resource(w[2]);

      if (destination && source) {
        D3D11_BOX box = { w[3], 0, 0, w[3] + w[4], 1, 1 };
        m_context->CopySubresourceRegion(destination, 0, w[1], 0, 0, source, 0, &box);
      }

      return true;
    }
    case VISTA_D3D_COPY_TEXTURE: {
      ID3D11Resource* destination = Resource(w[0]);
      ID3D11Resource* source = Resource(w[5]);

      if (destination && source) {
        D3D11_BOX box = { w[7], w[8], w[9], w[10], w[11], w[12] };
        m_context->CopySubresourceRegion(destination, w[1], w[2], w[3], w[4], source, w[6], &box);
      }

      return true;
    }
    case VISTA_D3D_READBACK: {
      ID3D11Resource* buffer = Resource(w[0]);

      if (!buffer) {
        return true;
      }

      D3D11_BUFFER_DESC desc = {};
      desc.ByteWidth = w[1];
      desc.Usage = D3D11_USAGE_STAGING;
      desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
      PendingRead_s read;
      read.buffer = w[0];
      result = m_device->CreateBuffer(&desc, nullptr, read.staging.Put());

      if (check("CreateBuffer (read-back)", w[0])) {
        m_context->CopyResource(read.staging.Get(), buffer);
        m_reads.push_back(std::move(read));
      }

      return true;
    }
    case VISTA_D3D_SET_TARGETS: {
      ID3D11RenderTargetView* targets[8] = {};
      UINT count = std::min<UINT>(w[0], 8);

      for (UINT i = 0; i < count; ++i) {
        targets[i] = As<ID3D11RenderTargetView>(w[2 + i]);
      }

      m_context->OMSetRenderTargets(count, targets, As<ID3D11DepthStencilView>(w[1]));
      return true;
    }
    case VISTA_D3D_CLEAR_RTV: {
      if (auto* view = As<ID3D11RenderTargetView>(w[0])) {
        const float rgba[4] = { Float(w[1]), Float(w[2]), Float(w[3]), Float(w[4]) };
        m_context->ClearRenderTargetView(view, rgba);
      }

      return true;
    }
    case VISTA_D3D_CLEAR_DSV: {
      if (auto* view = As<ID3D11DepthStencilView>(w[0])) {
        m_context->ClearDepthStencilView(view, D3D11_CLEAR_DEPTH, Float(w[1]), 0);
      }

      return true;
    }
    case VISTA_D3D_SET_VIEWPORT: {
      D3D11_VIEWPORT viewport = { Float(w[0]), Float(w[1]), Float(w[2]), Float(w[3]), Float(w[4]), Float(w[5]) };
      m_context->RSSetViewports(1, &viewport);
      return true;
    }
    case VISTA_D3D_SET_GRAPHICS: {
      const float factor[4] = {};
      m_context->VSSetShader(As<ID3D11VertexShader>(w[0]), nullptr, 0);
      m_context->PSSetShader(As<ID3D11PixelShader>(w[1]), nullptr, 0);
      m_context->IASetInputLayout(As<ID3D11InputLayout>(w[2]));
      m_context->OMSetBlendState(As<ID3D11BlendState>(w[3]), factor, 0xffffffff);
      m_context->RSSetState(As<ID3D11RasterizerState>(w[4]));
      m_context->OMSetDepthStencilState(As<ID3D11DepthStencilState>(w[5]), 0);
      m_context->IASetPrimitiveTopology(static_cast<D3D11_PRIMITIVE_TOPOLOGY>(w[6]));
      return true;
    }
    case VISTA_D3D_SET_COMPUTE:
      m_context->CSSetShader(As<ID3D11ComputeShader>(w[0]), nullptr, 0);
      return true;
    case VISTA_D3D_BIND: {
      uint32_t stage = w[0], kind = w[1], slot = w[2], object = w[3];

      if (kind == VISTA_D3D_CONSTANT_BUFFER && slot < k_constantSlots) {
        ID3D11Buffer* buffer = As<ID3D11Buffer>(object);

        if (stage == VISTA_D3D_VS) {
          m_context->VSSetConstantBuffers(slot, 1, &buffer);
        } else if (stage == VISTA_D3D_PS) {
          m_context->PSSetConstantBuffers(slot, 1, &buffer);
        } else {
          m_context->CSSetConstantBuffers(slot, 1, &buffer);
        }
      } else if (kind == VISTA_D3D_RESOURCE && slot < k_resourceSlots) {
        ID3D11ShaderResourceView* view = As<ID3D11ShaderResourceView>(object);

        if (stage == VISTA_D3D_VS) {
          m_context->VSSetShaderResources(slot, 1, &view);
        } else if (stage == VISTA_D3D_PS) {
          m_context->PSSetShaderResources(slot, 1, &view);
        } else {
          m_context->CSSetShaderResources(slot, 1, &view);
        }
      } else if (kind == VISTA_D3D_SAMPLER && slot < k_samplerSlots) {
        ID3D11SamplerState* sampler = As<ID3D11SamplerState>(object);

        if (stage == VISTA_D3D_VS) {
          m_context->VSSetSamplers(slot, 1, &sampler);
        } else if (stage == VISTA_D3D_PS) {
          m_context->PSSetSamplers(slot, 1, &sampler);
        } else {
          m_context->CSSetSamplers(slot, 1, &sampler);
        }
      } else if (kind == VISTA_D3D_UNORDERED && stage == VISTA_D3D_CS && slot < k_unorderedSlots) {
        ID3D11UnorderedAccessView* view = As<ID3D11UnorderedAccessView>(object);
        const UINT keep = UINT(-1);
        m_context->CSSetUnorderedAccessViews(slot, 1, &view, &keep);
      } else {
        Fail("a binding to slot " + std::to_string(slot) + " of kind " + std::to_string(kind) +
              " in stage " + std::to_string(stage));
      }

      return true;
    }
    case VISTA_D3D_UNBIND_ALL:
      UnbindAll();
      return true;
    case VISTA_D3D_SET_VERTEX_BUFFER: {
      ID3D11Buffer* buffer = As<ID3D11Buffer>(w[1]);
      UINT stride = w[2], offset = w[3];
      m_context->IASetVertexBuffers(w[0], 1, &buffer, &stride, &offset);
      return true;
    }
    case VISTA_D3D_SET_INDEX_BUFFER:
      m_context->IASetIndexBuffer(As<ID3D11Buffer>(w[0]), static_cast<DXGI_FORMAT>(w[1]), w[2]);
      return true;
    case VISTA_D3D_DRAW:
      m_context->DrawInstanced(w[0], w[1], w[2], w[3]);
      return true;
    case VISTA_D3D_DRAW_INDEXED:
      m_context->DrawIndexedInstanced(w[0], w[1], w[2], static_cast<INT>(w[3]), w[4]);
      return true;
    case VISTA_D3D_DRAW_INDIRECT:
      if (auto* buffer = As<ID3D11Buffer>(w[0])) {
        m_context->DrawInstancedIndirect(buffer, w[1]);
      }

      return true;
    case VISTA_D3D_DRAW_INDEXED_INDIRECT:
      if (auto* buffer = As<ID3D11Buffer>(w[0])) {
        m_context->DrawIndexedInstancedIndirect(buffer, w[1]);
      }

      return true;
    case VISTA_D3D_DISPATCH:
      m_context->Dispatch(w[0], w[1], w[2]);
      return true;
    case VISTA_D3D_PRESENT:
      ++m_presents;
      return true;
    case VISTA_D3D_OUTPUT_SIZE:
      return true;
    default:
      return false;
  }
}

std::vector<Read_s> Executor_c::TakeReads() {
  std::vector<Read_s> done;

  for (auto read = m_reads.begin(); read != m_reads.end();) {
    D3D11_MAPPED_SUBRESOURCE mapped = {};
    HRESULT result = m_context->Map(read->staging.Get(), 0, D3D11_MAP_READ, D3D11_MAP_FLAG_DO_NOT_WAIT, &mapped);

    if (result == DXGI_ERROR_WAS_STILL_DRAWING) {
      ++read;
      continue;
    }

    Read_s out;
    out.buffer = read->buffer;

    if (SUCCEEDED(result)) {
      D3D11_BUFFER_DESC desc = {};
      read->staging->GetDesc(&desc);
      const auto* bytes = static_cast<const uint8_t*>(mapped.pData);
      out.bytes.assign(bytes, bytes + desc.ByteWidth);
      m_context->Unmap(read->staging.Get(), 0);
    }

    // An empty read is one that failed.
    done.push_back(std::move(out));
    read = m_reads.erase(read);
  }

  return done;
}

ID3D11Texture2D* Executor_c::Texture(uint32_t id) const {
  auto found = m_objects.find(id);

  if (found == m_objects.end()) {
    return nullptr;
  }

  // Only 2D textures answer to the interface; anything else gives null.
  ID3D11Texture2D* texture = nullptr;

  if (FAILED(found->second.object->QueryInterface(__uuidof(ID3D11Texture2D), reinterpret_cast<void**>(&texture)))) {
    return nullptr;
  }

  // The executor keeps its own reference.
  texture->Release();
  return texture;
}

std::vector<std::string> Executor_c::TakeErrors() {
  std::vector<std::string> errors;
  errors.swap(m_errors);
  return errors;
}

// --- Renderer_c ------------------------------------------------------------------

namespace {

std::string LastError() {
  const char* message = vista_last_error();
  return message ? message : "unknown error";
}

}  // namespace

Renderer_c::Renderer_c(VistaEngine* engine, ID3D11Device* device, ID3D11DeviceContext* context,
                        uint32_t width, uint32_t height, VistaOutputFormat format)
  : m_engine(engine), m_executor(device, context) {
  m_attached = vista_renderer_attach(engine, width, height, format) == VISTA_OK;
}

Renderer_c::~Renderer_c() {
  if (m_attached) {
    vista_renderer_detach(m_engine);
  }
}

bool Renderer_c::Run(const VistaCommands& commands, ID3D11Texture2D* target, std::string* error) {
  m_executor.SetOutput(target);
  bool ran = m_executor.Run(commands.data, commands.bytes);

  // Whatever went wrong reaches the engine's events too.
  for (const std::string& failure : m_executor.TakeErrors()) {
    vista_renderer_report_error(m_engine, failure.c_str());

    if (error && error->empty()) {
      *error = failure;
    }
  }

  if (m_executor.DeviceLost()) {
    vista_renderer_report_lost(m_engine, "the Direct3D 11 device was removed");
  }

  for (const Read_s& read : m_executor.TakeReads()) {
    if (read.bytes.empty()) {
      vista_renderer_fail_read(m_engine, read.buffer);
    } else {
      vista_renderer_complete_read(m_engine, read.buffer, read.bytes.data(), read.bytes.size());
    }
  }

  return ran;
}

bool Renderer_c::Frame(double now_ms, ID3D11Texture2D* target, std::string* error) {
  if (!m_attached) {
    if (error) {
      *error = "no renderer is attached";
    }

    return false;
  }

  VistaCommands commands = {};
  VistaStatus status = vista_renderer_frame(m_engine, now_ms, &commands);
  std::string failure = status == VISTA_OK ? std::string() : LastError();
  // A frame that failed still hands out what was recorded before it.
  bool ran = Run(commands, target, error);

  if (status != VISTA_OK) {
    if (error) {
      *error = failure;
    }

    return false;
  }

  return ran;
}

bool Renderer_c::Flush(ID3D11Texture2D* target, std::string* error) {
  VistaCommands commands = {};

  if (!m_attached || vista_renderer_commands(m_engine, &commands) != VISTA_OK) {
    if (error) {
      *error = m_attached ? LastError() : "no renderer is attached";
    }

    return false;
  }

  return Run(commands, target, error);
}

bool Renderer_c::Resize(uint32_t width, uint32_t height, std::string* error) {
  if (!m_attached || vista_renderer_resize(m_engine, width, height) != VISTA_OK) {
    if (error) {
      *error = m_attached ? LastError() : "no renderer is attached";
    }

    return false;
  }

  return true;
}

}  // namespace VistaD3D11
