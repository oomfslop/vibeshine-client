//! The one shell: sidebar (hosts or settings sections) and content (apps,
//! host details or settings). No title bar, no status bar.

use gpui::{AnyElement, ClickEvent, Context, ElementId, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString, Styled, Window, div, img, prelude::*, px};

use crate::app::{Popup, PopupKind, Region, Root, ViewMode};
use crate::assets::icon;
use crate::backend::{App as NvApp, Host, HostState, PairState};
use crate::theme::{HEADER_H, LARGE_TILE_W, Layout, ROW_H, SIDEBAR_W, Theme, layout, panel_top_pad, type_scale};
use crate::ui::*;

/// Status dot color and whether it is drawn as a ring (unknown state).
pub fn host_state_color(theme: &Theme, host: &Host, streaming_from: bool, waking: bool) -> (gpui::Hsla, bool) {
    if streaming_from {
        return (theme.state_streaming, false);
    }
    if host.pairing_in_progress || waking {
        return (theme.state_pairing, false);
    }
    match host.state {
        HostState::Unknown => (theme.state_offline, true),
        HostState::Offline => (theme.state_offline, false),
        HostState::Online if !host.is_supported_server_version => (theme.state_error, false),
        HostState::Online if host.pair_state == PairState::NotPaired => (theme.state_offline, false),
        HostState::Online if host.current_game_id != 0 => (theme.state_busy, false),
        HostState::Online => (theme.state_online, false),
    }
}

/// Short state label for the sidebar row; `None` when the host is plainly online.
pub fn host_state_label(host: &Host, streaming_from: bool, waking: bool) -> Option<String> {
    if streaming_from {
        return Some("Streaming".into());
    }
    if host.pairing_in_progress {
        return Some("Pairing…".into());
    }
    if waking {
        return Some("Waking…".into());
    }
    match host.state {
        HostState::Unknown => Some("Checking…".into()),
        HostState::Offline if host.unreachable => Some("Blocked".into()),
        HostState::Offline => Some("Offline".into()),
        HostState::Online if !host.is_supported_server_version => Some("Unsupported".into()),
        HostState::Online if host.pair_state == PairState::NotPaired => Some("Not paired".into()),
        HostState::Online if host.is_paired() && host.apps.is_empty() && host.app_list_error.is_some() => Some("No apps".into()),
        HostState::Online if host.current_game_id != 0 => Some(host.running_app().map(|a| a.name.clone()).unwrap_or_else(|| "App running".into())),
        HostState::Online => None,
    }
}

/// Long state sentence for the Info popover headline.
pub fn host_state_sentence(host: &Host, streaming_from: bool, waking: bool) -> String {
    if streaming_from {
        return format!("Streaming {}", host.running_app().map(|a| a.name.as_str()).unwrap_or("an app"));
    }
    if host.pairing_in_progress {
        return "Pairing…".into();
    }
    if waking {
        return "Waking…".into();
    }
    match host.state {
        HostState::Unknown => format!("Contacting {}…", host.name),
        HostState::Offline => "Offline".into(),
        HostState::Online if !host.is_supported_server_version => "This GeForce Experience version isn't supported".into(),
        HostState::Online if host.pair_state == PairState::NotPaired => "Not paired".into(),
        HostState::Online if host.current_game_id != 0 => match host.running_app() {
            Some(a) => format!("Online · Running {}", a.name),
            None => "Online · App running".into(),
        },
        HostState::Online => "Online".into(),
    }
}

impl Root {
    pub fn render_shell(&mut self, width: f32, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let content_w = width - SIDEBAR_W;
        let settings = self.view == ViewMode::Settings;
        self.last_content_w = content_w;
        self.last_body_h = Self::body_height(window);

        div()
            .flex()
            .flex_1()
            .min_h(px(0.))
            .child(self.render_sidebar(&theme, settings, window, cx))
            .child(
                div()
                    .id("content")
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .bg(theme.bg)
                    .child(self.render_content_header(&theme, settings, content_w, Self::body_height(window), cx))
                    .child(if settings {
                        crate::views::settings::render_content(self, &theme, content_w, window, cx).into_any_element()
                    } else {
                        self.render_main_body(&theme, content_w, window, cx)
                    })
                    .children(self.render_toasts(&theme, cx)),
            )
    }

