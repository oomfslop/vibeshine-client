//! Popup menus, toasts and the connecting card.

use gpui::{AnyElement, ClickEvent, Context, IntoElement, ParentElement, Styled, Window, anchored, deferred, div, prelude::*, px};

use crate::app::{Popup, PopupKind, Root};
use crate::assets::icon;
use crate::theme::Theme;
use crate::ui::*;

impl Root {
    pub fn render_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.global::<Theme>().clone();
        let mut out: Vec<AnyElement> = Vec::new();

        if let Some(dialog) = crate::views::dialogs::render(self, &theme, cx) {
            out.push(dialog);
        }
        if let Some(palette) = crate::views::palette::render(self, &theme, window, cx) {
            out.push(palette);
        }
        if let Some(launcher) = crate::views::launcher::render(self, &theme, window, cx) {
            out.push(launcher);
        }
        if let Some(popup) = self.render_popup(&theme, window, cx) {
            out.push(popup);
        }
        out
    }

    /// Toasts: bottom-right of the content area, small, errors stay until dismissed.
    pub fn render_toasts(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.toasts.is_empty() && self.launch_pending.is_none() {
            return None;
        }
        let mut stack = div().absolute().right(px(12.)).bottom(px(12.)).flex().flex_col().items_end().gap(px(8.));
        if let Some(p) = &self.launch_pending {
            // What a launch from the launcher is waiting for
            let host = self.host_by_uuid(&p.host_uuid).map(|h| h.name.clone()).unwrap_or_default();
            let text = match &p.hwnd {
                None => format!("Opening {} on {host}…", p.label),
                Some(_) if self.desk_opening.is_some() || self.desk_stream.as_ref().is_some_and(|d| !d.connected) => "Starting the stream…".to_string(),
                Some(_) => format!("Bringing {} over…", p.label),
            };
            stack = stack.child(
                div()
                    .id("launch-pending")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(32.))
                    .pl(px(10.))
                    .pr(px(4.))
                    .rounded(px(6.))
                    .bg(theme.elevated)
                    .border_1()
                    .border_color(theme.border)
                    .shadow(vec![gpui::BoxShadow { color: theme.shadow, offset: gpui::point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.), inset: false }])
                    .child(spinner(12., theme.text_muted))
                    .child(div().text_size(crate::theme::type_scale::SM).text_color(theme.text).whitespace_nowrap().child(text))
                    .child(icon_button("launch-pending-x", icon::X).size(20., 10.).tooltip("Stop waiting (the app stays open on the host)", None).on_click(cx.listener(|this, _, _, cx| {
                        this.launch_pending = None;
                        cx.notify();
                    }))),
            );
        }
        for toast in &self.toasts {
            let id = toast.id;
            let is_error = matches!(toast.kind, ToastKind::Error | ToastKind::Warning);
            let color = if toast.kind == ToastKind::Error { theme.error } else { theme.warning };
            stack = stack.child(
                div()
                    .id(gpui::ElementId::NamedInteger("toast".into(), id))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(28.))
                    .px(px(10.))
                    .rounded(px(6.))
                    .bg(theme.elevated)
                    .border_1()
                    .border_color(theme.border)
                    .shadow(vec![gpui::BoxShadow { color: theme.shadow, offset: gpui::point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.), inset: false }])
                    .when(is_error, |d| d.child(icon_el(icon::CIRCLE_ALERT, 12., color)))
                    .child(div().text_size(crate::theme::type_scale::SM).text_color(theme.text).whitespace_nowrap().child(toast.text.clone()))
                    .when(is_error, |d| {
                        d.child(icon_button(gpui::ElementId::NamedInteger("toast-x".into(), id), icon::X).size(18., 10.).on_click(cx.listener(move |this, _, _, cx| {
                            this.toasts.retain(|t| t.id != id);
                            cx.notify();
                        })))
                    }),
            );
        }
        Some(stack.into_any_element())
    }

    pub fn popup_entries(&mut self, cx: &mut Context<Self>) -> Vec<MenuEntry> {
        let Some(popup) = &self.popup else { return Vec::new() };
        match popup.kind.clone() {
            PopupKind::HostMenu(uuid) => self.host_menu_entries(&uuid, cx),
            PopupKind::HostInfo(_) => Vec::new(),
            PopupKind::AppMenu(uuid, app_id) => self.app_menu_entries(&uuid, app_id, cx),
            PopupKind::Dropdown(kind) => crate::views::settings::dropdown_entries(self, kind, cx),
        }
    }

    pub fn popup_entry_count(&mut self, cx: &mut Context<Self>) -> usize {
        self.popup_entries(cx).len()
    }

    pub fn activate_popup_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(idx) = self.popup.as_ref().and_then(|p| p.highlighted) else { return };
        let entries = self.popup_entries(cx);
        if let Some(entry) = entries.into_iter().nth(idx)
            && let Some(f) = entry.on_select
                && !entry.disabled {
                    f(&ClickEvent::default(), window, cx);
                }
    }

    fn render_popup(&mut self, theme: &Theme, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Popup { position, highlighted, kind } = self.popup.as_ref()?;
        let position = *position;
        let highlighted = *highlighted;
        let kind = kind.clone();
        if let PopupKind::Dropdown(_) = &kind {
            // The menu itself is drawn by the dropdown control; only the scrim lives here.
            return Some(
                div()
                    .absolute()
                    .inset_0()
                    .id("popup-layer")
                    .occlude()
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.popup = None;
                        cx.notify();
                    }))
                    .on_mouse_down(gpui::MouseButton::Right, cx.listener(|this, _, _, cx| {
                        this.popup = None;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let body: AnyElement = if let PopupKind::HostInfo(uuid) = &kind {
            let host = self.host_by_uuid(uuid).cloned()?;
            let max_h = f32::from(window.viewport_size().height) - crate::theme::HEADER_H - 16.;
            crate::views::host_info::render_popover(self, theme, &host, max_h, cx)
        } else {
            let entries = self.popup_entries(cx);
            if entries.is_empty() {
                return None;
            }
            let min_w = match kind {
                PopupKind::Dropdown(_) => 160.,
                _ => 200.,
            };
            menu(theme, "popup-menu", entries, highlighted, min_w).into_any_element()
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .id("popup-layer")
                .occlude()
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.popup = None;
                    cx.notify();
                }))
                .on_mouse_down(gpui::MouseButton::Right, cx.listener(|this, _, _, cx| {
                    this.popup = None;
                    cx.notify();
                }))
                .child(
                    deferred(
                        anchored()
                            .position(position)
                            .snap_to_window_with_margin(px(8.))
                            .child(div().occlude().on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(body)),
                    )
                    .with_priority(2),
                )
                .into_any_element(),
        )
    }

    fn host_menu_entries(&mut self, uuid: &str, cx: &mut Context<Self>) -> Vec<MenuEntry> {
        let Some(host) = self.host_by_uuid(uuid).cloned() else { return Vec::new() };
        let mut entries = Vec::new();
        let u = uuid.to_string();
        entries.push(
            MenuEntry::new("Get Info", cx.listener(|this, _, _, cx| {
                close_popup(this, cx);
                this.open_host_info(cx);
            }))
            .shortcut("⌘I"),
        );
        entries.push(
            MenuEntry::new("Refresh", cx.listener(|this, _, w, cx| {
                close_popup(this, cx);
                this.action_refresh(&crate::app::Refresh, w, cx);
            }))
            .shortcut("⌘R"),
        );
        // State-dependent action
        let pairing = self.pairing_for(uuid).is_some();
        if pairing {
            let u = u.clone();
            entries.push(
                MenuEntry::new("Cancel pairing", cx.listener(move |this, _, _, cx| {
                    close_popup(this, cx);
                    this.cancel_pairing(&u, cx);
                }))
                .separator(),
            );
        } else if host.is_online() && !host.is_paired() {
            let u2 = u.clone();
            entries.push(
                MenuEntry::new("Pair…", cx.listener(move |this, _, _, cx| {
                    close_popup(this, cx);
                    this.start_pin_pairing(&u2, cx);
                }))
                .separator(),
            );
            if host.has_apollo_extensions {
                let u2 = u.clone();
                entries.push(MenuEntry::new("Use one-time PIN…", cx.listener(move |this, _, w, cx| {
                    close_popup(this, cx);
                    this.open_otp_pairing(&u2, w, cx);
                })));
            }
        } else if !host.is_online() {
            let u2 = u.clone();
            let waking = self.waking.contains_key(uuid);
            entries.push(
                MenuEntry::new("Wake", cx.listener(move |this, _, _, cx| {
                    close_popup(this, cx);
                    this.wake(&u2, cx);
                }))
                .separator()
                .disabled(host.mac.is_empty() || waking),
            );
        }
        let mut first_in_group = true;
        if host.is_online() && host.is_paired() {
            let u2 = u.clone();
            let show = host.show_hidden_apps;
            entries.push(
                MenuEntry::new(if show { "Hide hidden apps" } else { "Show hidden apps" }, cx.listener(move |this, _, w, cx| {
                    close_popup(this, cx);
                    let _ = &u2;
                    this.action_toggle_hidden(&crate::app::ToggleHiddenApps, w, cx);
                }))
                .shortcut("⇧⌘H")
                .disabled(!host.apps.iter().any(|a| a.hidden))
                .separator(),
            );
            first_in_group = false;
        }
        {
            let addr = host.active_address.clone().or_else(|| host.unique_addresses().into_iter().next()).map(|a| a.host).unwrap_or_default();
            let has = !addr.is_empty();
            let mut e = MenuEntry::new("Copy address", cx.listener(move |this, _, _, cx| {
                close_popup(this, cx);
                this.copy_to_clipboard("address", &addr, cx);
            }))
            .shortcut("⇧⌘C")
            .disabled(!has);
            if first_in_group {
                e = e.separator();
            }
            entries.push(e);
        }
        {
            let u2 = u.clone();
            entries.push(
                MenuEntry::new("Rename…", cx.listener(move |this, _, w, cx| {
                    close_popup(this, cx);
                    this.start_rename(&u2, w, cx);
                }))
                .shortcut("F2"),
            );
        }
        {
            let u2 = u.clone();
            entries.push(
                MenuEntry::new("Remove…", cx.listener(move |this, _, _, cx| {
                    close_popup(this, cx);
                    this.confirm_remove_host(&u2, cx);
                }))
                .shortcut("⌘⌫")
                .destructive()
                .separator(),
            );
        }
        entries
    }

    fn app_menu_entries(&mut self, uuid: &str, app_id: i32, cx: &mut Context<Self>) -> Vec<MenuEntry> {
        let Some(host) = self.host_by_uuid(uuid).cloned() else { return Vec::new() };
        let Some(app) = host.apps.iter().find(|a| a.id == app_id).cloned() else { return Vec::new() };
        let running = host.current_game_id == app.id;
        let mut entries = Vec::new();
        {
            let u = uuid.to_string();
            entries.push(
                MenuEntry::new(if running { "Resume" } else { "Start" }, cx.listener(move |this, _, _, cx| {
                    close_popup(this, cx);
                    this.launch(&u, app_id, cx);
                }))
                .shortcut("⏎"),
            );
        }
        if running {
            let u = uuid.to_string();
            entries.push(
                MenuEntry::new("Quit", cx.listener(move |this, _, _, cx| {
                    close_popup(this, cx);
                    this.confirm_quit_app(&u, cx);
                }))
                .shortcut("⌘⌫")
                .destructive(),
            );
        }
        {
            let u = uuid.to_string();
            let direct = app.direct_launch;
            entries.push(
                MenuEntry::new("Direct launch", cx.listener(move |this, _, _, cx| {
                    close_popup(this, cx);
                    this.manager.set_app_direct_launch(&u, app_id, !direct);
                }))
                .checked(direct)
                .disabled(app.hidden)
                .separator(),
            );
        }
        {
            let u = uuid.to_string();
            let hidden = app.hidden;
            entries.push(
                MenuEntry::new(if hidden { "Unhide" } else { "Hide" }, cx.listener(move |this, _, _, cx| {
                    close_popup(this, cx);
                    this.set_app_hidden(&u, app_id, !hidden);
                }))
                .disabled(!hidden && (running || app.direct_launch)),
            );
        }
        {
            let name = app.name.clone();
            entries.push(
                MenuEntry::new("Copy name", cx.listener(move |this, _, _, cx| {
                    close_popup(this, cx);
                    this.copy_to_clipboard("name", &name, cx);
                }))
                .separator(),
            );
        }
        entries
    }
}

fn close_popup(this: &mut Root, cx: &mut Context<Root>) {
    this.popup = None;
    cx.notify();
}
