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

}
