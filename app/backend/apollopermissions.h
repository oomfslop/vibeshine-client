#pragma once

#include <QCoreApplication>
#include <QStringList>

// Client permission bits advertised by Apollo hosts in the serverinfo
// "Permission" element. These mirror crypto::PERM in Apollo's source.
namespace ApolloPermissions {

enum Permission : quint32 {
    InputController = 1u << 8,
    InputTouch      = 1u << 9,
    InputPen        = 1u << 10,
    InputMouse      = 1u << 11,
    InputKeyboard   = 1u << 12,

    ClipboardSet    = 1u << 16,
    ClipboardRead   = 1u << 17,
    FileUpload      = 1u << 18,
    FileDownload    = 1u << 19,
    ServerCommand   = 1u << 20,

    ListApps        = 1u << 24,
    ViewStreams     = 1u << 25,
    LaunchApps      = 1u << 26,
};

inline bool has(quint32 permissions, Permission flag)
{
    return (permissions & flag) != 0;
}

// Human-readable list of the granted permissions
inline QStringList describe(quint32 permissions)
{
    struct {
        Permission flag;
        const char* name;
    } static const k_Names[] = {
        { InputController, QT_TRANSLATE_NOOP("ApolloPermissions", "Controller") },
        { InputTouch,      QT_TRANSLATE_NOOP("ApolloPermissions", "Touch") },
        { InputPen,        QT_TRANSLATE_NOOP("ApolloPermissions", "Pen") },
        { InputMouse,      QT_TRANSLATE_NOOP("ApolloPermissions", "Mouse") },
        { InputKeyboard,   QT_TRANSLATE_NOOP("ApolloPermissions", "Keyboard") },
        { ClipboardSet,    QT_TRANSLATE_NOOP("ApolloPermissions", "Set clipboard") },
        { ClipboardRead,   QT_TRANSLATE_NOOP("ApolloPermissions", "Read clipboard") },
        { FileUpload,      QT_TRANSLATE_NOOP("ApolloPermissions", "Upload files") },
        { FileDownload,    QT_TRANSLATE_NOOP("ApolloPermissions", "Download files") },
        { ServerCommand,   QT_TRANSLATE_NOOP("ApolloPermissions", "Server commands") },
        { ListApps,        QT_TRANSLATE_NOOP("ApolloPermissions", "List apps") },
        { ViewStreams,     QT_TRANSLATE_NOOP("ApolloPermissions", "View streams") },
        { LaunchApps,      QT_TRANSLATE_NOOP("ApolloPermissions", "Launch apps") },
    };

    QStringList granted;
    for (const auto& entry : k_Names) {
        if (has(permissions, entry.flag)) {
            granted.append(QCoreApplication::translate("ApolloPermissions", entry.name));
        }
    }
    return granted;
}

}
