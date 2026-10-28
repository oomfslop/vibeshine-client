#include "appwindow.h"

#include <SDL_syswm.h>
#import <AppKit/AppKit.h>
#import <objc/runtime.h>

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

void configureWindow(SDL_Window* window, const QString& title)
{
    NSWindow* ns = nsWindowFor(window);
    if (!ns) {
        return;
    }
    // Borderless, but keep the behaviours of a normal window: resizable from
    // the edges, minimizable (Cmd-M, Dock), and a title for Mission Control.
    ns.styleMask = NSWindowStyleMaskBorderless | NSWindowStyleMaskResizable | NSWindowStyleMaskMiniaturizable | NSWindowStyleMaskClosable;
    ns.titleVisibility = NSWindowTitleHidden;
    ns.titlebarAppearsTransparent = YES;
    ns.movableByWindowBackground = NO;
    ns.hasShadow = YES;
    ns.title = title.toNSString();
    // The picture is a shaped region (the host window plus whatever popups
    // stick out of it) drawn with rounded corners by the renderer; the rest
    // of the window is see-through.
    ns.opaque = NO;
    ns.backgroundColor = [NSColor clearColor];
    // SDL's content view paints itself black in drawRect: (a "no white
    // flash" measure) unless the window was created transparent, which the
    // SDL2 API cannot ask for. Replace that drawing with nothing so the
    // Metal layer's alpha reaches the screen.
    {
        Class cls = [ns.contentView class];
        Method draw = class_getInstanceMethod(cls, @selector(drawRect:));
        static IMP s_noop = imp_implementationWithBlock(^(id, NSRect) {});
        if (draw && method_getImplementation(draw) != s_noop) {
            method_setImplementation(draw, s_noop);
        }
    }
    ns.contentView.wantsLayer = YES;
    ns.contentView.layer.masksToBounds = NO;
    ns.contentView.layer.backgroundColor = [NSColor clearColor].CGColor;
    ns.contentView.layer.opaque = NO;
    for (NSView* sub in ns.contentView.subviews) {
        if (sub.layer) {
            sub.layer.opaque = NO;
            sub.layer.backgroundColor = [NSColor clearColor].CGColor;
        }
    }
    [ns.contentView setNeedsDisplay:YES];
    [ns invalidateShadow];
}

bool setFrame(SDL_Window* window, int x, int y, int w, int h)
{
    NSWindow* ns = nsWindowFor(window);
    if (!ns) {
        return false;
    }
    // SDL's global points have a top-left origin at the primary display's
    // corner; Cocoa's have a bottom-left origin there (SDL flips with the
    // primary display's height, so do the same).
    NSScreen* primary = [NSScreen screens].count ? [NSScreen screens][0] : [NSScreen mainScreen];
    const CGFloat screenTop = NSMaxY(primary.frame);
    NSRect frame = NSMakeRect(x, screenTop - y - h, w, h);
    [ns setFrame:frame display:YES animate:NO];
    return true;
}

void invalidateShadow(SDL_Window* window)
{
    NSWindow* ns = nsWindowFor(window);
    if (!ns) {
        return;
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        // Re-assert transparency: SDL may touch these when the window is
        // shown or restyled, and it is cheap to set them again here.
        ns.opaque = NO;
        ns.backgroundColor = [NSColor clearColor];
        if (ns.contentView.layer) {
            ns.contentView.layer.opaque = NO;
            ns.contentView.layer.backgroundColor = [NSColor clearColor].CGColor;
        }
        for (NSView* sub in ns.contentView.subviews) {
            if (sub.layer) {
                sub.layer.opaque = NO;
                sub.layer.backgroundColor = [NSColor clearColor].CGColor;
            }
        }
        SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION,
                    "AppWindow transparency: window opaque=%d bg=%s content=%s layer=%s(opaque=%d) subviews=%lu first=%s layer=%s(opaque=%d)",
                    ns.opaque, ns.backgroundColor.description.UTF8String,
                    NSStringFromClass([ns.contentView class]).UTF8String,
                    ns.contentView.layer ? NSStringFromClass([ns.contentView.layer class]).UTF8String : "none",
                    ns.contentView.layer ? (int)ns.contentView.layer.opaque : -1,
                    (unsigned long)ns.contentView.subviews.count,
                    ns.contentView.subviews.count ? NSStringFromClass([ns.contentView.subviews[0] class]).UTF8String : "none",
                    ns.contentView.subviews.count && ns.contentView.subviews[0].layer ? NSStringFromClass([ns.contentView.subviews[0].layer class]).UTF8String : "none",
                    ns.contentView.subviews.count && ns.contentView.subviews[0].layer ? (int)ns.contentView.subviews[0].layer.opaque : -1);
        [ns invalidateShadow];
    });
}

