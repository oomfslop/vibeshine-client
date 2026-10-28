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
void invalidateShadow(SDL_Window*) {}
bool setFrame(SDL_Window*, int, int, int, int) { return false; }
void syncChildren(SDL_Window*, const QVector<ChildInfo>&, double) {}
void closeChildren() {}
void raiseChildren(SDL_Window*) {}
QVector<ChildLayer> childLayers() { return {}; }
void forEachChildLayer(const std::function<void(const ChildLayer&)>&) {}
void dragChild(quint64, void*) {}
void releaseChildInput(ChildInput* input) { delete input; }
void beginDesk(SDL_Window*, const QRect&, double) {}
void syncDesk(const QVector<DeskWindow>&) {}
void setDeskIcon(const QString&, const QByteArray&) {}
quint64 deskFrontWindow() { return 0; }
void deskMinimize(quint64) {}
void deskTestSetFrame(quint64, int, int, int, int) {}
void deskTitleAction(quint64) {}
}
#endif

#include <QMutex>
#include <QMutexLocker>
#include <atomic>

namespace {
QMutex g_layoutLock;
AppWindow::Layout g_layout;
std::atomic<bool> g_active {false};
std::atomic<bool> g_desk {false};
}

namespace AppWindow {

void setLayout(const Layout& layout)
{
    QMutexLocker lock(&g_layoutLock);
    static unsigned s_version = 0;
    g_layout = layout;
    g_layout.version = ++s_version;
    g_active.store(true);
}

Layout layout()
{
    QMutexLocker lock(&g_layoutLock);
    return g_layout;
}

bool active()
{
    return g_active.load();
}

void setDeskActive(bool on)
{
    g_desk.store(on);
    if (on) {
        g_active.store(true);
    }
}

bool deskActive()
{
    return g_desk.load();
}

}
