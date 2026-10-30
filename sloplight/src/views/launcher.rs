//! App launcher (`Cmd+O`): search what the host can launch, like the Start
//! menu's search: its apps (with their icons), the programs on the host's
//! PATH, or the command line as typed. The window it opens is streamed.

use gpui::{AnyElement, Context, IntoElement, KeyDownEvent, ParentElement, Styled, Window, div, prelude::*, px};

use crate::app::{Root, ViewMode, fuzzy_score};
use crate::assets::icon;
use crate::launcher::LaunchTarget;
use crate::theme::{Theme, type_scale};
use crate::ui::*;

pub struct LauncherState {
    pub host_uuid: String,
    pub input: gpui::Entity<TextInput>,
    pub query: String,
    pub selected: usize,
    /// Rows that fit in the window at the last render
    pub visible: usize,
}

#[derive(Clone)]
enum Kind {
    App { id: String, icon: String },
    Program { path: String },
    Run,
}

#[derive(Clone)]
struct Item {
    label: String,
    detail: String,
    kind: Kind,
}

const SHOWN: usize = 12;

/// Opens the launcher for a host (the selected one when `uuid` is None)
pub fn open(root: &mut Root, uuid: Option<String>, window: &mut Window, cx: &mut Context<Root>) {
    if root.view == ViewMode::Settings || root.dialog.is_some() {
        return;
    }
    let Some(host) = uuid.and_then(|u| root.host_by_uuid(&u).cloned()).or_else(|| root.selected().cloned()) else { return };
    if !(host.is_online() && host.is_paired() && Root::host_supports_windows(&host)) {
        root.toast(ToastKind::Warning, format!("{} can't launch apps for window streaming", host.name), cx);
        return;
    }
    let input = cx.new(|cx| {
        let mut t = TextInput::new(cx, "Search apps or type a command", InputKind::Text);
        t.height = 32.;
        t
    });
    cx.subscribe(&input, |this: &mut Root, input, event: &TextInputEvent, cx| match event {
        TextInputEvent::Changed => {
            if let Some(l) = &mut this.launcher {
                l.query = input.read(cx).text().to_string();
                l.selected = 0;
            }
            cx.notify();
        }
        TextInputEvent::Submit => launch_selected(this, cx),
        TextInputEvent::Escape => close(this, cx),
        TextInputEvent::Arrow(d) => {
            let n = items(this).len().min(this.launcher.as_ref().map(|l| l.visible).unwrap_or(SHOWN));
            if let Some(l) = &mut this.launcher
                && n > 0 {
                    l.selected = ((l.selected as isize + *d as isize).rem_euclid(n as isize)) as usize;
                }
            cx.notify();
        }
        _ => {}
    })
    .detach();
    let focus = input.read(cx).focus_handle.clone();
    window.focus(&focus, cx);
    // The list as the host had it a minute ago is good enough to start with
    let stale = root.launchables.host_uuid != host.uuid || root.launchables.fetched_at.map(|t| t.elapsed().as_secs() > 60).unwrap_or(true);
    if stale {
        root.refresh_launchables(&host.uuid, false, cx);
    }
    root.launcher = Some(LauncherState { host_uuid: host.uuid.clone(), input, query: String::new(), selected: 0, visible: SHOWN });
    root.palette = None;
    root.popup = None;
    cx.notify();
}

fn close(root: &mut Root, cx: &mut Context<Root>) {
    root.launcher = None;
    root.focus_main(cx);
}

pub fn handle_key(root: &mut Root, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Root>) {
    if event.keystroke.key == "escape" {
        close(root, cx);
    }
}

fn launch_selected(root: &mut Root, cx: &mut Context<Root>) {
    let list = items(root);
    let Some(l) = &root.launcher else { return };
    let Some(item) = list.into_iter().nth(l.selected) else { return };
    let uuid = l.host_uuid.clone();
    let query = l.query.trim().to_string();
    close(root, cx);
    let target = match item.kind {
        Kind::App { id, .. } => {
            root.settings.launcher_recent.retain(|r| r != &id);
            root.settings.launcher_recent.insert(0, id.clone());
            root.settings.launcher_recent.truncate(8);
            root.save_settings();
            LaunchTarget::App { id }
        }
        // A program by its full path (a name could resolve to another one)
        Kind::Program { path } => LaunchTarget::Command { line: format!("\"{path}\"") },
        Kind::Run => LaunchTarget::Command { line: query },
    };
    root.launch_on_host(&uuid, target, item.label, cx);
    cx.notify();
}