void setTitle(SDL_Window* window, const QString& title)
{
    NSWindow* ns = nsWindowFor(window);
    if (ns) {
        ns.title = title.toNSString();
    }
}

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

void beginDrag(SDL_Window* window)
{
    NSWindow* ns = nsWindowFor(window);
    if (!ns) {
        return;
    }
    // SDL may have dequeued later events by the time it hands us the press, so
    // NSApp.currentEvent can be stale and the drag would start from the wrong
    // anchor (the window jumps). Synthesize the press at the mouse's current
    // position instead.
    NSPoint where = [ns mouseLocationOutsideOfEventStream];
    NSEvent* down = [NSEvent mouseEventWithType:NSEventTypeLeftMouseDown
                                       location:where
                                  modifierFlags:0
                                      timestamp:[[NSProcessInfo processInfo] systemUptime]
                                   windowNumber:ns.windowNumber
                                        context:nil
                                    eventNumber:0
                                     clickCount:1
                                       pressure:1.0];
    if (down) {
        [ns performWindowDragWithEvent:down];
    }
}

double backingScale(SDL_Window* window)
{
    NSWindow* ns = nsWindowFor(window);
    return ns ? ns.backingScaleFactor : 1.0;
}

void toggleZoom(SDL_Window* window)
{
    NSWindow* ns = nsWindowFor(window);
    if (ns) {
        [ns zoom:nil];
    }
}

// "Fill" (macOS 15): take the screen's visible area without becoming a
// full-screen Space; a second time restores the previous frame.
static NSMutableDictionary<NSNumber*, NSValue*>* g_fillRestoreFrames = nil;

void performTitleAction(SDL_Window* window)
{
    NSWindow* ns = nsWindowFor(window);
    if (!ns) {
        return;
    }
    NSString* action = [[NSUserDefaults standardUserDefaults] stringForKey:@"AppleActionOnDoubleClick"];
    if (action == nil) {
        action = @"Maximize";  // macOS default is Zoom
    }
    if ([action isEqualToString:@"None"]) {
        return;
    }
    if ([action isEqualToString:@"Minimize"]) {
        [ns miniaturize:nil];
        return;
    }
    if ([action isEqualToString:@"Fill"]) {
        if (!g_fillRestoreFrames) {
            g_fillRestoreFrames = [NSMutableDictionary new];
        }
        NSNumber* key = @(ns.windowNumber);
        NSRect fill = ns.screen.visibleFrame;
        if (NSEqualRects(ns.frame, fill) && g_fillRestoreFrames[key]) {
            [ns setFrame:[g_fillRestoreFrames[key] rectValue] display:YES animate:YES];
            [g_fillRestoreFrames removeObjectForKey:key];
        }
        else {
            g_fillRestoreFrames[key] = [NSValue valueWithRect:ns.frame];
            [ns setFrame:fill display:YES animate:YES];
        }
        return;
    }
    [ns zoom:nil];  // "Maximize"
}

}

