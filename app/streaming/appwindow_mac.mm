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
