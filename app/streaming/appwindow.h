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
#include <atomic>
#include <memory>

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
    int viewW = 0, viewH = 0;      // the Mac window's size in host px: the picture (w x h) sits top-left in it
    bool frozen = false;           // keep showing the last picture (the window is being dragged)
    bool popup = false;            // caption-less popup (menu, tooltip): its window carries margins to trim
    int hostX = 0, hostY = 0, hostW = 0, hostH = 0;  // the window's whole rect on the desk (untrimmed)
    // The window's rect on the desk over time, stamped on the video frames'
    // clock (90 kHz RTP time): each frame is cut with the rect that was true
    // when it was captured, so moves and resizes never show a stale crop.
    struct Geo {
        quint32 ts = 0;
        int x = 0, y = 0, w = 0, h = 0;
    };
    Geo geo[12];
    int geoCount = 0;              // oldest first
    int trimX = 0, trimY = 0, trimW = 0, trimH = 0;  // popups: the visible part, relative to the rect
    bool anchorRight = false, anchorBottom = false;  // mid-resize: the picture hugs these edges of the window
    std::shared_ptr<std::atomic<quint64>> shown;      // the picture size last drawn (w << 32 | h), written by the renderer
};
QVector<ChildLayer> childLayers();
// The renderer drew a picture of a new size for this window (render thread).
void deskShownChanged(quint64 hwnd, int w, int h);
// "What was that?" for an exception being handled: name, reason and where,
// for AppKit exceptions (call from a catch block).
QString describeCurrentException();
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
    quint32 ts = 0;                  // when the host saw this, on the video frames' clock
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
    QString kind;  // move, resize, minimize, restore, lower, activate, dragend, order
    int x = 0, y = 0, w = 0, h = 0;
    QVector<quint64> order;  // "order": our windows, top-most first
    bool focus = false;      // "order": also activate the top-most one
};
void deskMinimize(quint64 hwnd);
// A press on a window's caption turned into a drag (the mouse moved): the
// Mac window follows the mouse, its picture is frozen and the host window is
// left alone (lowered, so nothing shows through others) until deskEndDrag.
void deskBeginDrag(quint64 hwnd);
void deskEndDrag(quint64 hwnd);
// Test hook: a caption drag by (dx, dy) points over ~0.3 s, as the user would.
void deskTestDrag(quint64 hwnd, int dx, int dy);
// Test hook: a live resize as AppKit runs it (dw, dh points over ~0.5 s;
// fromLeft drags the left edge instead of the right).
void deskTestLiveResize(quint64 hwnd, int dw, int dh, bool fromLeft);
// Test hook: a tile as macOS does it: a live resize that sets the frame
// without asking the window (x, y, w, h in points, top-left origin).
void deskTestTile(quint64 hwnd, int x, int y, int w, int h);
// Test hook: bring this window in front of our others, as a window switcher would.
void deskTestFront(quint64 hwnd);
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