// ---------------------------------------------------------------------------
// Child dialogs as windows of their own
// ---------------------------------------------------------------------------

#import <QuartzCore/CAMetalLayer.h>
#include <mutex>
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

// A click on a child window: the keyboard must have somewhere to go (the
// main window, which forwards it; the host routes it to the clicked window),
// and a dialog comes to the front of the app.
- (void)focusMain
{
    if (self.mainWindow && !self.mainWindow.isKeyWindow) {
        [self.mainWindow makeKeyWindow];
    }
    if (self.dialog && self.window) {
        [self.window orderFront:nil];
    }
    AppWindow::deskNotePress(self.hwnd);
}
- (void)mouseDown:(NSEvent*)e { [self focusMain]; [self push:1 event:e button:1]; }
- (void)mouseUp:(NSEvent*)e { [self push:2 event:e button:1]; }
- (void)rightMouseDown:(NSEvent*)e { [self focusMain]; [self push:1 event:e button:3]; }
- (void)rightMouseUp:(NSEvent*)e { [self push:2 event:e button:3]; }
- (void)otherMouseDown:(NSEvent*)e { [self focusMain]; [self push:1 event:e button:2]; }
- (void)otherMouseUp:(NSEvent*)e { [self push:2 event:e button:2]; }
- (void)mouseMoved:(NSEvent*)e { [self push:0 event:e button:0]; }
- (void)mouseDragged:(NSEvent*)e { [self push:0 event:e button:0]; }
- (void)rightMouseDragged:(NSEvent*)e { [self push:0 event:e button:0]; }
- (void)otherMouseDragged:(NSEvent*)e { [self push:0 event:e button:0]; }
- (void)scrollWheel:(NSEvent*)e { [self push:3 event:e button:0]; }

@end

@interface AWChildWindow : NSWindow
@end

@implementation AWChildWindow
// Keyboard stays with the main (SDL) window: the host routes keys to whichever
// of its windows has focus, which the forwarded click just set.
- (BOOL)canBecomeKeyWindow { return NO; }
- (BOOL)canBecomeMainWindow { return NO; }
@end

