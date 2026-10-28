#include "appwindow.h"

#ifndef Q_OS_DARWIN
namespace AppWindow {
void configureWindow(SDL_Window*, const QString&) {}
void setTitle(SDL_Window*, const QString&) {}
void setAppIcon(const QByteArray&) {}
void beginDrag(SDL_Window*) {}
double backingScale(SDL_Window*) { return 1.0; }
}
#endif
