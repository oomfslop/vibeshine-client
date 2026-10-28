#pragma once

#include <QSemaphore>
#include <QAtomicInt>
#include <QSharedPointer>
#include <QQuickWindow>

#include <Limelight.h>
#include <opus_multistream.h>
#include "settings/streamingpreferences.h"
#include "input/input.h"
#include "video/decoder.h"
#include "audio/renderers/renderer.h"
#include "video/overlaymanager.h"
#include <QJsonObject>
#include <QJsonArray>
#include <QVector>
#include <QRect>
#include "quickmenu.h"

// SDL_USEREVENT codes shared with helper classes
#define SDL_CODE_QUICKMENU_TIMER 106
#define SDL_CODE_CLIPBOARD_TEXT_RECEIVED 107
#define SDL_CODE_QUICKMENU_TOAST 108
#define SDL_CODE_APPWINDOW_CMD 109

class SupportedVideoFormatList : public QList<int>
{
public:
    operator int() const
    {
        int value = 0;

        for (const int v : *this) {
            value |= v;
        }

        return value;
    }

    void
    removeByMask(int mask)
    {
        int i = 0;
        while (i < this->length()) {
            if (this->value(i) & mask) {
                this->removeAt(i);
            }
            else {
                i++;
            }
        }
    }

    void
    deprioritizeByMask(int mask)
    {
        QList<int> deprioritizedList;

        int i = 0;
        while (i < this->length()) {
            if (this->value(i) & mask) {
                deprioritizedList.append(this->takeAt(i));
            }
            else {
                i++;
            }
        }

        this->append(std::move(deprioritizedList));
    }

    int maskByServerCodecModes(int serverCodecModes)
    {
        int mask = 0;

        const QMap<int, int> mapping = {
            {SCM_H264, VIDEO_FORMAT_H264},
            {SCM_H264_HIGH8_444, VIDEO_FORMAT_H264_HIGH8_444},
            {SCM_HEVC, VIDEO_FORMAT_H265},
            {SCM_HEVC_MAIN10, VIDEO_FORMAT_H265_MAIN10},
            {SCM_HEVC_REXT8_444, VIDEO_FORMAT_H265_REXT8_444},
            {SCM_HEVC_REXT10_444, VIDEO_FORMAT_H265_REXT10_444},
            {SCM_AV1_MAIN8, VIDEO_FORMAT_AV1_MAIN8},
            {SCM_AV1_MAIN10, VIDEO_FORMAT_AV1_MAIN10},
            {SCM_AV1_HIGH8_444, VIDEO_FORMAT_AV1_HIGH8_444},
            {SCM_AV1_HIGH10_444, VIDEO_FORMAT_AV1_HIGH10_444},
        };

        for (QMap<int, int>::const_iterator it = mapping.cbegin(); it != mapping.cend(); ++it) {
            if (serverCodecModes & it.key()) {
                mask |= it.value();
                serverCodecModes &= ~it.key();
            }
        }

        // Make sure nobody forgets to update this for new SCM values
        SDL_assert(serverCodecModes == 0);

        int val = *this;
        return val & mask;
    }
};

class Session : public QObject
{
    Q_OBJECT

    friend class SdlInputHandler;
    friend class DeferredSessionCleanupTask;
    friend class AsyncConnectionStartThread;
    friend class ClipboardSyncTask;
    friend class QuickMenu;

public:
    explicit Session(NvComputer* computer, NvApp& app, StreamingPreferences *preferences = nullptr);
    virtual ~Session();

    Q_INVOKABLE bool initialize(QQuickWindow* qtWindow);
    Q_INVOKABLE void start();
    Q_INVOKABLE void interrupt();

    // Test harness: fixed window geometry and always-on-top so an
    // automated windowed stream stays visible (and un-throttled) while the
    // user works in other apps.
    void setWindowHandle(quint64 handle) { m_WindowHandle = handle; }
    // Draw only the local cursor: the host leaves its cursor out of the capture.
    // Always on for app windows.
    void setHideHostCursor(bool hide) { m_HideHostCursor = hide; }

