//! The Info popover (⌘I): a dense, read-only sheet of host facts anchored
//! under the host title. Rows copy their value on click.

use gpui::{AnyElement, ClickEvent, Context, FontWeight, IntoElement, ParentElement, Styled, Window, div, prelude::*, px};

use crate::app::Root;
use crate::assets::icon;
use crate::backend::host::{Software, scm};
use crate::backend::{Host, apollo};
use crate::theme::{Theme, font, type_scale};
use crate::ui::*;
use crate::views::main_view::host_state_sentence;

pub const POPOVER_W: f32 = 360.;
const LABEL_W: f32 = 96.;
const ROW_H: f32 = 22.;

type CopyFn = Box<dyn Fn(&ClickEvent, &mut Window, &mut gpui::App)>;

fn fact(theme: &Theme, id: &'static str, label: &'static str, value: impl Into<gpui::SharedString>, mono: bool, copy: Option<CopyFn>) -> AnyElement {
    let value: gpui::SharedString = value.into();
    let copyable = copy.is_some();
    let hover_bg = theme.element_hover;
    let value_w = POPOVER_W - 24. - LABEL_W - 8. - 20.;
    div()
        .id(id)
        .group("fact")
        .flex()
        .items_start()
        .min_h(px(ROW_H))
        .py(px(2.))
        .px(px(6.))
        .mx(px(-6.))
        .rounded(px(4.))
        .gap(px(8.))
        .when(copyable, |d| d.cursor_pointer().hover(move |s| s.bg(hover_bg)))
        .child(div().w(px(LABEL_W)).flex_none().text_size(type_scale::SM).line_height(type_scale::LH_SM).text_color(theme.text_muted).child(label))
        .child(
            div()
                .w(px(value_w))
                .flex_none()
                .text_size(type_scale::SM)
                .line_height(type_scale::LH_SM)
                .text_color(theme.text)
                .when(mono, |d| d.font(font(&theme.mono_font, FontWeight::NORMAL)).whitespace_nowrap().overflow_hidden().text_ellipsis())
                .child(value),
        )
        .when(copyable, |d| d.child(div().flex_none().invisible().group_hover("fact", |s| s.visible()).child(icon_el(icon::COPY, 12., theme.text_muted))))
        .when_some(copy, |d, f| d.on_click(move |ev, w, cx| f(ev, w, cx)))
        .into_any_element()
}

