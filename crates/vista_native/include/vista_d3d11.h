/*
 * vista_d3d11.h: the Direct3D 11 command stream VistaWASM's native
 * renderer produces (see vista_native.h, "Rendering").
 *
 * The engine draws with the same code as the browser build. Natively, it
 * records that work, and vista_native lowers it to Direct3D 11 calls, one
 * record each, for the host to run on its own device. The stream is a
 * sequence of records, each a VistaD3DRecord header followed by its
 * payload. Every field is 32 bits, little-endian, so `bytes` is a multiple
 * of 4. Run the records in order, all of them, and every stream in the
 * order it was handed out. ports/d3d11/executor is a complete executor.
 *
 * Objects are named by 32-bit ids: buffers, textures, views, samplers,
 * shaders, input layouts and state objects share one namespace. An id is
 * created once, used by later records, and released by VISTA_D3D_RELEASE;
 * ids are never reused while live. Id 0 is "none" (unbind). Id 1 is the
 * host's render target texture (VISTA_D3D_OUTPUT), which the stream never
 * creates or releases: the host supplies it before running each stream.
 *
 * Every enum value is Direct3D 11's own (DXGI_FORMAT, D3D11_BIND_FLAG,
 * D3D11_SRV_DIMENSION, D3D11_FILTER and so on), so records map onto the
 * API calls named in each comment without translation.
 *
 * Licence: AGPL-3.0-only, as VistaWASM.
 */

#ifndef VISTA_D3D11_H
#define VISTA_D3D11_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The host's render target, as a texture id. */
#define VISTA_D3D_OUTPUT 1u

typedef struct VistaD3DRecord {
  uint32_t op;     /* VistaD3DOp */
  uint32_t bytes;  /* payload bytes after this header */
} VistaD3DRecord;

typedef enum VistaD3DOp {
  /* --- objects ---------------------------------------------------------- */
  VISTA_D3D_CREATE_BUFFER = 1,         /* VistaD3DCreateBuffer: CreateBuffer, USAGE_DEFAULT */
  VISTA_D3D_CREATE_TEXTURE = 2,        /* VistaD3DCreateTexture: CreateTexture1D/2D/3D, USAGE_DEFAULT */
  VISTA_D3D_CREATE_SRV = 3,            /* VistaD3DCreateView: CreateShaderResourceView */
  VISTA_D3D_CREATE_UAV = 4,            /* VistaD3DCreateView: CreateUnorderedAccessView */
  VISTA_D3D_CREATE_RTV = 5,            /* VistaD3DCreateView: CreateRenderTargetView */
  VISTA_D3D_CREATE_DSV = 6,            /* VistaD3DCreateView: CreateDepthStencilView */
  VISTA_D3D_CREATE_SAMPLER = 7,        /* VistaD3DCreateSampler: CreateSamplerState */
  VISTA_D3D_CREATE_SHADER = 8,         /* VistaD3DCreateShader + bytecode */
  VISTA_D3D_CREATE_INPUT_LAYOUT = 9,   /* VistaD3DCreateInputLayout + elements */
  VISTA_D3D_CREATE_BLEND = 10,         /* VistaD3DCreateBlend: CreateBlendState */
  VISTA_D3D_CREATE_RASTERIZER = 11,    /* VistaD3DCreateRasterizer: CreateRasterizerState */
  VISTA_D3D_CREATE_DEPTH = 12,         /* VistaD3DCreateDepth: CreateDepthStencilState */
  VISTA_D3D_RELEASE = 13,              /* VistaD3DId: Release */
  /* --- data ------------------------------------------------------------- */
  VISTA_D3D_UPDATE_BUFFER = 20,        /* VistaD3DUpdateBuffer + bytes: UpdateSubresource */
  VISTA_D3D_UPDATE_TEXTURE = 21,       /* VistaD3DUpdateTexture + bytes: UpdateSubresource */
  VISTA_D3D_COPY_BUFFER = 22,          /* VistaD3DCopyBuffer: CopySubresourceRegion */
  VISTA_D3D_COPY_TEXTURE = 23,         /* VistaD3DCopyTexture: CopySubresourceRegion */
  VISTA_D3D_READBACK = 24,             /* VistaD3DReadback: see below */
  /* --- drawing ---------------------------------------------------------- */
  VISTA_D3D_SET_TARGETS = 30,          /* VistaD3DSetTargets: OMSetRenderTargets */
  VISTA_D3D_CLEAR_RTV = 31,            /* VistaD3DClearRtv: ClearRenderTargetView */
  VISTA_D3D_CLEAR_DSV = 32,            /* VistaD3DClearDsv: ClearDepthStencilView, depth only */
  VISTA_D3D_SET_VIEWPORT = 33,         /* VistaD3DSetViewport: RSSetViewports */
  VISTA_D3D_SET_GRAPHICS = 34,         /* VistaD3DSetGraphics: VS, PS, IA layout and topology, OM and RS states */
  VISTA_D3D_SET_COMPUTE = 35,          /* VistaD3DId: CSSetShader */
  VISTA_D3D_BIND = 36,                 /* VistaD3DBind: xxSetConstantBuffers / ShaderResources / Samplers, CSSetUnorderedAccessViews */
  VISTA_D3D_UNBIND_ALL = 37,           /* none: null every CB, SRV, sampler and UAV slot of VS, PS and CS, and the IA buffers */
  VISTA_D3D_SET_VERTEX_BUFFER = 38,    /* VistaD3DSetVertexBuffer: IASetVertexBuffers */
  VISTA_D3D_SET_INDEX_BUFFER = 39,     /* VistaD3DSetIndexBuffer: IASetIndexBuffer */
  VISTA_D3D_DRAW = 40,                 /* VistaD3DDraw: DrawInstanced */
  VISTA_D3D_DRAW_INDEXED = 41,         /* VistaD3DDrawIndexed: DrawIndexedInstanced */
  VISTA_D3D_DRAW_INDIRECT = 42,        /* VistaD3DIndirect: DrawInstancedIndirect */
  VISTA_D3D_DRAW_INDEXED_INDIRECT = 43,/* VistaD3DIndirect: DrawIndexedInstancedIndirect */
  VISTA_D3D_DISPATCH = 44,             /* VistaD3DDispatch: Dispatch */
  /* --- frames ----------------------------------------------------------- */
  VISTA_D3D_PRESENT = 50,              /* none: the frame in VISTA_D3D_OUTPUT is finished */
  VISTA_D3D_OUTPUT_SIZE = 51           /* VistaD3DOutputSize: the host's target is now this size */
} VistaD3DOp;

