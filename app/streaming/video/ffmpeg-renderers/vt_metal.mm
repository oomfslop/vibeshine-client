// Nasty hack to avoid conflict between AVFoundation and
// libavutil both defining AVMediaType
#define AVMediaType AVMediaType_FFmpeg
#include "vt.h"
#include "pacer/pacer.h"
#undef AVMediaType

#include <SDL_syswm.h>
#include <Limelight.h>
#include "streaming/session.h"
#include "streaming/streamutils.h"
#include "streaming/appwindow.h"
#include <QHash>
#include <algorithm>
#include <atomic>
#include <functional>
#include <tuple>
#include <memory>
#include <mutex>
#include <vector>
#include <QJsonObject>
#include <QRect>
#include "path.h"

#import <Cocoa/Cocoa.h>
#import <VideoToolbox/VideoToolbox.h>
#import <AVFoundation/AVFoundation.h>
#import <dispatch/dispatch.h>
#import <Metal/Metal.h>
#import <CoreImage/CoreImage.h>
#import <MetalKit/MetalKit.h>

extern "C" {
    #include <libavutil/pixdesc.h>
}

struct CscParams
{
    simd_half3x3 matrix;
    simd_half3 offsets;
};

struct ParamBuffer
{
    CscParams cscParams;
    simd_half2 chromaOffset;
    simd_half1 bitnessScaleFactor;
};

struct Vertex
{
    simd_float4 position;
    simd_float2 texCoord;
};

// Desk window quads: see vt_renderer.metal (VertexAW)
struct VertexAW
{
    simd_float4 position;
    simd_float2 texCoord;
    simd_float2 local;
    simd_float4 rect;
    simd_float2 whiteOffset;
};

class VTMetalRenderer;
// The renderer of the desk session (one at a time), for redraws asked for
// from the main thread (live resizes)
static std::mutex s_DeskRendererLock;
static VTMetalRenderer* s_DeskRenderer = nullptr;
void vtDeskRedraw(uint64_t hwnd);