    // App-window streaming: present the stream as a borderless window that
    // mirrors one host window (size in host pixels, caption height, title,
    // icon PNG). Implies windowed mode.
    struct HitRegion {
        QString kind;  // "drag", "min", "max", "close"
        QRect rect;    // host pixels relative to the window's frame
    };
    struct AppWindowInfo {
        QString title;
        QByteArray iconPng;
        int x = 0, y = 0;         // host window position (host screen pixels)
        int widthPx = 0;
        int heightPx = 0;
        int anchorX = 0, anchorY = 0;  // canvas pixel of the window's top-left
        int radius = 0;           // corner radius the host OS applies
        QVector<QRect> popups;    // host screen pixels, bottom-most first
        int captionPx = 0;
        int dpi = 96;
        QVector<HitRegion> hits;
    };
    static QVector<HitRegion> parseHitRegions(const QJsonArray& arr);
    void setAppWindow(const AppWindowInfo& info) { m_AppWindow = info; m_IsAppWindow = true; }
    bool isAppWindow() const { return m_IsAppWindow; }
    // Points per host pixel for the app window (logical-size parity with the host)
    double appWindowScale() const { return 96.0 / qMax(48, m_AppWindow.dpi); }

    void setTestWindow(int x, int y, int w, int h)
    {
        m_TestWindow = QRect(x, y, w, h);
        m_HasTestWindow = true;
    }
    Q_PROPERTY(QStringList launchWarnings MEMBER m_LaunchWarnings NOTIFY launchWarningsChanged);

    static
    void getDecoderInfo(SDL_Window* window,
                        bool& isHardwareAccelerated, bool& isFullScreenOnly,
                        bool& isHdrSupported, QSize& maxResolution);

    static Session* get()
    {
        return s_ActiveSession;
    }

    Overlay::OverlayManager& getOverlayManager()
    {
        return m_OverlayManager;
    }

    void flushWindowEvents();

    QStringList launchWarnings() const
    {
        return m_LaunchWarnings;
    }

    // Display to open the stream on when no Qt window is available
    // (helper mode), given as the display's top-left corner in global
    // screen coordinates.
    void setPreferredDisplayOrigin(int x, int y)
    {
        m_PreferredDisplayOrigin = QPoint(x, y);
        m_HasPreferredDisplayOrigin = true;
    }

    void setShouldExit(bool quitHostApp = false);

    // Ends the stream. If quitHostApp is true, the app on the host is quit
    // too, regardless of the user's preference.
    void requestDisconnect(bool quitHostApp);

    // Shows or hides the in-stream quick menu overlay
    void toggleQuickMenu();

    enum class ClipboardDirection {
        ToHost,
        FromHost,
    };

    // Starts an asynchronous clipboard sync with an Apollo host. The result
    // is reported via the quick menu toast if showResult is true.
    void syncClipboard(ClipboardDirection direction, bool showResult);

signals:
    void stageStarting(QString stage);

    void stageFailed(QString stage, int errorCode, QString failingPorts);

    void connectionStarted();

    void displayLaunchError(QString text);

    void quitStarting();

    void sessionFinished(int portTestResult);

    // Emitted after sessionFinished() when the session is ready to be destroyed
    void readyForDeletion();

    // App-window streaming: window state changes the launcher must forward to
    // the host (resize, focus, minimize, restore, close)
    void appWindowEvent(QJsonObject event);

    void launchWarningsChanged();

private:
    void exec();

    bool startConnectionAsync();

    bool validateLaunch(SDL_Window* testWindow);

    void emitLaunchWarning(QString text);

    bool populateDecoderProperties(SDL_Window* window);

    IAudioRenderer* createAudioRenderer(const POPUS_MULTISTREAM_CONFIGURATION opusConfig);

    bool initializeAudioRenderer();

    bool testAudio(int audioConfiguration);

    int getAudioRendererCapabilities(int audioConfiguration);

    void getWindowDimensions(int& x, int& y,
                             int& width, int& height);

    void toggleFullscreen();

    void notifyMouseEmulationMode(bool enabled);

    bool isClipboardSyncAllowed(ClipboardDirection direction);

    void onClipboardTextReceived(const QString& text);

    void updateOptimalWindowDisplayMode();

    enum class DecoderAvailability {
        None,
        Software,
        Hardware
    };

    static
    DecoderAvailability getDecoderAvailability(SDL_Window* window,
                                               StreamingPreferences::VideoDecoderSelection vds,
                                               int videoFormat, int width, int height, int frameRate);

    static
    bool chooseDecoder(StreamingPreferences::VideoDecoderSelection vds,
                       StreamingPreferences::RendererSelection renderer,
                       SDL_Window* window, int videoFormat, int width, int height,
                       int frameRate, bool enableVsync, bool enableFramePacing,
                       bool testOnly,
                       IVideoDecoder*& chosenDecoder);

    static
    void clStageStarting(int stage);

    static
    void clStageFailed(int stage, int errorCode);

    static
    void clConnectionTerminated(int errorCode);

    static
    void clLogMessage(const char* format, ...);

    static
    void clRumble(unsigned short controllerNumber, unsigned short lowFreqMotor, unsigned short highFreqMotor);

