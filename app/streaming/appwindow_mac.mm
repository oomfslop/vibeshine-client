#include "appwindow.h"

#include <SDL_syswm.h>
#import <AppKit/AppKit.h>

static void deskOnMain(const char* what, dispatch_block_t block);

static NSString* describeException(NSException* e)
{
    NSArray<NSString*>* stack = e.callStackSymbols;
    NSRange keep = NSMakeRange(0, MIN((NSUInteger)14, stack.count));
    return [NSString stringWithFormat:@"%@: %@\n  %@", e.name, e.reason, [[stack subarrayWithRange:keep] componentsJoinedByString:@"\n  "]];
}

#define DESK_TRY @try {
#define DESK_CATCH(what) } @catch (NSException* e) { SDL_LogError(SDL_LOG_CATEGORY_APPLICATION, "AppWindow %s: %s", what, describeException(e).UTF8String); }


namespace {

NSWindow* nsWindowFor(SDL_Window* window)
{
    if (!window) {
        return nil;
    }
    SDL_SysWMinfo info;
    SDL_VERSION(&info.version);
    if (!SDL_GetWindowWMInfo(window, &info) || info.subsystem != SDL_SYSWM_COCOA) {
        return nil;
    }
    return info.info.cocoa.window;
}

}

namespace AppWindow {

void setAppIcon(const QByteArray& png)
{
    if (png.isEmpty()) {
        return;
    }
    NSData* data = [NSData dataWithBytes:png.constData() length:(NSUInteger)png.size()];
    NSImage* image = [[NSImage alloc] initWithData:data];
    if (image) {
        [NSApp setApplicationIconImage:image];
    }
}

}

// ---------------------------------------------------------------------------
// Desk windows: one Mac window (with a Metal layer of its own) per host window
// ---------------------------------------------------------------------------

#import <QuartzCore/CAMetalLayer.h>
#include <mutex>
#include <climits>
#include "session.h"

namespace AppWindow {
void deskNotePress(quint64 hwnd);
}

@interface AWChildView : NSView
@property (nonatomic) quint64 hwnd;
@property (nonatomic) double pointsPerPx;
@property (nonatomic) BOOL dialog;
@property (nonatomic, assign) NSWindow* mainWindow;
@property (nonatomic, strong) NSTrackingArea* tracking;
@property (nonatomic) BOOL dragging;       // a caption drag is moving the window
@property (nonatomic) NSPoint downMouse;   // screen location of the last left press
@property (nonatomic) NSPoint downOrigin;  // window origin at that press
@end

@implementation AWChildView

- (instancetype)initWithFrame:(NSRect)frame
{
    self = [super initWithFrame:frame];
    if (self) {
        self.wantsLayer = YES;
        self.layerContentsRedrawPolicy = NSViewLayerContentsRedrawNever;
    }
    return self;
}

- (CALayer*)makeBackingLayer
{
    CAMetalLayer* layer = [CAMetalLayer layer];
    layer.opaque = NO;
    layer.backgroundColor = [NSColor clearColor].CGColor;
    layer.framebufferOnly = YES;
    return layer;
}

- (BOOL)isFlipped { return YES; }
- (BOOL)acceptsFirstMouse:(NSEvent*)event { return YES; }
- (BOOL)acceptsFirstResponder { return NO; }

- (void)updateDrawableSize
{
    CAMetalLayer* layer = (CAMetalLayer*)self.layer;
    const CGFloat scale = self.window ? self.window.backingScaleFactor : 2.0;
    layer.contentsScale = scale;
    layer.drawableSize = CGSizeMake(self.bounds.size.width * scale, self.bounds.size.height * scale);
}

- (void)setFrameSize:(NSSize)newSize
{
    [super setFrameSize:newSize];
    [self updateDrawableSize];
}

- (void)viewDidChangeBackingProperties
{
    [super viewDidChangeBackingProperties];
    [self updateDrawableSize];
}

- (void)updateTrackingAreas
{
    [super updateTrackingAreas];
    if (self.tracking) {
        [self removeTrackingArea:self.tracking];
    }
    self.tracking = [[NSTrackingArea alloc] initWithRect:self.bounds
                                                 options:NSTrackingMouseMoved | NSTrackingActiveAlways | NSTrackingInVisibleRect
                                                   owner:self
                                                userInfo:nil];
    [self addTrackingArea:self.tracking];
}

- (void)push:(int)kind event:(NSEvent*)event button:(int)button
{
    DESK_TRY
    [self pushUnguarded:kind event:event button:button];
    DESK_CATCH("view input")
}

- (void)pushUnguarded:(int)kind event:(NSEvent*)event button:(int)button
{
    const NSPoint p = [self convertPoint:event.locationInWindow fromView:nil];
    auto* input = new AppWindow::ChildInput;
    input->hwnd = self.hwnd;
    input->kind = kind;
    input->hostX = (int)lround(p.x / self.pointsPerPx);
    input->hostY = (int)lround(p.y / self.pointsPerPx);
    input->button = button;
    input->clicks = (int)event.clickCount;
    if (kind == 3) {
        input->scrollX = (float)event.scrollingDeltaX;
        input->scrollY = (float)event.scrollingDeltaY;
        if (!event.hasPreciseScrollingDeltas) {
            // Line deltas: WHEEL_DELTA per line on the host side
            input->scrollX *= 10.0f;
            input->scrollY *= 10.0f;
        }
    }
    if (kind == 1) {
        input->nsEvent = (void*)CFBridgingRetain(event);
    }
    SDL_Event ev = {};
    ev.type = SDL_USEREVENT;
    ev.user.code = SDL_CODE_APPWINDOW_CHILD_INPUT;
    ev.user.data1 = input;
    SDL_PushEvent(&ev);
}

// A click on a desk window: the keyboard must have somewhere to go (the
// hidden key window, which forwards it; the host routes it to the clicked
// window), and a window or dialog comes to the front of the app.
- (void)focusMain
{
    DESK_TRY
    [self focusMainUnguarded];
    DESK_CATCH("focusMain")
}

- (void)focusMainUnguarded
{
    if (self.mainWindow && !self.mainWindow.isKeyWindow) {
        [self.mainWindow makeKeyWindow];
    }
    if (self.dialog && self.window) {
        [self.window orderFront:nil];
    }
    AppWindow::deskNotePress(self.hwnd);
}
- (void)mouseDown:(NSEvent*)e
{
    self.dragging = NO;
    self.downMouse = [NSEvent mouseLocation];
    self.downOrigin = self.window.frame.origin;
    [self focusMain];
    [self push:1 event:e button:1];
}
- (void)mouseUp:(NSEvent*)e
{
    self.dragging = NO;  // the session ends the drag when the release reaches it
    [self push:2 event:e button:1];
}
- (void)rightMouseDown:(NSEvent*)e { [self focusMain]; [self push:1 event:e button:3]; }
- (void)rightMouseUp:(NSEvent*)e { [self push:2 event:e button:3]; }
- (void)otherMouseDown:(NSEvent*)e { [self focusMain]; [self push:1 event:e button:2]; }
- (void)otherMouseUp:(NSEvent*)e { [self push:2 event:e button:2]; }
- (void)mouseMoved:(NSEvent*)e { [self push:0 event:e button:0]; }
- (void)mouseDragged:(NSEvent*)e
{
    if (self.dragging) {
        // A caption drag: the window follows the mouse locally, nothing goes
        // to the host until it ends.
        const NSPoint m = [NSEvent mouseLocation];
        [self.window setFrameOrigin:NSMakePoint(self.downOrigin.x + m.x - self.downMouse.x, self.downOrigin.y + m.y - self.downMouse.y)];
        return;
    }
    [self push:0 event:e button:0];
}
- (void)rightMouseDragged:(NSEvent*)e { [self push:0 event:e button:0]; }
- (void)otherMouseDragged:(NSEvent*)e { [self push:0 event:e button:0]; }
- (void)scrollWheel:(NSEvent*)e { [self push:3 event:e button:0]; }