namespace {

struct ChildState {
    AWChildWindow* window = nil;
    AWChildView* view = nil;
    AppWindow::ChildInfo info;
};

std::mutex g_childMutex;                      // guards g_childLayers
QVector<AppWindow::ChildLayer> g_childLayers; // retained CAMetalLayer* per child
NSMutableDictionary<NSNumber*, NSValue*>* g_childWindows = nil;  // hwnd -> ChildState*
NSMutableArray<NSNumber*>* g_childOrder = nil;  // hwnds bottom-most first, as the host stacks them

void setChildLayer(quint64 hwnd, CALayer* metalLayer, int canvasX, int canvasY, int w, int h, int radius)
{
    std::lock_guard<std::mutex> lock(g_childMutex);
    for (auto& l : g_childLayers) {
        if (l.hwnd == hwnd) {
            l.canvasX = canvasX;
            l.canvasY = canvasY;
            l.w = w;
            l.h = h;
            l.radius = radius;
            return;
        }
    }
    AppWindow::ChildLayer layer;
    layer.hwnd = hwnd;
    layer.metalLayer = (void*)CFBridgingRetain(metalLayer);
    layer.canvasX = canvasX;
    layer.canvasY = canvasY;
    layer.w = w;
    layer.h = h;
    layer.radius = radius;
    g_childLayers.append(layer);
}

void replaceChildLayer(const ChildState& st)
{
    setChildLayer(st.info.hwnd, st.view.layer, st.info.canvasX, st.info.canvasY, st.info.w, st.info.h, st.info.radius);
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

// On the host the window sits at the top of a tall display, so menus always
// open downwards there; on the Mac the window may be near the bottom of the
// screen. Keep a menu inside the visible area: flip it above its anchor when
// it would run off the bottom, shift it in horizontally.
NSRect fitMenuOnScreen(NSRect frame, NSWindow* main)
{
    NSScreen* screen = main.screen ?: [NSScreen mainScreen];
    if (!screen) {
        return frame;
    }
    const NSRect vis = screen.visibleFrame;
    if (NSMinY(frame) < NSMinY(vis)) {
        // Flip: the menu's top becomes its bottom (approximates Windows' flip)
        const CGFloat flippedBottom = NSMaxY(frame);
        if (flippedBottom + frame.size.height <= NSMaxY(vis)) {
            frame.origin.y = flippedBottom;
        }
        else {
            frame.origin.y = NSMinY(vis);
        }
    }
    if (NSMaxY(frame) > NSMaxY(vis)) {
        frame.origin.y = NSMaxY(vis) - frame.size.height;
    }
    if (NSMaxX(frame) > NSMaxX(vis)) {
        frame.origin.x = NSMaxX(vis) - frame.size.width;
    }
    if (NSMinX(frame) < NSMinX(vis)) {
        frame.origin.x = NSMinX(vis);
    }
    return frame;
}

void closeChild(ChildState* st)
{
    removeChildLayer(st->info.hwnd);
    if (st->window) {
        if (st->window.parentWindow) {
            [st->window.parentWindow removeChildWindow:st->window];
        }
        [st->window orderOut:nil];
        [st->window close];
    }
    delete st;
}

}  // namespace

namespace AppWindow {

void syncChildren(SDL_Window* mainWindow, const QVector<ChildInfo>& children, double pointsPerHostPx)
{
    NSWindow* ns = nsWindowFor(mainWindow);
    if (!ns) {
        SDL_LogWarn(SDL_LOG_CATEGORY_APPLICATION, "AppWindow syncChildren: no NSWindow");
        return;
    }
    QVector<ChildInfo> wanted = children;
    dispatch_async(dispatch_get_main_queue(), ^{
        if (!g_childWindows) {
            g_childWindows = [NSMutableDictionary new];
        }
        // Drop the ones that are gone
        NSMutableArray<NSNumber*>* gone = [NSMutableArray new];
        for (NSNumber* key in g_childWindows) {
            bool still = false;
            for (const ChildInfo& c : wanted) {
                if (c.hwnd == key.unsignedLongLongValue) {
                    still = true;
                }
            }
            if (!still) {
                [gone addObject:key];
            }
        }
        for (NSNumber* key in gone) {
            closeChild((ChildState*)g_childWindows[key].pointerValue);
            [g_childWindows removeObjectForKey:key];
        }
        // Remember the host's stacking order (the layout lists popups bottom-most first)
        g_childOrder = [NSMutableArray new];
        for (const ChildInfo& c : wanted) {
            [g_childOrder addObject:@(c.hwnd)];
        }
        // Create or update the rest
        for (const ChildInfo& c : wanted) {
            NSNumber* key = @(c.hwnd);
            const CGFloat w = MAX(32.0, c.w * pointsPerHostPx);
            const CGFloat h = MAX(24.0, c.h * pointsPerHostPx);
            ChildState* st = g_childWindows[key] ? (ChildState*)g_childWindows[key].pointerValue : nullptr;
            if (!st) {
                st = new ChildState;
                st->info = c;
                // Dialogs open centred over the main window and are then the
                // user's to move; menus and tooltips sit exactly where they are
                // on the host relative to the window.
                const NSRect mainFrame = ns.frame;
                CGFloat x, top;
                if (c.dialog) {
                    x = NSMidX(mainFrame) - w / 2;
                    top = NSMidY(mainFrame) + h / 2;
                }
                else {
                    x = mainFrame.origin.x + c.offsetX * pointsPerHostPx;
                    top = NSMaxY(mainFrame) - c.offsetY * pointsPerHostPx;
                }
                NSRect frame = NSMakeRect(x, top - h, w, h);
                if (!c.dialog) {
                    frame = fitMenuOnScreen(frame, ns);
                }
                st->window = [[AWChildWindow alloc] initWithContentRect:frame
                                                              styleMask:NSWindowStyleMaskBorderless
                                                                backing:NSBackingStoreBuffered
                                                                  defer:NO];
                st->window.releasedWhenClosed = NO;
                st->window.opaque = NO;
                st->window.backgroundColor = [NSColor clearColor];
                st->window.hasShadow = YES;
                st->window.title = c.title.toNSString();
                st->window.movableByWindowBackground = NO;
                st->window.level = ns.level;
                st->view = [[AWChildView alloc] initWithFrame:NSMakeRect(0, 0, w, h)];
                st->view.hwnd = c.hwnd;
                st->view.pointsPerPx = pointsPerHostPx;
                st->view.dialog = c.dialog;
                st->view.mainWindow = ns;
                st->window.contentView = st->view;
                [st->view updateDrawableSize];
                [st->view updateTrackingAreas];
                g_childWindows[key] = [NSValue valueWithPointer:st];
                replaceChildLayer(*st);
                if (c.dialog) {
                    // A sibling kept above the main window: it must not move
                    // with it (that is the user's call), only stay on top. New
                    // dialogs go in front of everything of ours (a dialog opened
                    // from another dialog belongs above it).
                    st->window.level = ns.level;
                    [st->window orderFront:nil];
                }
                else {
                    [ns addChildWindow:st->window ordered:NSWindowAbove];
                    [st->window orderFront:nil];
                }
                SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION, "AppWindow child 0x%llx created: %dx%d pt at %d,%d (canvas %d,%d)",
                            (unsigned long long)c.hwnd, (int)w, (int)h, (int)frame.origin.x, (int)frame.origin.y, c.canvasX, c.canvasY);
            }
            else {
                st->info = c;
                st->view.pointsPerPx = pointsPerHostPx;
                NSRect frame = st->window.frame;
                if (!c.dialog) {
                    // Menus follow their host offset from the parent
                    const NSRect mainFrame = ns.frame;
                    frame.origin.x = mainFrame.origin.x + c.offsetX * pointsPerHostPx;
                    frame.origin.y = NSMaxY(mainFrame) - c.offsetY * pointsPerHostPx - h;
                    frame.size = NSMakeSize(w, h);
                    frame = fitMenuOnScreen(frame, ns);
                }
                if (fabs(frame.size.width - w) >= 1.0 || fabs(frame.size.height - h) >= 1.0 || !NSEqualRects(frame, st->window.frame)) {
                    if (c.dialog) {
                        frame.origin.y = NSMaxY(frame) - h;  // keep the top-left where it is
                    }
                    frame.size = NSMakeSize(w, h);
                    [st->window setFrame:frame display:YES animate:NO];
                }
                if (![st->window.title isEqualToString:c.title.toNSString()]) {
                    st->window.title = c.title.toNSString();
                }
                replaceChildLayer(*st);
            }
            [st->window invalidateShadow];
        }
    });
}

void closeDeskWindows();

void closeChildren()
{
    dispatch_async(dispatch_get_main_queue(), ^{
        closeDeskWindows();
        if (!g_childWindows) {
            return;
        }
        for (NSNumber* key in g_childWindows) {
            closeChild((ChildState*)g_childWindows[key].pointerValue);
        }
        [g_childWindows removeAllObjects];
    });
}

void raiseChildren(SDL_Window* mainWindow)
{
    NSWindow* ns = nsWindowFor(mainWindow);
    if (!ns) {
        return;
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        if (!g_childWindows) {
            return;
        }
        // Bottom-most first, each above the previous: the host's stacking order
        NSArray<NSNumber*>* order = g_childOrder ?: g_childWindows.allKeys;
        NSInteger above = ns.windowNumber;
        for (NSNumber* key in order) {
            NSValue* v = g_childWindows[key];
            if (!v) {
                continue;
            }
            auto* st = (ChildState*)v.pointerValue;
            if (st->info.dialog && st->window) {
                [st->window orderWindow:NSWindowAbove relativeTo:above];
                above = st->window.windowNumber;
            }
        }
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

void dragChild(quint64 hwnd, void* nsEvent)
{
    NSEvent* event = (__bridge NSEvent*)nsEvent;
    dispatch_async(dispatch_get_main_queue(), ^{
        if (!g_childWindows) {
            return;
        }
        NSValue* v = g_childWindows[@(hwnd)];
        if (v && event) {
            auto* st = (ChildState*)v.pointerValue;
            [st->window performWindowDragWithEvent:event];
        }
    });
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
};

NSMutableDictionary<NSNumber*, NSValue*>* g_desk = nil;  // hwnd -> DeskState*
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
void updateDeskCrop(DeskState* st)
{
    int x, y, w, h;
    cocoaToDesk(st->window.frame, x, y, w, h);
    setChildLayer(st->info.hwnd, st->view.layer, st->info.x, st->info.y, MAX(1, w), MAX(1, h), st->info.radius);
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
    DeskState* st = deskState(self.hwnd);
    if (!st || st->applying) {
        return;
    }
    st->userMoving = true;
    [NSObject cancelPreviousPerformRequestsWithTarget:self selector:@selector(moveSettled) object:nil];
    [self performSelector:@selector(moveSettled) withObject:nil afterDelay:0.05];
}

// The host is told once the drag is over: moving it live would race the
// picture (the crop follows the host's position, the frame arrives later).
- (void)moveSettled
{
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
    if (x == st->info.x && y == st->info.y) {
        return;
    }
    st->pendingMove = true;
    st->pendingSince = CACurrentMediaTime();
    pushDeskEvent(self.hwnd, "move", x, y, 0, 0);
}

- (void)windowDidResize:(NSNotification*)notification
{
    DeskState* st = deskState(self.hwnd);
    if (!st || st->applying) {
        return;
    }
    updateDeskCrop(st);
    const double now = CACurrentMediaTime();
    [NSObject cancelPreviousPerformRequestsWithTarget:self selector:@selector(resizeSettled) object:nil];
    if (now - st->lastResizeSent >= 0.05) {
        [self resizeSettled];
    }
    else {
        [self performSelector:@selector(resizeSettled) withObject:nil afterDelay:0.05];
    }
}

- (void)resizeSettled
{
    DeskState* st = deskState(self.hwnd);
    if (!st) {
        return;
    }
    int x, y, w, h;
    cocoaToDesk(st->window.frame, x, y, w, h);
    if (w == st->info.w && h == st->info.h && !st->pendingResize) {
        return;
    }
    st->pendingResize = true;
    st->pendingSince = CACurrentMediaTime();
    st->lastResizeSent = st->pendingSince;
    pushDeskEvent(self.hwnd, "resize", 0, 0, w, h);
    // A zoom moves the window as well
    if (x != st->info.x || y != st->info.y) {
        st->pendingMove = true;
        pushDeskEvent(self.hwnd, "move", x, y, 0, 0);
    }
}

- (void)windowDidEndLiveResize:(NSNotification*)notification
{
    [NSObject cancelPreviousPerformRequestsWithTarget:self selector:@selector(resizeSettled) object:nil];
    [self resizeSettled];
}

- (void)windowDidMiniaturize:(NSNotification*)notification
{
    DeskState* st = deskState(self.hwnd);
    if (!st || st->applying) {
        return;
    }
    st->pendingMinimize = true;
    st->pendingSince = CACurrentMediaTime();
    pushDeskEvent(self.hwnd, "minimize", 0, 0, 0, 0);
}

- (void)windowDidDeminiaturize:(NSNotification*)notification
{
    DeskState* st = deskState(self.hwnd);
    if (!st || st->applying) {
        return;
    }
    st->pendingRestore = true;
    st->pendingSince = CACurrentMediaTime();
    pushDeskEvent(self.hwnd, "restore", 0, 0, 0, 0);
    updateDeskIcon();
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
    g_deskPointsPerPx = pointsPerHostPx > 0 ? pointsPerHostPx : 1.0;
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
    dispatch_async(dispatch_get_main_queue(), ^{
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
            closeDesk((DeskState*)g_desk[key].pointerValue);
            [g_desk removeObjectForKey:key];
        }
        NSWindow* prevPane = nil;  // the pane just below this one in the host's order
        for (const DeskWindow& w : wanted) {
            DeskState* st = deskState(w.hwnd);
            const NSRect wantFrame = deskToCocoa(w.x, w.y, w.w, w.h);
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
                st->window.hasShadow = YES;
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
                if (![st->window.title isEqualToString:w.title.toNSString()]) {
                    st->window.title = w.title.toNSString();
                }
                // Unconfirmed changes of ours: confirmed when the host shows them, dropped when it never does
                int hx, hy, hw, hh;
                cocoaToDesk(st->window.frame, hx, hy, hw, hh);
                if (st->pendingMove && (abs(hx - w.x) <= 1 && abs(hy - w.y) <= 1)) {
                    st->pendingMove = false;
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
                const bool userBusy = st->userMoving || st->pendingMove || st->pendingResize || st->window.inLiveResize;
                if (!userBusy && !minimized) {
                    const NSRect cur = st->window.frame;
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
                if (before.radius != w.radius) {
                    [st->window invalidateShadow];
                }
            }
            (void)prevPane;
        }
        // Stacking: the host's order, bottom-most first, unless the user just
        // raised one of ours and the host has not caught up yet.
        {
            QVector<NSWindow*> hostOrder;  // bottom-most first, panes only, not minimized
            for (const DeskWindow& w : wanted) {
                DeskState* st = deskState(w.hwnd);
                if (st && isDeskPane(w) && !st->window.miniaturized) {
                    hostOrder.append(st->window);
                }
            }
            bool pressPending = false;
            if (g_deskPressed && now - g_deskPressedAt < 0.6) {
                pressPending = hostOrder.isEmpty() || deskState(g_deskPressed) == nullptr || hostOrder.last() != deskState(g_deskPressed)->window;
            }
            if (!pressPending) {
                g_deskPressed = 0;
            }
            if (!pressPending && hostOrder.size() > 1) {
                QVector<NSWindow*> macOrder;  // bottom-most first
                for (NSWindow* w in [[NSApp orderedWindows] reverseObjectEnumerator]) {
                    if (hostOrder.contains(w)) {
                        macOrder.append(w);
                    }
                }
                if (macOrder != hostOrder) {
                    for (int i = 1; i < hostOrder.size(); i++) {
                        [hostOrder[i] orderWindow:NSWindowAbove relativeTo:hostOrder[i - 1].windowNumber];
                    }
                }
            }
        }
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
    dispatch_async(dispatch_get_main_queue(), ^{
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

void deskMinimize(quint64 hwnd)
{
    dispatch_async(dispatch_get_main_queue(), ^{
        DeskState* st = deskState(hwnd);
        if (st && st->window) {
            [st->window miniaturize:nil];  // the delegate tells the host
        }
    });
}

void deskTestSetFrame(quint64 hwnd, int x, int y, int w, int h)
{
    dispatch_async(dispatch_get_main_queue(), ^{
        DeskState* st = deskState(hwnd);
        if (st && st->window) {
            // Not "applying": the delegate treats it as the user's doing
            [st->window setFrame:NSMakeRect(x, primaryTop() - y - h, w, h) display:YES animate:NO];
        }
    });
}

void deskTitleAction(quint64 hwnd)
{
    dispatch_async(dispatch_get_main_queue(), ^{
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