    static
    void clConnectionStatusUpdate(int connectionStatus);

    static
    void clSetHdrMode(bool enabled);

    static
    void clRumbleTriggers(uint16_t controllerNumber, uint16_t leftTrigger, uint16_t rightTrigger);

    static
    void clSetMotionEventState(uint16_t controllerNumber, uint8_t motionType, uint16_t reportRateHz);

    static
    void clSetControllerLED(uint16_t controllerNumber, uint8_t r, uint8_t g, uint8_t b);

    static
    void clSetAdaptiveTriggers(uint16_t controllerNumber, uint8_t eventFlags, uint8_t typeLeft, uint8_t typeRight, uint8_t *left, uint8_t *right);

    static
    int arInit(int audioConfiguration,
               const POPUS_MULTISTREAM_CONFIGURATION opusConfig,
               void* arContext, int arFlags);

    static
    void arCleanup();

    static
    void arDecodeAndPlaySample(char* sampleData, int sampleLength);

    static
    int drSetup(int videoFormat, int width, int height, int frameRate, void*, int);

    static
    void drCleanup();

    static
    int drSubmitDecodeUnit(PDECODE_UNIT du);

    StreamingPreferences* m_Preferences;
    bool m_IsFullScreen;
    SupportedVideoFormatList m_SupportedVideoFormats; // Sorted in order of descending priority
    STREAM_CONFIGURATION m_StreamConfig;
    DECODER_RENDERER_CALLBACKS m_VideoCallbacks;
    AUDIO_RENDERER_CALLBACKS m_AudioCallbacks;
    NvComputer* m_Computer;
    NvApp m_App;
    SDL_Window* m_Window;
    IVideoDecoder* m_VideoDecoder;
    SDL_mutex* m_DecoderLock;
    bool m_AudioDisabled;
    bool m_AudioMuted;
    Uint32 m_FullScreenFlag;
    QQuickWindow* m_QtWindow;
    QPoint m_PreferredDisplayOrigin;
    bool m_HasPreferredDisplayOrigin;

    QRect m_TestWindow;
    bool m_HasTestWindow = false;
    // App-window streaming: host window handle to capture (0 = whole display)
    quint64 m_WindowHandle = 0;
    bool m_HideHostCursor = false;
    AppWindowInfo m_AppWindow;
    bool m_IsAppWindow = false;
    bool m_AppWindowDragging = false;
    // Last window size (points) that came from the host, so its SIZE_CHANGED
    // echo isn't reported back as a user resize.
    int m_AppWindowHostW = 0, m_AppWindowHostH = 0;
    // Region of the host screen the Mac window currently covers: the host
    // window plus whatever popups stick out of it (host pixels).
    QRect m_AppWindowExt;
    QPoint m_AppWindowInset;  // host window top-left relative to the Mac window's (host pixels)
    void applyAppWindowLayout(bool placeWindow);
    QPoint appWindowHostPoint(int x, int y) const;
    // Last size we asked the host for (points) and when the user last resized,
    // so host geometry echoes don't fight a live resize.
    int m_AppWindowRequestedW = 0, m_AppWindowRequestedH = 0;
    qint64 m_AppWindowUserResizeMs = 0;
    void handleAppWindowCommand(const QJsonObject& cmd);
    bool m_UnexpectedTermination;
    SdlInputHandler* m_InputHandler;
    int m_MouseEmulationRefCount;
    int m_FlushingWindowEventsRef;
    QStringList m_LaunchWarnings;
    bool m_ShouldExit;

    bool m_AsyncConnectionSuccess;
    int m_PortTestResults;

    int m_ActiveVideoFormat;
    int m_ActiveVideoWidth;
    int m_ActiveVideoHeight;
    int m_ActiveVideoFrameRate;

    OpusMSDecoder* m_OpusDecoder;
    IAudioRenderer* m_AudioRenderer;
    OPUS_MULTISTREAM_CONFIGURATION m_ActiveAudioConfig;
    OPUS_MULTISTREAM_CONFIGURATION m_OriginalAudioConfig;
    int m_AudioSampleCount;
    Uint32 m_DropAudioEndTime;

    Overlay::OverlayManager m_OverlayManager;
    QuickMenu m_QuickMenu;
    QString m_LastSyncedClipboardText;
    // Shared with ClipboardSyncTask, which may outlive this Session
    QSharedPointer<QAtomicInt> m_ClipboardSyncInProgress;

    static CONNECTION_LISTENER_CALLBACKS k_ConnCallbacks;
    static Session* s_ActiveSession;
    static QSemaphore s_ActiveSessionSemaphore;
};
