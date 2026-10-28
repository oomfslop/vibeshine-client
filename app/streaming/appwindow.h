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

}
