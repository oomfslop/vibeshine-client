#include "appwindow.h"

#ifndef Q_OS_DARWIN
namespace AppWindow {
void configureWindow(SDL_Window*, const QString&) {}
void setTitle(SDL_Window*, const QString&) {}
void setAppIcon(const QByteArray&) {}
void beginDrag(SDL_Window*) {}
double backingScale(SDL_Window*) { return 1.0; }
void toggleZoom(SDL_Window*) {}
void performTitleAction(SDL_Window*) {}
}
#endif

#include <atomic>
namespace {
std::atomic<int> g_cropW {0}, g_cropH {0};
}
namespace AppWindow {
void setCrop(int w, int h) { g_cropW.store(w); g_cropH.store(h); }
void crop(int& w, int& h) { w = g_cropW.load(); h = g_cropH.load(); }
}