typedef struct VistaD3DId {
  uint32_t id;
} VistaD3DId;

/* D3D11_BUFFER_DESC: ByteWidth, BindFlags, MiscFlags; Usage DEFAULT, no
 * CPU access, StructureByteStride 0. Contents start as zeros. */
typedef struct VistaD3DCreateBuffer {
  uint32_t id;
  uint32_t byte_width;
  uint32_t bind_flags;
  uint32_t misc_flags;
} VistaD3DCreateBuffer;

/* dimension 1, 2 or 3. For 1 and 2, depth_or_layers is ArraySize; for 3,
 * Depth. Format is a DXGI_FORMAT (typeless for depth textures). Contents
 * start as zeros. */
typedef struct VistaD3DCreateTexture {
  uint32_t id;
  uint32_t dimension;
  uint32_t width;
  uint32_t height;
  uint32_t depth_or_layers;
  uint32_t mip_levels;
  uint32_t format;
  uint32_t bind_flags;
} VistaD3DCreateTexture;

/* A view of `resource`. `dimension` is the D3D11_SRV/UAV/RTV/DSV_DIMENSION
 * of the record's kind; a, b, c and d are, by dimension:
 *
 *   SRV BUFFEREX            FirstElement, NumElements, Flags
 *   SRV TEXTURE2D / 3D      MostDetailedMip, MipLevels
 *   SRV TEXTURE2DARRAY      MostDetailedMip, MipLevels, FirstArraySlice, ArraySize
 *   UAV BUFFER              FirstElement, NumElements, Flags
 *   UAV TEXTURE2D           MipSlice
 *   UAV TEXTURE2DARRAY      MipSlice, FirstArraySlice, ArraySize
 *   UAV TEXTURE3D           MipSlice, FirstWSlice, WSize
 *   RTV / DSV TEXTURE2D     MipSlice
 *   RTV / DSV TEXTURE2DARRAY MipSlice, FirstArraySlice, ArraySize
 *
 * A resource of VISTA_D3D_OUTPUT means the host's render target. */