@end

@interface AWChildWindow : NSWindow
@end

@implementation AWChildWindow
// Keyboard stays with the hidden key (SDL) window: the host routes keys to
// whichever of its windows has focus, which the forwarded click just set.
- (BOOL)canBecomeKeyWindow { return NO; }
- (BOOL)canBecomeMainWindow { return NO; }
@end

namespace {

std::mutex g_childMutex;                      // guards g_childLayers
QVector<AppWindow::ChildLayer> g_childLayers; // retained CAMetalLayer* per desk window

void setChildLayer(quint64 hwnd, CALayer* metalLayer, int canvasX, int canvasY, int w, int h, int radius,
                   bool popup = false, int hostX = 0, int hostY = 0, int hostW = 0, int hostH = 0, int viewW = 0, int viewH = 0)
{
    std::lock_guard<std::mutex> lock(g_childMutex);
    AppWindow::ChildLayer* layer = nullptr;
    for (auto& l : g_childLayers) {
        if (l.hwnd == hwnd) {
            layer = &l;
        }
    }
    if (!layer) {
        AppWindow::ChildLayer fresh;
        fresh.hwnd = hwnd;
        fresh.metalLayer = (void*)CFBridgingRetain(metalLayer);
        g_childLayers.append(fresh);
        layer = &g_childLayers.last();
    }
    layer->canvasX = canvasX;
    layer->canvasY = canvasY;
    layer->w = w;
    layer->h = h;
    layer->radius = radius;
    layer->popup = popup;
    layer->hostX = hostX;
    layer->hostY = hostY;
    layer->hostW = hostW;
    layer->hostH = hostH;
    layer->viewW = viewW > 0 ? viewW : w;
    layer->viewH = viewH > 0 ? viewH : h;
    if (!layer->shown) {
        layer->shown = std::make_shared<std::atomic<quint64>>(0);
    }
}

void setChildLayerShape(quint64 hwnd, const QRect& trim, bool anchorRight, bool anchorBottom)
{
    std::lock_guard<std::mutex> lock(g_childMutex);
    for (auto& l : g_childLayers) {
        if (l.hwnd == hwnd) {
            l.trimX = trim.x();
            l.trimY = trim.y();
            l.trimW = trim.width() > 0 ? trim.width() : 0;
            l.trimH = trim.height() > 0 ? trim.height() : 0;
            l.anchorRight = anchorRight;
            l.anchorBottom = anchorBottom;
        }
    }
}

// The host's rect of a window as of ts (frame clock); kept for a short while
void pushDeskGeometry(quint64 hwnd, quint32 ts, int x, int y, int w, int h)
{
    std::lock_guard<std::mutex> lock(g_childMutex);
    for (auto& l : g_childLayers) {
        if (l.hwnd != hwnd) {
            continue;
        }
        if (l.geoCount > 0) {
            const auto& last = l.geo[l.geoCount - 1];
            if (last.x == x && last.y == y && last.w == w && last.h == h) {
                return;
            }
        }
        if (l.geoCount == (int)(sizeof(l.geo) / sizeof(l.geo[0]))) {
            for (int i = 1; i < l.geoCount; i++) {
                l.geo[i - 1] = l.geo[i];
            }
            l.geoCount--;
        }
        l.geo[l.geoCount++] = {ts, x, y, w, h};
    }
}

quint64 shownSize(quint64 hwnd)
{
    std::lock_guard<std::mutex> lock(g_childMutex);
    for (auto& l : g_childLayers) {
        if (l.hwnd == hwnd && l.shown) {
            return l.shown->load();
        }
    }
    return 0;
}

void setChildLayerFrozen(quint64 hwnd, bool frozen)
{
    std::lock_guard<std::mutex> lock(g_childMutex);
    for (auto& l : g_childLayers) {
        if (l.hwnd == hwnd) {
            l.frozen = frozen;
        }
    }
}

void removeChildLayer(quint64 hwnd)
{
    std::lock_guard<std::mutex> lock(g_childMutex);
    for (int i = 0; i < g_childLayers.size(); i++) {
        if (g_childLayers[i].hwnd == hwnd) {
            CFBridgingRelease(g_childLayers[i].metalLayer);
            g_childLayers.removeAt(i);
            return;
        }
    }
}

}  // namespace

namespace AppWindow {

void closeDeskWindows();

void closeChildren()
{
    deskOnMain("closeChildren", ^{
        closeDeskWindows();
    });
}

QVector<ChildLayer> childLayers()
{
    std::lock_guard<std::mutex> lock(g_childMutex);
    return g_childLayers;
}

void forEachChildLayer(const std::function<void(const ChildLayer&)>& fn)
{
    std::lock_guard<std::mutex> lock(g_childMutex);
    for (const ChildLayer& l : g_childLayers) {
        fn(l);
    }
}

void releaseChildInput(ChildInput* input)
{
    if (input->nsEvent) {
        CFBridgingRelease(input->nsEvent);
    }
    delete input;
}

}

// ---------------------------------------------------------------------------
// Desk mode
// ---------------------------------------------------------------------------

#import <QuartzCore/QuartzCore.h>
#include <atomic>

@interface AWDeskDelegate : NSObject<NSWindowDelegate>
@property (nonatomic) quint64 hwnd;
@end


// Main-queue work for the desk runs inside the session's event pump; an
// AppKit exception escaping it used to end the session's loop. Contain it
// and say what it was.
static void deskOnMain(const char* what, dispatch_block_t block)
{
    dispatch_async(dispatch_get_main_queue(), ^{
        @try {
            block();
        }
        @catch (NSException* e) {
            SDL_LogError(SDL_LOG_CATEGORY_APPLICATION, "AppWindow %s: %s", what, describeException(e).UTF8String);
        }
    });
}

