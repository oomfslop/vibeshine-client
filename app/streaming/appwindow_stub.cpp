#include "appwindow.h"

#ifndef Q_OS_DARWIN
namespace AppWindow {
void setAppIcon(const QByteArray&) {}
void beginDesk(SDL_Window*, const QRect&, double, int, int, const DeskwmCallbacks&) {}
void closeChildren() {}
QString describeCurrentException() { return QStringLiteral("?"); }
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
