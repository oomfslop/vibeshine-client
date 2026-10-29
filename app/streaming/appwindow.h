#pragma once

// App-window streaming (desk mode): every host window on the client's desk
// is a native Mac window of its own; the SDL window is a hidden key window
// that only carries the keyboard. The windows themselves are deskwm's (Rust,
// app/deskwm): Mac windows, mouse input, drags, resizes, stacking and focus.
// This is the helper's side of it: the process icon, starting and ending the
// desk, and telling the renderer it is a desk session.
//
// Only macOS has an implementation; elsewhere these are no-ops.

#include <SDL.h>
#include <QByteArray>
#include <QRect>
#include <QString>

#include "deskwm/deskwm.h"

namespace AppWindow {

// Use this PNG as the process icon (Dock, Cmd-Tab).
void setAppIcon(const QByteArray& png);

// Desk mode: keyWindow becomes the hidden key window; screenPts is the
// mirrored screen in SDL global points (top-left origin); deskW x deskH the
// stream's size in host px.
void beginDesk(SDL_Window* keyWindow, const QRect& screenPts, double pointsPerHostPx, int deskW, int deskH,
               const DeskwmCallbacks& callbacks);

// Close every desk window (safe from any thread).
void closeChildren();

// "What was that?" for an exception being handled: name, reason and where,
// for AppKit exceptions (call from a catch block).
QString describeCurrentException();

// A desk session is under way (set by beginDesk; the renderer draws nothing
// in the SDL window and draws each desk window's layer instead).
bool deskActive();
void setDeskActive(bool on);

}
