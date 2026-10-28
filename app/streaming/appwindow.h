#pragma once

// App-window streaming (desk mode): the stream is the client's desk, a
// private host display that mirrors this screen. Every window on it is a
// native Mac window of its own (position, size and stacking mirrored from the
// host's truth, except while the user is moving or resizing one); the SDL
// window is a hidden key window that only carries the keyboard. The host
// app's icon goes on the Dock / app switcher.
//
// Only macOS has an implementation; elsewhere these are no-ops.

#include <SDL.h>
#include <QByteArray>
#include <QRect>
#include <QString>
#include <QVector>
#include <QtGlobal>
#include <functional>

namespace AppWindow {

// Use this PNG as the process icon (Dock, Cmd-Tab).
void setAppIcon(const QByteArray& png);

// Close every desk window (main thread; safe from any thread).
void closeChildren();

// What the renderer draws into: one Metal layer per desk window (retained
// CAMetalLayer*). canvasX/Y, w, h: the crop of the frame it shows (host px).
struct ChildLayer {
    quint64 hwnd = 0;
    void* metalLayer = nullptr;    // CAMetalLayer*, retained while in this list
    int canvasX = 0, canvasY = 0, w = 0, h = 0;
    int radius = 0;
    bool visible = true;           // false while the window is minimized or fully covered: nothing to draw
    bool popup = false;            // caption-less popup (menu, tooltip): its window carries margins to trim
    int hostX = 0, hostY = 0, hostW = 0, hostH = 0;  // the window's whole rect on the desk (untrimmed)
};
QVector<ChildLayer> childLayers();
// Runs fn for each child layer while holding the lock that keeps the layers
// alive: use this from the render thread.
void forEachChildLayer(const std::function<void(const ChildLayer&)>& fn);

// Mouse input from a desk window, pushed to SDL as SDL_CODE_APPWINDOW_CHILD_INPUT
// (event.user.data1 = new ChildInput). hostX/hostY are host px relative to the
// window's top-left; kind: 0 move, 1 press, 2 release, 3 scroll.
struct ChildInput {
    quint64 hwnd = 0;
    int kind = 0;
    int hostX = 0, hostY = 0;
    int button = 0;                // 1 left, 2 middle, 3 right (press/release)
    int clicks = 0;
    float scrollX = 0, scrollY = 0;  // scroll, in lines
    void* nsEvent = nullptr;       // retained NSEvent* of the press (for a native drag)
};
// Called by the session when a press on a window's caption should drag it.
void dragChild(quint64 hwnd, void* nsEvent);
void releaseChildInput(ChildInput* input);

// Units are host pixels unless noted.
struct DeskWindow {
    quint64 hwnd = 0, owner = 0;
    int kind = 0;                    // 0 window, 1 dialog (owned, captioned), 2 popup (caption-less)
    int x = 0, y = 0, w = 0, h = 0;  // desk-relative outer rect, as the host has it
    int radius = 0, caption = 0, z = 0;
    bool focused = false;
    QString title, state, iconKey;   // state: normal, minimized, maximized
};
// Make the SDL window the hidden key window and set the screen mapping:
// screenPts is the mirrored screen in SDL global points (top-left origin).
void beginDesk(SDL_Window* keyWindow, const QRect& screenPts, double pointsPerHostPx);
// Mirror the desk (bottom-most first): create, move, resize, stack, minimize
// and close Mac windows to match. Safe from any thread.
void syncDesk(const QVector<DeskWindow>& windows);
// The PNG for windows with this icon key; the Dock shows the front window's.
void setDeskIcon(const QString& iconKey, const QByteArray& png);
// Frontmost desk window of ours (0 = none).
quint64 deskFrontWindow();
// What the user did to a desk window, pushed as SDL_CODE_APPWINDOW_DESK_EVENT
// (event.user.data1 = new DeskEvent). kind: move (x, y), resize (w, h),
// minimize, restore.
struct DeskEvent {
    quint64 hwnd = 0;
    QString kind;
    int x = 0, y = 0, w = 0, h = 0;
};
void deskMinimize(quint64 hwnd);
// Points per host pixel of the desk: 1 / the mirrored screen's backing scale,
// so one desk pixel is one device pixel (valid after beginDesk).
double deskPointsPerPx();
// Popups: the part of the host window's rect that is really the popup (the
// rest is the desk's background showing through its margins); relative to
// the rect, w = 0 clears it.
void setDeskTrim(quint64 hwnd, int x, int y, int w, int h);
// Test hook: set a desk window's frame as if the user had moved/resized it
// (SDL global points, top-left origin).
void deskTestSetFrame(quint64 hwnd, int x, int y, int w, int h);
void deskTitleAction(quint64 hwnd);  // the title-bar double-click / maximize-button action
// A desk session is under way (set by beginDesk; the renderer draws nothing
// in the SDL window and composites each desk window's layer instead).
bool deskActive();
void setDeskActive(bool on);

}
