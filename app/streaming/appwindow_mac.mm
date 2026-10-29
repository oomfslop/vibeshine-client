#include "appwindow.h"

#include <SDL_syswm.h>
#import <AppKit/AppKit.h>
#include <exception>

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

void beginDesk(SDL_Window* keyWindow, const QRect& screenPts, double pointsPerHostPx, int deskW, int deskH,
               const DeskwmCallbacks& callbacks)
{
    NSWindow* ns = nsWindowFor(keyWindow);
    if (!ns) {
        return;
    }
    setDeskActive(true);
    deskwm_begin((__bridge void*)ns, screenPts.x(), screenPts.y(), screenPts.width(), screenPts.height(),
                 pointsPerHostPx, deskW, deskH, callbacks);
}

void closeChildren()
{
    deskwm_end();
}

QString describeCurrentException()
{
    @try {
        throw;
    }
    @catch (NSException* e) {
        NSArray<NSString*>* stack = e.callStackSymbols;
        NSRange keep = NSMakeRange(0, MIN((NSUInteger)10, stack.count));
        return QString::fromNSString([NSString stringWithFormat:@"%@: %@\n  %@", e.name, e.reason,
                                      [[stack subarrayWithRange:keep] componentsJoinedByString:@"\n  "]]);
    }
    @catch (...) {
    }
    return QStringLiteral("(not an AppKit exception)");
}

}