/// The matches, best first: apps, then PATH programs, then the command as
/// typed; with no query, the apps by name
fn items(root: &Root) -> Vec<Item> {
    let Some(l) = &root.launcher else { return Vec::new() };
    if root.launchables.host_uuid != l.host_uuid {
        return Vec::new();
    }
    let q = l.query.trim();
    let list = &root.launchables.list;
    if q.is_empty() {
        // Launched lately first, then the rest by name
        let recent = &root.settings.launcher_recent;
        let rank = |id: &str| recent.iter().position(|r| r == id).unwrap_or(usize::MAX);
        let mut apps: Vec<(usize, Item)> = list
            .apps
            .iter()
            .map(|a| (rank(&a.id), Item { label: a.name.clone(), detail: if rank(&a.id) == usize::MAX { String::new() } else { "recent".into() }, kind: Kind::App { id: a.id.clone(), icon: a.icon.clone() } }))
            .collect();
        apps.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.label.to_lowercase().cmp(&b.1.label.to_lowercase())));
        return apps.into_iter().map(|(_, i)| i).take(SHOWN).collect();
    }
    // The program part of a command line is what matches PATH programs
    let program = q.split_whitespace().next().unwrap_or(q);
    let mut scored: Vec<(i32, Item)> = Vec::new();
    for a in &list.apps {
        if let Some(s) = fuzzy_score(&a.name, q) {
            scored.push((s + 20, Item { label: a.name.clone(), detail: String::new(), kind: Kind::App { id: a.id.clone(), icon: a.icon.clone() } }));
        }
    }
    for c in &list.commands {
        if let Some(s) = fuzzy_score(&c.name, program) {
            // An exact name ranks with the apps; partial ones after them
            let exact = c.name.eq_ignore_ascii_case(program);
            scored.push((if exact { s + 40 } else { s - 10 }, Item { label: c.name.clone(), detail: c.path.clone(), kind: Kind::Program { path: c.path.clone() } }));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.len().cmp(&b.1.label.len())));
    let mut out: Vec<Item> = scored.into_iter().map(|(_, i)| i).take(SHOWN - 1).collect();
    // Arguments make it a command line: running it as typed comes first
    let run = Item { label: format!("Run “{q}”"), detail: "command line".into(), kind: Kind::Run };
    if q.contains(char::is_whitespace) || out.is_empty() {
        out.insert(0, run);
    } else {
        out.push(run);
    }
    out
}

