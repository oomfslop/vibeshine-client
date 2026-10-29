#include <metal_stdlib>
using namespace metal;

struct Vertex
{
    float4 position [[ position ]];
    float2 texCoords;
};

struct CscParams
{
    half3x3 matrix;
    half3 offsets;
    half2 chromaOffset;
    half bitnessScaleFactor;
};

constexpr sampler s(coord::normalized, address::clamp_to_edge, filter::linear);

vertex Vertex vs_draw(constant Vertex *vertices [[ buffer(0) ]], uint id [[ vertex_id ]])
{
    return vertices[id];
}

fragment half4 ps_draw_biplanar(Vertex v [[ stage_in ]],
                                constant CscParams &cscParams [[ buffer(0) ]],
                                texture2d<half> luminancePlane [[ texture(0) ]],
                                texture2d<half> chrominancePlane [[ texture(1) ]])
{
    float2 chromaOffset = float2(cscParams.chromaOffset) / float2(luminancePlane.get_width(),
                                                                  luminancePlane.get_height());
    half3 yuv = half3(luminancePlane.sample(s, v.texCoords).r,
                      chrominancePlane.sample(s, v.texCoords + chromaOffset).rg);
    yuv *= cscParams.bitnessScaleFactor;
    yuv -= cscParams.offsets;

    return half4(yuv * cscParams.matrix, 1.0h);
}

fragment half4 ps_draw_triplanar(Vertex v [[ stage_in ]],
                                 constant CscParams &cscParams [[ buffer(0) ]],
                                 texture2d<half> luminancePlane [[ texture(0) ]],
                                 texture2d<half> chrominancePlaneU [[ texture(1) ]],
                                 texture2d<half> chrominancePlaneV [[ texture(2) ]])
{
    float2 chromaOffset = float2(cscParams.chromaOffset) / float2(luminancePlane.get_width(),
                                                                  luminancePlane.get_height());
    half3 yuv = half3(luminancePlane.sample(s, v.texCoords).r,
                      chrominancePlaneU.sample(s, v.texCoords + chromaOffset).r,
                      chrominancePlaneV.sample(s, v.texCoords + chromaOffset).r);
    yuv *= cscParams.bitnessScaleFactor;
    yuv -= cscParams.offsets;

    return half4(yuv * cscParams.matrix, 1.0h);
}

fragment half4 ps_draw_rgb(Vertex v [[ stage_in ]],
                           texture2d<half> rgbTexture [[ texture(0) ]])
{
    return rgbTexture.sample(s, v.texCoords);
}

// Desk windows: each Mac window shows its window's picture from the frame
// (its slot in the host's atlas), on a transparent background. Opaque
// windows get their rounded corners here; windows with transparency (menus
// with their shadows, tooltips) come twice in the frame, on black and on
// white, and alpha is what differs: a = 1 - (white - black), the colour is
// the copy on black (premultiplied by that alpha already).
struct VertexAW
{
    float4 position [[ position ]];
    float2 texCoords;
    float2 local;        // pixel position inside the rectangle
    float4 rect;         // width, height, corner radius, 1 = alpha from the copy on white
    float2 whiteOffset;  // texCoords of the copy on white minus those of the copy on black
};

vertex VertexAW vs_draw_aw(constant VertexAW *vertices [[ buffer(0) ]], uint id [[ vertex_id ]])
{
    return vertices[id];
}

static half aw_coverage(float2 local, float4 rect)
{
    float r = rect.z;
    if (r <= 0.0) {
        return 1.0h;
    }
    float2 half_size = rect.xy * 0.5;
    float2 d = abs(local - half_size) - (half_size - float2(r));
    float dist = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - r;
    return half(saturate(0.5 - dist));
}

// Anything sampled outside the frame is transparent.
static half desk_coverage(float2 texCoords, float2 local, float4 rect)
{
    half cov = aw_coverage(local, rect);
    if (texCoords.x < 0.0 || texCoords.y < 0.0 || texCoords.x > 1.0 || texCoords.y > 1.0) {
        cov = 0.0h;
    }
    return cov;
}

static half3 biplanar_rgb(float2 uv, constant CscParams &p, texture2d<half> luma, texture2d<half> chroma)
{
    float2 chromaOffset = float2(p.chromaOffset) / float2(luma.get_width(), luma.get_height());
    half3 yuv = half3(luma.sample(s, uv).r, chroma.sample(s, uv + chromaOffset).rg);
    yuv *= p.bitnessScaleFactor;
    yuv -= p.offsets;
    return yuv * p.matrix;
}

static half3 triplanar_rgb(float2 uv, constant CscParams &p, texture2d<half> luma, texture2d<half> u, texture2d<half> v)
{
    float2 chromaOffset = float2(p.chromaOffset) / float2(luma.get_width(), luma.get_height());
    half3 yuv = half3(luma.sample(s, uv).r, u.sample(s, uv + chromaOffset).r, v.sample(s, uv + chromaOffset).r);
    yuv *= p.bitnessScaleFactor;
    yuv -= p.offsets;
    return yuv * p.matrix;
}

static half4 desk_out(half3 black, half3 white, VertexAW v)
{
    half cov = desk_coverage(v.texCoords, v.local, v.rect);
    if (v.rect.w > 0.5) {
        half a = saturate(1.0h - dot(white - black, half3(1.0h / 3.0h)));
        half3 c = clamp(black, half3(0.0h), half3(a));
        return half4(c * cov, a * cov);
    }
    return half4(black * cov, cov);
}

fragment half4 ps_draw_biplanar_desk(VertexAW v [[ stage_in ]],
                                     constant CscParams &cscParams [[ buffer(0) ]],
                                     texture2d<half> luminancePlane [[ texture(0) ]],
                                     texture2d<half> chrominancePlane [[ texture(1) ]])
{
    half3 black = biplanar_rgb(v.texCoords, cscParams, luminancePlane, chrominancePlane);
    half3 white = v.rect.w > 0.5 ? biplanar_rgb(v.texCoords + v.whiteOffset, cscParams, luminancePlane, chrominancePlane) : black;
    return desk_out(black, white, v);
}

fragment half4 ps_draw_triplanar_desk(VertexAW v [[ stage_in ]],
                                      constant CscParams &cscParams [[ buffer(0) ]],
                                      texture2d<half> luminancePlane [[ texture(0) ]],
                                      texture2d<half> chrominancePlaneU [[ texture(1) ]],
                                      texture2d<half> chrominancePlaneV [[ texture(2) ]])
{
    half3 black = triplanar_rgb(v.texCoords, cscParams, luminancePlane, chrominancePlaneU, chrominancePlaneV);
    half3 white = v.rect.w > 0.5 ? triplanar_rgb(v.texCoords + v.whiteOffset, cscParams, luminancePlane, chrominancePlaneU, chrominancePlaneV) : black;
    return desk_out(black, white, v);
}
