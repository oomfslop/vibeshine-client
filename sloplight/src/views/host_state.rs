//! The centered state panel shown instead of the app grid when a host has
//! nothing to browse: not paired, pairing, offline, blocked, unsupported, or
//! its app list failed. One title, one explanation, one primary action.

use gpui::{AnyElement, Context, FontWeight, IntoElement, ParentElement, Styled, Window, div, prelude::*, px};

use crate::app::Root;
use crate::assets::icon;
use crate::backend::host::Software;
use crate::backend::{Host, HostState, PairState};
use crate::theme::{Theme, font, type_scale};
use crate::ui::*;

const PANEL_W: f32 = 420.;

/// The one action the state panel offers; ⏎ runs it and the renderer draws it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrimaryAction {
    Pair,
    RetryPair,
    Wake,
    OpenLanSettings,
    Refresh,
}

impl PrimaryAction {
    pub fn label(self) -> &'static str {
        match self {
            PrimaryAction::Pair => "Pair",
            PrimaryAction::RetryPair => "Try again",
            PrimaryAction::Wake => "Wake",
            PrimaryAction::OpenLanSettings => "Open Local Network Settings",
            PrimaryAction::Refresh => "Refresh",
        }
    }

    pub fn kbd(self) -> &'static str {
        match self {
            PrimaryAction::Refresh => "⌘R",
            _ => "⏎",
        }
    }
}

/// Which action the panel for `host` offers, if any (None while waiting).
pub fn primary_action(root: &Root, host: &Host) -> Option<PrimaryAction> {
    let waking = root.waking.contains_key(&host.uuid);
    if let Some(p) = root.pairing_for(&host.uuid) {
        return if !p.otp && !p.in_progress && p.error.is_some() { Some(PrimaryAction::RetryPair) } else { None };
    }
    match host.state {
        HostState::Unknown => None,
        HostState::Offline if host.unreachable && crate::backend::platform::resolution_for(&crate::backend::http::HttpError::HostUnreachable).is_some() => Some(PrimaryAction::OpenLanSettings),
        HostState::Offline if waking => None,
        HostState::Offline if !host.mac.is_empty() => Some(PrimaryAction::Wake),
        HostState::Offline => Some(PrimaryAction::Refresh),
        HostState::Online if !host.is_supported_server_version => Some(PrimaryAction::Refresh),
        HostState::Online if host.pair_state == PairState::NotPaired => Some(PrimaryAction::Pair),
        HostState::Online => Some(PrimaryAction::Refresh),
    }
}

impl Root {
    /// Runs the state panel's primary action for the selected host (⏎).
    pub fn run_primary_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.selected().cloned() else { return };
        let Some(action) = primary_action(self, &host) else { return };
        match action {
            PrimaryAction::Pair | PrimaryAction::RetryPair => self.start_pin_pairing(&host.uuid, cx),
            PrimaryAction::Wake => self.wake(&host.uuid, cx),
            PrimaryAction::OpenLanSettings => {
                #[cfg(target_os = "macos")]
                cx.open_url(crate::backend::platform::LOCAL_NETWORK_SETTINGS_URL);
            }
            PrimaryAction::Refresh => self.action_refresh(&crate::app::Refresh, window, cx),
        }
    }
}

fn primary_button(root: &Root, host: &Host, action: PrimaryAction, cx: &mut Context<Root>) -> AnyElement {
    let _ = root;
    let u = host.uuid.clone();
    let mut b = button("state-primary", action.label()).primary().kbd(action.kbd());
    if action == PrimaryAction::OpenLanSettings {
        b = b.icon(icon::EXTERNAL_LINK);
    }
    b.on_click(cx.listener(move |this, _, w, cx| {
        this.select_host(Some(u.clone()), cx);
        this.run_primary_action(w, cx);
    }))
    .into_any_element()
}