typedef struct VistaD3DCreateView {
  uint32_t id;
  uint32_t resource;
  uint32_t format;
  uint32_t dimension;
  uint32_t a;
  uint32_t b;
  uint32_t c;
  uint32_t d;
} VistaD3DCreateView;

/* D3D11_SAMPLER_DESC; MipLODBias 0, BorderColor 0. */
typedef struct VistaD3DCreateSampler {
  uint32_t id;
  uint32_t filter;
  uint32_t address_u;
  uint32_t address_v;
  uint32_t address_w;
  uint32_t max_anisotropy;
  uint32_t comparison_func;
  float min_lod;
  float max_lod;
} VistaD3DCreateSampler;

/* stage: 0 vertex, 1 pixel, 2 compute. `size` bytes of DXBC follow,
 * padded with zeros to a multiple of 4. Keep a vertex shader's bytecode:
 * input layouts are created against it. */
typedef struct VistaD3DCreateShader {
  uint32_t id;
  uint32_t stage;
  uint32_t size;
} VistaD3DCreateShader;

/* `count` VistaD3DInputElement follow, for vertex shader `vertex_shader`.
 * Every SemanticName is "LOC". */
typedef struct VistaD3DCreateInputLayout {
  uint32_t id;
  uint32_t vertex_shader;
  uint32_t count;
} VistaD3DCreateInputLayout;

typedef struct VistaD3DInputElement {
  uint32_t semantic_index;
  uint32_t format;
  uint32_t input_slot;
  uint32_t aligned_byte_offset;
  uint32_t per_instance;        /* D3D11_INPUT_PER_INSTANCE_DATA when 1 */
  uint32_t instance_step_rate;
} VistaD3DInputElement;

typedef struct VistaD3DTargetBlend {
  uint32_t blend_enable;
  uint32_t src_blend;
  uint32_t dest_blend;
  uint32_t blend_op;
  uint32_t src_blend_alpha;
  uint32_t dest_blend_alpha;
  uint32_t blend_op_alpha;
  uint32_t write_mask;
} VistaD3DTargetBlend;

/* AlphaToCoverageEnable FALSE, IndependentBlendEnable TRUE. */
typedef struct VistaD3DCreateBlend {
  uint32_t id;
  VistaD3DTargetBlend targets[8];
} VistaD3DCreateBlend;

/* FillMode SOLID; ScissorEnable, MultisampleEnable and
 * AntialiasedLineEnable FALSE. */
typedef struct VistaD3DCreateRasterizer {
  uint32_t id;
  uint32_t cull_mode;
  uint32_t front_counter_clockwise;
  int32_t depth_bias;
  float depth_bias_clamp;
  float slope_scaled_depth_bias;
  uint32_t depth_clip_enable;
} VistaD3DCreateRasterizer;

/* StencilEnable FALSE. */
typedef struct VistaD3DCreateDepth {
  uint32_t id;
  uint32_t depth_enable;
  uint32_t depth_write_mask;
  uint32_t depth_func;
} VistaD3DCreateDepth;

/* `size` bytes follow (padded to 4) for bytes [offset, offset + size) of
 * the buffer. Constant buffers are always written whole, from 0, which
 * Direct3D 11.0 requires: pass a null box for them. */
typedef struct VistaD3DUpdateBuffer {
  uint32_t buffer;
  uint32_t offset;
  uint32_t size;
} VistaD3DUpdateBuffer;

/* `size` bytes follow (padded to 4), for the box in `subresource`, with
 * these pitches. */
typedef struct VistaD3DUpdateTexture {
  uint32_t texture;
  uint32_t subresource;
  uint32_t left;
  uint32_t top;
  uint32_t front;
  uint32_t right;
  uint32_t bottom;
  uint32_t back;
  uint32_t row_pitch;
  uint32_t depth_pitch;
  uint32_t size;
} VistaD3DUpdateTexture;

typedef struct VistaD3DCopyBuffer {
  uint32_t destination;
  uint32_t destination_offset;
  uint32_t source;
  uint32_t source_offset;
  uint32_t size;
} VistaD3DCopyBuffer;

