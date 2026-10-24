#include "quickmenu.h"
#include "session.h"
#include "backend/apollopermissions.h"
#include "utils.h"

#include <Limelight.h>

#include <QCoreApplication>

#define TOAST_DURATION_MS 2500

QuickMenu::QuickMenu(Session* session)
    : m_Session(session),
      m_Open(false),
      m_Page(Page::Main),
      m_SelectedIndex(0),
      m_ToastTimer(0)
{
}

void QuickMenu::toggle()
{
    if (m_Open) {
        close();
    }
    else {
        open();
    }
}

void QuickMenu::open()
{
    if (m_Open) {
        return;
    }

    SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION, "Opening quick menu");

    m_Open = true;
    m_Page = Page::Main;
    m_SelectedIndex = 0;
    m_Toast.clear();
    cancelToastTimer();

    // Release any keys and gamepad buttons that are held down (including the
    // ones that make up the toggle combo), since their release events will be
    // consumed by the menu rather than being sent to the host.
    if (m_Session->m_InputHandler != nullptr) {
        m_Session->m_InputHandler->raiseAllKeys();
        m_Session->m_InputHandler->releaseAllGamepadButtons();
    }

    rebuildItems();
    render();
}

void QuickMenu::close()
{
    if (!m_Open) {
        return;
    }

    SDL_LogInfo(SDL_LOG_CATEGORY_APPLICATION, "Closing quick menu");

    m_Open = false;
    m_Items.clear();

    if (m_Session->m_InputHandler != nullptr) {
        // Make sure nothing held while the menu was open gets stuck on the host
        m_Session->m_InputHandler->raiseAllKeys();
        m_Session->m_InputHandler->releaseAllGamepadButtons();
    }

    // Keep the overlay visible if a toast is still pending, otherwise hide it
    if (m_Toast.isEmpty()) {
        m_Session->getOverlayManager().setOverlayState(Overlay::OverlayQuickMenu, false);
    }
    else {
        render();
    }
}

void QuickMenu::showToast(const QString& message)
{
    m_Toast = message;
    render();
    startToastTimer();
}

void QuickMenu::onTimerExpired()
{
    m_ToastTimer = 0;
    m_Toast.clear();

    if (m_Open) {
        render();
    }
    else {
        m_Session->getOverlayManager().setOverlayState(Overlay::OverlayQuickMenu, false);
    }
}

Uint32 QuickMenu::toastTimerCallback(Uint32, void* param)
{
    // Punt to the main thread, since the overlay state must be
    // manipulated there and SDL timers run on their own thread.
    SDL_Event event = {};
    event.type = SDL_USEREVENT;
    event.user.code = SDL_CODE_QUICKMENU_TIMER;
    event.user.data1 = param;
    SDL_PushEvent(&event);

    // Don't reschedule
    return 0;
}

void QuickMenu::startToastTimer()
{
    cancelToastTimer();
    m_ToastTimer = SDL_AddTimer(TOAST_DURATION_MS, toastTimerCallback, this);
}

void QuickMenu::cancelToastTimer()
{
    if (m_ToastTimer != 0) {
        SDL_RemoveTimer(m_ToastTimer);
        m_ToastTimer = 0;
    }
}

void QuickMenu::rebuildItems()
{
    m_Items.clear();

    NvComputer* computer = m_Session->m_Computer;
    bool apollo = computer->hasApolloExtensions;
    quint32 permissions = computer->apolloPermissions;

    if (m_Page == Page::Main) {
        m_Items.append({ QCoreApplication::translate("QuickMenu", "Resume"), Action::Resume, 0 });
        m_Items.append({ QCoreApplication::translate("QuickMenu", "Performance stats"), Action::ToggleStats, 0 });
        if (WMUtils::isRunningDesktopEnvironment()) {
            m_Items.append({ QCoreApplication::translate("QuickMenu", "Fullscreen"), Action::ToggleFullscreen, 0 });
            m_Items.append({ QCoreApplication::translate("QuickMenu", "Mouse capture"), Action::ToggleMouseCapture, 0 });
        }
        if (apollo && ApolloPermissions::has(permissions, ApolloPermissions::ClipboardSet)) {
            m_Items.append({ QCoreApplication::translate("QuickMenu", "Send clipboard to host"), Action::SendClipboard, 0 });
        }
        if (apollo && ApolloPermissions::has(permissions, ApolloPermissions::ClipboardRead)) {
            m_Items.append({ QCoreApplication::translate("QuickMenu", "Get clipboard from host"), Action::FetchClipboard, 0 });
        }
        if (apollo && ApolloPermissions::has(permissions, ApolloPermissions::ServerCommand) && !computer->serverCommands.isEmpty()) {
            m_Items.append({ QCoreApplication::translate("QuickMenu", "Server commands..."), Action::OpenServerCommands, 0 });
        }
        m_Items.append({ QCoreApplication::translate("QuickMenu", "Disconnect"), Action::Disconnect, 0 });
        m_Items.append({ QCoreApplication::translate("QuickMenu", "Quit app and disconnect"), Action::QuitAppAndDisconnect, 0 });
    }
    else {
        for (int i = 0; i < computer->serverCommands.count(); i++) {
            m_Items.append({ computer->serverCommands[i], Action::ExecuteServerCommand, i });
        }
        m_Items.append({ QCoreApplication::translate("QuickMenu", "Back"), Action::Back, 0 });
    }

    if (m_SelectedIndex >= m_Items.count()) {
        m_SelectedIndex = m_Items.count() - 1;
    }
    if (m_SelectedIndex < 0) {
        m_SelectedIndex = 0;
    }
}