pub fn render(root: &mut Root, theme: &Theme, host: &Host, content_w: f32, content_h: f32, _window: &mut Window, cx: &mut Context<Root>) -> AnyElement {
    let uuid = host.uuid.clone();
    let name = host.name.clone();
    let streaming = root.stream.as_ref().map(|s| s.host_uuid == host.uuid).unwrap_or(false);
    let waking = root.waking.contains_key(&host.uuid);
    let (mut color, mut ring) = crate::views::main_view::host_state_color(theme, host, streaming, waking);
    let pairing = root.pairing_for(&host.uuid);
    let action = primary_action(root, host);

    let os_blocked = host.state == HostState::Offline && host.unreachable && action == Some(PrimaryAction::OpenLanSettings);
    if os_blocked {
        color = theme.warning;
        ring = false;
    }

    // ---- title + explanation per state
    let title: String;
    let mut explanation: Vec<AnyElement> = Vec::new();
    let mut extra_actions: Vec<AnyElement> = Vec::new();
    let mut waiting: Option<String> = None;
    let muted = |theme: &Theme, s: String| text_sm(theme, s).text_size(type_scale::BASE).line_height(type_scale::LH_BASE).text_center().into_any_element();
    let mono = |theme: &Theme, s: String| mono_sm(theme, s).text_color(theme.text_muted).text_center().into_any_element();

    if let Some(p) = pairing {
        if p.otp {
            title = format!("Pair {name} with a one-time PIN");
            explanation.push(muted(theme, "Create a one-time PIN in Apollo's web UI, then enter it here.".into()));
        } else {
            title = format!("Pairing with {name}");
            explanation.push(muted(
                theme,
                if host.software() == Software::GeForceExperience { format!("Enter this PIN in the dialog on {name}'s screen.") } else { format!("Enter this PIN on the PIN page of {name}'s web UI.") },
            ));
        }
    } else {
        match host.state {
            HostState::Unknown => {
                title = format!("Connecting to {name}…");
            }
            HostState::Offline if os_blocked => {
                title = "macOS is blocking local network access".into();
                explanation.push(muted(theme, "Allow Sloplight in System Settings › Privacy & Security › Local Network, then retry.".into()));
                extra_actions.push(button("state-retry", "Retry").kbd("⌘R").on_click(cx.listener(|this, _, w, cx| this.action_refresh(&crate::app::Refresh, w, cx))).into_any_element());
            }
            HostState::Offline => {
                title = format!("{name} is offline");
                let addr = host.active_address.clone().or_else(|| host.unique_addresses().into_iter().next()).map(|a| a.to_string());
                if waking {
                    waiting = Some(format!("Waking {name}…"));
                } else if host.mac.is_empty() {
                    if let Some(a) = addr {
                        explanation.push(mono(theme, a));
                    }
                    explanation.push(muted(theme, format!("Wake isn't available until {name} has been online once.")));
                } else {
                    let line = match addr {
                        Some(a) => format!("{a}  ·  {}", host.mac_string()),
                        None => host.mac_string(),
                    };
                    explanation.push(mono(theme, line));
                }
            }
            HostState::Online if !host.is_supported_server_version => {
                title = format!("Can't stream from {name}");
                let sw = if host.app_version.is_empty() { host.software().label().to_string() } else { format!("{} {}", host.software().label(), host.app_version) };
                explanation.push(muted(theme, format!("Its {sw} isn't supported by this version of Sloplight.")));
            }
            HostState::Online if host.pair_state == PairState::NotPaired => {
                title = format!("{name} isn't paired");
                explanation.push(muted(theme, "Pair once to see and launch its apps.".into()));
                if host.has_apollo_extensions {
                    let u = uuid.clone();
                    extra_actions.push(button("state-otp", "Use one-time PIN").on_click(cx.listener(move |this, _, w, cx| this.open_otp_pairing(&u, w, cx))).into_any_element());
                }
            }
            HostState::Online => match &host.app_list_error {
                Some((403, _)) => {
                    title = format!("{name} isn't sharing its apps");
                    explanation.push(muted(theme, "Grant \"List apps\" to this client in Apollo's web UI, then refresh.".into()));
                }
                Some((code, _)) => {
                    title = format!("Couldn't load {name}'s apps");
                    explanation.push(muted(theme, format!("The host returned error {code}.")));
                }
                None => {
                    title = format!("No apps on {name}");
                }
            },
        }
    }

    // ---- action row
    let mut actions: Vec<AnyElement> = Vec::new();
    if let Some(w) = waiting {
        actions.push(div().flex().items_center().gap(px(8.)).child(spinner(12., theme.text_muted)).child(text_sm(theme, w)).into_any_element());
    } else if let Some(a) = action {
        actions.push(primary_button(root, host, a, cx));
    }
    actions.extend(extra_actions);

    let title_row = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .when(host.state == HostState::Unknown && pairing.is_none(), |d| d.child(spinner(14., theme.text_muted)))
        .when(host.state != HostState::Unknown || pairing.is_some(), |d| d.child(status_dot(color, ring)))
        .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(15.)).line_height(px(20.)).text_color(theme.text).child(title));

    let top_pad = crate::theme::panel_top_pad(content_h);
    let mut panel = div().flex().flex_col().items_center().gap(px(6.)).w(px(PANEL_W.min(content_w - 32.))).child(title_row).children(explanation);
    if !actions.is_empty() {
        panel = panel.child(div().flex().items_center().justify_center().gap(px(8.)).mt(px(10.)).children(actions));
    }

    // ---- pairing block
    if let Some(p) = pairing {
        panel = panel.child(pairing_block(theme, host, p, cx).mt(px(10.)));
    }

    div().id("host-state").flex_1().min_h(px(0.)).overflow_y_scroll().flex().flex_col().items_center().pt(px(top_pad)).pb(px(24.)).px(px(16.)).child(panel).into_any_element()
}