// A desk frame carries the number of the layout it shows, in the atlas'
// bottom rows: 16 blocks of 16 px, black or white, 12 bits of sequence and a
// 4-bit check (its bit count). -1 when it is not there (or not readable).
static int readDeskMarker(AVFrame* frame)
{
    if (frame->format != AV_PIX_FMT_VIDEOTOOLBOX || frame->width < 256 || frame->height < 8) {
        return -1;
    }
    CVPixelBufferRef pixBuf = reinterpret_cast<CVPixelBufferRef>(frame->data[3]);
    if (!CVPixelBufferIsPlanar(pixBuf) || CVPixelBufferLockBaseAddress(pixBuf, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) {
        return -1;
    }
    const OSType format = CVPixelBufferGetPixelFormatType(pixBuf);
    const bool wide = format == kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange || format == kCVPixelFormatType_420YpCbCr10BiPlanarFullRange;
    const uint8_t* base = static_cast<const uint8_t*>(CVPixelBufferGetBaseAddressOfPlane(pixBuf, 0));
    const size_t stride = CVPixelBufferGetBytesPerRowOfPlane(pixBuf, 0);
    int code = -1;
    if (base != nullptr) {
        const uint8_t* row = base + stride * (size_t)(frame->height - 4);
        code = 0;
        for (int i = 0; i < 16; i++) {
            const int x = i * 16 + 8;
            const int luma = wide ? reinterpret_cast<const uint16_t*>(row)[x] >> 8 : row[x];
            if (luma > 128) {
                code |= 1 << i;
            }
        }
    }
    CVPixelBufferUnlockBaseAddress(pixBuf, kCVPixelBufferLock_ReadOnly);
    if (code < 0) {
        return -1;
    }
    const int seq = code & 0xfff;
    int bits = 0;
    for (int i = 0; i < 12; i++) {
        bits += (seq >> i) & 1;
    }
    return ((code >> 12) & 0xf) == (bits & 0xf) ? seq : -1;
}

// A frame came ahead of its layout and was not drawn: draw it when it comes
static std::atomic<bool> s_DeskHeld { false };

// Test hook: the next desk frame (the whole atlas, as decoded) goes to a PNG
static std::atomic<bool> s_DumpDeskFrame { false };
static QString s_DumpDeskFramePath;

void vtDeskDumpNextFrame(const QString& path)
{
    s_DumpDeskFramePath = path;
    s_DumpDeskFrame.store(true);
}

// Every desk window's layer, from deskwm (Rust), under its lock
static void forEachDeskLayer(const std::function<void(const DeskwmLayer&)>& fn)
{
    deskwm_for_each_layer([](void* ctx, const DeskwmLayer* layer) {
        (*static_cast<const std::function<void(const DeskwmLayer&)>*>(ctx))(*layer);
    }, (void*)&fn);
}

#define MAX_VIDEO_PLANES 3

class VTMetalRenderer;

@interface DisplayLinkDelegate : NSObject <CAMetalDisplayLinkDelegate>

- (id)initWithRenderer:(VTMetalRenderer *)renderer;

@end

class VTMetalRenderer : public VTBaseRenderer
{
public:
    VTMetalRenderer(bool hwAccel)
        : VTBaseRenderer(RendererType::VTMetal),
          m_HwAccel(hwAccel),
          m_Window(nullptr),
          m_HwContext(nullptr),
          m_MetalLayer(nullptr),
          m_MetalDisplayLink(nullptr),
          m_LatestUnrenderedFrame(nullptr),
          m_FrameLock(SDL_CreateMutex()),
          m_FrameReady(SDL_CreateCond()),
          m_TextureCache(nullptr),
          m_CscParamsBuffer(nullptr),
          m_VideoVertexBuffer(nullptr),
          m_OverlayTextures{},
          m_OverlayLock(0),
          m_VideoPipelineState(nullptr),
          m_OverlayPipelineState(nullptr),
          m_ShaderLibrary(nullptr),
          m_CommandQueue(nullptr),
          m_SwMappingTextures{},
          m_MetalView(nullptr),
          m_LastFrameWidth(-1),
          m_LastFrameHeight(-1),
          m_LastDrawableWidth(-1),
          m_LastDrawableHeight(-1)
    {
    }

    virtual ~VTMetalRenderer() override
    { @autoreleasepool {
        {
            std::lock_guard<std::mutex> lock(s_DeskRendererLock);
            if (s_DeskRenderer == this) {
                s_DeskRenderer = nullptr;
            }
        }
        {
            std::lock_guard<std::mutex> lock(m_DeskFrameLock);
            av_frame_free(&m_LastDeskFrame);
            av_frame_free(&m_HeldDeskFrame);
        }
        // Stop the display link and free associated state
        stopDisplayLink();
        av_frame_free(&m_LatestUnrenderedFrame);
        SDL_DestroyCond(m_FrameReady);
        SDL_DestroyMutex(m_FrameLock);

        if (m_HwContext != nullptr) {
            av_buffer_unref(&m_HwContext);
        }

        if (m_CscParamsBuffer != nullptr) {
            [m_CscParamsBuffer release];
        }

        if (m_VideoVertexBuffer != nullptr) {
            [m_VideoVertexBuffer release];
        }

        if (m_VideoPipelineState != nullptr) {
            [m_VideoPipelineState release];
        }

        for (int i = 0; i < Overlay::OverlayMax; i++) {
            if (m_OverlayTextures[i] != nullptr) {
                [m_OverlayTextures[i] release];
            }
        }

        for (int i = 0; i < MAX_VIDEO_PLANES; i++) {
            if (m_SwMappingTextures[i] != nullptr) {
                [m_SwMappingTextures[i] release];
            }
        }

        if (m_OverlayPipelineState != nullptr) {
            [m_OverlayPipelineState release];
        }

        if (m_ShaderLibrary != nullptr) {
            [m_ShaderLibrary release];
        }

        if (m_CommandQueue != nullptr) {
            [m_CommandQueue release];
        }

        if (m_TextureCache != nullptr) {
            CFRelease(m_TextureCache);
        }

        if (m_MetalView != nullptr) {
            SDL_Metal_DestroyView(m_MetalView);
        }
    }}

    bool updateVideoRegionSizeForFrame(AVFrame* frame)
    {
        if (m_AppWindowMode) {
            // Desk mode: the SDL window shows nothing; the desk's windows are
            // drawn by renderChildWindows.
            m_VideoVertexCount = 0;
            return true;
        }

        int drawableWidth, drawableHeight;
        SDL_Metal_GetDrawableSize(m_Window, &drawableWidth, &drawableHeight);

        // Check if anything has changed since the last vertex buffer upload
        if (m_VideoVertexBuffer &&
                frame->width == m_LastFrameWidth && frame->height == m_LastFrameHeight &&
                drawableWidth == m_LastDrawableWidth && drawableHeight == m_LastDrawableHeight) {
            // Nothing to do
            return true;
        }

        [m_VideoVertexBuffer release];
        m_VideoVertexBuffer = nullptr;
        auto bufferOptions = MTLCPUCacheModeWriteCombined | MTLResourceStorageModeManaged;

        // Determine the correct scaled size for the video region
        SDL_Rect src, dst;
        src.x = src.y = 0;
        src.w = frame->width;
        src.h = frame->height;
        dst.x = dst.y = 0;
        dst.w = drawableWidth;
        dst.h = drawableHeight;
        StreamUtils::scaleSourceToDestinationSurface(&src, &dst);

        {
            int winW = 0, winH = 0;
            SDL_GetWindowSize(m_Window, &winW, &winH);
            SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION,
                        "Metal video region: frame %dx%d, drawable %dx%d, window %dx%d, video rect (%d,%d) %dx%d",
                        frame->width, frame->height, drawableWidth, drawableHeight, winW, winH,
                        dst.x, dst.y, dst.w, dst.h);
        }

        // Convert screen space to normalized device coordinates
        SDL_FRect renderRect;
        StreamUtils::screenSpaceToNormalizedDeviceCoords(&dst, &renderRect, drawableWidth, drawableHeight);

        Vertex verts[] =
        {
            { { renderRect.x, renderRect.y, 0.0f, 1.0f }, { 0.0f, 1.0f } },
            { { renderRect.x, renderRect.y+renderRect.h, 0.0f, 1.0f }, { 0.0f, 0} },
            { { renderRect.x+renderRect.w, renderRect.y, 0.0f, 1.0f }, { 1.0f, 1.0f} },
            { { renderRect.x+renderRect.w, renderRect.y+renderRect.h, 0.0f, 1.0f }, { 1.0f, 0} },
        };
        m_VideoVertexCount = 4;
        m_VideoVertexBuffer = [m_MetalLayer.device newBufferWithBytes:verts length:sizeof(verts) options:bufferOptions];

        if (!m_VideoVertexBuffer && m_VideoVertexCount > 0) {
            SDL_LogError(SDL_LOG_CATEGORY_APPLICATION,
                         "Failed to create video vertex buffer");
            return false;
        }

        m_LastFrameWidth = frame->width;
        m_LastFrameHeight = frame->height;
        m_LastDrawableWidth = drawableWidth;
        m_LastDrawableHeight = drawableHeight;

        return true;
    }

    int getFramePlaneCount(AVFrame* frame)
    {
        if (frame->format == AV_PIX_FMT_VIDEOTOOLBOX) {
            return CVPixelBufferGetPlaneCount((CVPixelBufferRef)frame->data[3]);
        }
        else {
            return av_pix_fmt_count_planes((AVPixelFormat)frame->format);
        }
    }

    int getBitnessScaleFactor(AVFrame* frame)
    {
        if (frame->format == AV_PIX_FMT_VIDEOTOOLBOX) {
            // VideoToolbox frames never require scaling
            return 1;
        }
        else {
            const AVPixFmtDescriptor* formatDesc = av_pix_fmt_desc_get((AVPixelFormat)frame->format);
            if (!formatDesc) {
                // This shouldn't be possible but handle it anyway
                SDL_assert(formatDesc);
                return 1;
            }

            // This assumes plane 0 is exclusively the Y component
            SDL_assert(formatDesc->comp[0].step == 1 || formatDesc->comp[0].step == 2);
            return pow(2, (formatDesc->comp[0].step * 8) - formatDesc->comp[0].depth);
        }
    }

    bool updateColorSpaceForFrame(AVFrame* frame)
    {
        if (!hasFrameFormatChanged(frame) && !m_HdrMetadataChanged) {
            return true;
        }

        int colorspace = getFrameColorspace(frame);
        CGColorSpaceRef newColorSpace;
        ParamBuffer paramBuffer;

        // Stop the display link before changing the Metal layer
        stopDisplayLink();

        switch (colorspace) {
        case COLORSPACE_REC_709:
            m_MetalLayer.colorspace = newColorSpace = CGColorSpaceCreateWithName(kCGColorSpaceITUR_709);
            m_MetalLayer.pixelFormat = MTLPixelFormatBGRA8Unorm;
            break;
        case COLORSPACE_REC_2020:
            m_MetalLayer.pixelFormat = MTLPixelFormatBGR10A2Unorm;
            if (frame->color_trc == AVCOL_TRC_SMPTE2084) {
                // https://developer.apple.com/documentation/metal/hdr_content/using_color_spaces_to_display_hdr_content
                m_MetalLayer.colorspace = newColorSpace = CGColorSpaceCreateWithName(kCGColorSpaceITUR_2100_PQ);
            }
            else {
                m_MetalLayer.colorspace = newColorSpace = CGColorSpaceCreateWithName(kCGColorSpaceITUR_2020);
            }
            break;
        default:
        case COLORSPACE_REC_601:
            m_MetalLayer.colorspace = newColorSpace = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
            m_MetalLayer.pixelFormat = MTLPixelFormatBGRA8Unorm;
            break;
        }

        std::array<float, 9> cscMatrix;
        std::array<float, 3> yuvOffsets;
        std::array<float, 2> chromaOffset;
        getFramePremultipliedCscConstants(frame, cscMatrix, yuvOffsets);
        getFrameChromaCositingOffsets(frame, chromaOffset);

        paramBuffer.cscParams.matrix = simd_matrix(simd_make_half3(cscMatrix[0], cscMatrix[3], cscMatrix[6]),
                                                   simd_make_half3(cscMatrix[1], cscMatrix[4], cscMatrix[7]),
                                                   simd_make_half3(cscMatrix[2], cscMatrix[5], cscMatrix[8]));
        paramBuffer.cscParams.offsets = simd_make_half3(yuvOffsets[0],
                                                        yuvOffsets[1],
                                                        yuvOffsets[2]);
        paramBuffer.chromaOffset = simd_make_half2(chromaOffset[0],
                                                   chromaOffset[1]);

        // Set the EDR metadata for HDR10 to enable OS tonemapping
        if (frame->color_trc == AVCOL_TRC_SMPTE2084 && m_MasteringDisplayColorVolume != nullptr) {
            m_MetalLayer.EDRMetadata = [CAEDRMetadata HDR10MetadataWithDisplayInfo:(__bridge NSData*)m_MasteringDisplayColorVolume
                                                                       contentInfo:(__bridge NSData*)m_ContentLightLevelInfo
                                                                opticalOutputScale:203.0];
        }
        else {
            m_MetalLayer.EDRMetadata = nullptr;
        }

        paramBuffer.bitnessScaleFactor = getBitnessScaleFactor(frame);

        // The CAMetalLayer retains the CGColorSpace
        CGColorSpaceRelease(newColorSpace);

        // Create the new colorspace parameter buffer for our fragment shader
        [m_CscParamsBuffer release];
        auto bufferOptions = MTLCPUCacheModeWriteCombined | MTLResourceStorageModeManaged;
        m_CscParamsBuffer = [m_MetalLayer.device newBufferWithBytes:(void*)&paramBuffer length:sizeof(paramBuffer) options:bufferOptions];
        if (!m_CscParamsBuffer) {
            SDL_LogError(SDL_LOG_CATEGORY_APPLICATION,
                         "Failed to create CSC parameters buffer");
            return false;
        }

        int planes = getFramePlaneCount(frame);
        SDL_assert(planes == 2 || planes == 3);

        MTLRenderPipelineDescriptor *pipelineDesc = [[MTLRenderPipelineDescriptor new] autorelease];
        if (m_AppWindowMode) {
            // Desk windows: rounded, premultiplied output over a transparent
            // window, transparent outside the frame
            pipelineDesc.vertexFunction = [[m_ShaderLibrary newFunctionWithName:@"vs_draw_aw"] autorelease];
            pipelineDesc.fragmentFunction = [[m_ShaderLibrary newFunctionWithName:planes == 2 ? @"ps_draw_biplanar_desk" : @"ps_draw_triplanar_desk"] autorelease];
            pipelineDesc.colorAttachments[0].blendingEnabled = YES;
            pipelineDesc.colorAttachments[0].rgbBlendOperation = MTLBlendOperationAdd;
            pipelineDesc.colorAttachments[0].alphaBlendOperation = MTLBlendOperationAdd;
            pipelineDesc.colorAttachments[0].sourceRGBBlendFactor = MTLBlendFactorOne;
            pipelineDesc.colorAttachments[0].sourceAlphaBlendFactor = MTLBlendFactorOne;
            pipelineDesc.colorAttachments[0].destinationRGBBlendFactor = MTLBlendFactorOneMinusSourceAlpha;
            pipelineDesc.colorAttachments[0].destinationAlphaBlendFactor = MTLBlendFactorOneMinusSourceAlpha;
        }
        else {
            pipelineDesc.vertexFunction = [[m_ShaderLibrary newFunctionWithName:@"vs_draw"] autorelease];
            pipelineDesc.fragmentFunction = [[m_ShaderLibrary newFunctionWithName:planes == 2 ? @"ps_draw_biplanar" : @"ps_draw_triplanar"] autorelease];
        }
        pipelineDesc.colorAttachments[0].pixelFormat = m_MetalLayer.pixelFormat;
        [m_VideoPipelineState release];
        m_VideoPipelineState = [m_MetalLayer.device newRenderPipelineStateWithDescriptor:pipelineDesc error:nullptr];
        if (!m_VideoPipelineState) {
            SDL_LogError(SDL_LOG_CATEGORY_APPLICATION,
                         "Failed to create video pipeline state");
            return false;
        }

        pipelineDesc = [[MTLRenderPipelineDescriptor new] autorelease];
        pipelineDesc.vertexFunction = [[m_ShaderLibrary newFunctionWithName:@"vs_draw"] autorelease];
        pipelineDesc.fragmentFunction = [[m_ShaderLibrary newFunctionWithName:@"ps_draw_rgb"] autorelease];
        pipelineDesc.colorAttachments[0].pixelFormat = m_MetalLayer.pixelFormat;
        pipelineDesc.colorAttachments[0].blendingEnabled = YES;
        pipelineDesc.colorAttachments[0].rgbBlendOperation = MTLBlendOperationAdd;
        pipelineDesc.colorAttachments[0].alphaBlendOperation = MTLBlendOperationAdd;
        pipelineDesc.colorAttachments[0].sourceRGBBlendFactor = MTLBlendFactorSourceAlpha;
        pipelineDesc.colorAttachments[0].sourceAlphaBlendFactor = MTLBlendFactorSourceAlpha;
        pipelineDesc.colorAttachments[0].destinationRGBBlendFactor = MTLBlendFactorOneMinusSourceAlpha;
        pipelineDesc.colorAttachments[0].destinationAlphaBlendFactor = MTLBlendFactorOneMinusSourceAlpha;
        [m_OverlayPipelineState release];
        m_OverlayPipelineState = [m_MetalLayer.device newRenderPipelineStateWithDescriptor:pipelineDesc error:nullptr];
        if (!m_OverlayPipelineState) {
            SDL_LogError(SDL_LOG_CATEGORY_APPLICATION,
                         "Failed to create overlay pipeline state");
            return false;
        }

        m_HdrMetadataChanged = false;
        return true;
    }

    id<MTLTexture> mapPlaneForSoftwareFrame(AVFrame* frame, int planeIndex)
    {
        const AVPixFmtDescriptor* formatDesc = av_pix_fmt_desc_get((AVPixelFormat)frame->format);
        if (!formatDesc) {
            // This shouldn't be possible but handle it anyway
            SDL_assert(formatDesc);
            return nil;
        }

        SDL_assert(planeIndex < MAX_VIDEO_PLANES);

        NSUInteger planeWidth = planeIndex ? AV_CEIL_RSHIFT(frame->width, formatDesc->log2_chroma_w) : frame->width;
        NSUInteger planeHeight = planeIndex ? AV_CEIL_RSHIFT(frame->height, formatDesc->log2_chroma_h) : frame->height;

        // Recreate the texture if the plane size changes
        if (m_SwMappingTextures[planeIndex] && (m_SwMappingTextures[planeIndex].width != planeWidth ||
                                                m_SwMappingTextures[planeIndex].height != planeHeight)) {
            [m_SwMappingTextures[planeIndex] release];
            m_SwMappingTextures[planeIndex] = nil;
        }

        if (!m_SwMappingTextures[planeIndex]) {
            MTLPixelFormat metalFormat;

            switch (formatDesc->comp[planeIndex].step) {
            case 1:
                metalFormat = MTLPixelFormatR8Unorm;
                break;
            case 2:
                metalFormat = MTLPixelFormatR16Unorm;
                break;
            default:
                SDL_LogError(SDL_LOG_CATEGORY_APPLICATION,
                             "Unhandled plane step: %d (plane: %d)",
                             formatDesc->comp[planeIndex].step,
                             planeIndex);
                SDL_assert(false);
                return nil;
            }

            auto texDesc = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:metalFormat
                                                                              width:planeWidth
                                                                             height:planeHeight
                                                                          mipmapped:NO];
            texDesc.cpuCacheMode = MTLCPUCacheModeWriteCombined;
            texDesc.storageMode = MTLStorageModeManaged;
            texDesc.usage = MTLTextureUsageShaderRead;

            m_SwMappingTextures[planeIndex] = [m_MetalLayer.device newTextureWithDescriptor:texDesc];
            if (!m_SwMappingTextures[planeIndex]) {
                SDL_LogError(SDL_LOG_CATEGORY_APPLICATION,
                             "Failed to allocate software frame texture");
                return nil;
            }
        }

        [m_SwMappingTextures[planeIndex] replaceRegion:MTLRegionMake2D(0, 0, planeWidth, planeHeight)
                                           mipmapLevel:0
                                             withBytes:frame->data[planeIndex]
                                           bytesPerRow:frame->linesize[planeIndex]];

        return m_SwMappingTextures[planeIndex];
    }

    bool createTexturesFromFrame(AVFrame* frame, std::array<CVMetalTextureRef, MAX_VIDEO_PLANES>& cvMetalTextures)
    {
        SDL_assert(frame->format == AV_PIX_FMT_VIDEOTOOLBOX);

        CVPixelBufferRef pixBuf = reinterpret_cast<CVPixelBufferRef>(frame->data[3]);
        size_t planes = getFramePlaneCount(frame);

        // Create Metal textures for the planes of the CVPixelBuffer
        for (size_t i = 0; i < planes; i++) {
            MTLPixelFormat fmt;

            switch (CVPixelBufferGetPixelFormatType(pixBuf)) {
            case kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange:
            case kCVPixelFormatType_444YpCbCr8BiPlanarVideoRange:
            case kCVPixelFormatType_420YpCbCr8BiPlanarFullRange:
            case kCVPixelFormatType_444YpCbCr8BiPlanarFullRange:
                fmt = (i == 0) ? MTLPixelFormatR8Unorm : MTLPixelFormatRG8Unorm;
                break;

            case kCVPixelFormatType_420YpCbCr10BiPlanarFullRange:
            case kCVPixelFormatType_444YpCbCr10BiPlanarFullRange:
            case kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange:
            case kCVPixelFormatType_444YpCbCr10BiPlanarVideoRange:
                fmt = (i == 0) ? MTLPixelFormatR16Unorm : MTLPixelFormatRG16Unorm;
                break;

            default:
                SDL_LogError(SDL_LOG_CATEGORY_APPLICATION,
                             "Unknown pixel format: %x",
                             CVPixelBufferGetPixelFormatType(pixBuf));
                return false;
            }

            CVReturn err = CVMetalTextureCacheCreateTextureFromImage(kCFAllocatorDefault, m_TextureCache, pixBuf, nullptr, fmt,
                                                                     CVPixelBufferGetWidthOfPlane(pixBuf, i),
                                                                     CVPixelBufferGetHeightOfPlane(pixBuf, i),
                                                                     i,
                                                                     &cvMetalTextures[i]);
            if (err != kCVReturnSuccess) {
                for (size_t j = 0; j < i; j++) {
                    CFRelease(cvMetalTextures[j]);
                }
                SDL_LogError(SDL_LOG_CATEGORY_APPLICATION,
                             "CVMetalTextureCacheCreateTextureFromImage() failed: %d",
                             err);
                return false;
            }
        }

        return true;
    }

    bool testRenderFrame(AVFrame *frame) override
    { @autoreleasepool {
        if (frame->format == AV_PIX_FMT_VIDEOTOOLBOX) {
            std::array<CVMetalTextureRef, MAX_VIDEO_PLANES> cvMetalTextures;
            size_t planes = getFramePlaneCount(frame);
            SDL_assert(planes <= MAX_VIDEO_PLANES);

            // Test that we can actually create Metal textures from the CVPixelBufferRef
            if (!createTexturesFromFrame(frame, cvMetalTextures)) {
                return false;
            }

            for (size_t i = 0; i < planes; i++) {
                CFRelease(cvMetalTextures[i]);
            }
        }
        else {
            // Mapping software frames should always work
        }

        return true;
    }}

    // Child dialogs of an app window have Mac windows of their own, each
    // showing its rectangle of the canvas: draw those with the same pipeline
    // and textures, one drawable per child, on the same command buffer.
    // only: just that window; deferred: drawables to present after the
    // command buffer is scheduled (a layer presenting with the main thread's
    // transaction, in a live resize), else presented with the buffer
    void renderChildWindows(AVFrame* frame, id<MTLCommandBuffer> commandBuffer, size_t planes,
                            const std::array<CVMetalTextureRef, MAX_VIDEO_PLANES>& cvMetalTextures,
                            uint64_t only = 0, std::vector<id<CAMetalDrawable>>* deferred = nullptr)
    {
        m_DeskResult = 0;
        // Windows in a live resize are drawn by the main thread, in step with
        // their size: a new frame asks it to
        QVector<uint64_t> toMain;
        // Which layout this frame shows: its geometry is the one to draw it with
        const int frameSeq = readDeskMarker(frame);
        const int layoutSeq = deskwm_layout_seq();
        static const bool debugSeq = qEnvironmentVariableIsSet("DESKWM_DEBUG_RESIZE");
        if (debugSeq && deferred == nullptr) {
            SDL_Log("desk frame#%d layout#%d", frameSeq, layoutSeq);
        }
        // A frame whose number cannot be read (damaged by the encoder) is
        // not guessed at: windows keep what they show (but a host that keeps
        // sending unreadable numbers is drawn anyway, by time)
        static int unreadable = 0;
        if (deferred == nullptr && layoutSeq >= 0) {
            unreadable = frameSeq < 0 ? unreadable + 1 : 0;
            if (frameSeq < 0 && unreadable < 30) {
                m_DeskResult = 2;
                return;
            }
        }
        if (frameSeq >= 0 && layoutSeq >= 0) {
            const int ahead = (frameSeq - layoutSeq) & 0xfff;
            if (ahead != 0 && ahead < 2048) {
                // Its layout has not come yet: every window keeps what it
                // shows until it does (then this frame is drawn)
                s_DeskHeld.store(true);
                m_DeskResult = 1;
                return;
            }
        }
        // Picture sizes go back to deskwm after the walk (it holds the layers' lock meanwhile)
        QVector<std::tuple<uint64_t, int, int>> shown;
        forEachDeskLayer([&](const DeskwmLayer& child) {
            CAMetalLayer* layer = (__bridge CAMetalLayer*)child.metal_layer;
            if (!layer || !child.visible || child.geo_count == 0) {
                return;
            }
            if (layer.device != m_MetalLayer.device || layer.pixelFormat != m_MetalLayer.pixelFormat) {
                layer.device = m_MetalLayer.device;
                layer.pixelFormat = m_MetalLayer.pixelFormat;
                layer.colorspace = m_MetalLayer.colorspace;
                layer.displaySyncEnabled = m_MetalLayer.displaySyncEnabled;
            }
            if (only != 0 && child.hwnd != only) {
                return;
            }
            if (deferred == nullptr && layer.presentsWithTransaction) {
                toMain.append(child.hwnd);
                return;
            }
            if (layer.drawableSize.width < 1 || layer.drawableSize.height < 1) {
                return;
            }
            // The window's picture in this frame: the geometry of the layout
            // the frame shows (its newest change at or before that number);
            // without numbers, the newest geometry stamped at or before the
            // frame's capture time. None yet (a window that has just
            // appeared): nothing to draw.
            int pick = -1;
            bool numbered = false;
            if (frameSeq >= 0) {
                for (int i = (int)child.geo_count - 1; i >= 0; i--) {
                    numbered = numbered || child.geo[i].seq >= 0;
                    if (child.geo[i].seq >= 0 && ((frameSeq - child.geo[i].seq) & 0xfff) < 2048) {
                        pick = i;
                        break;
                    }
                }
                if (pick < 0 && numbered) {
                    return;  // the window came after this frame
                }
            }
            if (pick < 0) {
                pick = (int)child.geo_count - 1;
            }
            if (!numbered && frame->pts != AV_NOPTS_VALUE) {
                const quint32 pts = (quint32)frame->pts;
                pick = -1;
                for (int i = (int)child.geo_count - 1; i >= 0; i--) {
                    if ((qint32)(pts - child.geo[i].ts) >= 0) {
                        pick = i;
                        break;
                    }
                }
                if (pick < 0) {
                    return;
                }
            }
            const DeskwmGeo g = child.geo[pick];
            if (g.w <= 0 || g.h <= 0) {
                return;
            }
            // Never wait for a drawable: a window that is not being presented
            // (hidden, mid-animation, in Mission Control) would stall every
            // other window. Two in flight and nothing presented lately: skip.
            std::shared_ptr<Inflight> inflight = m_Inflight.value(child.hwnd);
            if (!inflight) {
                inflight = std::make_shared<Inflight>();
                m_Inflight.insert(child.hwnd, inflight);
            }
            const double now = CACurrentMediaTime();
            if (inflight->count.load() >= 2) {
                if (now - inflight->lastPresented.load() < 1.0) {
                    return;
                }
                inflight->count.store(0);  // give it another try once a second
            }
            id<CAMetalDrawable> drawable = [layer nextDrawable];
            if (!drawable) {
                return;
            }
            inflight->count.fetch_add(1);
            std::shared_ptr<Inflight> captured = inflight;
            [drawable addPresentedHandler:^(id<MTLDrawable>) {
                captured->count.fetch_sub(1);
                captured->lastPresented.store(CACurrentMediaTime());
            }];
            // The picture in host px (a full atlas may hold it scaled down)
            const float scale = g.scale > 0.0f ? g.scale : 1.0f;
            int pw = (int)lroundf(g.w / scale), ph = (int)lroundf(g.h / scale);
            shown.append(std::make_tuple(child.hwnd, pw, ph));
            // Mid-resize: never larger than the size last asked of the host
            // (it may already show the window smaller than this layout says)
            if (child.cap_w > 0 && child.cap_h > 0 && !(g.ax >= 0 && g.ay >= 0)) {
                pw = qMin(pw, child.cap_w);
                ph = qMin(ph, child.cap_h);
            }
            // Drawn 1:1 against the edges it hugs; cut (never scaled) where the
            // window is smaller than the picture
            const int vw = qMax(1, child.view_w), vh = qMax(1, child.view_h);
            const int dw = qMin(pw, vw), dh = qMin(ph, vh);
            static const bool debugResize = qEnvironmentVariableIsSet("DESKWM_DEBUG_RESIZE");
            if (debugResize && layer.presentsWithTransaction) {
                SDL_Log("resize draw %llx t=%.4f view=%dx%d picture=%dx%d %s frame#%d layout#%d", (unsigned long long)child.hwnd, CACurrentMediaTime(), vw, vh, pw, ph, deferred ? "main" : "frame", frameSeq, layoutSeq);
            }
            const float sx = (float)g.x + (child.anchor_right ? (pw - dw) * scale : 0.0f);
            const float sy = (float)g.y + (child.anchor_bottom ? (ph - dh) * scale : 0.0f);
            const int ox = child.anchor_right ? vw - dw : 0, oy = child.anchor_bottom ? vh - dh : 0;
            const bool alpha = g.ax >= 0 && g.ay >= 0;
            // Opaque windows get their rounded corners here, at the Mac
            // window's own corners (in a live resize the picture may be
            // smaller or larger than it); a picture with alpha carries its
            // own shape
            const simd_float4 rect = { (float)(alpha ? dw : vw), (float)(alpha ? dh : vh), alpha ? 0.0f : (float)child.radius, alpha ? 1.0f : 0.0f };
            // The copy on white (alpha_scale times the size): per corner, its
            // texture coordinates minus the picture's
            const float ar = g.alpha_scale > 0.0f ? g.alpha_scale : 1.0f;
            auto whiteAt = [&](float tx, float ty) -> simd_float2 {
                if (!alpha) {
                    return simd_float2 { 0.0f, 0.0f };
                }
                return simd_float2 { (g.ax + (tx - g.x) * ar - tx) / frame->width, (g.ay + (ty - g.y) * ar - ty) / frame->height };
            };
            // One quad: drawable px [dx0, dx1) x [dy0, dy1) from frame px at (u0, v0) (u1, v1)
            std::vector<VertexAW> verts;
            auto quad = [&](float dx0, float dy0, float dx1, float dy1, float tx0, float ty0, float tx1, float ty1) {
                const float xl = -1.0f + 2.0f * dx0 / vw, xr = -1.0f + 2.0f * dx1 / vw;
                const float yt = 1.0f - 2.0f * dy0 / vh, yb = 1.0f - 2.0f * dy1 / vh;
                const float u0 = tx0 / frame->width, u1 = tx1 / frame->width;
                const float v0 = ty0 / frame->height, v1 = ty1 / frame->height;
                const float lx0 = alpha ? dx0 - ox : dx0, lx1 = alpha ? dx1 - ox : dx1, ly0 = alpha ? dy0 - oy : dy0, ly1 = alpha ? dy1 - oy : dy1;
                const simd_float2 w00 = whiteAt(tx0, ty0), w10 = whiteAt(tx1, ty0), w01 = whiteAt(tx0, ty1), w11 = whiteAt(tx1, ty1);
                verts.push_back({ { xl, yb, 0.0f, 1.0f }, { u0, v1 }, { lx0, ly1 }, rect, w01 });
                verts.push_back({ { xl, yt, 0.0f, 1.0f }, { u0, v0 }, { lx0, ly0 }, rect, w00 });
                verts.push_back({ { xr, yb, 0.0f, 1.0f }, { u1, v1 }, { lx1, ly1 }, rect, w11 });
                verts.push_back({ { xr, yb, 0.0f, 1.0f }, { u1, v1 }, { lx1, ly1 }, rect, w11 });
                verts.push_back({ { xl, yt, 0.0f, 1.0f }, { u0, v0 }, { lx0, ly0 }, rect, w00 });
                verts.push_back({ { xr, yt, 0.0f, 1.0f }, { u1, v0 }, { lx1, ly0 }, rect, w10 });
            };
            if (alpha && g.strip_width > 0 && scale == 1.0f) {
                // The copy on white only at the edges: there alpha comes from
                // the two copies, the inside is opaque. Popups never resize
                // live: 1:1 against their anchors.
                const int b = g.strip_width;
                const int px0 = child.anchor_right ? pw - dw : 0, py0 = child.anchor_bottom ? ph - dh : 0;
                const simd_float4 opaque = { (float)dw, (float)dh, 0.0f, 0.0f };
                // Part [x0, x1) x [y0, y1) of the picture; its copy on white starts at
                // (wx, wy) for picture point (rx, ry) (opaque: none)
                auto band = [&](int x0, int y0, int x1, int y1, bool op, int wx, int wy, int rx, int ry) {
                    x0 = qMax(x0, px0); x1 = qMin(x1, px0 + dw);
                    y0 = qMax(y0, py0); y1 = qMin(y1, py0 + dh);
                    if (x0 >= x1 || y0 >= y1) {
                        return;
                    }
                    const simd_float2 off = op ? simd_float2 { 0.0f, 0.0f }
                                               : simd_float2 { (float)(wx - rx - g.x) / frame->width, (float)(wy - ry - g.y) / frame->height };
                    const float dx0 = ox + x0 - px0, dx1 = ox + x1 - px0, dy0 = oy + y0 - py0, dy1 = oy + y1 - py0;
                    const float tx0 = g.x + x0, tx1 = g.x + x1, ty0 = g.y + y0, ty1 = g.y + y1;
                    const float xl = -1.0f + 2.0f * dx0 / vw, xr = -1.0f + 2.0f * dx1 / vw;
                    const float yt = 1.0f - 2.0f * dy0 / vh, yb = 1.0f - 2.0f * dy1 / vh;
                    const float u0 = tx0 / frame->width, u1 = tx1 / frame->width, v0 = ty0 / frame->height, v1 = ty1 / frame->height;
                    const simd_float4 r = op ? opaque : rect;
                    const float lx0 = dx0 - ox, lx1 = dx1 - ox, ly0 = dy0 - oy, ly1 = dy1 - oy;
                    verts.push_back({ { xl, yb, 0.0f, 1.0f }, { u0, v1 }, { lx0, ly1 }, r, off });
                    verts.push_back({ { xl, yt, 0.0f, 1.0f }, { u0, v0 }, { lx0, ly0 }, r, off });
                    verts.push_back({ { xr, yb, 0.0f, 1.0f }, { u1, v1 }, { lx1, ly1 }, r, off });
                    verts.push_back({ { xr, yb, 0.0f, 1.0f }, { u1, v1 }, { lx1, ly1 }, r, off });
                    verts.push_back({ { xl, yt, 0.0f, 1.0f }, { u0, v0 }, { lx0, ly0 }, r, off });
                    verts.push_back({ { xr, yt, 0.0f, 1.0f }, { u1, v0 }, { lx1, ly0 }, r, off });
                };
                band(0, 0, pw, b, false, g.strips[0], g.strips[1], 0, 0);
                band(0, ph - b, pw, ph, false, g.strips[2], g.strips[3], 0, ph - b);
                band(0, b, b, ph - b, false, g.strips[4], g.strips[5], 0, b);
                band(pw - b, b, pw, ph - b, false, g.strips[6], g.strips[7], pw - b, b);
                band(b, b, pw - b, ph - b, true, 0, 0, 0, 0);
            }
            else if (alpha) {
                // Popups never resize live: 1:1 against their anchors
                quad(ox, oy, ox + dw, oy + dh, sx, sy, sx + dw * scale, sy + dh * scale);
            }
            else {
                // Opaque windows: the picture keeps both edges where the
                // window has them. Its last 320 px (caption buttons,
                // scroll bar) stay against the right edge, its last 120 px
                // (status bar) against the bottom; a window smaller than its
                // picture (the host has not drawn the new size yet) loses
                // the difference in the middle, a larger one gets the
                // middle stretched there.
                struct Seg {
                    int d0, d1;  // window px
                    int s0, s1;  // picture px (one column, stretched, when the sizes differ)
                };
                auto segments = [](int P, int V, int most) {
                    std::vector<Seg> out;
                    if (P == V) {
                        out.push_back({ 0, V, 0, P });
                        return out;
                    }
                    const int S = qMax(0, qMin(most, qMin(P, V) / 3));
                    if (V > P) {
                        out.push_back({ 0, P - S, 0, P - S });
                        out.push_back({ P - S, V - S, qMax(0, P - S - 1), qMax(1, P - S) });
                        out.push_back({ V - S, V, P - S, P });
                    }
                    else {
                        out.push_back({ 0, V - S, 0, V - S });
                        out.push_back({ V - S, V, P - S, P });
                    }
                    out.erase(std::remove_if(out.begin(), out.end(), [](const Seg& g) { return g.d1 <= g.d0 || g.s1 <= g.s0; }), out.end());
                    return out;
                };
                const std::vector<Seg> xs = segments(pw, vw, 320), ys = segments(ph, vh, 120);
                // Part [x0, x1) x [y0, y1) of the picture, at (ax, ay) in the frame, sc frame px per picture px
                auto region = [&](int x0, int y0, int x1, int y1, float ax, float ay, float sc) {
                    for (const Seg& gx : xs) {
                        const int a0 = qMax(x0, gx.s0), a1 = qMin(x1, gx.s1);
                        if (a0 >= a1) {
                            continue;
                        }
                        const bool stretchX = gx.s1 - gx.s0 != gx.d1 - gx.d0;
                        const float dx0 = stretchX ? gx.d0 : gx.d0 + (a0 - gx.s0), dx1 = stretchX ? gx.d1 : gx.d0 + (a1 - gx.s0);
                        const float tx0 = stretchX ? ax + (a0 + 0.5f - x0) * sc : ax + (a0 - x0) * sc;
                        const float tx1 = stretchX ? tx0 : ax + (a1 - x0) * sc;
                        for (const Seg& gy : ys) {
                            const int b0 = qMax(y0, gy.s0), b1 = qMin(y1, gy.s1);
                            if (b0 >= b1) {
                                continue;
                            }
                            const bool stretchY = gy.s1 - gy.s0 != gy.d1 - gy.d0;
                            const float dy0 = stretchY ? gy.d0 : gy.d0 + (b0 - gy.s0), dy1 = stretchY ? gy.d1 : gy.d0 + (b1 - gy.s0);
                            const float ty0 = stretchY ? ay + (b0 + 0.5f - y0) * sc : ay + (b0 - y0) * sc;
                            const float ty1 = stretchY ? ty0 : ay + (b1 - y0) * sc;
                            quad(dx0, dy0, dx1, dy1, tx0, ty0, tx1, ty1);
                        }
                    }
                };
                region(0, 0, pw, ph, (float)g.x, (float)g.y, scale);
                // The parts the user sees, 1:1 over the smaller picture
                for (uint32_t k = 0; k < g.piece_count && k < DESKWM_PIECE_MAX; k++) {
                    const DeskwmPiece& p = g.pieces[k];
                    region(p.x, p.y, p.x + p.w, p.y + p.h, (float)p.ax, (float)p.ay, 1.0f);
                }
            }
            id<MTLBuffer> vb = [m_MetalLayer.device newBufferWithBytes:verts.data() length:verts.size() * sizeof(VertexAW) options:MTLResourceStorageModeShared];
            auto pass = [MTLRenderPassDescriptor renderPassDescriptor];
            pass.colorAttachments[0].texture = drawable.texture;
            pass.colorAttachments[0].loadAction = MTLLoadActionClear;
            pass.colorAttachments[0].clearColor = MTLClearColorMake(0.0, 0.0, 0.0, 0.0);
            pass.colorAttachments[0].storeAction = MTLStoreActionStore;
            auto encoder = [commandBuffer renderCommandEncoderWithDescriptor:pass];
            [encoder setRenderPipelineState:m_VideoPipelineState];
            if (frame->format == AV_PIX_FMT_VIDEOTOOLBOX) {
                for (size_t i = 0; i < planes; i++) {
                    [encoder setFragmentTexture:CVMetalTextureGetTexture(cvMetalTextures[i]) atIndex:i];
                }
            }
            else {
                for (size_t i = 0; i < planes; i++) {
                    [encoder setFragmentTexture:mapPlaneForSoftwareFrame(frame, i) atIndex:i];
                }
            }
            [encoder setFragmentBuffer:m_CscParamsBuffer offset:0 atIndex:0];
            [encoder setVertexBuffer:vb offset:0 atIndex:0];
            [encoder drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:verts.size()];
            [encoder endEncoding];
            if (deferred != nullptr && layer.presentsWithTransaction) {
                deferred->push_back(drawable);
            }
            else {
                [commandBuffer presentDrawable:drawable];
            }
            [vb release];
        });
        for (const auto& [hwnd, w, h] : shown) {
            deskwm_shown(hwnd, w, h);
        }
        for (uint64_t hwnd : toMain) {
            dispatch_async(dispatch_get_main_queue(), ^{
                vtDeskRedraw(hwnd);
            });
        }
    }

