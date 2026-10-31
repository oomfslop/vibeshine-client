#pragma once

#include "identitymanager.h"
#include "nvapp.h"
#include "nvaddress.h"

#include <Limelight.h>

#include <QUrl>
#include <QNetworkAccessManager>
#include <QNetworkReply>

class NvComputer;

class NvDisplayMode
{
public:
    bool operator==(const NvDisplayMode& other) const
    {
        return width == other.width &&
                height == other.height &&
                refreshRate == other.refreshRate;
    }

    int width;
    int height;
    int refreshRate;
};
Q_DECLARE_TYPEINFO(NvDisplayMode, Q_PRIMITIVE_TYPE);

class GfeHttpResponseException : public std::exception
{
public:
    GfeHttpResponseException(int statusCode, QString message) :
        m_StatusCode(statusCode),
        m_StatusMessage(message.toUtf8())
    {

    }

    const char* what() const throw()
    {
        return m_StatusMessage.constData();
    }

    const char* getStatusMessage() const
    {
        return m_StatusMessage.constData();
    }

    int getStatusCode() const
    {
        return m_StatusCode;
    }

    QString toQString() const
    {
        return QString::fromUtf8(m_StatusMessage) + " (Error " + QString::number(m_StatusCode) + ")";
    }

private:
    int m_StatusCode;
    QByteArray m_StatusMessage;
};

class QtNetworkReplyException : public std::exception
{
public:
    QtNetworkReplyException(QNetworkReply::NetworkError error, QString errorText) :
        m_Error(error),
        m_ErrorText(errorText.toUtf8())
    {

    }

    const char* what() const throw()
    {
        return m_ErrorText.constData();
    }

    const char* getErrorText() const
    {
        return m_ErrorText.constData();
    }

    QNetworkReply::NetworkError getError() const
    {
        return m_Error;
    }

    QString toQString() const
    {
        return QString::fromUtf8(m_ErrorText) + " (Error " + QString::number(m_Error) + ")";
    }

private:
    QNetworkReply::NetworkError m_Error;
    QByteArray m_ErrorText;
};

// Extra launch parameters understood by Apollo hosts. All of these are
// ignored by GFE and Sunshine, so it is safe to pass them unconditionally.
class NvLaunchOptions
{
public:
    // App UUID to launch (Apollo). Empty if the host didn't provide one.
    QString appUuid;

    // Fractional refresh rate in Hz (e.g. 59.94). Zero means the integer
    // fps from the stream configuration is used.
    double fractionalFps = 0;

    // Ask the host to stream from a virtual display.
    bool useVirtualDisplay = false;

    // Host-side render scale factor in percent (100 = disabled).
    int scaleFactor = 100;

    // Ask a Vibeshine/Vibepollo host for its VRR low-latency policy: the
    // virtual display runs at 1000 Hz and frames are captured right after
    // the game presents them, instead of on the display's refresh grid.
    bool clientVrrRequested = false;
    // App-window streaming (Vibeshine fork): capture only this host window
    bool hideCursor = false;  // the client draws its own cursor; leave the host cursor out of the capture
    // Desk session (Vibeshine fork): capture this client's desk, the private
    // host display that mirrors the client's screen (opened beforehand).
    bool desk = false;
};

class NvHTTP : public QObject
{
    Q_OBJECT

public:
    enum NvLogLevel {
        NVLL_NONE,
        NVLL_ERROR,
        NVLL_VERBOSE
    };

    explicit NvHTTP(NvAddress address, uint16_t httpsPort, QSslCertificate serverCert, bool useTrueUid, QNetworkAccessManager* nam = nullptr);

    explicit NvHTTP(NvComputer* computer, QNetworkAccessManager* nam = nullptr);

    static
    int
    getCurrentGame(QString serverInfo);

    QString
    getServerInfo(NvLogLevel logLevel, bool fastFail = false);

    static
    void
    verifyResponseStatus(QString xml);

    static
    QString
    getXmlString(QString xml,
                 QString tagName);

    static
    QStringList
    getXmlStringList(QString xml,
                     QString tagName);

    static
    QByteArray
    getXmlStringFromHex(QString xml,
                        QString tagName);

    QString
    openConnectionToString(QUrl baseUrl,
                           QString command,
                           QString arguments,
                           int timeoutMs,
                           NvLogLevel logLevel = NvLogLevel::NVLL_VERBOSE);

    void setServerCert(QSslCertificate serverCert);
    void setAddress(NvAddress address);
    void setHttpsPort(uint16_t port);
    void setTrueUid(bool useTrueUid);

    NvAddress address();

    QSslCertificate serverCert();

    uint16_t httpPort();

    uint16_t httpsPort();

    static
    QVector<int>
    parseQuad(QString quad);

    void
    quitApp();

    void
    startApp(QString verb,
             bool isGfe,
             int appId,
             PSTREAM_CONFIGURATION streamConfig,
             bool sops,
             bool localAudio,
             int gamepadMask,
             bool persistGameControllersOnDisconnect,
             const NvLaunchOptions& launchOptions,
             QString& rtspSessionUrl);

    // Apollo clipboard sync endpoints. These require an active stream from
    // this client and the matching clipboard permission on the host.
    // Both throw GfeHttpResponseException/QtNetworkReplyException on failure.
    QString
    getClipboardText();

    // App-window streaming: what the host app says is under a frame-relative
    // pixel ("drag", "min", "max", "close", "border", "client", "none").
    QString
    hitTestWindow(quint64 windowHandle, int x, int y);

    // Reuse the connection between requests (Vibeshine's app-window requests:
    // one round trip each instead of a TCP and TLS handshake first). Not for
    // GFE, which cannot take persistent connections.
    void setKeepAlive(bool keepAlive) { m_KeepAlive = keepAlive; }

    // Desk streaming. deskLayout long-polls everything on the desk (JSON,
    // see the server's /actions/desk-layout); windowAction runs an op on a
    // host window (move onto the desk = "adopt" with desk-relative x, y);
    // windowIcon is the window's icon as PNG (empty when none).
    QString
    deskLayout(quint64 etag, int waitMs);

    bool
    windowAction(quint64 windowHandle, const QString& op, int x = 0, int y = 0, int w = 0, int h = 0);

    QByteArray
    windowIcon(quint64 windowHandle);

    // Vibeshine: type in this language on the host (the client's keyboard
    // input source, "en", "ru", ...). False when the host can't.
    bool
    setKeyboardLanguage(const QString& language);

    // Stack the desk's windows like this (comma-separated handles, top-most first)
    bool
    deskOrder(const QString& hwnds, bool focus);

    void
    setClipboardText(const QString& text);

    QVector<NvApp>
    getAppList();

    QImage
    getBoxArt(int appId);

    static
    QVector<NvDisplayMode>
    getDisplayModeList(QString serverInfo);

    QUrl m_BaseUrlHttp;
    QUrl m_BaseUrlHttps;
private:
    void
    handleSslErrors(QNetworkReply* reply, const QList<QSslError>& errors);

    QNetworkReply*
    openConnection(QUrl baseUrl,
                   QString command,
                   QString arguments,
                   int timeoutMs,
                   NvLogLevel logLevel,
                   const QByteArray* postBody = nullptr,
                   QString postContentType = QString());

    NvAddress m_Address;
    QNetworkAccessManager* m_Nam;
    QSslCertificate m_ServerCert;
    bool m_UseTrueUid;
    bool m_KeepAlive = false;
};
