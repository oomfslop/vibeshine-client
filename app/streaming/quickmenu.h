#pragma once

#include "SDL_compat.h"

#include <QString>
#include <QVector>

class Session;

// In-stream quick menu rendered as a text overlay by the active video renderer.
//
// The menu is navigated with the keyboard (arrow keys, Enter, Escape) or a gamepad
// (D-pad, A, B) and provides access to streaming controls as well as Apollo-specific
// features such as clipboard sync and server commands. While the menu is open, all
// keyboard and gamepad input is consumed by the menu instead of being sent to the host.
class QuickMenu
{
public:
    explicit QuickMenu(Session* session);

    bool isOpen() const
    {
        return m_Open;
    }

    void toggle();
    void open();
    void close();

    // Shows a transient message in the menu overlay area. If the menu
    // is not open, the message is displayed on its own and hidden after
    // a short delay.
    void showToast(const QString& message);

    // Called from the main loop when the toast timer fires
    void onTimerExpired();

    // Input handlers. These return true if the event was consumed and must
    // not be forwarded to the input handler (and therefore the host).
    bool handleKeyEvent(SDL_KeyboardEvent* event);
    bool handleControllerButtonEvent(SDL_ControllerButtonEvent* event);
    bool handleMouseButtonEvent(SDL_MouseButtonEvent* event);

private:
    enum class Page {
        Main,
        ServerCommands,
    };

    enum class Action {
        Resume,
        ToggleStats,
        ToggleFullscreen,
        ToggleMouseCapture,
        SendClipboard,
        FetchClipboard,
        OpenServerCommands,
        ExecuteServerCommand,
        Back,
        Disconnect,
        QuitAppAndDisconnect,
    };

    struct Item {
        QString label;
        Action action;
        int argument;
    };

    void rebuildItems();
    void render();
    void moveSelection(int delta);
    void activateSelection();
    void goBack();
    void performAction(const Item& item);
    void startToastTimer();
    void cancelToastTimer();

    static Uint32 toastTimerCallback(Uint32 interval, void* param);

    Session* m_Session;
    bool m_Open;
    Page m_Page;
    int m_SelectedIndex;
    QVector<Item> m_Items;
    QString m_Toast;
    SDL_TimerID m_ToastTimer;
};