public:
    // Main thread: draws one window again from the latest frame, presented
    // with the current Core Animation transaction (its new size)
    void redrawDeskWindow(uint64_t hwnd)
    { @autoreleasepool {
        // 0: all windows, once a frame held back for its layout can be drawn
        if (hwnd == 0 && !s_DeskHeld.exchange(false)) {
            return;
        }
        std::lock_guard<std::mutex> lock(m_DeskFrameLock);
        // 0: the held frame (its layout has come); a window: the last drawn
        AVFrame* frame = hwnd == 0 ? m_HeldDeskFrame : m_LastDeskFrame;
        if (frame == nullptr || frame->buf[0] == nullptr || frame->format != AV_PIX_FMT_VIDEOTOOLBOX) {
            return;
        }
        std::array<CVMetalTextureRef, MAX_VIDEO_PLANES> cvMetalTextures;
        size_t planes = getFramePlaneCount(frame);
        if (!createTexturesFromFrame(frame, cvMetalTextures)) {
            return;
        }
        auto commandBuffer = [m_CommandQueue commandBuffer];
        CVPixelBufferRef pixBuf = reinterpret_cast<CVPixelBufferRef>(frame->data[3]);
        CVPixelBufferRetain(pixBuf);
        [commandBuffer addCompletedHandler:^(id<MTLCommandBuffer>) {
            for (size_t i = 0; i < planes; i++) {
                CFRelease(cvMetalTextures[i]);
            }
            CVPixelBufferRelease(pixBuf);
        }];
        std::vector<id<CAMetalDrawable>> deferred;
        renderChildWindows(frame, commandBuffer, planes, cvMetalTextures, hwnd, &deferred);
        [commandBuffer commit];
        if (hwnd == 0 && m_DeskResult == 0 && m_LastDeskFrame != nullptr) {
            av_frame_unref(m_LastDeskFrame);
            av_frame_move_ref(m_LastDeskFrame, m_HeldDeskFrame);
        }
        [commandBuffer waitUntilScheduled];
        for (id<CAMetalDrawable> drawable : deferred) {
            [drawable present];
        }
    }}

