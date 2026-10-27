#include "streamhelper.h"
#include "backend/identitymanager.h"
#include "backend/nvcomputer.h"
#include "backend/nvapp.h"
#include "settings/streamingpreferences.h"
#include "streaming/session.h"

#include <QCoreApplication>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QTimer>
#include <QDateTime>
#include <QTextStream>

#include <cstdio>
#include <csignal>

#include <SDL.h>

// Ends the stream the way closing the window does, so stats get logged and
// the host sees a normal disconnect. Used for SIGTERM/SIGINT and the
// optional "max_duration_ms" config key (end-to-end test runs).
static void requestStreamQuit()
{
    SDL_Event event;
    SDL_zero(event);
    event.type = SDL_QUIT;
    event.quit.timestamp = SDL_GetTicks();
    SDL_PushEvent(&event);
}

static void onTerminateSignal(int)
{
    requestStreamQuit();
}

namespace CliStreamHelper
{

Launcher::Launcher(QObject* parent)
    : QObject(parent),
      m_Computer(nullptr),
      m_Session(nullptr),
      m_Finished(false)
{
}

Launcher::~Launcher()
{
    delete m_Computer;
}

void Launcher::emitEvent(const QJsonObject& event)
{
    QByteArray line = QJsonDocument(event).toJson(QJsonDocument::Compact);
    line.append('\n');
    fwrite(line.constData(), 1, line.size(), stdout);
    fflush(stdout);
}

bool Launcher::loadConfigFromStdin()
{
    QFile in;
    if (!in.open(stdin, QIODevice::ReadOnly)) {
        emitEvent({{"event", "launch_error"}, {"text", "Unable to read helper config"}});
        return false;
    }
    QByteArray data = in.readAll();
    QJsonParseError error;
    QJsonDocument doc = QJsonDocument::fromJson(data, &error);
    if (!doc.isObject()) {
        emitEvent({{"event", "launch_error"}, {"text", "Invalid helper config: " + error.errorString()}});
        return false;
    }
    m_Config = doc.object();

    QJsonObject identity = m_Config.value("identity").toObject();
    if (!IdentityManager::setOverride(identity.value("certificate_pem").toString().toUtf8(),
                                      identity.value("private_key_pem").toString().toUtf8(),
                                      identity.value("unique_id").toString())) {
        emitEvent({{"event", "launch_error"}, {"text", "Invalid client identity in helper config"}});
        return false;
    }
    return true;
}

static int jsonInt(const QJsonObject& o, const char* key, int def)
{
    return o.contains(key) ? o.value(key).toInt(def) : def;
}

static bool jsonBool(const QJsonObject& o, const char* key, bool def)
{
    return o.contains(key) ? o.value(key).toBool(def) : def;
}

void Launcher::applySettings(const QJsonObject& s)
{
    StreamingPreferences* p = StreamingPreferences::get();

    p->width = jsonInt(s, "width", p->width);
    p->height = jsonInt(s, "height", p->height);
    p->fps = jsonInt(s, "fps", p->fps);
    p->fractionalFps = s.value("fractional_fps").toDouble(0);
    p->bitrateKbps = jsonInt(s, "bitrate_kbps", p->bitrateKbps);
    p->unlockBitrate = jsonBool(s, "unlock_bitrate", p->unlockBitrate);
    p->autoAdjustBitrate = jsonBool(s, "auto_adjust_bitrate", p->autoAdjustBitrate);
    p->enableVsync = jsonBool(s, "enable_vsync", p->enableVsync);
    p->framePacing = jsonBool(s, "frame_pacing", p->framePacing);
    p->enableHdr = jsonBool(s, "enable_hdr", p->enableHdr);
    p->enableYUV444 = jsonBool(s, "enable_yuv444", p->enableYUV444);
    p->showPerformanceOverlay = jsonBool(s, "show_performance_overlay", p->showPerformanceOverlay);
    p->windowMode = static_cast<StreamingPreferences::WindowMode>(jsonInt(s, "window_mode", p->windowMode));
    p->useVirtualDisplay = jsonBool(s, "use_virtual_display", p->useVirtualDisplay);
    p->vrrCapture = jsonBool(s, "vrr_capture", p->vrrCapture);
    p->resolutionScaleFactor = jsonInt(s, "resolution_scale_factor", p->resolutionScaleFactor);
    p->clipboardSync = jsonBool(s, "clipboard_sync", p->clipboardSync);
    p->audioConfig = static_cast<StreamingPreferences::AudioConfig>(jsonInt(s, "audio_config", p->audioConfig));
    p->playAudioOnHost = jsonBool(s, "play_audio_on_host", p->playAudioOnHost);
    p->muteOnFocusLoss = jsonBool(s, "mute_on_focus_loss", p->muteOnFocusLoss);
    p->absoluteMouseMode = jsonBool(s, "absolute_mouse_mode", p->absoluteMouseMode);
    p->absoluteTouchMode = jsonBool(s, "absolute_touch_mode", p->absoluteTouchMode);
    p->swapMouseButtons = jsonBool(s, "swap_mouse_buttons", p->swapMouseButtons);
    p->reverseScrollDirection = jsonBool(s, "reverse_scroll_direction", p->reverseScrollDirection);
    p->captureSysKeysMode = static_cast<StreamingPreferences::CaptureSysKeysMode>(jsonInt(s, "capture_sys_keys_mode", p->captureSysKeysMode));
    p->multiController = jsonBool(s, "multi_controller", p->multiController);
    p->swapFaceButtons = jsonBool(s, "swap_face_buttons", p->swapFaceButtons);
    p->gamepadMouse = jsonBool(s, "gamepad_mouse", p->gamepadMouse);
    p->backgroundGamepad = jsonBool(s, "background_gamepad", p->backgroundGamepad);
    p->gameOptimizations = jsonBool(s, "game_optimizations", p->gameOptimizations);
    p->quitAppAfter = jsonBool(s, "quit_app_after", p->quitAppAfter);
    p->connectionWarnings = jsonBool(s, "connection_warnings", p->connectionWarnings);
    p->configurationWarnings = jsonBool(s, "configuration_warnings", p->configurationWarnings);
    p->richPresence = jsonBool(s, "rich_presence", p->richPresence);
    p->keepAwake = jsonBool(s, "keep_awake", p->keepAwake);
    p->videoDecoderSelection = static_cast<StreamingPreferences::VideoDecoderSelection>(jsonInt(s, "video_decoder_selection", p->videoDecoderSelection));
    p->videoCodecConfig = static_cast<StreamingPreferences::VideoCodecConfig>(jsonInt(s, "video_codec_config", p->videoCodecConfig));
    p->rendererSelection = static_cast<StreamingPreferences::RendererSelection>(jsonInt(s, "renderer_selection", p->rendererSelection));
    p->packetSize = jsonInt(s, "packet_size", p->packetSize);
    p->detectNetworkBlocking = jsonBool(s, "detect_network_blocking", p->detectNetworkBlocking);
}

void Launcher::execute()
{
    applySettings(m_Config.value("settings").toObject());

    QJsonObject h = m_Config.value("host").toObject();
    m_Computer = new NvComputer();
    m_Computer->name = h.value("name").toString();
    m_Computer->uuid = h.value("uuid").toString();
    m_Computer->hasCustomName = false;
    m_Computer->activeAddress = NvAddress(h.value("address").toString(), (uint16_t)h.value("port").toInt(DEFAULT_HTTP_PORT));
    m_Computer->manualAddress = m_Computer->activeAddress;
    m_Computer->activeHttpsPort = (uint16_t)h.value("https_port").toInt(DEFAULT_HTTPS_PORT);
    m_Computer->serverCert = QSslCertificate(h.value("server_cert_pem").toString().toUtf8());
    m_Computer->appVersion = h.value("app_version").toString();
    m_Computer->gfeVersion = h.value("gfe_version").toString();
    m_Computer->gpuModel = h.value("gpu_model").toString();
    m_Computer->isNvidiaServerSoftware = h.value("is_nvidia_server_software").toBool(false);
    m_Computer->serverCodecModeSupport = h.value("server_codec_mode_support").toInt(SCM_H264);
    m_Computer->maxLumaPixelsHEVC = h.value("max_luma_pixels_hevc").toInt(0);
    m_Computer->currentGameId = h.value("current_game_id").toInt(0);
    m_Computer->hasApolloExtensions = h.value("has_apollo_extensions").toBool(false);
    m_Computer->apolloPermissions = (quint32)h.value("apollo_permissions").toDouble(0);
    for (const QJsonValue& v : h.value("server_commands").toArray()) {
        m_Computer->serverCommands.append(v.toString());
    }
    m_Computer->state = NvComputer::CS_ONLINE;
    m_Computer->pairState = NvComputer::PS_PAIRED;
    m_Computer->isSupportedServerVersion = true;
    m_Computer->virtualDisplayCapable = false;
    m_Computer->virtualDisplayDriverReady = false;

    QJsonObject a = m_Config.value("app").toObject();
    NvApp app;
    app.id = a.value("id").toInt();
    app.name = a.value("name").toString();
    app.uuid = a.value("uuid").toString();
    app.hdrSupported = a.value("hdr_supported").toBool(false);
    m_Computer->appList.append(app);

    if (m_Computer->serverCert.isNull()) {
        emitEvent({{"event", "launch_error"}, {"text", "The host certificate in the helper config is invalid"}});
        QCoreApplication::exit(1);
        return;
    }

    m_Session = new Session(m_Computer, app, StreamingPreferences::get());

    QJsonArray origin = m_Config.value("display_origin").toArray();
    if (origin.size() == 2) {
        m_Session->setPreferredDisplayOrigin(origin.at(0).toInt(), origin.at(1).toInt());
    }

    connect(m_Session, &Session::stageStarting, this, &Launcher::onStageStarting);
    connect(m_Session, &Session::stageFailed, this, &Launcher::onStageFailed);
    connect(m_Session, &Session::connectionStarted, this, &Launcher::onConnectionStarted);
    connect(m_Session, &Session::displayLaunchError, this, &Launcher::onDisplayLaunchError);
    connect(m_Session, &Session::quitStarting, this, &Launcher::onQuitStarting);
    connect(m_Session, &Session::sessionFinished, this, &Launcher::onSessionFinished);
    connect(m_Session, &Session::readyForDeletion, this, &Launcher::onReadyForDeletion);

    if (!m_Session->initialize(nullptr)) {
        // initialize() emits displayLaunchError for the reason
        onSessionFinished(0);
        onReadyForDeletion();
        return;
    }

    QJsonArray warnings;
    for (const QString& w : m_Session->launchWarnings()) {
        warnings.append(w);
    }
    emitEvent({{"event", "warnings"}, {"warnings", warnings}});

    // Run the session (returns once exec() has been scheduled)
    std::signal(SIGTERM, onTerminateSignal);
    std::signal(SIGINT, onTerminateSignal);

    QTimer::singleShot(0, this, [this]() {
        m_Session->start();
    });
}

void Launcher::onStageStarting(QString stage)
{
    emitEvent({{"event", "stage_starting"}, {"stage", stage}});
}

void Launcher::onStageFailed(QString stage, int errorCode, QString failingPorts)
{
    emitEvent({{"event", "stage_failed"}, {"stage", stage}, {"error_code", errorCode}, {"failing_ports", failingPorts}});
}

void Launcher::onConnectionStarted()
{
    emitEvent({{"event", "connection_started"}});
    int maxMs = m_Config.value("max_duration_ms").toInt(0);
    if (maxMs > 0) {
        QTimer::singleShot(maxMs, this, []() { requestStreamQuit(); });
    }
    // End-to-end input test: send a relative mouse move every N ms and report
    // the wall-clock send time so a host-side probe can measure input latency.
    int inputMs = m_Config.value("test_input_period_ms").toInt(0);
    if (inputMs > 0) {
        auto* timer = new QTimer(this);
        auto* dir = new int(1);
        connect(timer, &QTimer::timeout, this, [this, dir]() {
            *dir = -*dir;
            qint64 t = QDateTime::currentMSecsSinceEpoch();
            LiSendMouseMoveEvent((short)(40 * *dir), 0);
            emitEvent({{"event", "test_input"}, {"t_ms", (double)t}});
        });
        timer->start(inputMs);
    }
}

void Launcher::onDisplayLaunchError(QString text)
{
    emitEvent({{"event", "launch_error"}, {"text", text}});
}

void Launcher::onQuitStarting()
{
    emitEvent({{"event", "quit_starting"}});
}

void Launcher::onSessionFinished(int portTestResult)
{
    if (!m_Finished) {
        m_Finished = true;
        emitEvent({{"event", "session_finished"}, {"port_test_result", portTestResult}});
    }
}

void Launcher::onReadyForDeletion()
{
    if (m_Session != nullptr) {
        m_Session->deleteLater();
        m_Session = nullptr;
    }
    QCoreApplication::quit();
}

}