typedef struct VistaD3DCopyTexture {
  uint32_t destination;
  uint32_t destination_subresource;
  uint32_t x;
  uint32_t y;
  uint32_t z;
  uint32_t source;
  uint32_t source_subresource;
  uint32_t left;
  uint32_t top;
  uint32_t front;
  uint32_t right;
  uint32_t bottom;
  uint32_t back;
} VistaD3DCopyTexture;

/* Read the whole of `buffer` (`size` bytes) once the GPU has finished the
 * work before this record, and hand the bytes to
 * vista_renderer_complete_read() then, or vista_renderer_fail_read(). It
 * need not be at once: copy it to a staging buffer here and map that a
 * frame or two later, so rendering never waits for it. */
typedef struct VistaD3DReadback {
  uint32_t buffer;
  uint32_t size;
} VistaD3DReadback;

/* `count` render target views (0 to 8) and a depth view (0 for none). */
typedef struct VistaD3DSetTargets {
  uint32_t count;
  uint32_t depth;
  uint32_t targets[8];
} VistaD3DSetTargets;

typedef struct VistaD3DClearRtv {
  uint32_t view;
  float rgba[4];
} VistaD3DClearRtv;

typedef struct VistaD3DClearDsv {
  uint32_t view;
  float depth;
} VistaD3DClearDsv;

typedef struct VistaD3DSetViewport {
  float x;
  float y;
  float width;
  float height;
  float min_depth;
  float max_depth;
} VistaD3DSetViewport;

/* The pipeline for the draws that follow. pixel_shader 0 for none (depth
 * only), input_layout 0 for none. topology is a
 * D3D11_PRIMITIVE_TOPOLOGY. Blend factor 0, sample mask all ones, stencil
 * reference 0. */
typedef struct VistaD3DSetGraphics {
  uint32_t vertex_shader;
  uint32_t pixel_shader;
  uint32_t input_layout;
  uint32_t blend;
  uint32_t rasterizer;
  uint32_t depth;
  uint32_t topology;
} VistaD3DSetGraphics;

typedef enum VistaD3DStage {
  VISTA_D3D_VS = 0,
  VISTA_D3D_PS = 1,
  VISTA_D3D_CS = 2
} VistaD3DStage;

typedef enum VistaD3DSlot {
  VISTA_D3D_CONSTANT_BUFFER = 0,  /* object: a buffer */
  VISTA_D3D_RESOURCE = 1,         /* object: a shader resource view */
  VISTA_D3D_SAMPLER = 2,          /* object: a sampler */
  VISTA_D3D_UNORDERED = 3         /* object: an unordered access view (compute only) */
} VistaD3DSlot;

/* Bind `object` (0 to unbind) at register `slot` of `kind` in `stage`.
 * UAVs are bound with an initial count of -1 (keep counters). */
typedef struct VistaD3DBind {
  uint32_t stage;
  uint32_t kind;
  uint32_t slot;
  uint32_t object;
} VistaD3DBind;

typedef struct VistaD3DSetVertexBuffer {
  uint32_t slot;
  uint32_t buffer;
  uint32_t stride;
  uint32_t offset;
} VistaD3DSetVertexBuffer;

typedef struct VistaD3DSetIndexBuffer {
  uint32_t buffer;
  uint32_t format;
  uint32_t offset;
} VistaD3DSetIndexBuffer;

typedef struct VistaD3DDraw {
  uint32_t vertex_count;
  uint32_t instance_count;
  uint32_t start_vertex;
  uint32_t start_instance;
} VistaD3DDraw;

typedef struct VistaD3DDrawIndexed {
  uint32_t index_count;
  uint32_t instance_count;
  uint32_t start_index;
  int32_t base_vertex;
  uint32_t start_instance;
} VistaD3DDrawIndexed;

typedef struct VistaD3DIndirect {
  uint32_t buffer;
  uint32_t offset;
} VistaD3DIndirect;

typedef struct VistaD3DDispatch {
  uint32_t x;
  uint32_t y;
  uint32_t z;
} VistaD3DDispatch;

typedef struct VistaD3DOutputSize {
  uint32_t width;
  uint32_t height;
  uint32_t format;
} VistaD3DOutputSize;

#ifdef __cplusplus
}
#endif

#endif /* VISTA_D3D11_H */