pub fn render_popover(root: &mut Root, theme: &Theme, host: &Host, max_h: f32, cx: &mut Context<Root>) -> AnyElement {
    let streaming = root.stream.as_ref().map(|s| s.host_uuid == host.uuid).unwrap_or(false);
    let waking = root.waking.contains_key(&host.uuid);
    let (color, ring) = crate::views::main_view::host_state_color(theme, host, streaming, waking);
    let software = if host.app_version.is_empty() { host.software().label().to_string() } else { format!("{} {}", host.software().label(), host.app_version) };
    let headline = format!("{} · {software}", host_state_sentence(host, streaming, waking));

    let copy = |label: &'static str, value: String| -> Option<CopyFn> {
        let f = cx.listener(move |this: &mut Root, _: &ClickEvent, _, cx| this.copy_to_clipboard(label, &value, cx));
        Some(Box::new(f))
    };

    let mut rows: Vec<AnyElement> = Vec::new();
    if let Some(a) = host.active_address.clone().or_else(|| host.unique_addresses().into_iter().next()) {
        rows.push(fact(theme, "i-addr", "Address", a.to_string(), true, copy("address", a.to_string())));
    }
    if host.active_https_port != 0 && host.active_https_port != 47984 {
        rows.push(fact(theme, "i-https", "HTTPS port", host.active_https_port.to_string(), true, None));
    }
    if !host.mac.is_empty() {
        rows.push(fact(theme, "i-mac", "MAC", host.mac_string(), true, copy("MAC", host.mac_string())));
    }
    if !host.gpu_model.is_empty() {
        rows.push(fact(theme, "i-gpu", "GPU", host.gpu_model.clone(), false, None));
    }
    if host.server_codec_mode_support != 0 {
        let s = host.server_codec_mode_support;
        let mut codecs = Vec::new();
        for (name, bit) in [("H.264", scm::H264), ("HEVC", scm::HEVC), ("AV1", scm::AV1_MAIN8)] {
            if s & bit != 0 {
                codecs.push(name);
            }
        }
        if s & (scm::HEVC_MAIN10 | scm::AV1_MAIN10) != 0 {
            codecs.push("HDR");
        }
        if s & (scm::H264_HIGH8_444 | scm::HEVC_REXT8_444 | scm::AV1_HIGH8_444) != 0 {
            codecs.push("4:4:4");
        }
        rows.push(fact(theme, "i-codecs", "Codecs", codecs.join(" · "), false, None));
    }
    if host.software() == Software::Apollo {
        let vd = if !host.virtual_display_capable {
            "No virtual display"
        } else if host.virtual_display_driver_ready {
            "Virtual display ready"
        } else {
            "Virtual display driver not ready"
        };
        rows.push(fact(theme, "i-vd", "Display", vd, false, None));
        if host.is_paired() && host.is_online() {
            let names: [(&str, u32); 11] = [
                ("Controller", apollo::INPUT_CONTROLLER),
                ("Touch", apollo::INPUT_TOUCH),
                ("Pen", apollo::INPUT_PEN),
                ("Mouse", apollo::INPUT_MOUSE),
                ("Keyboard", apollo::INPUT_KBD),
                ("Set clipboard", apollo::CLIPBOARD_SET),
                ("Read clipboard", apollo::CLIPBOARD_READ),
                ("Server commands", apollo::SERVER_CMD),
                ("List apps", apollo::LIST),
                ("View streams", apollo::VIEW),
                ("Launch apps", apollo::LAUNCH),
            ];
            let granted = names.iter().filter(|(_, b)| apollo::has(host.apollo_permissions, *b)).count();
            let missing: Vec<&str> = names.iter().filter(|(_, b)| !apollo::has(host.apollo_permissions, *b)).map(|(n, _)| *n).collect();
            let access = if missing.is_empty() { "All granted".to_string() } else { format!("{granted} of {} granted", names.len()) };
            rows.push(fact(theme, "i-access", "Access", access, false, None));
            if !missing.is_empty() {
                rows.push(fact(theme, "i-missing", "", format!("Missing: {}", missing.join(", ")), false, None));
            }
        }
        if !host.server_commands.is_empty() {
            rows.push(fact(theme, "i-cmds", "Commands", host.server_commands.join(", "), false, None));
        }
    }
    let more_open = root.details_more;
    if more_open {
        rows.push(fact(theme, "i-uuid", "UUID", host.uuid.clone(), true, copy("UUID", host.uuid.clone())));
        if !host.gfe_version.is_empty() {
            rows.push(fact(theme, "i-gfe", "GFE version", host.gfe_version.clone(), true, None));
        }
        for (id, label, a) in [("i-local", "Local", &host.local_address), ("i-remote", "Remote", &host.remote_address), ("i-v6", "IPv6", &host.ipv6_address), ("i-manual", "Manual", &host.manual_address)] {
            if let Some(a) = a {
                rows.push(fact(theme, id, label, a.to_string(), true, copy(label, a.to_string())));
            }
        }
    }

    div()
        .id("host-info")
        .w(px(POPOVER_W))
        .max_h(px(max_h))
        .overflow_y_scroll()
        .p(px(12.))
        .rounded(px(6.))
        .bg(theme.elevated)
        .border_1()
        .border_color(theme.border)
        .shadow(vec![gpui::BoxShadow { color: theme.shadow, offset: gpui::point(px(0.), px(4.)), blur_radius: px(16.), spread_radius: px(0.) }])
        .font(font(&theme.ui_font, FontWeight::NORMAL))
        .text_color(theme.text)
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(div().flex().items_center().gap(px(8.)).child(status_dot(color, ring)).child(div().font_weight(FontWeight::MEDIUM).text_size(type_scale::BASE).line_height(type_scale::LH_BASE).child(headline)))
        .child(div().flex().flex_col().children(rows))
        .child(
            button("info-more", if more_open { "Less" } else { "More" })
                .link()
                .icon(if more_open { icon::CHEVRON_DOWN } else { icon::CHEVRON_RIGHT })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.details_more = !this.details_more;
                    cx.notify();
                })),
        )
        .into_any_element()
}