pub fn render(root: &mut Root, theme: &Theme, window: &mut Window, cx: &mut Context<Root>) -> Option<AnyElement> {
    // As many rows as fit under the search field in this window
    let height = f32::from(window.bounds().size.height);
    let top = (height * 0.12).min(110.);
    let fit = ((height - top - 44. - 28. - 8. - 24.) / 36.).floor().max(3.) as usize;
    let mut list = items(root);
    list.truncate(fit);
    if let Some(l) = &mut root.launcher {
        l.visible = fit;
    }
    let l = root.launcher.as_ref()?;
    let host_name = root.host_by_uuid(&l.host_uuid).map(|h| h.name.clone()).unwrap_or_default();
    let selected = l.selected.min(list.len().saturating_sub(1));
    let input = l.input.clone();
    let host_uuid = l.host_uuid.clone();
    let state = &root.launchables;
    let mut rows = div().flex().flex_col().p(px(4.));
    if state.host_uuid == l.host_uuid && state.loading && state.list.apps.is_empty() {
        rows = rows.child(
            div()
                .h(px(36.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(icon_el(icon::LOADER, 14., theme.text_muted))
                .child(text_sm(theme, format!("Asking {host_name} what it can launch…"))),
        );
    } else if let Some(err) = state.error.as_ref().filter(|_| state.list.apps.is_empty()) {
        rows = rows.child(div().h(px(36.)).px(px(8.)).flex().items_center().text_size(type_scale::SM).text_color(theme.warning).child(format!("Couldn't list apps: {err}")));
    } else if list.is_empty() {
        rows = rows.child(div().h(px(36.)).px(px(8.)).flex().items_center().child(text_sm(theme, "No apps")));
    }
    for (i, item) in list.into_iter().enumerate() {
        let is_sel = i == selected;
        let hover_bg = theme.element_hover;
        let glyph: AnyElement = match &item.kind {
            Kind::App { icon, .. } => match state.icons.get(icon) {
                Some(path) => gpui::img(path.clone()).size(px(22.)).flex_none().into_any_element(),
                None => div().size(px(22.)).flex().items_center().justify_center().flex_none().child(icon_el(icon::APP_WINDOW, 16., theme.text_muted)).into_any_element(),
            },
            Kind::Program { .. } | Kind::Run => div().size(px(22.)).flex().items_center().justify_center().flex_none().child(icon_el(icon::TERMINAL, 16., theme.text_muted)).into_any_element(),
        };
        rows = rows.child(
            div()
                .id(gpui::ElementId::NamedInteger("launcher-row".into(), i as u64))
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(36.))
                .px(px(8.))
                .rounded(px(5.))
                .when(is_sel, |d| d.bg(theme.selection))
                .when(!is_sel, |d| d.hover(move |s| s.bg(hover_bg)))
                .cursor_pointer()
                .child(glyph)
                .child(div().flex_1().min_w(px(0.)).whitespace_nowrap().overflow_hidden().text_ellipsis().child(item.label.clone()))
                .when(!item.detail.is_empty(), |d| {
                    d.child(div().max_w(px(260.)).whitespace_nowrap().overflow_hidden().text_ellipsis().text_size(type_scale::SM).text_color(theme.text_muted).child(item.detail.clone()))
                })
                .when(is_sel, |d| d.child(kbd(theme, "⏎")))
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    if let Some(l) = &mut this.launcher {
                        l.selected = i;
                    }
                    launch_selected(this, cx);
                })),
        );
    }
    let refreshing = state.loading;
    let header = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(8.))
        .h(px(44.))
        .border_b_1()
        .border_color(theme.border)
        .child(icon_el(icon::SEARCH, 14., theme.text_muted))
        .child(div().flex_1().child(input))
        .child(div().text_size(type_scale::SM).text_color(theme.text_muted).whitespace_nowrap().child(format!("on {host_name}")))
        .child(
            icon_button("launcher-refresh", if refreshing { icon::LOADER } else { icon::ROTATE_CCW })
                .size(22., 12.)
                .tooltip("Look for newly installed apps", None)
                .on_click(cx.listener(move |this, _, _, cx| this.refresh_launchables(&host_uuid, true, cx))),
        );
    let footer = div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(10.))
        .h(px(28.))
        .border_t_1()
        .border_color(theme.border)
        .text_size(type_scale::XS)
        .text_color(theme.text_muted)
        .child(div().flex().items_center().gap(px(4.)).child(kbd(theme, "⏎")).child("launch and stream"))
        .child(div().flex().items_center().gap(px(4.)).child(kbd(theme, "↑↓")).child("choose"))
        .child(div().flex().items_center().gap(px(4.)).child(kbd(theme, "esc")).child("close"));
    Some(
        div()
            .id("launcher-scrim")
            .absolute()
            .inset_0()
            .bg(theme.scrim)
            .occlude()
            .flex()
            .justify_center()
            .items_start()
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| close(this, cx)))
            .child(
                div()
                    .id("launcher")
                    .mt(px(top))
                    .w(px(600.))
                    .rounded(px(10.))
                    .bg(theme.elevated)
                    .border_1()
                    .border_color(theme.border)
                    .shadow(vec![gpui::BoxShadow { color: theme.shadow, offset: gpui::point(px(0.), px(6.)), blur_radius: px(24.), spread_radius: px(0.), inset: false }])
                    .overflow_hidden()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(header)
                    .child(rows)
                    .child(footer),
            )
            .into_any_element(),
    )
}