namespace {

struct DeskState {
    AWChildWindow* window = nil;
    AWChildView* view = nil;
    AWDeskDelegate* delegate = nil;
    AppWindow::DeskWindow info;
    bool applying = false;       // we are changing the window: its notifications are not the user's
    bool userMoving = false;     // a drag is under way, nothing sent yet
    bool pendingMove = false;    // a move was sent; the host has not shown it yet
    bool pendingResize = false;
    bool pendingMinimize = false, pendingRestore = false;
    double pendingSince = 0;     // CACurrentMediaTime() of the last change sent
    double lastResizeSent = 0;
    double lastMoveSent = 0;
    double goneSince = 0;        // left the layout at this time; closed for good a little later (layered skins flicker)
    bool dragging = false;       // a caption drag is under way (picture frozen, host untouched)
    bool frozen = false;         // picture frozen: during a drag and until the host shows the drop
    bool inResize = false;       // a live resize by the user is under way
    NSRect liveStart = NSZeroRect;  // the frame when it began
    NSSize proposed = NSZeroSize;   // the size the user asks for (points); the host gets it
    bool anchorRight = false, anchorBottom = false;  // the edges the picture hugs (the ones not being dragged)
    bool settling = false;       // after a resize: the frame snaps to the picture once the host is done
    double settleDeadline = 0;
    int sentX = INT_MIN, sentY = INT_MIN;  // the last position we asked the host for
    bool userPlaced = false;     // the user moved it: the host's position is only a crop, not the Mac position
    int hostX = 0, hostY = 0;    // the host's position when the user last placed it
    QRect trim;                  // popups: the visible part of the host rect (empty = all of it)
};

NSMutableDictionary<NSNumber*, NSValue*>* g_desk = nil;
bool g_forceOrderPush = false;  // a drag ended: the Mac's order goes to the host, whatever the host did meanwhile  // hwnd -> DeskState*
NSWindow* g_deskKeyWindow = nil;
NSRect g_deskScreen = NSZeroRect;   // SDL global points, top-left origin
double g_deskPointsPerPx = 1.0;
NSMutableDictionary<NSString*, NSImage*>* g_deskIcons = nil;
std::atomic<quint64> g_deskFront {0};
quint64 g_deskPressed = 0;          // last desk window the user pressed (its raise is pending on the host)
double g_deskPressedAt = 0;
constexpr double kPendingGraceSec = 1.5;  // after this, the host's truth wins over an unconfirmed change

CGFloat primaryTop()
{
    NSScreen* primary = [NSScreen screens].count ? [NSScreen screens][0] : [NSScreen mainScreen];
    return NSMaxY(primary.frame);
}

NSRect deskToCocoa(int x, int y, int w, int h)
{
    const double s = g_deskPointsPerPx;
    const CGFloat px = g_deskScreen.origin.x + x * s;
    const CGFloat top = g_deskScreen.origin.y + y * s;
    const CGFloat pw = MAX(1.0, w * s), ph = MAX(1.0, h * s);
    return NSMakeRect(px, primaryTop() - top - ph, pw, ph);
}

void cocoaToDesk(NSRect frame, int& x, int& y, int& w, int& h)
{
    const double s = g_deskPointsPerPx;
    x = (int)lround((frame.origin.x - g_deskScreen.origin.x) / s);
    y = (int)lround((primaryTop() - NSMaxY(frame) - g_deskScreen.origin.y) / s);
    w = (int)lround(frame.size.width / s);
    h = (int)lround(frame.size.height / s);
}

DeskState* deskState(quint64 hwnd)
{
    NSValue* v = g_desk ? g_desk[@(hwnd)] : nil;
    return v ? (DeskState*)v.pointerValue : nullptr;
}

void pushDeskEvent(quint64 hwnd, const char* kind, int x, int y, int w, int h)
{
    auto* ev = new AppWindow::DeskEvent;
    ev->hwnd = hwnd;
    ev->kind = QString::fromLatin1(kind);
    ev->x = x;
    ev->y = y;
    ev->w = w;
    ev->h = h;
    SDL_Event e = {};
    e.type = SDL_USEREVENT;
    e.user.code = SDL_CODE_APPWINDOW_DESK_EVENT;
    e.user.data1 = ev;
    SDL_PushEvent(&e);
}

// The crop a desk window shows: the host's position (where its pixels are in
// the frame) and the Mac window's size (the picture follows a resize at once).
NSRect deskWantFrame(const DeskState* st)
{
    const AppWindow::DeskWindow& w = st->info;
    if (w.kind == 2 && st->trim.width() > 0) {
        return deskToCocoa(w.x + st->trim.x(), w.y + st->trim.y(), st->trim.width(), st->trim.height());
    }
    return deskToCocoa(w.x, w.y, w.w, w.h);
}

// The crop is the host's rect (its size too: the picture is what the host
// has, drawn top-left in the Mac window, which may be larger mid-resize).
void updateDeskCrop(DeskState* st)
{
    int x, y, vw, vh;
    cocoaToDesk(st->window.frame, x, y, vw, vh);
    const bool popup = st->info.kind == 2;
    const bool trimmed = popup && st->trim.width() > 0;
    const int cx = st->info.x + (trimmed ? st->trim.x() : 0), cy = st->info.y + (trimmed ? st->trim.y() : 0);
    // The picture is the host's size, cut to the Mac window when that is
    // smaller (mid-resize): never scaled.
    const int cw = MIN(trimmed ? st->trim.width() : st->info.w, MAX(1, vw));
    const int ch = MIN(trimmed ? st->trim.height() : st->info.h, MAX(1, vh));
    setChildLayer(st->info.hwnd, st->view.layer, cx, cy, MAX(1, cw), MAX(1, ch), st->info.radius,
                  popup, st->info.x, st->info.y, st->info.w, st->info.h, MAX(1, vw), MAX(1, vh));
    setChildLayerShape(st->info.hwnd, trimmed ? st->trim : QRect(), st->anchorRight, st->anchorBottom);
}

// A window nobody can see (minimized, fully covered, off screen) has no
// drawables to give: rendering into it would stall every other window.
void updateDeskVisible(DeskState* st)
{
    const bool visible = st->window.visible && !st->window.miniaturized && (st->window.occlusionState & NSWindowOcclusionStateVisible);
    std::lock_guard<std::mutex> lock(g_childMutex);
    for (auto& l : g_childLayers) {
        if (l.hwnd == st->info.hwnd) {
            l.visible = visible;
        }
    }
}

bool isDeskPane(const AppWindow::DeskWindow& w)  // a window or dialog, as opposed to a popup
{
    return w.kind != 2;
}

void updateDeskIcon()
{
    quint64 front = 0;
    for (NSWindow* w in [NSApp orderedWindows]) {
        if (![w isKindOfClass:[AWChildWindow class]]) {
            continue;
        }
        auto* view = (AWChildView*)w.contentView;
        DeskState* st = [view isKindOfClass:[AWChildView class]] ? deskState(view.hwnd) : nullptr;
        if (st && isDeskPane(st->info)) {
            front = st->info.hwnd;
            break;
        }
    }
    if (front == g_deskFront.load()) {
        return;
    }
    g_deskFront.store(front);
    DeskState* st = deskState(front);
    NSImage* image = st && g_deskIcons ? g_deskIcons[st->info.iconKey.toNSString()] : nil;
    if (image) {
        [NSApp setApplicationIconImage:image];
    }
}

// The host shows the drop; the video needs a few frames more to show it.
void scheduleUnfreeze(quint64 hwnd)
{
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.15 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
        DeskState* st = deskState(hwnd);
        if (st && st->frozen && !st->dragging) {
            st->frozen = false;
            setChildLayerFrozen(hwnd, false);
            updateDeskCrop(st);
            g_forceOrderPush = true;
        }
    });
}

// Window order, two ways. The Mac is the truth for what the user does
// (clicks, Cmd-Tab, Mission Control): that order goes to the host, so the
// picture of every window shows itself where it overlaps others. The host is
// the truth for what happens there on its own (a new dialog on top, an app
// raising a window): that order comes to the Mac. g_lastOrder is the order
// both last agreed on.
QVector<quint64> g_hostOrder;  // panes, bottom-most first, from the last layout
QVector<quint64> g_lastOrder;
double g_orderPushedAt = 0;
double g_activationChangedAt = 0;  // the app became (in)active: macOS restacks our windows itself

void applyHostOrder(const QVector<quint64>& order)
{
    for (int i = 1; i < order.size(); i++) {
        DeskState* below = deskState(order[i - 1]);
        DeskState* st = deskState(order[i]);
        if (below && st && below->window && st->window) {
            [st->window orderWindow:NSWindowAbove relativeTo:below->window.windowNumber];
        }
    }
}

QString orderString(const QVector<quint64>& order)
{
    QStringList parts;
    for (quint64 h : order) {
        parts.append(QString::number(h, 16));
    }
    return parts.join(' ');
}

