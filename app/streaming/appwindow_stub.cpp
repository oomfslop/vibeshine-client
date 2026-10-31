#include "appwindow.h"

#ifndef Q_OS_DARWIN
namespace AppWindow {
void setAppIcon(const QByteArray&) {}
void beginDesk(SDL_Window*, const QRect&, double, int, int, const DeskwmCallbacks&) {}
void closeChildren() {}
QString describeCurrentException() { return QStringLiteral("?"); }
}

void deskwm_layout(const char*, size_t) {}
void deskwm_hittest_result(uint64_t, const char*) {}
void deskwm_set_icon(const char*, const uint8_t*, size_t) {}
void deskwm_command(const char*) {}
double deskwm_points_per_px(void) { return 0; }
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
