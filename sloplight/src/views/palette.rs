//! Quick launch palette (`Cmd+K`): apps across hosts, host actions, settings.

use gpui::{AnyElement, Context, IntoElement, KeyDownEvent, ParentElement, Styled, Window, div, prelude::*, px};

use crate::app::{Root, ViewMode};
use crate::app::fuzzy_score;
use crate::assets::icon;
use crate::theme::Theme;
use crate::ui::*;

#[derive(Clone)]
pub enum PaletteAction {
    LaunchApp { uuid: String, app_id: i32 },
    /// The app launcher of a host (window streaming)
    OpenLauncher { uuid: String },
    HostPrimary { uuid: String },
    HostDetails { uuid: String },
    AddHost,
    Refresh,
    Settings { section: Option<usize> },
    ToggleHidden,
    OpenUrl(&'static str),
}

pub struct PaletteItem {
    pub label: String,
    pub detail: String,
    pub icon: &'static str,
    pub running: bool,
    pub action: PaletteAction,
}

pub struct PaletteState {
    pub input: gpui::Entity<TextInput>,
    pub query: String,
    pub selected: usize,
}

pub fn open(root: &mut Root, window: &mut Window, cx: &mut Context<Root>) {
    if root.view == ViewMode::Settings || root.dialog.is_some() {
        return;
    }
    let input = cx.new(|cx| {
        let mut t = TextInput::new(cx, "Launch an app or run a command", InputKind::Text);
        t.height = 32.;
        t
    });
    cx.subscribe(&input, |this: &mut Root, input, event: &TextInputEvent, cx| match event {
        TextInputEvent::Changed => {
            if let Some(p) = &mut this.palette {
                p.query = input.read(cx).text().to_string();
                p.selected = 0;
            }
            cx.notify();
        }
        TextInputEvent::Submit => run_selected(this, cx),
        TextInputEvent::Escape => {
            this.palette = None;
            this.focus_main(cx);
        }
        TextInputEvent::Arrow(d) => {
            let n = items(this).len();
            if let Some(p) = &mut this.palette
                && n > 0 {
                    p.selected = ((p.selected as isize + *d as isize).rem_euclid(n as isize)) as usize;
                }
            cx.notify();
        }
        _ => {}
    })
    .detach();
    let input_focus = input.read(cx).focus_handle.clone();
    window.focus(&input_focus, cx);
    root.palette = Some(PaletteState { input, query: String::new(), selected: 0 });
    root.popup = None;
    cx.notify();
}

pub fn handle_key(root: &mut Root, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Root>) {
    if event.keystroke.key == "escape" {
        root.palette = None;
        root.focus_main(cx);
    }
}

fn run_selected(root: &mut Root, cx: &mut Context<Root>) {
    let list = items(root);
    let Some(p) = &root.palette else { return };
    let Some(item) = list.into_iter().nth(p.selected) else { return };
    root.palette = None;
    root.focus_main(cx);
    match item.action {
        PaletteAction::LaunchApp { uuid, app_id } => {
            root.select_host(Some(uuid.clone()), cx);
            root.launch(&uuid, app_id, cx);
        }
        PaletteAction::OpenLauncher { uuid } => root.pending_launcher = Some(uuid),
        PaletteAction::HostPrimary { uuid } => root.activate_host(&uuid, cx),
        PaletteAction::HostDetails { uuid } => {
            root.select_host(Some(uuid), cx);
            root.open_host_info(cx);
        }
        PaletteAction::AddHost => root.pending_add_host = true,
        PaletteAction::Refresh => root.manager.refresh(),
        PaletteAction::Settings { section } => {
            root.view = ViewMode::Settings;
            root.settings_view.sync_from(&root.settings, cx);
            if let Some(s) = section {
                root.settings_view.section = s;
            }
        }
        PaletteAction::ToggleHidden => {
            if let Some(h) = root.selected().cloned() {
                root.manager.set_show_hidden_apps(&h.uuid, !h.show_hidden_apps);
            }
        }
        PaletteAction::OpenUrl(u) => cx.open_url(u),
    }
    cx.notify();
}

pub fn items(root: &Root) -> Vec<PaletteItem> {
    let Some(p) = &root.palette else { return Vec::new() };
    let q = p.query.trim();
    let mut scored: Vec<(i32, usize, PaletteItem)> = Vec::new();
    let mut push = |rank: usize, item: PaletteItem| {
        let hay = format!("{} {}", item.label, item.detail);
        let score = if q.is_empty() { Some(0) } else { fuzzy_score(&hay, q) };
        if let Some(s) = score {
            scored.push((s, rank, item));
        }
    };
    for host in &root.hosts {
        if host.is_online() && host.is_paired() {
            for app in &host.apps {
                if app.hidden && !host.show_hidden_apps {
                    continue;
                }
                push(0, PaletteItem { label: app.name.clone(), detail: host.name.clone(), icon: icon::PLAY, running: host.current_game_id == app.id, action: PaletteAction::LaunchApp { uuid: host.uuid.clone(), app_id: app.id } });
            }
        }
    }
    for host in &root.hosts {
        let verb = if host.is_online() && host.is_paired() {
            "Open"
        } else if host.is_online() {
            "Pair"
        } else {
            "Wake"
        };
        push(1, PaletteItem { label: format!("{} {}", if verb == "Open" { "Go to" } else { verb }, host.name), detail: String::new(), icon: if verb == "Wake" { icon::POWER } else { icon::MONITOR }, running: false, action: PaletteAction::HostPrimary { uuid: host.uuid.clone() } });
        if host.is_online() && host.is_paired() && Root::host_supports_windows(host) {
            push(1, PaletteItem { label: format!("Launch an app on {}", host.name), detail: "⌘O".into(), icon: icon::APP_WINDOW, running: false, action: PaletteAction::OpenLauncher { uuid: host.uuid.clone() } });
        }
        push(1, PaletteItem { label: format!("{} info", host.name), detail: String::new(), icon: icon::INFO, running: false, action: PaletteAction::HostDetails { uuid: host.uuid.clone() } });
    }
    push(2, PaletteItem { label: "Add host".into(), detail: "⌘N".into(), icon: icon::PLUS, running: false, action: PaletteAction::AddHost });
    push(2, PaletteItem { label: "Refresh".into(), detail: "⌘R".into(), icon: icon::REFRESH, running: false, action: PaletteAction::Refresh });
    push(2, PaletteItem { label: "Settings".into(), detail: "⌘,".into(), icon: icon::SETTINGS, running: false, action: PaletteAction::Settings { section: None } });
    push(2, PaletteItem { label: "Show hidden apps".into(), detail: "⇧⌘H".into(), icon: icon::EYE, running: false, action: PaletteAction::ToggleHidden });
    push(2, PaletteItem { label: "Setup guide".into(), detail: String::new(), icon: icon::EXTERNAL_LINK, running: false, action: PaletteAction::OpenUrl("https://github.com/moonlight-stream/moonlight-docs/wiki/Setup-Guide") });
    push(2, PaletteItem { label: "Troubleshooting".into(), detail: String::new(), icon: icon::EXTERNAL_LINK, running: false, action: PaletteAction::OpenUrl("https://github.com/moonlight-stream/moonlight-docs/wiki/Troubleshooting") });
    push(2, PaletteItem { label: "Moonlight Discord".into(), detail: String::new(), icon: icon::EXTERNAL_LINK, running: false, action: PaletteAction::OpenUrl("https://moonlight-stream.org/discord") });
    for (i, section) in crate::views::settings::SECTIONS.iter().enumerate() {
        for row in crate::views::settings::section_rows(section.id) {
            push(3, PaletteItem { label: format!("Settings: {}", row), detail: section.title.to_string(), icon: icon::SETTINGS, running: false, action: PaletteAction::Settings { section: Some(i) } });
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, item)| item).take(10).collect()
}

pub fn render(root: &mut Root, theme: &Theme, _window: &mut Window, cx: &mut Context<Root>) -> Option<AnyElement> {
    let list = items(root);
    let p = root.palette.as_ref()?;
    let selected = p.selected.min(list.len().saturating_sub(1));
    let input = p.input.clone();
    let mut rows = div().flex().flex_col().p(px(4.));
    if list.is_empty() {
        rows = rows.child(div().h(px(28.)).px(px(8.)).flex().items_center().child(text_sm(theme, "No matches")));
    }
    for (i, item) in list.into_iter().enumerate() {
        let is_sel = i == selected;
        let hover_bg = theme.element_hover;
        rows = rows.child(
            div()
                .id(gpui::ElementId::NamedInteger("palette-row".into(), i as u64))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(28.))
                .px(px(8.))
                .rounded(px(4.))
                .when(is_sel, |d| d.bg(theme.selection))
                .when(!is_sel, |d| d.hover(move |s| s.bg(hover_bg)))
                .cursor_pointer()
                .child(icon_el(item.icon, 14., if is_sel { theme.text } else { theme.text_muted }))
                .child(div().flex_1().whitespace_nowrap().overflow_hidden().text_ellipsis().child(item.label.clone()))
                .when(!item.detail.is_empty(), |d| d.child(text_sm(theme, item.detail.clone())))
                .when(item.running, |d| d.child(badge(theme, "Running", theme.badge_running, theme.badge_running.alpha(0.12))))
                .when(is_sel, |d| d.child(kbd(theme, "⏎")))
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    if let Some(p) = &mut this.palette {
                        p.selected = i;
                    }
                    run_selected(this, cx);
                })),
        );
    }
    Some(
        div()
            .id("palette-scrim")
            .absolute()
            .inset_0()
            .bg(theme.scrim)
            .occlude()
            .flex()
            .justify_center()
            .items_start()
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.palette = None;
                this.focus_main(cx);
            }))
            .child(
                div()
                    .id("palette")
                    .mt(gpui::relative(0.2))
                    .w(px(560.))
                    .rounded(px(8.))
                    .bg(theme.elevated)
                    .border_1()
                    .border_color(theme.border)
                    .shadow(vec![gpui::BoxShadow { color: theme.shadow, offset: gpui::point(px(0.), px(4.)), blur_radius: px(16.), spread_radius: px(0.), inset: false }])
                    .overflow_hidden()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().flex().items_center().gap(px(8.)).px(px(8.)).h(px(40.)).border_b_1().border_color(theme.border).child(icon_el(icon::SEARCH, 14., theme.text_muted)).child(div().flex_1().child(input)))
                    .child(rows),
            )
            .into_any_element(),
    )
}
