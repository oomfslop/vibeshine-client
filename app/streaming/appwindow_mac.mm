#include "appwindow.h"

#include <SDL_syswm.h>
#import <AppKit/AppKit.h>

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
    // Rounded corners like native windows; the video is drawn by a Metal
    // layer, so clip the content view.
    ns.contentView.wantsLayer = YES;
    ns.contentView.layer.cornerRadius = 10.0;
    ns.contentView.layer.masksToBounds = YES;
    [ns invalidateShadow];
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

}
