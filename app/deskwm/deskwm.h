// deskwm: host windows of a Vibeshine desk as native macOS windows.
//
// The Rust library owns the Mac side of app-window streaming: one NSWindow
// per host window (with a CAMetalLayer the renderer draws into), the mouse
// (sent straight to the host), drags, resizes, minimizing, stacking and
// focus. The helper keeps the network (layout poller, window ops, hit
// tests), the keyboard and the stream; the renderer cuts each window's
// picture out of the frame with deskwm_for_each_layer.
#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct DeskwmCallbacks {
    void* ctx;
    // A window op for the host: "adopt" (x, y), "resize" (w, h), "minimize",
    // "restore", "activate", "close", "dismiss", "maximize". Called on the
    // main thread; must not block.
    void (*window_op)(void* ctx, uint64_t hwnd, const char* op, int32_t x, int32_t y, int32_t w, int32_t h);
    // Stack the desk's windows like this, top-most first; focus: activate the first.
    void (*order)(void* ctx, const uint64_t* hwnds, size_t count, bool focus);
    // What is under (x, y) of the window (window-relative host px)? Answer with
    // deskwm_hittest_result(token, hit) from any thread.
    void (*hittest)(void* ctx, uint64_t hwnd, int32_t x, int32_t y, uint64_t token);
    // Something for the launcher (a JSON object), e.g. a window closed.
    void (*event)(void* ctx, const char* json);
    // level: 0 info, 1 warning, 2 error
    void (*log)(void* ctx, int32_t level, const char* message);
} DeskwmCallbacks;

// A window's picture in a frame: where it is (x, y, w, h: its slot in the
// atlas) from the frame stamped ts on, its copy on white (ax, ay; -1 = the
// picture is opaque) and its scale in the atlas. A picture drawn smaller
// comes with the parts the user sees 1:1 as well (pieces: x, y, w, h in the
// picture in host px, ax, ay in the frame), to draw over it.
typedef struct DeskwmPiece {
    int32_t x, y, w, h;
    int32_t ax, ay;
} DeskwmPiece;

#define DESKWM_PIECE_MAX 8

typedef struct DeskwmGeo {
    uint32_t ts;
    int32_t x, y, w, h;
    int32_t ax, ay;
    float scale;
    uint32_t piece_count;
    DeskwmPiece pieces[DESKWM_PIECE_MAX];
} DeskwmGeo;

#define DESKWM_GEO_MAX 12

typedef struct DeskwmLayer {
    uint64_t hwnd;
    void* metal_layer;      // CAMetalLayer*, alive while deskwm_for_each_layer runs
    int32_t view_w, view_h; // the Mac window's content in host px (the picture sits in it 1:1)
    int32_t radius;         // corner radius in host px (0: square)
    bool visible;
    bool anchor_right, anchor_bottom;  // mid-resize: the picture hugs these edges
    uint32_t geo_count;     // oldest first
    DeskwmGeo geo[DESKWM_GEO_MAX];
} DeskwmLayer;

// key_window: the helper's NSWindow (kept hidden; it carries the keyboard).
// screen: the mirrored Mac screen in global points, top-left origin (SDL's).
// desk_w/desk_h: the desk in host px (the stream's size).
void deskwm_begin(void* key_window, double screen_x, double screen_y, double screen_w, double screen_h,
                  double points_per_px, int32_t desk_w, int32_t desk_h, DeskwmCallbacks callbacks);
void deskwm_end(void);
// A /actions/desk-layout reply (any thread).
void deskwm_layout(const char* json, size_t len);
void deskwm_hittest_result(uint64_t token, const char* hit);
void deskwm_set_icon(const char* key, const uint8_t* png, size_t len);
// Test hooks: {"cmd":"click"|"scroll"|"frame"|"drag"|"liveresize"|"tile"|"front", ...}
void deskwm_command(const char* json);
uint64_t deskwm_front_window(void);
double deskwm_points_per_px(void);

// Renderer (any thread): every layer, under the lock that keeps them alive.
void deskwm_for_each_layer(void (*fn)(void* ctx, const DeskwmLayer* layer), void* ctx);
// The renderer drew a picture of this size for the window.
void deskwm_shown(uint64_t hwnd, int32_t w, int32_t h);

#ifdef __cplusplus
}
#endif
