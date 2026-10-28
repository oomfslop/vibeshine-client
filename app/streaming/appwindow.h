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

namespace AppWindow {

// Make the window borderless but resizable/miniaturizable and give it a title.
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

// Crop (in stream pixels) the renderer should show, top-left anchored:
// app-window streams are padded canvases larger than the window. 0 = none.
void setCrop(int w, int h);
void crop(int& w, int& h);

}
