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

void replaceChildLayer(const ChildState& st)
{
    std::lock_guard<std::mutex> lock(g_childMutex);
    for (auto& l : g_childLayers) {
        if (l.hwnd == st.info.hwnd) {
            l.canvasX = st.info.canvasX;
            l.canvasY = st.info.canvasY;
            l.w = st.info.w;
            l.h = st.info.h;
            l.radius = st.info.radius;
            return;
        }
    }
    AppWindow::ChildLayer layer;
    layer.hwnd = st.info.hwnd;
    layer.metalLayer = (void*)CFBridgingRetain(st.view.layer);
    layer.canvasX = st.info.canvasX;
    layer.canvasY = st.info.canvasY;
    layer.w = st.info.w;
    layer.h = st.info.h;
    layer.radius = st.info.radius;
    g_childLayers.append(layer);
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

void closeChildren()
{
    dispatch_async(dispatch_get_main_queue(), ^{
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