void deskSyncOrder()
{
    if (!g_desk) {
        return;
    }
    static double s_lastReport = 0;
    QVector<quint64> mac;  // bottom-most first
    for (NSWindow* w in [[NSApp orderedWindows] reverseObjectEnumerator]) {
        if (![w isKindOfClass:[AWChildWindow class]] || ![w.contentView isKindOfClass:[AWChildView class]]) {
            continue;
        }
        const quint64 hwnd = ((AWChildView*)w.contentView).hwnd;
        DeskState* st = deskState(hwnd);
        if (!st || !isDeskPane(st->info) || st->goneSince != 0 || w.miniaturized || !w.visible) {
            continue;
        }
        if (st->dragging || st->frozen) {
            return;  // a drag and its drop: hands off until it settles
        }
        mac.append(hwnd);
    }
    QVector<quint64> host;
    for (quint64 h : g_hostOrder) {
        if (mac.contains(h)) {
            host.append(h);
        }
    }
    QVector<quint64> macCommon;
    for (quint64 h : mac) {
        if (host.contains(h)) {
            macCommon.append(h);
        }
    }
    if (macCommon.size() < 2) {
        g_lastOrder = macCommon;
        return;
    }
    const double now = CACurrentMediaTime();
    bool sameSet = macCommon.size() == g_lastOrder.size();
    for (quint64 h : macCommon) {
        sameSet = sameSet && g_lastOrder.contains(h);
    }
    if (now - s_lastReport > 2.0 && (macCommon != host || host != g_lastOrder)) {
        s_lastReport = now;
        SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION, "Desk order: mac [%s] host [%s] agreed [%s]",
                    qPrintable(orderString(macCommon)), qPrintable(orderString(host)), qPrintable(orderString(g_lastOrder)));
    }
    if (!sameSet) {
        // Windows came or went: the host decided where they are
        SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION, "Desk order: windows changed, host order [%s]", qPrintable(orderString(host)));
        applyHostOrder(host);
        g_lastOrder = host;
        return;
    }
    if (macCommon != g_lastOrder && !g_forceOrderPush && (!NSApp.isActive || now - g_activationChangedAt < 1.0)) {
        // Not the user: the app switcher or (de)activation reshuffled our
        // windows. Put them back as agreed; never send that to the host.
        if (now - s_lastReport > 2.0) {
            s_lastReport = now;
            SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION, "Desk order: ignoring a reorder while %s", NSApp.isActive ? "activating" : "inactive");
        }
        applyHostOrder(g_lastOrder);
        return;
    }
    if (macCommon != g_lastOrder || g_forceOrderPush) {
        g_forceOrderPush = false;
        // Reordered here: tell the host, top-most first
        SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION, "Desk order: reordered here [%s] (was [%s])", qPrintable(orderString(macCommon)), qPrintable(orderString(g_lastOrder)));
        const quint64 oldTop = g_lastOrder.isEmpty() ? 0 : g_lastOrder.last();
        g_lastOrder = macCommon;
        g_orderPushedAt = now;
        auto* ev = new AppWindow::DeskEvent;
        ev->kind = QStringLiteral("order");
        for (int i = macCommon.size() - 1; i >= 0; i--) {
            ev->order.append(macCommon[i]);
        }
        SDL_Event e = {};
        e.type = SDL_USEREVENT;
        e.user.code = SDL_CODE_APPWINDOW_DESK_EVENT;
        e.user.data1 = ev;
        SDL_PushEvent(&e);
        if (macCommon.last() != oldTop && NSApp.isActive) {
            pushDeskEvent(macCommon.last(), "activate", 0, 0, 0, 0);  // the keyboard follows the front window
        }
        return;
    }
    if (host != g_lastOrder && now - g_orderPushedAt > 1.0) {
        // Reordered on the host by itself
        SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION, "Desk order: reordered on the host [%s] (was [%s])", qPrintable(orderString(host)), qPrintable(orderString(g_lastOrder)));
        applyHostOrder(host);
        g_lastOrder = host;
    }
}

// Size the window to the picture, keeping the edges the picture hugs. While
// the user resizes, never below the size they ask for; after a resize, to
// the picture exactly once the host is done (or at the deadline).
void fitFrameToPicture(DeskState* st, CGFloat pw, CGFloat ph)
{
    const NSRect cur = st->window.frame;
    const CGFloat w = st->inResize ? MAX(st->proposed.width, pw) : pw;
    const CGFloat h = st->inResize ? MAX(st->proposed.height, ph) : ph;
    if (fabs(w - cur.size.width) < 0.5 && fabs(h - cur.size.height) < 0.5) {
        return;
    }
    const CGFloat x = st->anchorRight ? NSMaxX(cur) - w : NSMinX(cur);
    const CGFloat y = st->anchorBottom ? NSMinY(cur) : NSMaxY(cur) - h;
    st->applying = true;
    [st->window setFrame:NSMakeRect(x, y, w, h) display:YES animate:NO];
    st->applying = false;
    updateDeskCrop(st);
}

void settleResize(quint64 hwnd, bool deadline)
{
    if (DeskState* d = deskState(hwnd)) {
        SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION, "Desk: settle 0x%llx deadline=%d frame %.0f,%.0f %.0fx%.0f anchors %d%d host %d,%d %dx%d",
                    (unsigned long long)hwnd, deadline, d->window.frame.origin.x, d->window.frame.origin.y, d->window.frame.size.width, d->window.frame.size.height,
                    d->anchorRight, d->anchorBottom, d->info.x, d->info.y, d->info.w, d->info.h);
    }
    DeskState* st = deskState(hwnd);
    if (!st || !st->settling || st->inResize) {
        return;
    }
    const quint64 shown = shownSize(hwnd);
    const double s = g_deskPointsPerPx;
    const int sw = (int)(shown >> 32), sh = (int)(shown & 0xffffffff);
    const int wantW = (int)lround(st->proposed.width / s), wantH = (int)lround(st->proposed.height / s);
    const bool done = abs(sw - wantW) <= 2 && abs(sh - wantH) <= 2;
    if (!done && !deadline && CACurrentMediaTime() < st->settleDeadline) {
        return;
    }
    if (sw > 0 && sh > 0) {
        fitFrameToPicture(st, sw * s, sh * s);
    }
    st->settling = false;
    st->anchorRight = st->anchorBottom = false;
    updateDeskCrop(st);
    // The host may have refused the size (minimum sizes): put it where the
    // window really is now
    int x, y, w, h;
    cocoaToDesk(st->window.frame, x, y, w, h);
    if (x != st->info.x || y != st->info.y) {
        st->sentX = x;
        st->sentY = y;
        st->pendingMove = true;
        st->pendingSince = CACurrentMediaTime();
        pushDeskEvent(hwnd, "move", x, y, 0, 0);
    }
}

void closeDesk(DeskState* st)
{
    removeChildLayer(st->info.hwnd);
    if (st->window) {
        [NSObject cancelPreviousPerformRequestsWithTarget:st->delegate];
        st->window.delegate = nil;
        if (st->window.parentWindow) {
            [st->window.parentWindow removeChildWindow:st->window];
        }
        for (NSWindow* child in [st->window.childWindows copy]) {
            [st->window removeChildWindow:child];
        }
        [st->window orderOut:nil];
        [st->window close];
    }
    delete st;
}

}  // namespace

@implementation AWDeskDelegate

- (void)windowDidMove:(NSNotification*)notification
{
    DESK_TRY
    DeskState* st = deskState(self.hwnd);
    if (!st || st->applying) {
        return;
    }
    if (st->dragging) {
        return;  // our caption drag: deskEndDrag tells the host
    }
    st->userMoving = true;
    [NSObject cancelPreviousPerformRequestsWithTarget:self selector:@selector(moveSettled) object:nil];
    [self performSelector:@selector(moveSettled) withObject:nil afterDelay:0.05];
    DESK_CATCH("windowDidMove")
}

// The host is told once the move is over: moving it live would race the
// picture (the crop follows the host's position, the frame arrives later).
- (void)moveSettled
{
    DESK_TRY
    DeskState* st = deskState(self.hwnd);
    if (!st) {
        return;
    }
    if (NSEvent.pressedMouseButtons & 1) {
        [self performSelector:@selector(moveSettled) withObject:nil afterDelay:0.05];
        return;
    }
    st->userMoving = false;
    int x, y, w, h;
    cocoaToDesk(st->window.frame, x, y, w, h);
    if (x == st->info.x && y == st->info.y && !st->pendingMove) {
        return;
    }
    st->pendingMove = true;
    st->pendingSince = CACurrentMediaTime();
    st->sentX = x;
    st->sentY = y;
    pushDeskEvent(self.hwnd, "move", x, y, 0, 0);
    DESK_CATCH("moveSettled")
}