void QuickMenu::render()
{
    QString text;

    if (m_Open) {
        text += (m_Page == Page::Main ?
                     QCoreApplication::translate("QuickMenu", "Quick menu  (Ctrl+Alt+Shift+\\)") :
                     QCoreApplication::translate("QuickMenu", "Server commands")) + "\n\n";

        for (int i = 0; i < m_Items.count(); i++) {
            text += (i == m_SelectedIndex ? "> " : "  ") + m_Items[i].label + '\n';
        }

        text += '\n' + QCoreApplication::translate("QuickMenu", "Up/Down select   Enter/A confirm   Esc/B back");
    }

    if (!m_Toast.isEmpty()) {
        if (!text.isEmpty()) {
            text += "\n\n";
        }
        text += m_Toast;
    }

    Overlay::OverlayManager& overlayManager = m_Session->getOverlayManager();
    if (text.isEmpty()) {
        overlayManager.setOverlayState(Overlay::OverlayQuickMenu, false);
    }
    else {
        overlayManager.setOverlayState(Overlay::OverlayQuickMenu, true);
        overlayManager.updateOverlayText(Overlay::OverlayQuickMenu, text.toUtf8().constData());
    }
}

void QuickMenu::moveSelection(int delta)
{
    if (m_Items.isEmpty()) {
        return;
    }

    m_SelectedIndex = (m_SelectedIndex + delta + m_Items.count()) % m_Items.count();
    render();
}

void QuickMenu::activateSelection()
{
    if (m_SelectedIndex >= 0 && m_SelectedIndex < m_Items.count()) {
        // Copy the item since performAction() may rebuild the list
        Item item = m_Items[m_SelectedIndex];
        performAction(item);
    }
}

void QuickMenu::goBack()
{
    if (m_Page != Page::Main) {
        m_Page = Page::Main;
        m_SelectedIndex = 0;
        rebuildItems();
        render();
    }
    else {
        close();
    }
}

void QuickMenu::performAction(const Item& item)
{
    switch (item.action) {
    case Action::Resume:
        close();
        break;

    case Action::ToggleStats:
    {
        Overlay::OverlayManager& overlayManager = m_Session->getOverlayManager();
        overlayManager.setOverlayState(Overlay::OverlayDebug, !overlayManager.isOverlayEnabled(Overlay::OverlayDebug));
        close();
        break;
    }

    case Action::ToggleFullscreen:
        close();
        m_Session->toggleFullscreen();
        break;

    case Action::ToggleMouseCapture:
        close();
        if (m_Session->m_InputHandler != nullptr) {
            m_Session->m_InputHandler->setCaptureActive(!m_Session->m_InputHandler->isCaptureActive());
        }
        break;

    case Action::SendClipboard:
        close();
        m_Session->syncClipboard(Session::ClipboardDirection::ToHost, true);
        break;

    case Action::FetchClipboard:
        close();
        m_Session->syncClipboard(Session::ClipboardDirection::FromHost, true);
        break;

    case Action::OpenServerCommands:
        m_Page = Page::ServerCommands;
        m_SelectedIndex = 0;
        rebuildItems();
        render();
        break;

    case Action::ExecuteServerCommand:
        close();
        if (LiSendExecServerCmd((uint8_t)item.argument)) {
            showToast(QCoreApplication::translate("QuickMenu", "Running \"%1\" on host").arg(item.label));
        }
        else {
            showToast(QCoreApplication::translate("QuickMenu", "Couldn't send server command"));
        }
        break;

    case Action::Back:
        goBack();
        break;

    case Action::Disconnect:
        close();
        m_Session->requestDisconnect(false);
        break;

    case Action::QuitAppAndDisconnect:
        close();
        m_Session->requestDisconnect(true);
        break;
    }
}

bool QuickMenu::handleKeyEvent(SDL_KeyboardEvent* event)
{
    if (!m_Open) {
        return false;
    }

    // Let the special key combos (including the one that toggles this menu)
    // through to the input handler so they keep working while the menu is open.
    if ((event->keysym.mod & KMOD_CTRL) &&
            (event->keysym.mod & KMOD_ALT) &&
            (event->keysym.mod & KMOD_SHIFT)) {
        return false;
    }

    if (event->state != SDL_PRESSED) {
        // Consume key releases
        return true;
    }

    switch (event->keysym.sym) {
    case SDLK_UP:
    case SDLK_w:
        moveSelection(-1);
        break;
    case SDLK_DOWN:
    case SDLK_s:
        moveSelection(1);
        break;
    case SDLK_RETURN:
    case SDLK_RETURN2:
    case SDLK_KP_ENTER:
    case SDLK_SPACE:
        if (!event->repeat) {
            activateSelection();
        }
        break;
    case SDLK_ESCAPE:
    case SDLK_BACKSPACE:
        if (!event->repeat) {
            goBack();
        }
        break;
    default:
        break;
    }

    return true;
}

bool QuickMenu::handleControllerButtonEvent(SDL_ControllerButtonEvent* event)
{
    if (!m_Open) {
        return false;
    }

    if (event->state != SDL_PRESSED) {
        return true;
    }

    switch (event->button) {
    case SDL_CONTROLLER_BUTTON_DPAD_UP:
        moveSelection(-1);
        break;
    case SDL_CONTROLLER_BUTTON_DPAD_DOWN:
        moveSelection(1);
        break;
    case SDL_CONTROLLER_BUTTON_A:
        activateSelection();
        break;
    case SDL_CONTROLLER_BUTTON_B:
        goBack();
        break;
    default:
        break;
    }

    return true;
}

bool QuickMenu::handleMouseButtonEvent(SDL_MouseButtonEvent*)
{
    // Don't let clicks reach the host while the menu is open
    return m_Open;
}