    fn render_sidebar(&mut self, theme: &Theme, settings: bool, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Top strip: traffic lights, then + and ⚙
        let strip = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(4.))
            .h(px(HEADER_H))
            .flex_none()
            .pl(px(78.))
            .pr(px(8.))
            .window_control_area(gpui::WindowControlArea::Drag)
            .when(!settings, |d| d.child(icon_button("add-host", icon::PLUS).tooltip("Add host", Some("⌘N")).on_click(cx.listener(|this, _, window, cx| this.open_add_host(window, cx)))))
            .child(
                icon_button("settings", icon::SETTINGS)
                    .toggled(settings)
                    .tooltip("Settings", Some("⌘,"))
                    .on_click(cx.listener(|this, _, w, cx| this.action_settings(&crate::app::OpenSettings, w, cx))),
            );

        let list: AnyElement = if settings {
            crate::views::settings::render_sections(self, theme, cx).into_any_element()
        } else {
            self.render_host_list(theme, window, cx).into_any_element()
        };

        div()
            .id("sidebar")
            .flex()
            .flex_col()
            .w(px(SIDEBAR_W))
            .flex_none()
            .h_full()
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border)
            .child(strip)
            .child(div().id("sidebar-list").flex_1().min_h(px(0.)).overflow_y_scroll().child(list))
    }

    fn render_host_list(&mut self, theme: &Theme, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let order = self.sidebar_order();
        let focused = self.region == Region::Sidebar;
        let streaming_from = self.stream.as_ref().map(|s| s.host_uuid.clone());
        let mut list = div()
            .flex()
            .flex_col()
            .px(px(6.))
            .gap(px(1.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.region != Region::Sidebar {
                        this.region = Region::Sidebar;
                        cx.notify();
                    }
                }),
            );
        for uuid in order {
            let Some(host) = self.host_by_uuid(&uuid).cloned() else { continue };
            let selected = self.selected_host.as_deref() == Some(&uuid);
            let is_streaming = streaming_from.as_deref() == Some(&uuid);
            let waking = self.waking.contains_key(&uuid);
            let (color, ring) = host_state_color(theme, &host, is_streaming, waking);
            let label = host_state_label(&host, is_streaming, waking);
            let uuid_click = uuid.clone();
            let uuid_dbl = uuid.clone();
            let uuid_menu = uuid.clone();
            let row_bg = if selected {
                Some(if focused { theme.selection } else { theme.selection_unfocused })
            } else {
                None
            };
            let hover_bg = theme.element_hover;
            list = list.child(
                div()
                    .id(ElementId::Name(format!("host-{uuid}").into()))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(ROW_H))
                    .px(px(6.))
                    .rounded(px(4.))
                    .when_some(row_bg, |d, bg| d.bg(bg))
                    .when(!selected, |d| d.hover(move |s| s.bg(hover_bg)))
                    .cursor_pointer()
                    .child(status_dot(color, ring))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(type_scale::BASE)
                            .line_height(type_scale::LH_BASE)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(host.name.clone()),
                    )
                    .when_some(label, |d, l| d.child(text_sm(theme, truncate_chars(&l, 12)).flex_none().whitespace_nowrap()))
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        this.region = Region::Sidebar;
                        this.select_host(Some(uuid_click.clone()), cx);
                        if let ClickEvent::Mouse(m) = ev
                            && m.up.click_count >= 2 {
                                this.activate_host(&uuid_dbl, cx);
                            }
                        this.focus_main(cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                            this.select_host(Some(uuid_menu.clone()), cx);
                            this.popup = Some(Popup { kind: PopupKind::HostMenu(uuid_menu.clone()), position: ev.position, highlighted: None });
                            cx.notify();
                        }),
                    ),
            );
        }
        if self.hosts.is_empty() {
            list = list.child(div().px(px(6.)).h(px(ROW_H)).flex().items_center().child(text_sm(theme, "No hosts yet")));
        }
        list
    }

    fn render_content_header(&mut self, theme: &Theme, settings: bool, content_w: f32, body_h: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let filter_w = if content_w < 576. { 140. } else { 180. };
        let l: Layout = layout(content_w, body_h);
        // The filter keeps the same right inset in both views so it doesn't jump
        // when settings open; only the "Settings" title follows the centered column.
        let left_inset = if settings { 16. + l.settings_offset } else { l.pad };
        let right_inset = l.pad;
        let (filter, filter_text) = if settings {
            (self.settings_view.filter.clone(), self.settings_view.filter_text.clone())
        } else {
            (self.filter.clone(), self.filter_text.clone())
        };
        filter.update(cx, |t, _| {
            t.quiet = true;
            t.placeholder = if settings { "Filter settings".into() } else { "Filter".into() };
        });
        let left: AnyElement = if settings {
            text_ui(theme, "Settings").font_weight(FontWeight::MEDIUM).into_any_element()
        } else if self.selected().is_none() {
            div().into_any_element()
        } else {
            let host = self.selected().cloned().unwrap();
            let open = matches!(&self.popup, Some(Popup { kind: PopupKind::HostMenu(u) | PopupKind::HostInfo(u), .. }) if *u == host.uuid);
            let hover_bg = theme.element_hover;
            let open_bg = theme.selection_unfocused;
            let uuid = host.uuid.clone();
            // App-window streaming: a "Windows" picker next to the host name
            // (opens the host popover, whose Windows section lists them).
            let windows_button = if host.is_paired() && host.is_online() && Self::host_supports_windows(&host) {
                let n = self.window_streams.iter().filter(|w| w.host_uuid == host.uuid).count();
                let label = if n > 0 { format!("Windows · {n} streaming") } else { "Windows".to_string() };
                Some(
                    button("host-windows", label)
                        .ghost()
                        .icon(icon::APP_WINDOW)
                        .on_click(cx.listener(|this, _, _, cx| this.open_host_info(cx))),
                )
            } else {
                None
            };
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div()
                .id("host-title")
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(24.))
                .px(px(6.))
                .mx(px(-6.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(open, |d| d.bg(open_bg))
                .when(!open, |d| d.hover(move |s| s.bg(hover_bg)))
                .child(div().font_weight(FontWeight::MEDIUM).text_color(theme.text).whitespace_nowrap().overflow_hidden().text_ellipsis().max_w(px(260.)).child(host.name.clone()))
                .child(icon_el(icon::CHEVRON_DOWN, 12., theme.text_muted))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.popup = Some(Popup { kind: PopupKind::HostMenu(uuid.clone()), position: this.host_title_anchor(), highlighted: None });
                    cx.notify();
                })))
                .children(windows_button)
                .into_any_element()
        };
        let show_filter = settings || self.host_browsable();
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(HEADER_H))
            .flex_none()
            .pl(px(left_inset))
            .pr(px(right_inset))
            .window_control_area(gpui::WindowControlArea::Drag)
            .child(left)
            .when(show_filter, |d| {
                d.child(
                    div()
                        .w(px(filter_w))
                        .relative()
                        .child(filter)
                        .when(filter_text.is_empty(), |d| d.child(div().absolute().right(px(6.)).top(px(4.)).child(kbd(theme, "⌘F")))),
                )
            })
    }

    fn render_main_body(&mut self, theme: &Theme, content_w: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(host) = self.selected().cloned() else {
            let body_h = Self::body_height(window);
            return self.render_empty_state(theme, body_h, cx).into_any_element();
        };
        if self.host_browsable() {
            self.render_grid(theme, &host, content_w, window, cx)
        } else {
            let content_h = Self::body_height(window);
            crate::views::host_state::render(self, theme, &host, content_w, content_h, window, cx)
        }
    }

    fn render_empty_state(&mut self, theme: &Theme, body_h: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let mdns = self.settings.enable_mdns;
        let icon_el_: AnyElement = if mdns { spinner(24., theme.text_muted).into_any_element() } else { icon_el(icon::RADAR, 24., theme.text_muted).into_any_element() };
        let mut actions: Vec<AnyElement> = vec![button("add", "Add host").primary().kbd("⌘N").on_click(cx.listener(|this, _, w, cx| this.open_add_host(w, cx))).into_any_element()];
        if !mdns {
            actions.push(
                button("mdns-on", "Turn on discovery")
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings.enable_mdns = true;
                        this.manager.set_mdns_enabled(true);
                        this.save_settings();
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        #[cfg(target_os = "macos")]
        if mdns {
            actions.push(button("lan-settings", "Local Network settings").ghost().icon(icon::EXTERNAL_LINK).on_click(|_, _, cx| cx.open_url(crate::backend::platform::LOCAL_NETWORK_SETTINGS_URL)).into_any_element());
        }
        actions.push(button("guide", "Setup guide").ghost().icon(icon::EXTERNAL_LINK).on_click(|_, _, cx| cx.open_url("https://github.com/moonlight-stream/moonlight-docs/wiki/Setup-Guide")).into_any_element());
        let mut body = String::from("Hosts running Apollo, Sunshine or GeForce Experience appear here automatically.");
        if mdns
            && let Some(hint) = crate::backend::platform::discovery_hint() {
                body.push(' ');
                body.push_str(hint);
            }
        state_view(
            theme,
            body_h,
            Some(icon_el_),
            if mdns { "Looking for hosts on your network…" } else { "Automatic discovery is off" },
            Some(body),
            actions,
        )
    }

    /// Fluid tile geometry: columns and tile width for the content width.
    pub fn grid_geometry(content_w: f32, body_h: f32) -> (usize, f32, f32) {
        let l = layout(content_w, body_h);
        let avail = (content_w - 2. * l.pad).max(l.tile_target);
        let cols = (((avail + l.gap_x) / (l.tile_target + l.gap_x)).floor() as usize).max(1);
        let tile_w = ((avail - l.gap_x * (cols as f32 - 1.)) / cols as f32).floor();
        let tile_h = (tile_w * 4. / 3.).round();
        (cols, tile_w, tile_h)
    }

    /// Height of the content pane below the header.
    pub fn body_height(window: &Window) -> f32 {
        f32::from(window.viewport_size().height) - HEADER_H
    }

    fn render_grid(&mut self, theme: &Theme, host: &Host, content_w: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let apps = self.visible_apps();
        let hidden_count = host.apps.iter().filter(|a| a.hidden).count();
        let body_h = Self::body_height(window);
        let l = layout(content_w, body_h);
        let (cols, tile_w, tile_h) = Self::grid_geometry(content_w, body_h);
        self.grid_columns = cols;

        let show_hidden_btn = |cx: &mut Context<Self>| button("show-hidden", "Show hidden apps").primary().kbd("⇧⌘H").on_click(cx.listener(|this, _, w, cx| this.action_toggle_hidden(&crate::app::ToggleHiddenApps, w, cx))).into_any_element();
        if apps.is_empty() {
            if !self.filter_text.is_empty() {
                let k = self.hidden_matching();
                let (body, actions) = if k > 0 && !host.show_hidden_apps {
                    (Some(format!("{k} hidden app{} match.", if k == 1 { "" } else { "s" })), vec![show_hidden_btn(cx)])
                } else {
                    (Some("Esc clears the filter.".into()), vec![])
                };
                return state_view(theme, body_h, None, format!("No apps match \"{}\"", self.filter_text), body, actions).into_any_element();
            }
            if hidden_count > 0 {
                let body = if hidden_count == 1 { "1 app is hidden on this host.".to_string() } else { format!("{hidden_count} apps are hidden on this host.") };
                return state_view(theme, body_h, Some(icon_el(icon::EYE_OFF, 24., theme.text_muted).into_any_element()), format!("All apps on {} are hidden", host.name), Some(body), vec![show_hidden_btn(cx)]).into_any_element();
            }
            return state_view(theme, body_h, Some(icon_el(icon::LAYOUT_GRID, 24., theme.text_muted).into_any_element()), format!("No apps on {}", host.name), None, vec![]).into_any_element();
        }

        let focused = self.region == Region::Grid;
        let launching = self.stream.as_ref().filter(|s| s.host_uuid == host.uuid).map(|s| s.app_id);
        let uuid = host.uuid.clone();
        let split = self.visible_count().min(apps.len());
        let mut tiles: Vec<AnyElement> = Vec::with_capacity(apps.len());
        for (i, app) in apps.iter().enumerate() {
            let selected = self.grid_selected == Some(i);
            tiles.push(self.render_tile(theme, &uuid, app, i, tile_w, tile_h, selected && focused, host.current_game_id == app.id, launching == Some(app.id), cx));
        }
        let hidden_tiles: Vec<AnyElement> = tiles.split_off(split);
        let wrap = move |tiles: Vec<AnyElement>| div().flex().flex_wrap().gap_x(px(l.gap_x)).gap_y(px(l.gap_y)).children(tiles);
        let mut column = div().flex().flex_col().px(px(l.pad)).pt(px(l.pad_top)).pb(px(l.pad));
        if !tiles.is_empty() {
            column = column.child(wrap(tiles));
        }
        if hidden_count > 0 {
            let show = host.show_hidden_apps;
            column = column.child(self.render_hidden_header(theme, hidden_count, show, split > 0, cx));
            if show && !hidden_tiles.is_empty() {
                column = column.child(wrap(hidden_tiles).mt(px(8.)));
            }
        }
        div()
            .id("grid-scroll")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.region != Region::Grid {
                        this.region = Region::Grid;
                        cx.notify();
                    }
                }),
            )
            .child(column)
            .into_any_element()
    }

    /// The "› Hidden  N ─────" disclosure row between the visible and hidden tiles.
    fn render_hidden_header(&mut self, theme: &Theme, count: usize, show: bool, has_visible: bool, cx: &mut Context<Self>) -> AnyElement {
        let hover_bg = theme.element_hover;
        div()
            .id("hidden-header")
            .group("hh")
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(6.))
            .mx(px(-6.))
            .when(has_visible, |d| d.mt(px(24.)))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg))
            .child(icon_el(if show { icon::CHEVRON_DOWN } else { icon::CHEVRON_RIGHT }, 12., theme.text_muted))
            .child(div().text_size(type_scale::SM).line_height(type_scale::LH_SM).font_weight(FontWeight::MEDIUM).text_color(theme.text_muted).child("Hidden"))
            .child(div().text_size(type_scale::SM).line_height(type_scale::LH_SM).text_color(theme.text_disabled).child(count.to_string()))
            .child(div().flex_1().h(px(1.)).bg(theme.border).ml(px(4.)))
            .when(show, |d| d.child(div().text_size(type_scale::SM).text_color(theme.text_muted).child("Hide")))
            .child(div().invisible().group_hover("hh", |s| s.visible()).child(kbd(theme, "⇧⌘H")))
            .on_click(cx.listener(|this, _, w, cx| this.action_toggle_hidden(&crate::app::ToggleHiddenApps, w, cx)))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_tile(&mut self, theme: &Theme, uuid: &str, app: &NvApp, index: usize, tile_w: f32, tile_h: f32, selected: bool, running: bool, launching: bool, cx: &mut Context<Self>) -> AnyElement {
        let key = (uuid.to_string(), app.id);
        if !self.box_art_requested.contains(&key) {
            self.box_art_requested.insert(key.clone());
            if self.manager.box_art_path(uuid, app.id).is_none() && !self.manager.box_art_known_missing(uuid, app.id) {
                self.manager.request_box_art(uuid, app);
            }
        }
        let art_path = self.manager.box_art_path(uuid, app.id);
        let uuid_s = uuid.to_string();
        let uuid_menu = uuid.to_string();
        let app_id = app.id;
        let name = app.name.clone();
        let hidden = app.hidden;

        let large = tile_w >= LARGE_TILE_W;
        // The ring sits 4pt outside the art (2pt gap + 2pt stroke), so its outer
        // radius is the art radius + 4 to keep the corners concentric.
        let (radius, icon_size, spinner_size, caption_gap) = if large { (8., 32., 20., 8.) } else { (6., 24., 16., 6.) };
        let ring_radius = radius + 4.;
        let mut art = div().relative().w(px(tile_w)).h(px(tile_h)).rounded(px(radius)).overflow_hidden().bg(theme.surface).border_1().border_color(theme.text.alpha(0.06)).when(hidden, |d| d.opacity(0.5));
        art = match art_path {
            // The image sits inside the card's 1pt border, so its radius is 1pt smaller.
            Some(path) => art.child(img(path).size_full().rounded(px(radius - 1.)).object_fit(gpui::ObjectFit::Cover).when(launching, |i| i.opacity(0.5))),
            None => art.child(div().size_full().flex().items_center().justify_center().child(icon_el(icon::GAMEPAD, icon_size, theme.text_muted))),
        };
        if launching {
            art = art.child(div().absolute().inset_0().flex().items_center().justify_center().child(spinner(spinner_size, theme.text)));
        }
        let ring = if running {
            Some(theme.accent)
        } else if selected {
            Some(theme.border_focused)
        } else {
            None
        };
        let hover_overlay = theme.text.alpha(0.06);
        let caption: SharedString = if launching { "Starting…".into() } else { name.clone().into() };
        div()
            .id(ElementId::NamedInteger("tile".into(), app_id as u64))
            .group("tile")
            .flex()
            .flex_col()
            .gap(px(caption_gap))
            .w(px(tile_w))
            .cursor_pointer()
            .child(
                // The art keeps its exact size and position; the selection ring is an
                // overlay drawn 4px outside it so it never changes the layout.
                div()
                    .relative()
                    .w(px(tile_w))
                    .h(px(tile_h))
                    .child(art)
                    .child(div().absolute().inset_0().rounded(px(radius)).invisible().group_hover("tile", move |s| s.visible().bg(hover_overlay)))
                    .when_some(ring, |d, c| d.child(div().absolute().top(px(-4.)).left(px(-4.)).right(px(-4.)).bottom(px(-4.)).rounded(px(ring_radius)).border_2().border_color(c))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .w_full()
                    .px(px(2.))
                    .when(running && !launching, |d| d.child(div().flex_none().size(px(6.)).rounded(px(3.)).bg(theme.state_busy)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(if large { type_scale::BASE } else { type_scale::SM })
                            .line_height(if large { type_scale::LH_BASE } else { type_scale::LH_SM })
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(if launching || hidden { theme.text_muted } else { theme.text })
                            .when(selected, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(caption),
                    ),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.region = Region::Grid;
                this.grid_selected = Some(index);
                this.focus_main(cx);
                this.launch(&uuid_s, app_id, cx);
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                    this.region = Region::Grid;
                    this.grid_selected = Some(index);
                    this.popup = Some(Popup { kind: PopupKind::AppMenu(uuid_menu.clone(), app_id), position: ev.position, highlighted: None });
                    cx.notify();
                }),
            )
            .into_any_element()
    }
}

/// Truncates to `max` characters with an ellipsis.
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Centered state view: icon, title, body, actions.
pub fn state_view(theme: &Theme, body_h: f32, icon: Option<AnyElement>, title: impl Into<SharedString>, body: Option<String>, actions: Vec<AnyElement>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .size_full()
        .pt(px(panel_top_pad(body_h)))
        .pb(px(24.))
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.))
                .max_w(px(360.))
                .text_center()
                .children(icon)
                .child(text_lg(theme, title).text_center())
                .when_some(body, |d, b| d.child(text_sm(theme, b).text_center().text_size(type_scale::BASE).line_height(type_scale::LH_BASE)))
                .when(!actions.is_empty(), |d| d.child(div().flex().flex_wrap().justify_center().gap(px(8.)).mt(px(8.)).children(actions))),
        )
}