public:

    // Caller frees frame after we return
    // Desk mode: nothing is drawn in the (hidden) SDL window, so no drawable
    // of its layer is needed; the desk windows' layers get the frame.
    void renderDeskFrame(AVFrame* frame)
    { @autoreleasepool {
        // One draw at a time: main-thread redraws use the same state
        std::lock_guard<std::mutex> lock(m_DeskFrameLock);
        if (s_DumpDeskFrame.exchange(false) && frame->format == AV_PIX_FMT_VIDEOTOOLBOX) {
            CIImage* image = [CIImage imageWithCVPixelBuffer:reinterpret_cast<CVPixelBufferRef>(frame->data[3])];
            CGColorSpaceRef rgb = CGColorSpaceCreateDeviceRGB();
            NSURL* url = [NSURL fileURLWithPath:s_DumpDeskFramePath.toNSString()];
            [[CIContext context] writePNGRepresentationOfImage:image toURL:url format:kCIFormatRGBA8 colorSpace:rgb options:@{} error:nil];
            CGColorSpaceRelease(rgb);
        }
        std::array<CVMetalTextureRef, MAX_VIDEO_PLANES> cvMetalTextures;
        size_t planes = getFramePlaneCount(frame);
        SDL_assert(planes <= MAX_VIDEO_PLANES);
        if (frame->format == AV_PIX_FMT_VIDEOTOOLBOX) {
            if (!createTexturesFromFrame(frame, cvMetalTextures)) {
                return;
            }
        }
        auto commandBuffer = [m_CommandQueue commandBuffer];
        if (frame->format == AV_PIX_FMT_VIDEOTOOLBOX) {
            CVPixelBufferRef pixBuf = reinterpret_cast<CVPixelBufferRef>(frame->data[3]);
            CVPixelBufferRetain(pixBuf);
            [commandBuffer addCompletedHandler:^(id<MTLCommandBuffer>) {
                for (size_t i = 0; i < planes; i++) {
                    CFRelease(cvMetalTextures[i]);
                }
                CVPixelBufferRelease(pixBuf);
            }];
        }
        renderChildWindows(frame, commandBuffer, planes, cvMetalTextures);
        [commandBuffer commit];
        // Redraws use the last frame drawn; a frame held for its layout is
        // drawn when the layout comes; an unreadable one never
        AVFrame* keep = m_DeskResult == 0 ? m_LastDeskFrame : m_DeskResult == 1 ? m_HeldDeskFrame : nullptr;
        if (keep != nullptr) {
            av_frame_unref(keep);
            av_frame_ref(keep, frame);
        }
        if (m_DeskResult == 0 && m_HeldDeskFrame != nullptr) {
            av_frame_unref(m_HeldDeskFrame);
        }
    }}

    virtual void renderFrameIntoDrawable(AVFrame* frame, id<CAMetalDrawable> drawable)
    { @autoreleasepool {
        std::array<CVMetalTextureRef, MAX_VIDEO_PLANES> cvMetalTextures;
        size_t planes = getFramePlaneCount(frame);
        SDL_assert(planes <= MAX_VIDEO_PLANES);

        if (frame->format == AV_PIX_FMT_VIDEOTOOLBOX) {
            if (!createTexturesFromFrame(frame, cvMetalTextures)) {
                return;
            }
        }

        // Prepare a render pass to render into the next drawable
        auto renderPassDescriptor = [MTLRenderPassDescriptor renderPassDescriptor];
        renderPassDescriptor.colorAttachments[0].texture = drawable.texture;
        renderPassDescriptor.colorAttachments[0].loadAction = MTLLoadActionClear;
        renderPassDescriptor.colorAttachments[0].clearColor = MTLClearColorMake(0.0, 0.0, 0.0, 0.0);
        renderPassDescriptor.colorAttachments[0].storeAction = MTLStoreActionStore;
        auto commandBuffer = [m_CommandQueue commandBuffer];
        auto renderEncoder = [commandBuffer renderCommandEncoderWithDescriptor:renderPassDescriptor];

        // Bind textures and buffers then draw the video region
        [renderEncoder setRenderPipelineState:m_VideoPipelineState];
        if (frame->format == AV_PIX_FMT_VIDEOTOOLBOX) {
            for (size_t i = 0; i < planes; i++) {
                [renderEncoder setFragmentTexture:CVMetalTextureGetTexture(cvMetalTextures[i]) atIndex:i];
            }
            // Keep the decoded pixel buffer alive until the GPU has read it, since
            // we no longer block on command buffer completion below.
            CVPixelBufferRef pixBuf = reinterpret_cast<CVPixelBufferRef>(frame->data[3]);
            CVPixelBufferRetain(pixBuf);
            [commandBuffer addCompletedHandler:^(id<MTLCommandBuffer>) {
                // Free textures after completion of rendering per CVMetalTextureCache requirements
                for (size_t i = 0; i < planes; i++) {
                    CFRelease(cvMetalTextures[i]);
                }
                CVPixelBufferRelease(pixBuf);
            }];
        }
        else {
            for (size_t i = 0; i < planes; i++) {
                [renderEncoder setFragmentTexture:mapPlaneForSoftwareFrame(frame, i) atIndex:i];
            }
        }
        [renderEncoder setFragmentBuffer:m_CscParamsBuffer offset:0 atIndex:0];
        if (m_VideoVertexBuffer && m_VideoVertexCount > 0) {
            // Nothing in desk mode (m_VideoVertexCount is 0): the picture goes
            // to the desk windows' layers below.
            [renderEncoder setVertexBuffer:m_VideoVertexBuffer offset:0 atIndex:0];
            [renderEncoder drawPrimitives:MTLPrimitiveTypeTriangleStrip vertexStart:0 vertexCount:m_VideoVertexCount];
        }

        // Now draw any overlays that are enabled
        for (int i = 0; i < Overlay::OverlayMax; i++) {
            id<MTLTexture> overlayTexture = nullptr;

            // Try to acquire a reference on the overlay texture
            SDL_AtomicLock(&m_OverlayLock);
            overlayTexture = [m_OverlayTextures[i] retain];
            SDL_AtomicUnlock(&m_OverlayLock);

            if (overlayTexture) {
                SDL_FRect renderRect = {};
                if (i == Overlay::OverlayStatusUpdate) {
                    // Bottom Left
                    renderRect.x = 0;
                    renderRect.y = 0;
                }
                else if (i == Overlay::OverlayDebug) {
                    // Top left
                    renderRect.x = 0;
                    renderRect.y = m_LastDrawableHeight - overlayTexture.height;
                }
                else if (i == Overlay::OverlayQuickMenu) {
                    // Centered
                    renderRect.x = (m_LastDrawableWidth - overlayTexture.width) / 2;
                    renderRect.y = (m_LastDrawableHeight - overlayTexture.height) / 2;
                }

                renderRect.w = overlayTexture.width;
                renderRect.h = overlayTexture.height;

                // Convert screen space to normalized device coordinates
                StreamUtils::screenSpaceToNormalizedDeviceCoords(&renderRect, m_LastDrawableWidth, m_LastDrawableHeight);

                Vertex verts[] =
                {
                    { { renderRect.x, renderRect.y, 0.0f, 1.0f }, { 0.0f, 1.0f } },
                    { { renderRect.x, renderRect.y+renderRect.h, 0.0f, 1.0f }, { 0.0f, 0} },
                    { { renderRect.x+renderRect.w, renderRect.y, 0.0f, 1.0f }, { 1.0f, 1.0f} },
                    { { renderRect.x+renderRect.w, renderRect.y+renderRect.h, 0.0f, 1.0f }, { 1.0f, 0} },
                };

                [renderEncoder setRenderPipelineState:m_OverlayPipelineState];
                [renderEncoder setFragmentTexture:overlayTexture atIndex:0];
                [renderEncoder setVertexBytes:verts length:sizeof(verts) atIndex:0];
                [renderEncoder drawPrimitives:MTLPrimitiveTypeTriangleStrip vertexStart:0 vertexCount:SDL_arraysize(verts)];

                [overlayTexture release];
            }
        }

        [renderEncoder endEncoding];

        if (m_AppWindowMode) {
            renderChildWindows(frame, commandBuffer, planes, cvMetalTextures);
        }

        // Flip to the newly rendered buffer
        [commandBuffer presentDrawable:drawable];
        [commandBuffer commit];

        // Don't wait for the GPU here: the completion handler above releases the
        // texture cache references and the pixel buffer, so blocking would only
        // delay the next frame by the GPU time of this one.
    }}

    // Caller frees frame after we return
    virtual void renderFrame(AVFrame* frame) override
    { @autoreleasepool {
        // Handle changes to the frame's colorspace from last time we rendered
        if (!updateColorSpaceForFrame(frame)) {
            // Trigger the main thread to recreate the decoder
            SDL_Event event;
            event.type = SDL_RENDER_DEVICE_RESET;
            SDL_PushEvent(&event);
            return;
        }

        // Handle changes to the video size or drawable size
        if (!updateVideoRegionSizeForFrame(frame)) {
            // Trigger the main thread to recreate the decoder
            SDL_Event event;
            event.type = SDL_RENDER_DEVICE_RESET;
            SDL_PushEvent(&event);
            return;
        }

        // Start the display link if necessary
        startDisplayLink();

        if (hasDisplayLink()) {
            // Move the buffers into a new AVFrame
            AVFrame* newFrame = av_frame_alloc();
            av_frame_move_ref(newFrame, frame);

            // Replace any existing unrendered frame with this new one
            // and signal the CAMetalDisplayLink callback
            AVFrame* oldFrame = nullptr;
            SDL_LockMutex(m_FrameLock);
            if (m_LatestUnrenderedFrame != nullptr) {
                oldFrame = m_LatestUnrenderedFrame;
            }
            m_LatestUnrenderedFrame = newFrame;
            SDL_UnlockMutex(m_FrameLock);
            SDL_CondSignal(m_FrameReady);

            av_frame_free(&oldFrame);
        }
        else if (m_AppWindowMode) {
            renderDeskFrame(frame);
        }
        else {
            // Render to the next drawable right now when CAMetalDisplayLink is not in use
            id<CAMetalDrawable> drawable = [m_MetalLayer nextDrawable];
            if (drawable == nullptr) {
                return;
            }

            renderFrameIntoDrawable(frame, drawable);
        }
    }}

    id<MTLDevice> getMetalDevice() {
        if (qgetenv("VT_FORCE_METAL") == "0") {
            SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION,
                        "Avoiding Metal renderer due to VT_FORCE_METAL=0 override.");
            return nullptr;
        }

        NSArray<id<MTLDevice>> *devices = [MTLCopyAllDevices() autorelease];
        if (devices.count == 0) {
            SDL_LogError(SDL_LOG_CATEGORY_APPLICATION,
                         "No Metal device found!");
            return nullptr;
        }

        // First, try to find a low power (Intel) or unified memory (Apple Silicon) GPU
        for (id<MTLDevice> device in devices) {
            if (device.isLowPower || device.hasUnifiedMemory) {
                return device;
            }
        }

        // Next, we'll just try to pick something internal to the system
        for (id<MTLDevice> device in devices) {
            if (!device.isRemovable) {
                return device;
            }
        }

        // Use the system-default device
        return [MTLCreateSystemDefaultDevice() autorelease];
    }

    virtual bool initialize(PDECODER_PARAMETERS params) override
    { @autoreleasepool {
        int err;

        m_Window = params->window;
        m_FrameRateRange = CAFrameRateRangeMake(params->frameRate, params->frameRate, params->frameRate);

        id<MTLDevice> device = getMetalDevice();
        if (!device) {
            m_InitFailureReason = InitFailureReason::NoSoftwareSupport;
            return false;
        }

        SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION,
                    "Selected Metal device: %s",
                    device.name.UTF8String);

        if (m_HwAccel && !checkDecoderCapabilities(device, params)) {
            return false;
        }

        err = av_hwdevice_ctx_create(&m_HwContext,
                                     AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
                                     nullptr,
                                     nullptr,
                                     0);
        if (err < 0) {
            SDL_LogWarn(SDL_LOG_CATEGORY_APPLICATION,
                        "av_hwdevice_ctx_create() failed for VT decoder: %d",
                        err);
            m_InitFailureReason = InitFailureReason::NoSoftwareSupport;
            return false;
        }

        // Create the Metal texture cache for our CVPixelBuffers
        CFStringRef keys[1] = { kCVMetalTextureUsage };
        NSUInteger values[1] = { MTLTextureUsageShaderRead };
        auto cacheAttributes = CFDictionaryCreate(kCFAllocatorDefault, (const void**)keys, (const void**)values, 1, nullptr, nullptr);
        err = CVMetalTextureCacheCreate(kCFAllocatorDefault, cacheAttributes, device, nullptr, &m_TextureCache);
        CFRelease(cacheAttributes);

        if (err != kCVReturnSuccess) {
            SDL_LogError(SDL_LOG_CATEGORY_APPLICATION,
                         "CVMetalTextureCacheCreate() failed: %d",
                         err);
            return false;
        }

        // Compile our shaders
        QString shaderSource = QString::fromUtf8(Path::readDataFile("vt_renderer.metal"));
        m_ShaderLibrary = [device newLibraryWithSource:shaderSource.toNSString() options:nullptr error:nullptr];
        if (!m_ShaderLibrary) {
            SDL_LogError(SDL_LOG_CATEGORY_APPLICATION,
                         "Failed to compile shaders");
            return false;
        }

        // Create a command queue for submission
        m_CommandQueue = [device newCommandQueue];

        // Add the Metal view to the window if we're not in test-only mode
        //
        // NB: Test-only renderers may be created on a non-main thread, so
        // we don't want to touch the view hierarchy in that context.
        if (!params->testOnly) {
            m_MetalView = SDL_Metal_CreateView(m_Window);
            if (!m_MetalView) {
                SDL_LogError(SDL_LOG_CATEGORY_APPLICATION,
                             "SDL_Metal_CreateView() failed: %s",
                             SDL_GetError());
                return false;
            }

            m_MetalLayer = (CAMetalLayer*)SDL_Metal_GetLayer(m_MetalView);

            // Choose a device
            m_MetalLayer.device = device;

            // Desk mode: the SDL window is the hidden key window and shows
            // nothing; the desk windows' layers composite with alpha
            m_AppWindowMode = AppWindow::deskActive();
            if (m_AppWindowMode) {
                m_MetalLayer.opaque = NO;
                std::lock_guard<std::mutex> lock(s_DeskRendererLock);
                s_DeskRenderer = this;
            }

            // Allow EDR content if we're streaming in a 10-bit format
            m_MetalLayer.wantsExtendedDynamicRangeContent = !!(params->videoFormat & VIDEO_FORMAT_MASK_10BIT);

            // Allow tearing if V-Sync is off (also requires direct display path)
            m_MetalLayer.displaySyncEnabled = params->enableVsync;

            {
                // Three drawables (the default): with two, full-screen compositing
                // holds a drawable long enough to block nextDrawable, and the
                // render thread then drops ~3 of every 120 frames (measured);
                // the queued latency is the same because V-Sync is off and the
                // GPU is never waited on. MOONLIGHT_METAL_DRAWABLES=2|3 overrides.
                NSUInteger drawables = 3;
                if (const char* env = getenv("MOONLIGHT_METAL_DRAWABLES")) {
                    int v = atoi(env);
                    if (v >= 2 && v <= 3) drawables = v;
                }
                m_MetalLayer.maximumDrawableCount = drawables;
                SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION, "Metal layer drawables: %d", (int)drawables);
            }
        }

        return true;
    }}

    virtual void notifyOverlayUpdated(Overlay::OverlayType type) override
    { @autoreleasepool {
        SDL_Surface* newSurface = Session::get()->getOverlayManager().getUpdatedOverlaySurface(type);
        bool overlayEnabled = Session::get()->getOverlayManager().isOverlayEnabled(type);
        if (newSurface == nullptr && overlayEnabled) {
            // The overlay is enabled and there is no new surface. Leave the old texture alone.
            return;
        }

        SDL_AtomicLock(&m_OverlayLock);
        auto oldTexture = m_OverlayTextures[type];
        m_OverlayTextures[type] = nullptr;
        SDL_AtomicUnlock(&m_OverlayLock);

        [oldTexture release];

        // If the overlay is disabled, we're done
        if (!overlayEnabled) {
            SDL_FreeSurface(newSurface);
            return;
        }

        // Create a texture to hold our pixel data
        SDL_assert(!SDL_MUSTLOCK(newSurface));
        SDL_assert(newSurface->format->format == SDL_PIXELFORMAT_ARGB8888);
        auto texDesc = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
                                                                          width:newSurface->w
                                                                         height:newSurface->h
                                                                      mipmapped:NO];
        texDesc.cpuCacheMode = MTLCPUCacheModeWriteCombined;
        texDesc.storageMode = MTLStorageModeManaged;
        texDesc.usage = MTLTextureUsageShaderRead;
        auto newTexture = [m_MetalLayer.device newTextureWithDescriptor:texDesc];

        // Load the pixel data into the new texture
        [newTexture replaceRegion:MTLRegionMake2D(0, 0, newSurface->w, newSurface->h)
                      mipmapLevel:0
                        withBytes:newSurface->pixels
                      bytesPerRow:newSurface->pitch];

        // The surface is no longer required
        SDL_FreeSurface(newSurface);
        newSurface = nullptr;

        SDL_AtomicLock(&m_OverlayLock);
        m_OverlayTextures[type] = newTexture;
        SDL_AtomicUnlock(&m_OverlayLock);
    }}

    virtual bool prepareDecoderContext(AVCodecContext* context, AVDictionary**) override
    {
        if (m_HwAccel) {
            context->hw_device_ctx = av_buffer_ref(m_HwContext);
        }

        SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION,
                    "Using Metal renderer with %s decoding",
                    m_HwAccel ? "hardware" : "software");

        return true;
    }

    void startDisplayLink()
    {
        if (@available(macOS 14, *)) {
            if (m_MetalDisplayLink != nullptr || !m_MetalLayer.displaySyncEnabled || !isAppleSilicon()) {
                return;
            }

            m_MetalDisplayLink = [[CAMetalDisplayLink alloc] initWithMetalLayer:m_MetalLayer];
            m_MetalDisplayLink.preferredFrameLatency = 1.0f;
            m_MetalDisplayLink.preferredFrameRateRange = m_FrameRateRange;
            m_MetalDisplayLink.delegate = [[DisplayLinkDelegate alloc] initWithRenderer:this];
            [m_MetalDisplayLink addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];
        }
    }

    void stopDisplayLink()
    {
        if (@available(macOS 14, *)) {
            if (m_MetalDisplayLink == nullptr) {
                return;
            }

            [m_MetalDisplayLink invalidate];
            m_MetalDisplayLink = nullptr;
        }
    }

    bool hasDisplayLink()
    {
        if (@available(macOS 14, *)) {
            if (m_MetalDisplayLink != nullptr) {
                return true;
            }
        }

        return false;
    }

    int getDecoderColorspace() override
    {
        // macOS seems to handle Rec 601 best
        return COLORSPACE_REC_601;
    }

    int getDecoderCapabilities() override
    {
        return CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC |
               CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1;
    }

    int getRendererAttributes() override
    {
        // Metal supports HDR output
        return RENDERER_ATTRIBUTE_HDR_SUPPORT;
    }

    bool isPixelFormatSupported(int videoFormat, AVPixelFormat pixelFormat) override
    {
        if (m_HwAccel) {
            return pixelFormat == AV_PIX_FMT_VIDEOTOOLBOX;
        }
        else {
            if (pixelFormat == AV_PIX_FMT_VIDEOTOOLBOX) {
                // VideoToolbox frames are always supported
                return true;
            }
            else {
                // Otherwise it's supported if we can map it
                const int expectedPixelDepth = (videoFormat & VIDEO_FORMAT_MASK_10BIT) ? 10 : 8;
                const int expectedLog2ChromaW = (videoFormat & VIDEO_FORMAT_MASK_YUV444) ? 0 : 1;
                const int expectedLog2ChromaH = (videoFormat & VIDEO_FORMAT_MASK_YUV444) ? 0 : 1;

                const AVPixFmtDescriptor* formatDesc = av_pix_fmt_desc_get(pixelFormat);
                if (!formatDesc) {
                    // This shouldn't be possible but handle it anyway
                    SDL_assert(formatDesc);
                    return false;
                }

                int planes = av_pix_fmt_count_planes(pixelFormat);
                return (planes == 2 || planes == 3) &&
                       formatDesc->comp[0].depth == expectedPixelDepth &&
                       formatDesc->log2_chroma_w == expectedLog2ChromaW &&
                       formatDesc->log2_chroma_h == expectedLog2ChromaH;
            }
        }
    }

    bool notifyWindowChanged(PWINDOW_STATE_CHANGE_INFO info) override
    {
        auto unhandledStateFlags = info->stateChangeFlags;

        // We can always handle size changes
        unhandledStateFlags &= ~WINDOW_STATE_CHANGE_SIZE;

        // We can handle monitor changes
        unhandledStateFlags &= ~WINDOW_STATE_CHANGE_DISPLAY;

        // If nothing is left, we handled everything
        return unhandledStateFlags == 0;
    }

    void renderLatestFrameOnDrawable(id<CAMetalDrawable> drawable, CFTimeInterval targetTimestamp)
    {
        AVFrame* frame = nullptr;

        // Determine how long we can wait depending on how long our CAMetalDisplayLink
        // says we have until the next frame needs to be rendered. We will wait up to
        // half the per-frame interval for a new frame to become available.
        int waitTimeMs = ((targetTimestamp - CACurrentMediaTime()) * 1000) / 2;
        if (waitTimeMs < 0) {
            return;
        }

        // Wait for a new frame to be ready
        SDL_LockMutex(m_FrameLock);
        if (m_LatestUnrenderedFrame != nullptr || SDL_CondWaitTimeout(m_FrameReady, m_FrameLock, waitTimeMs) == 0) {
            frame = m_LatestUnrenderedFrame;
            m_LatestUnrenderedFrame = nullptr;
        }
        SDL_UnlockMutex(m_FrameLock);

        // Render a frame if we got one in time
        if (frame != nullptr) {
            if (m_AppWindowMode) {
                renderDeskFrame(frame);
            }
            else {
                renderFrameIntoDrawable(frame, drawable);
            }
            av_frame_free(&frame);
        }
    }