fn pairing_block(theme: &Theme, host: &Host, p: &crate::app::InlinePairing, cx: &mut Context<Root>) -> gpui::Div {
    let u = p.uuid.clone();
    let mut block = div().flex().flex_col().gap(px(10.)).w_full().p(px(16.)).rounded(px(6.)).bg(theme.surface).border_1().border_color(theme.border);
    if let Some(err) = &p.error {
        block = block.child(div().flex().items_start().gap(px(6.)).child(icon_el(icon::CIRCLE_ALERT, 14., theme.error)).child(div().flex_1().text_color(theme.error).child(err.clone())));
    }
    if p.otp {
        let pin_ok = {
            let t = p.otp_pin.read(cx).text().to_string();
            t.len() == 4 && t.chars().all(|c| c.is_ascii_digit())
        };
        let reveal = p.reveal;
        p.otp_pass.update(cx, |t, _| t.reveal_password = reveal);
        let (pin_in, pass_in) = (p.otp_pin.clone(), p.otp_pass.clone());
        block = block
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().w(px(72.)).flex_none().child(pin_in))
                    .child(div().flex_1().min_w(px(0.)).child(pass_in))
                    .child(icon_button("otp-reveal", if reveal { icon::EYE_OFF } else { icon::EYE }).tooltip(if reveal { "Hide passphrase" } else { "Show passphrase" }, None).on_click({
                        let u = u.clone();
                        cx.listener(move |this, _, _, cx| {
                            if let Some(p) = this.pairings.iter_mut().find(|p| p.uuid == u) {
                                p.reveal = !p.reveal;
                            }
                            cx.notify();
                        })
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .child(button("otp-cancel", "Cancel").ghost().kbd("esc").on_click({
                        let u = u.clone();
                        cx.listener(move |this, _, _, cx| this.cancel_pairing(&u, cx))
                    }))
                    .child(button("otp-pair", if p.in_progress { "Pairing…" } else { "Pair" }).primary().kbd("⏎").disabled(!pin_ok).loading(p.in_progress).on_click({
                        let u = u.clone();
                        cx.listener(move |this, _, _, cx| this.submit_otp(&u, cx))
                    })),
            );
    } else {
        let pin = p.pin.clone();
        let pin_copy = pin.clone();
        block = block
            .child(
                div()
                    .id("pin-box")
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .cursor_pointer()
                    .child(div().font(font(&theme.mono_font, FontWeight::SEMIBOLD)).text_size(px(28.)).line_height(px(36.)).text_color(theme.text_accent).child(pin.chars().map(|c| c.to_string()).collect::<Vec<_>>().join("  ")))
                    .child(icon_el(icon::COPY, 14., theme.text_muted))
                    .on_click(cx.listener(move |this, _, _, cx| this.copy_to_clipboard("PIN", &pin_copy, cx))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when(p.in_progress, |d| d.child(spinner(12., theme.text_muted)).child(text_sm(theme, format!("Waiting for {}…", host.name)).flex_1()))
                    .when(!p.in_progress, |d| d.child(div().flex_1()))
                    .when(!p.in_progress && p.error.is_some(), |d| {
                        let u = u.clone();
                        d.child(button("pair-retry", "Try again").primary().kbd("⏎").on_click(cx.listener(move |this, _, _, cx| this.start_pin_pairing(&u, cx))))
                    })
                    .child(button("pair-cancel", "Cancel").ghost().kbd("esc").on_click({
                        let u = u.clone();
                        cx.listener(move |this, _, _, cx| this.cancel_pairing(&u, cx))
                    })),
            );
    }
    block
}