- (void)windowWillStartLiveResize:(NSNotification*)notification
{
    DESK_TRY
    DeskState* st = deskState(self.hwnd);
    if (!st) {
        return;
    }
    st->inResize = true;
    st->settling = false;
    st->liveStart = st->window.frame;
    st->proposed = st->window.frame.size;
    st->anchorRight = st->anchorBottom = false;
    DESK_CATCH("windowWillStartLiveResize")
}

// The window never gets smaller than the picture the host has drawn: a
// shrink waits for the host, so the window's own border is always visible
// and nothing is ever squeezed or cut. Growing shows the picture against the
// edges that stay put, with the rest of the window empty until the host
// catches up.
- (NSSize)windowWillResize:(NSWindow*)sender toSize:(NSSize)size
{
    DESK_TRY
    DeskState* st = deskState(self.hwnd);
    if (!st || st->applying || !st->inResize) {
        return size;
    }
    st->proposed = size;
    const quint64 shown = shownSize(self.hwnd);
    const double s = g_deskPointsPerPx;
    const CGFloat sw = (CGFloat)(shown >> 32) * s, sh = (CGFloat)(shown & 0xffffffff) * s;
    [NSObject cancelPreviousPerformRequestsWithTarget:self selector:@selector(resizeSettled) object:nil];
    const double now = CACurrentMediaTime();
    if (now - st->lastResizeSent >= 0.033) {
        [self performSelector:@selector(resizeSettled) withObject:nil afterDelay:0.0];
    }
    else {
        [self performSelector:@selector(resizeSettled) withObject:nil afterDelay:0.033];
    }
    return NSMakeSize(MAX(size.width, sw), MAX(size.height, sh));
    DESK_CATCH("windowWillResize")
    return size;
}

- (void)windowDidResize:(NSNotification*)notification
{
    DESK_TRY
    DeskState* st = deskState(self.hwnd);
    if (!st || st->applying) {
        return;
    }
    if (st->inResize) {
        // The edges being dragged move; the picture hugs the others
        const NSRect f = st->window.frame, a = st->liveStart;
        if (fabs(NSMinX(f) - NSMinX(a)) > 0.5 && fabs(NSMaxX(f) - NSMaxX(a)) < 0.5) {
            st->anchorRight = true;
        }
        if (fabs(NSMaxY(f) - NSMaxY(a)) > 0.5 && fabs(NSMinY(f) - NSMinY(a)) < 0.5) {
            st->anchorBottom = true;  // Cocoa y grows upwards: the top edge moved
        }
        updateDeskCrop(st);
        return;  // windowWillResize sends the sizes
    }
    // Zoom, tiling, Fill: not a live resize
    st->proposed = st->window.frame.size;
    updateDeskCrop(st);
    [NSObject cancelPreviousPerformRequestsWithTarget:self selector:@selector(resizeSettled) object:nil];
    [self performSelector:@selector(resizeSettled) withObject:nil afterDelay:0.033];
    DESK_CATCH("windowDidResize")
}

- (void)resizeSettled
{
    DESK_TRY
    DeskState* st = deskState(self.hwnd);
    if (!st) {
        return;
    }
    const double s = g_deskPointsPerPx;
    const int w = (int)lround(st->proposed.width / s), h = (int)lround(st->proposed.height / s);
    if (w <= 0 || h <= 0) {
        return;
    }
    if (w != st->info.w || h != st->info.h || st->pendingResize) {
        st->pendingResize = true;
        st->pendingSince = CACurrentMediaTime();
        st->lastResizeSent = st->pendingSince;
        pushDeskEvent(self.hwnd, "resize", 0, 0, w, h);
    }
    // Where the picture's top-left will be: against the edges that stay put
    const NSRect f = st->window.frame;
    const CGFloat left = st->anchorRight ? NSMaxX(f) - st->proposed.width : NSMinX(f);
    const CGFloat top = st->anchorBottom ? NSMinY(f) + st->proposed.height : NSMaxY(f);
    int x, y, dw, dh;
    cocoaToDesk(NSMakeRect(left, top - st->proposed.height, st->proposed.width, st->proposed.height), x, y, dw, dh);
    if (x != st->info.x || y != st->info.y) {
        st->pendingMove = true;
        st->pendingSince = CACurrentMediaTime();
        st->sentX = x;
    st->sentY = y;
    pushDeskEvent(self.hwnd, "move", x, y, 0, 0);  // also when the size is already right
    }
    DESK_CATCH("resizeSettled")
}

- (void)windowDidEndLiveResize:(NSNotification*)notification
{
    DESK_TRY
    DeskState* st = deskState(self.hwnd);
    if (!st) {
        return;
    }
    [NSObject cancelPreviousPerformRequestsWithTarget:self selector:@selector(resizeSettled) object:nil];
    [self resizeSettled];
    st->inResize = false;
    // The frame snaps to the picture once the host has drawn the final size
    st->settling = true;
    st->settleDeadline = CACurrentMediaTime() + 1.5;
    const quint64 hwnd = self.hwnd;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(1.6 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
        DESK_TRY
        settleResize(hwnd, true);
        DESK_CATCH("settleResize")
    });
    settleResize(hwnd, false);
    DESK_CATCH("windowDidEndLiveResize")
}

- (void)windowDidChangeOcclusionState:(NSNotification*)notification
{
    DESK_TRY
    if (DeskState* st = deskState(self.hwnd)) {
        updateDeskVisible(st);
    }
    DESK_CATCH("windowDidChangeOcclusionState")
}

- (void)windowDidMiniaturize:(NSNotification*)notification
{
    DESK_TRY
    DeskState* st = deskState(self.hwnd);
    if (!st) {
        return;
    }
    updateDeskVisible(st);
    if (st->applying) {
        return;
    }
    st->pendingMinimize = true;
    st->pendingSince = CACurrentMediaTime();
    pushDeskEvent(self.hwnd, "minimize", 0, 0, 0, 0);
    DESK_CATCH("windowDidMiniaturize")
}

- (void)windowDidDeminiaturize:(NSNotification*)notification
{
    DESK_TRY
    DeskState* st = deskState(self.hwnd);
    if (!st) {
        return;
    }
    updateDeskVisible(st);
    if (st->applying) {
        return;
    }
    st->pendingRestore = true;
    st->pendingSince = CACurrentMediaTime();
    pushDeskEvent(self.hwnd, "restore", 0, 0, 0, 0);
    updateDeskIcon();
    DESK_CATCH("windowDidDeminiaturize")
}

@end

