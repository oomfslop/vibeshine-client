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

// App windows: the stream is a canvas holding the host window and its popups;
// each is drawn as its own rounded rectangle on a transparent window.
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

fragment half4 ps_draw_biplanar_aw(VertexAW v [[ stage_in ]],
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
    // Below the colour canvas (the top two thirds of the frame) sits its alpha
    // as grey at half resolution
    float2 alphaCoords = v.texCoords * 0.5 + float2(0.0, 2.0 / 3.0);
    half3 ayuv = half3(luminancePlane.sample(s, alphaCoords).r,
                       chrominancePlane.sample(s, alphaCoords + chromaOffset).rg);
    ayuv *= cscParams.bitnessScaleFactor;
    ayuv -= cscParams.offsets;
    half alpha = clamp((ayuv * cscParams.matrix).g, 0.0h, 1.0h);
    half cov = aw_coverage(v.local, v.rect);
    return half4((yuv * cscParams.matrix) * cov, alpha * cov);
}

fragment half4 ps_draw_triplanar_aw(VertexAW v [[ stage_in ]],
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
    float2 alphaCoords = v.texCoords * 0.5 + float2(0.0, 2.0 / 3.0);
    half3 ayuv = half3(luminancePlane.sample(s, alphaCoords).r,
                       chrominancePlaneU.sample(s, alphaCoords + chromaOffset).r,
                       chrominancePlaneV.sample(s, alphaCoords + chromaOffset).r);
    ayuv *= cscParams.bitnessScaleFactor;
    ayuv -= cscParams.offsets;
    half alpha = clamp((ayuv * cscParams.matrix).g, 0.0h, 1.0h);
    half cov = aw_coverage(v.local, v.rect);
    return half4((yuv * cscParams.matrix) * cov, alpha * cov);
}