private:
    bool m_HwAccel;
    SDL_Window* m_Window;
    AVBufferRef* m_HwContext;
    CAMetalLayer* m_MetalLayer;
    CAMetalDisplayLink* m_MetalDisplayLink API_AVAILABLE(macos(14.0));
    CAFrameRateRange m_FrameRateRange;
    AVFrame* m_LatestUnrenderedFrame;
    SDL_mutex* m_FrameLock;
    SDL_cond* m_FrameReady;
    CVMetalTextureCacheRef m_TextureCache;
    id<MTLBuffer> m_CscParamsBuffer;
    id<MTLBuffer> m_VideoVertexBuffer;
    id<MTLTexture> m_OverlayTextures[Overlay::OverlayMax];
    SDL_SpinLock m_OverlayLock;
    id<MTLRenderPipelineState> m_VideoPipelineState;
    id<MTLRenderPipelineState> m_OverlayPipelineState;
    id<MTLLibrary> m_ShaderLibrary;
    id<MTLCommandQueue> m_CommandQueue;
    id<MTLTexture> m_SwMappingTextures[MAX_VIDEO_PLANES];
    SDL_MetalView m_MetalView;
    int m_LastFrameWidth;
    int m_LastFrameHeight;
    int m_LastDrawableWidth;
    int m_LastDrawableHeight;
    int m_VideoVertexCount = 0;
    bool m_AppWindowMode = false;  // desk session: see AppWindow::deskActive()
    std::mutex m_DeskFrameLock;
    AVFrame* m_LastDeskFrame = av_frame_alloc();  // the last frame drawn, for redraws between frames
    AVFrame* m_HeldDeskFrame = av_frame_alloc();  // a frame waiting for its layout
    int m_DeskResult = 0;  // the last renderChildWindows: 0 drawn, 1 held for its layout, 2 not readable
    QHash<quint64, QRect> m_PopupTrims;
    struct Inflight {
        std::atomic<int> count {0};
        std::atomic<double> lastPresented {0.0};
    };
    QHash<quint64, std::shared_ptr<Inflight>> m_Inflight;
};

@implementation DisplayLinkDelegate {
    VTMetalRenderer* _renderer;
}

- (id)initWithRenderer:(VTMetalRenderer *)renderer {
    _renderer = renderer;
    return self;
}

- (void)metalDisplayLink:(CAMetalDisplayLink *)link
             needsUpdate:(CAMetalDisplayLinkUpdate *)update API_AVAILABLE(macos(14.0)) {
    _renderer->renderLatestFrameOnDrawable(update.drawable, update.targetTimestamp);
}

@end

IFFmpegRenderer* VTMetalRendererFactory::createRenderer(bool hwAccel) {
    return new VTMetalRenderer(hwAccel);
}

void vtDeskRedraw(uint64_t hwnd)
{
    std::lock_guard<std::mutex> lock(s_DeskRendererLock);
    if (s_DeskRenderer != nullptr) {
        s_DeskRenderer->redrawDeskWindow(hwnd);
    }
}