namespace AppWindow {

void deskNotePress(quint64 hwnd)
{
    if (deskState(hwnd) && isDeskPane(deskState(hwnd)->info)) {
        g_deskPressed = hwnd;
        g_deskPressedAt = CACurrentMediaTime();
        updateDeskIcon();
    }
}

void beginDesk(SDL_Window* keyWindow, const QRect& screenPts, double pointsPerHostPx)
{
    NSWindow* ns = nsWindowFor(keyWindow);
    if (!ns) {
        return;
    }
    g_deskKeyWindow = ns;
    g_deskScreen = NSMakeRect(screenPts.x(), screenPts.y(), screenPts.width(), screenPts.height());
    {
        static id s_activeObserver = nil;
        if (!s_activeObserver) {
            [[NSNotificationCenter defaultCenter] addObserverForName:NSApplicationDidResignActiveNotification
                                                              object:nil queue:[NSOperationQueue mainQueue]
                                                          usingBlock:^(NSNotification*) {
                g_activationChangedAt = CACurrentMediaTime();
            }];
            s_activeObserver = [[NSNotificationCenter defaultCenter] addObserverForName:NSApplicationDidBecomeActiveNotification
                                                                                 object:nil queue:[NSOperationQueue mainQueue]
                                                                             usingBlock:^(NSNotification*) {
                g_activationChangedAt = CACurrentMediaTime();
                @try {
                    applyHostOrder(g_lastOrder);
                }
                @catch (NSException* e) {
                    SDL_LogError(SDL_LOG_CATEGORY_APPLICATION, "AppWindow activation: %s: %s", e.name.UTF8String, e.reason.UTF8String);
                }
            }];
        }
        // Order changes without a layout change (clicks, Cmd-Tab) are caught here
        static dispatch_source_t s_orderTimer = nullptr;
        if (!s_orderTimer) {
            s_orderTimer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
            dispatch_source_set_timer(s_orderTimer, dispatch_time(DISPATCH_TIME_NOW, 0), (uint64_t)(0.1 * NSEC_PER_SEC), (uint64_t)(0.02 * NSEC_PER_SEC));
            dispatch_source_set_event_handler(s_orderTimer, ^{
                @try {
                    deskSyncOrder();
                }
                @catch (NSException* e) {
                    SDL_LogError(SDL_LOG_CATEGORY_APPLICATION, "AppWindow deskSyncOrder: %s: %s", e.name.UTF8String, e.reason.UTF8String);
                }
            });
            dispatch_resume(s_orderTimer);
        }
    }
    // One desk pixel per device pixel: the mirrored screen's backing scale
    g_deskPointsPerPx = pointsPerHostPx > 0 ? pointsPerHostPx : 1.0;
    {
        const NSPoint corner = NSMakePoint(screenPts.x() + 1, primaryTop() - screenPts.y() - 1);
        for (NSScreen* screen in [NSScreen screens]) {
            if (NSPointInRect(corner, screen.frame) && screen.backingScaleFactor > 0) {
                g_deskPointsPerPx = 1.0 / screen.backingScaleFactor;
            }
        }
    }
    setDeskActive(true);
    // The key window carries the keyboard and nothing else: invisible, out of
    // the way of the mouse, absent from Mission Control and window cycling.
    ns.styleMask = NSWindowStyleMaskBorderless;
    ns.alphaValue = 0.0;
    ns.opaque = NO;
    ns.backgroundColor = [NSColor clearColor];
    ns.hasShadow = NO;
    ns.ignoresMouseEvents = YES;
    ns.excludedFromWindowsMenu = YES;
    ns.collectionBehavior = NSWindowCollectionBehaviorStationary | NSWindowCollectionBehaviorIgnoresCycle |
                            NSWindowCollectionBehaviorTransient | NSWindowCollectionBehaviorFullScreenAuxiliary;
    ns.title = @"Moonlight desk";
    const NSRect at = deskToCocoa(0, 0, 1, 1);
    [ns setFrame:NSMakeRect(at.origin.x, at.origin.y, 1, 1) display:NO animate:NO];
    SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION, "Desk: screen %d,%d %dx%d pt, %.4f pt/px, key window %ld",
                screenPts.x(), screenPts.y(), screenPts.width(), screenPts.height(), g_deskPointsPerPx, (long)ns.windowNumber);
}

