#pragma once

// App-window streaming: platform glue that turns the SDL stream window into
// a borderless, resizable "native-looking" window for one host application
// window (the host's own decorations are part of the video), with the host
// app's icon on the Dock / app switcher.
//
// Only macOS has an implementation; elsewhere these are no-ops.

#include <SDL.h>
#include <QByteArray>
#include <QString>
#include <QVector>
#include <QtGlobal>
#include <functional>

namespace AppWindow {

// Make the window borderless but resizable/miniaturizable, transparent (the
// picture is a shaped region of it) and give it a title.
void configureWindow(SDL_Window* window, const QString& title);

// Set the window's title (shown in Mission Control / Exposé).
void setTitle(SDL_Window* window, const QString& title);

// Use this PNG as the process icon (Dock, Cmd-Tab).
void setAppIcon(const QByteArray& png);

// Start a native window drag from the current mouse-down event.
void beginDrag(SDL_Window* window);

// Backing scale factor of the window's screen (1.0 when unknown).
double backingScale(SDL_Window* window);

// Toggle the window between its normal frame and the screen's visible area.
void toggleZoom(SDL_Window* window);

// Do what macOS does on a title-bar double-click, following the user's
// "Double-click a window's title bar to" preference (Fill / Zoom / Minimize /
// Do nothing). Used for the Windows maximize button and caption double-clicks.
void performTitleAction(SDL_Window* window);

// Recompute the window's shadow from its (transparent) content. Safe from any
// thread; cheap enough to call whenever the shape changes.
void invalidateShadow(SDL_Window* window);

// Move and resize the window in one step (SDL points, top-left origin), so
// the picture never shows an intermediate frame when popups extend it.
// Returns false when unsupported (callers fall back to SDL's two calls).
bool setFrame(SDL_Window* window, int x, int y, int w, int h);

// What the renderer shows. The stream is a canvas larger than the host
// window; the Mac window covers the union of the host window and its popups
// (menus, drop-downs, tooltips, dialogs) and draws each of them as its own
// rounded rectangle, transparent elsewhere. Units are host pixels.
struct LayoutItem {
    int x = 0, y = 0, w = 0, h = 0;  // relative to the Mac window's top-left
    int radius = 0;                  // corner radius (0 = square)
};
struct Layout {
    int originX = 0, originY = 0;  // canvas pixel under the Mac window's top-left
    int w = 0, h = 0;              // Mac window size in host pixels
    QVector<LayoutItem> items;     // bottom-most first; empty = not an app window
    unsigned version = 0;          // bumped on every change
};
void setLayout(const Layout& layout);
Layout layout();
bool active();  // an app-window layout has been set

// Child dialogs of the host window are shown as windows of their own: each
// draws its own rectangle of the canvas. The Mac position is the user's
// (never sent to the host); a new child appears next to the main window at
// the offset it has on the host.
struct ChildInfo {
    quint64 hwnd = 0;
    int offsetX = 0, offsetY = 0;  // host px, relative to the host window's top-left
    int w = 0, h = 0;              // host px
    int canvasX = 0, canvasY = 0;  // where its picture sits on the canvas
    int radius = 0;
    int caption = 0;               // caption height in host px (0 = unknown)
    QString title;
};
// Create/resize/remove child windows to match (main thread; safe from any thread).
void syncChildren(SDL_Window* mainWindow, const QVector<ChildInfo>& children, double pointsPerHostPx);
void closeChildren();

// What the renderer draws into: one Metal layer per child (retained CAMetalLayer*).
struct ChildLayer {
    quint64 hwnd = 0;
    void* metalLayer = nullptr;    // CAMetalLayer*, retained while in this list
    int canvasX = 0, canvasY = 0, w = 0, h = 0;
    int radius = 0;
};
QVector<ChildLayer> childLayers();
// Runs fn for each child layer while holding the lock that keeps the layers
// alive: use this from the render thread.
void forEachChildLayer(const std::function<void(const ChildLayer&)>& fn);

// Mouse input from a child window, pushed to SDL as SDL_CODE_APPWINDOW_CHILD_INPUT
// (event.user.data1 = new ChildInput). hostX/hostY are host px relative to the
// child's top-left; kind: 0 move, 1 press, 2 release, 3 scroll.
struct ChildInput {
    quint64 hwnd = 0;
    int kind = 0;
    int hostX = 0, hostY = 0;
    int button = 0;                // 1 left, 2 middle, 3 right (press/release)
    int clicks = 0;
    float scrollX = 0, scrollY = 0;  // scroll, in lines
    void* nsEvent = nullptr;       // retained NSEvent* of the press (for a native drag)
};
// Called by the session when a press on a child's caption should drag it.
void dragChild(quint64 hwnd, void* nsEvent);
void releaseChildInput(ChildInput* input);

}
