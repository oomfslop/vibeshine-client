#pragma once

#include <QObject>
#include <QJsonObject>

class Session;
class NvComputer;

namespace CliStreamHelper
{

// Runs a single streaming session from a JSON description read on stdin,
// reporting progress as JSON lines on stdout. This is how the GPUI launcher
// (sloplight) drives the C++ streaming core.
class Launcher : public QObject
{
    Q_OBJECT

public:
    explicit Launcher(QObject* parent = nullptr);
    ~Launcher();

    // Reads the config from stdin. Returns false (after printing an error
    // event) if it is unusable.
    bool loadConfigFromStdin();

    // Must be called once the event loop is running
    Q_INVOKABLE void execute();

private slots:
    void onStageStarting(QString stage);
    void onStageFailed(QString stage, int errorCode, QString failingPorts);
    void onConnectionStarted();
    void onDisplayLaunchError(QString text);
    void onQuitStarting();
    void onSessionFinished(int portTestResult);
    void onReadyForDeletion();

    void onAppWindowEvent(QJsonObject event);

private:
    void startStdinCommandReader();
    void emitEvent(const QJsonObject& event);
    void applySettings(const QJsonObject& settings);

    QJsonObject m_Config;
    NvComputer* m_Computer;
    Session* m_Session;
    bool m_Finished;
};

}