void syncDesk(const QVector<DeskWindow>& windows)
{
    QVector<DeskWindow> wanted = windows;
    deskOnMain("syncDesk", ^{
        if (!g_desk) {
            g_desk = [NSMutableDictionary new];
        }
        const double now = CACurrentMediaTime();
        // Gone from the desk: gone from the screen
        NSMutableArray<NSNumber*>* gone = [NSMutableArray new];
        for (NSNumber* key in g_desk) {
            bool still = false;
            for (const DeskWindow& w : wanted) {
                still = still || w.hwnd == key.unsignedLongLongValue;
            }
            if (!still) {
                [gone addObject:key];
            }
        }
        for (NSNumber* key in gone) {
            auto* st = (DeskState*)g_desk[key].pointerValue;
            if (st->goneSince == 0) {
                // Hidden at once; closed only if it stays gone (layered skin
                // parts and some menus flicker in and out within a frame).
                st->goneSince = now;
                st->applying = true;
                [st->window orderOut:nil];
                st->applying = false;
                updateDeskVisible(st);
            }
            else if (now - st->goneSince > 0.5) {
                closeDesk(st);
                [g_desk removeObjectForKey:key];
            }
        }
        NSWindow* prevPane = nil;  // the pane just below this one in the host's order
        for (const DeskWindow& w : wanted) {
            DeskState* st = deskState(w.hwnd);
            NSRect wantFrame = deskToCocoa(w.x, w.y, w.w, w.h);
            if (!st) {
                st = new DeskState;
                st->info = w;
                NSWindowStyleMask style = NSWindowStyleMaskBorderless;
                if (isDeskPane(w)) {
                    style |= NSWindowStyleMaskResizable | NSWindowStyleMaskMiniaturizable;
                }
                st->window = [[AWChildWindow alloc] initWithContentRect:wantFrame styleMask:style backing:NSBackingStoreBuffered defer:NO];
                st->window.releasedWhenClosed = NO;
                st->window.opaque = NO;
                st->window.backgroundColor = [NSColor clearColor];
                // Popups: their shadow band is keyed out (transparent), so the
                // macOS shadow follows the menu itself; it is recomputed once
                // the first pictures are in.
                st->window.hasShadow = YES;
                if (!isDeskPane(w)) {
                    AWChildWindow* popupWindow = st->window;
                    for (double t : {0.15, 0.5}) {
                        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(t * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
                            [popupWindow invalidateShadow];
                        });
                    }
                }
                st->window.title = w.title.toNSString();
                st->window.movableByWindowBackground = NO;
                st->window.level = NSNormalWindowLevel;
                if (!isDeskPane(w)) {
                    st->window.collectionBehavior = NSWindowCollectionBehaviorTransient | NSWindowCollectionBehaviorIgnoresCycle;
                }
                st->view = [[AWChildView alloc] initWithFrame:NSMakeRect(0, 0, wantFrame.size.width, wantFrame.size.height)];
                st->view.hwnd = w.hwnd;
                st->view.pointsPerPx = g_deskPointsPerPx;
                st->view.dialog = isDeskPane(w);
                st->view.mainWindow = g_deskKeyWindow;
                st->window.contentView = st->view;
                st->delegate = [AWDeskDelegate new];
                st->delegate.hwnd = w.hwnd;
                st->window.delegate = st->delegate;
                [st->view updateDrawableSize];
                [st->view updateTrackingAreas];
                g_desk[@(w.hwnd)] = [NSValue valueWithPointer:st];
                updateDeskCrop(st);
                pushDeskGeometry(w.hwnd, w.ts, w.x, w.y, w.w, w.h);
                updateDeskVisible(st);
                st->applying = true;
                DeskState* owner = deskState(w.owner);
                if (!isDeskPane(w) && owner && owner->window) {
                    // Menus, tooltips, drop-downs ride on their owner
                    [owner->window addChildWindow:st->window ordered:NSWindowAbove];
                    [st->window orderFront:nil];
                }
                else if (!isDeskPane(w)) {
                    [st->window orderFront:nil];
                }
                else if (w.state == "minimized") {
                    [st->window miniaturize:nil];
                }
                else {
                    // A new window or dialog appears in front, as it did on the host
                    [st->window orderFront:nil];
                }
                st->applying = false;
                SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION, "Desk window 0x%llx (%s, kind %d) created at %d,%d %dx%d px -> %.0f,%.0f %.0fx%.0f pt",
                            (unsigned long long)w.hwnd, qPrintable(w.title), w.kind, w.x, w.y, w.w, w.h,
                            wantFrame.origin.x, wantFrame.origin.y, wantFrame.size.width, wantFrame.size.height);
            }
            else {
                const DeskWindow before = st->info;
                st->info = w;
                wantFrame = deskWantFrame(st);
                if (st->goneSince != 0) {
                    // Back before it was closed
                    st->goneSince = 0;
                    st->applying = true;
                    DeskState* owner = deskState(w.owner);
                    if (!isDeskPane(w) && owner && owner->window && st->window.parentWindow != owner->window) {
                        [owner->window addChildWindow:st->window ordered:NSWindowAbove];
                    }
                    [st->window orderFront:nil];
                    st->applying = false;
                }
                if (![st->window.title isEqualToString:w.title.toNSString()]) {
                    st->window.title = w.title.toNSString();
                }
                // Unconfirmed changes of ours: confirmed when the host shows them, dropped when it never does
                int hx, hy, hw, hh;
                cocoaToDesk(st->window.frame, hx, hy, hw, hh);
                if (st->pendingMove && (abs(hx - w.x) <= 1 && abs(hy - w.y) <= 1)) {
                    if (st->frozen && !st->dragging) {
                        scheduleUnfreeze(w.hwnd);  // once the video shows it there
                    }
                    st->pendingMove = false;
                    st->userPlaced = true;
                    st->hostX = w.x;
                    st->hostY = w.y;
                }
                if (st->pendingResize && (abs(hw - w.w) <= 1 && abs(hh - w.h) <= 1)) {
                    st->pendingResize = false;
                }
                if (st->pendingMinimize && w.state == "minimized") {
                    st->pendingMinimize = false;
                }
                if (st->pendingRestore && w.state != "minimized") {
                    st->pendingRestore = false;
                }
                if ((st->pendingMove || st->pendingResize || st->pendingMinimize || st->pendingRestore) && now - st->pendingSince > kPendingGraceSec) {
                    if (st->frozen && !st->dragging) {
                        scheduleUnfreeze(w.hwnd);
                    }
                    if (st->pendingMove) {
                        // The host put it elsewhere (kept it on the desk): the
                        // Mac window stays where the user left it, the crop follows.
                        st->userPlaced = true;
                        st->hostX = w.x;
                        st->hostY = w.y;
                    }
                    st->pendingMove = st->pendingResize = st->pendingMinimize = st->pendingRestore = false;
                }
                const bool minimized = w.state == "minimized";
                if (minimized != (bool)st->window.miniaturized && !st->pendingMinimize && !st->pendingRestore) {
                    st->applying = true;
                    if (minimized) {
                        [st->window miniaturize:nil];
                    }
                    else {
                        [st->window deminiaturize:nil];
                    }
                    st->applying = false;
                }
                const bool userBusy = st->userMoving || st->pendingMove || st->pendingResize || st->window.inLiveResize || st->inResize || st->settling;
                if (!userBusy && !minimized) {
                    const NSRect cur = st->window.frame;
                    if (st->userPlaced && isDeskPane(w)) {
                        if ((w.x != st->hostX || w.y != st->hostY) && w.x == st->sentX && w.y == st->sentY) {
                            // Our own move arriving: nothing to follow
                            st->hostX = w.x;
                            st->hostY = w.y;
                            wantFrame.origin.x = cur.origin.x;
                            wantFrame.origin.y = NSMaxY(cur) - wantFrame.size.height;
                        }
                        else if (w.x != st->hostX || w.y != st->hostY) {
                            SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION, "Desk: 0x%llx host moved %d,%d -> %d,%d by itself; following",
                                        (unsigned long long)w.hwnd, st->hostX, st->hostY, w.x, w.y);
                            // The host moved it on its own: follow by the same amount
                            const double s = g_deskPointsPerPx;
                            wantFrame.origin.x = cur.origin.x + (w.x - st->hostX) * s;
                            wantFrame.origin.y = NSMaxY(cur) - (w.y - st->hostY) * s - wantFrame.size.height;
                            st->hostX = w.x;
                            st->hostY = w.y;
                        }
                        else {
                            // Only the size is the host's; the top-left is the user's
                            wantFrame.origin.x = cur.origin.x;
                            wantFrame.origin.y = NSMaxY(cur) - wantFrame.size.height;
                        }
                    }
                    if (fabs(cur.origin.x - wantFrame.origin.x) > 0.5 || fabs(cur.origin.y - wantFrame.origin.y) > 0.5 ||
                        fabs(cur.size.width - wantFrame.size.width) > 0.5 || fabs(cur.size.height - wantFrame.size.height) > 0.5) {
                        st->applying = true;
                        [st->window setFrame:wantFrame display:YES animate:NO];
                        st->applying = false;
                    }
                }
                if (!isDeskPane(w)) {
                    DeskState* owner = deskState(w.owner);
                    NSWindow* wantParent = owner ? owner->window : nil;
                    if (st->window.parentWindow != wantParent) {
                        if (st->window.parentWindow) {
                            [st->window.parentWindow removeChildWindow:st->window];
                        }
                        if (wantParent) {
                            [wantParent addChildWindow:st->window ordered:NSWindowAbove];
                        }
                    }
                }
                updateDeskCrop(st);
                pushDeskGeometry(w.hwnd, w.ts, w.x, w.y, w.w, w.h);
                updateDeskVisible(st);
                if (before.radius != w.radius) {
                    [st->window invalidateShadow];
                }
            }
            (void)prevPane;
        }
        // Stacking: two-way, see deskSyncOrder
        g_hostOrder.clear();
        for (const DeskWindow& w : wanted) {
            if (isDeskPane(w) && w.state != "minimized") {
                g_hostOrder.append(w.hwnd);
            }
        }
        deskSyncOrder();
        updateDeskIcon();
    });
}

void setDeskIcon(const QString& iconKey, const QByteArray& png)
{
    if (iconKey.isEmpty() || png.isEmpty()) {
        return;
    }
    NSData* data = [NSData dataWithBytes:png.constData() length:(NSUInteger)png.size()];
    NSString* key = iconKey.toNSString();  // the block must not keep the caller's reference
    deskOnMain("setDeskIcon", ^{
        NSImage* image = [[NSImage alloc] initWithData:data];
        if (!image) {
            return;
        }
        if (!g_deskIcons) {
            g_deskIcons = [NSMutableDictionary new];
        }
        g_deskIcons[key] = image;
        g_deskFront.store(0);  // re-evaluate
        updateDeskIcon();
    });
}

quint64 deskFrontWindow()
{
    return g_deskFront.load();
}

double deskPointsPerPx()
{
    return g_deskPointsPerPx;
}

void deskShownChanged(quint64 hwnd, int w, int h)
{
    deskOnMain("deskShownChanged", ^{
        DeskState* st = deskState(hwnd);
        if (!st || !st->window || st->dragging || !isDeskPane(st->info)) {
            return;
        }
        if (st->inResize) {
            fitFrameToPicture(st, w * g_deskPointsPerPx, h * g_deskPointsPerPx);
        }
        else if (st->settling) {
            settleResize(hwnd, false);
        }
    });
}

QString describeCurrentException()
{
    @try {
        throw;
    }
    @catch (NSException* e) {
        return QString::fromNSString(describeException(e));
    }
    @catch (...) {
    }
    return QStringLiteral("(not an AppKit exception)");
}

