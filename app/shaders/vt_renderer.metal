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

// Desk windows: each Mac window shows a rounded rectangle of the frame (the
// client's desk) on a transparent background.
struct VertexAW
{
    float4 position [[ position ]];
    float2 texCoords;
    float2 local;   // pixel position inside the rectangle
    float4 rect;    // width, height, corner radius, unused
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

// The whole frame is picture; anything sampled outside the frame (a window
// hanging off the desk) is transparent.
static half desk_coverage(float2 texCoords, float2 local, float4 rect)
{
    half cov = aw_coverage(local, rect);
    if (texCoords.x < 0.0 || texCoords.y < 0.0 || texCoords.x > 1.0 || texCoords.y > 1.0) {
        cov = 0.0h;
    }
    return cov;
}

// Popups (rect.w = band width): a Windows 11 menu's window is larger than
// the menu, its edges holding the soft shadow drawn over whatever is under
// it on the host. Near-black pixels in that band are what the host has
// beneath (the desk's black, a dark window), not the menu: transparent.
static half desk_key(half3 rgb, float2 local, float4 rect)
{
    if (rect.w <= 0.0) {
        return 1.0h;
    }
    float edge = min(min(local.x, local.y), min(rect.x - local.x, rect.y - local.y));
    if (edge >= rect.w) {
        return 1.0h;
    }
    half bright = max(rgb.r, max(rgb.g, rgb.b));
    return smoothstep(0.035h, 0.085h, bright);
}

fragment half4 ps_draw_biplanar_desk(VertexAW v [[ stage_in ]],
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
    half3 rgb = yuv * cscParams.matrix;
    half cov = desk_coverage(v.texCoords, v.local, v.rect) * desk_key(rgb, v.local, v.rect);
    return half4(rgb * cov, cov);
}

fragment half4 ps_draw_triplanar_desk(VertexAW v [[ stage_in ]],
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
    half3 rgb = yuv * cscParams.matrix;
    half cov = desk_coverage(v.texCoords, v.local, v.rect) * desk_key(rgb, v.local, v.rect);
    return half4(rgb * cov, cov);
}
