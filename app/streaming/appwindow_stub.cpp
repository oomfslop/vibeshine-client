#include "appwindow.h"

#ifndef Q_OS_DARWIN
namespace AppWindow {
void setAppIcon(const QByteArray&) {}
void closeChildren() {}
QVector<ChildLayer> childLayers() { return {}; }
void forEachChildLayer(const std::function<void(const ChildLayer&)>&) {}
void dragChild(quint64, void*) {}
void releaseChildInput(ChildInput* input) { delete input; }
void beginDesk(SDL_Window*, const QRect&, double, int, int) {}
void syncDesk(const QVector<DeskWindow>&) {}
void setDeskIcon(const QString&, const QByteArray&) {}
quint64 deskFrontWindow() { return 0; }
void deskMinimize(quint64) {}
void deskBeginDrag(quint64) {}
void deskTestLiveResize(quint64, int, int, bool) {}
void deskTestTile(quint64, int, int, int, int) {}
void deskTestFront(quint64) {}
void deskShownChanged(quint64, int, int) {}
QString describeCurrentException() { return QStringLiteral("?"); }
void deskEndDrag(quint64) {}
void deskTestDrag(quint64, int, int) {}
double deskPointsPerPx() { return 1.0; }
void setDeskTrim(quint64, int, int, int, int) {}
void deskTestSetFrame(quint64, int, int, int, int) {}
void deskTitleAction(quint64) {}
}
#endif

#include <atomic>

namespace {
std::atomic<bool> g_desk {false};
}

namespace AppWindow {

void setDeskActive(bool on)
{
    g_desk.store(on);
}

bool deskActive()
{
    return g_desk.load();
}

}