void setDeskTrim(quint64 hwnd, int x, int y, int w, int h)
{
    deskOnMain("setDeskTrim", ^{
        DeskState* st = deskState(hwnd);
        if (!st || !st->window) {
            return;
        }
        const QRect trim = w > 0 && h > 0 ? QRect(x, y, w, h) : QRect();
        if (trim == st->trim) {
            return;
        }
        st->trim = trim;
        const NSRect frame = deskWantFrame(st);
        st->applying = true;
        [st->window setFrame:frame display:YES animate:NO];
        st->applying = false;
        updateDeskCrop(st);
    });
}

void dragChild(quint64 hwnd, void* nsEvent)
{
    NSEvent* event = (__bridge NSEvent*)nsEvent;
    deskOnMain("dragChild", ^{
        DeskState* st = deskState(hwnd);
        if (st && st->window && event) {
            [st->window performWindowDragWithEvent:event];
        }
    });
}

void deskMinimize(quint64 hwnd)
{
    deskOnMain("deskMinimize", ^{
        DeskState* st = deskState(hwnd);
        if (st && st->window) {
            [st->window miniaturize:nil];  // the delegate tells the host
        }
    });
}

void deskEndDrag(quint64 hwnd);

// The window server owns a native drag and swallows its mouse-up: watch the
// button instead.
static void pollDragEnd(quint64 hwnd)
{
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.016 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
        DeskState* st = deskState(hwnd);
        if (!st || !st->dragging) {
            return;
        }
        if (NSEvent.pressedMouseButtons & 1) {
            pollDragEnd(hwnd);
            return;
        }
        deskEndDrag(hwnd);
    });
}

static void beginDragNow(quint64 hwnd, bool force)
{
        DeskState* st = deskState(hwnd);
        if (!st || !st->window || (!force && !(NSEvent.pressedMouseButtons & 1))) {
            return;  // released already: it was a click after all
        }
        st->dragging = true;
        st->frozen = true;
        setChildLayerFrozen(hwnd, true);
        // Out of the way on the host, so the windows it covered there show
        // their own content on the Mac while it is dragged around. Not for
        // dialogs or windows with dialogs: Windows keeps owned windows above
        // their owner and would bury the whole family.
        bool family = st->info.owner != 0;
        for (NSNumber* key in g_desk) {
            DeskState* other = (DeskState*)g_desk[key].pointerValue;
            family = family || (other->info.owner == hwnd && isDeskPane(other->info));
        }
        if (!family) {
            pushDeskEvent(hwnd, "lower", 0, 0, 0, 0);
        }
        if (force) {
            return;  // test drags move the window themselves
        }
        // The system's own window drag: tiling at screen edges and corners,
        // Mission Control, other screens. It takes the mouse until release.
        NSWindow* win = st->window;
        const NSPoint where = [win mouseLocationOutsideOfEventStream];
        NSEvent* down = [NSEvent mouseEventWithType:NSEventTypeLeftMouseDown location:where modifierFlags:0
                                          timestamp:[[NSProcessInfo processInfo] systemUptime] windowNumber:win.windowNumber
                                            context:nil eventNumber:0 clickCount:1 pressure:1.0];
        if (down) {
            [win performWindowDragWithEvent:down];
        }
        pollDragEnd(hwnd);
}

void deskBeginDrag(quint64 hwnd)
{
    deskOnMain("deskBeginDrag", ^{
        beginDragNow(hwnd, false);
    });
}

void deskTestDrag(quint64 hwnd, int dx, int dy)
{
    deskOnMain("deskTestDrag", ^{
        beginDragNow(hwnd, true);
        DeskState* st = deskState(hwnd);
        if (!st || !st->dragging) {
            return;
        }
        const NSPoint origin = st->window.frame.origin;
        for (int i = 1; i <= 20; i++) {
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(i * 0.016 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
                DeskState* s = deskState(hwnd);
                if (s && s->dragging) {
                    [s->window setFrameOrigin:NSMakePoint(origin.x + dx * i / 20.0, origin.y - dy * i / 20.0)];
                }
            });
        }
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.4 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
            deskEndDrag(hwnd);
        });
    });
}

void deskEndDrag(quint64 hwnd)
{
    deskOnMain("deskEndDrag", ^{
        DeskState* st = deskState(hwnd);
        if (!st || !st->window) {
            return;
        }
        st->view.dragging = NO;
        pushDeskEvent(hwnd, "dragend", 0, 0, 0, 0);  // the session forgets the pending press
        if (!st->dragging) {
            return;
        }
        st->dragging = false;
        int x, y, w, h;
        cocoaToDesk(st->window.frame, x, y, w, h);
        const double now = CACurrentMediaTime();
        st->pendingMove = true;
        st->pendingSince = now;
        st->sentX = x;
        st->sentY = y;
        pushDeskEvent(hwnd, "move", x, y, 0, 0);
        pushDeskEvent(hwnd, "activate", 0, 0, 0, 0);
        if (x == st->info.x && y == st->info.y) {
            scheduleUnfreeze(hwnd);
        }
        // Tiling resizes the window after the drop and the host may never
        // match it exactly: thaw anyway (without waiting for a layout change)
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.6 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
            DESK_TRY
            scheduleUnfreeze(hwnd);
            DESK_CATCH("thaw")
        });
    });
}

void deskTestLiveResize(quint64 hwnd, int dw, int dh, bool fromLeft)
{
    deskOnMain("deskTestLiveResize", ^{
        DeskState* st = deskState(hwnd);
        if (!st || !st->window) {
            return;
        }
        NSWindow* win = st->window;
        id<NSWindowDelegate> del = win.delegate;
        const NSRect start = win.frame;
        [del windowWillStartLiveResize:[NSNotification notificationWithName:NSWindowWillStartLiveResizeNotification object:win]];
        for (int i = 1; i <= 30; i++) {
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(i * 0.016 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
                DESK_TRY
                const NSSize want = NSMakeSize(start.size.width + dw * i / 30.0, start.size.height + dh * i / 30.0);
                const NSSize got = [del windowWillResize:win toSize:want];
                // AppKit keeps the edges not being dragged: right edge for a left drag, top edge always here
                const CGFloat x = fromLeft ? NSMaxX(start) - got.width : NSMinX(start);
                [win setFrame:NSMakeRect(x, NSMaxY(start) - got.height, got.width, got.height) display:YES animate:NO];
                DESK_CATCH("test live resize step")
            });
        }
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.55 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
            DESK_TRY
            [del windowDidEndLiveResize:[NSNotification notificationWithName:NSWindowDidEndLiveResizeNotification object:win]];
            DESK_CATCH("test live resize end")
        });
    });
}

void deskTestSetFrame(quint64 hwnd, int x, int y, int w, int h)
{
    deskOnMain("deskTestSetFrame", ^{
        DeskState* st = deskState(hwnd);
        if (st && st->window) {
            // Not "applying": the delegate treats it as the user's doing
            [st->window setFrame:NSMakeRect(x, primaryTop() - y - h, w, h) display:YES animate:NO];
        }
    });
}

void deskTitleAction(quint64 hwnd)
{
    deskOnMain("deskTitleAction", ^{
        DeskState* st = deskState(hwnd);
        if (!st || !st->window) {
            return;
        }
        NSString* action = [[NSUserDefaults standardUserDefaults] stringForKey:@"AppleActionOnDoubleClick"] ?: @"Maximize";
        if ([action isEqualToString:@"None"]) {
            return;
        }
        if ([action isEqualToString:@"Minimize"]) {
            [st->window miniaturize:nil];
            return;
        }
        if ([action isEqualToString:@"Fill"]) {
            [st->window setFrame:st->window.screen.visibleFrame display:YES animate:YES];
            return;
        }
        [st->window zoom:nil];
    });
}

void closeDeskWindows()
{
    if (!g_desk) {
        return;
    }
    for (NSNumber* key in g_desk) {
        closeDesk((DeskState*)g_desk[key].pointerValue);
    }
    [g_desk removeAllObjects];
}

}
