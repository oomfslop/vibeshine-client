//! Stateless widgets from the design spec. Each is a small builder that
//! renders once (`RenderOnce`), styled from the global `Theme`.
#![allow(dead_code)]

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, AnyView, App, BoxShadow, ClickEvent, Div, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, SharedString, Stateful, StatefulInteractiveElement, Styled, Transformation, Window, div, percentage, point, px,
    svg, prelude::*,
};

use crate::assets::icon;
use crate::theme::{CONTROL_H, Theme, font, type_scale};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

// ------------------------------------------------------------------ icons

pub fn icon_el(path: &'static str, size: f32, color: Hsla) -> gpui::Svg {
    svg().path(path).size(px(size)).flex_none().text_color(color)
}

pub fn spinner(size: f32, color: Hsla) -> impl IntoElement {
    svg()
        .path(icon::LOADER)
        .size(px(size))
        .flex_none()
        .text_color(color)
        .with_animation("spinner", Animation::new(Duration::from_millis(800)).repeat(), |svg, delta| {
            svg.with_transformation(Transformation::rotate(percentage(delta)))
        })
}

// ------------------------------------------------------------------ text helpers

pub fn text_ui(theme: &Theme, s: impl Into<SharedString>) -> Div {
    div().font(font(&theme.ui_font, FontWeight::NORMAL)).text_size(type_scale::BASE).line_height(type_scale::LH_BASE).text_color(theme.text).child(s.into())
}

pub fn text_sm(theme: &Theme, s: impl Into<SharedString>) -> Div {
    div().text_size(type_scale::SM).line_height(type_scale::LH_SM).text_color(theme.text_muted).child(s.into())
}

pub fn text_strong(theme: &Theme, s: impl Into<SharedString>) -> Div {
    div().font_weight(FontWeight::MEDIUM).text_size(type_scale::BASE).line_height(type_scale::LH_BASE).text_color(theme.text).child(s.into())
}

pub fn text_lg(theme: &Theme, s: impl Into<SharedString>) -> Div {
    div().font_weight(FontWeight::SEMIBOLD).text_size(type_scale::LG).line_height(type_scale::LH_LG).text_color(theme.text).child(s.into())
}

pub fn mono_sm(theme: &Theme, s: impl Into<SharedString>) -> Div {
    div().font(font(&theme.mono_font, FontWeight::NORMAL)).text_size(type_scale::SM).line_height(type_scale::LH_SM).text_color(theme.text).child(s.into())
}

// ------------------------------------------------------------------ kbd chip

pub fn kbd(theme: &Theme, keys: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .h(px(16.))
        .px(px(4.))
        .rounded(px(3.))
        .bg(theme.element)
        .text_size(type_scale::XS)
        .line_height(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_muted)
        .child(keys.into())
}

// ------------------------------------------------------------------ button

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Ghost,
    Danger,
    Link,
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    variant: ButtonVariant,
    icon: Option<&'static str>,
    kbd: Option<SharedString>,
    disabled: bool,
    compact: bool,
    loading: bool,
    on_click: Option<ClickHandler>,
}

pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    Button { id: id.into(), label: label.into(), variant: ButtonVariant::Secondary, icon: None, kbd: None, disabled: false, compact: false, loading: false, on_click: None }
}

impl Button {
    pub fn variant(mut self, v: ButtonVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn primary(self) -> Self {
        self.variant(ButtonVariant::Primary)
    }
    pub fn ghost(self) -> Self {
        self.variant(ButtonVariant::Ghost)
    }
    pub fn danger(self) -> Self {
        self.variant(ButtonVariant::Danger)
    }
    pub fn link(self) -> Self {
        self.variant(ButtonVariant::Link)
    }
    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.kbd = Some(keys.into());
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let disabled = self.disabled || self.loading;
        let (bg, fg, border): (Hsla, Hsla, Option<Hsla>) = match (self.variant, disabled) {
            (ButtonVariant::Primary, false) => (theme.accent, theme.accent_fg, None),
            (ButtonVariant::Primary, true) => (theme.element, theme.text_disabled, None),
            (ButtonVariant::Secondary, false) => (theme.element, theme.text, Some(theme.border)),
            (ButtonVariant::Secondary, true) => (theme.element, theme.text_disabled, Some(theme.border)),
            (ButtonVariant::Ghost, false) => (gpui::transparent_black(), theme.text, None),
            (ButtonVariant::Ghost, true) => (gpui::transparent_black(), theme.text_disabled, None),
            (ButtonVariant::Danger, false) => (theme.error_subtle, theme.error, None),
            (ButtonVariant::Danger, true) => (theme.element, theme.text_disabled, Some(theme.border)),
            (ButtonVariant::Link, false) => (gpui::transparent_black(), theme.text_accent, None),
            (ButtonVariant::Link, true) => (gpui::transparent_black(), theme.text_disabled, None),
        };
        let hover_bg = match self.variant {
            ButtonVariant::Primary => theme.accent_hover,
            ButtonVariant::Secondary | ButtonVariant::Ghost => theme.element_hover,
            ButtonVariant::Danger => theme.error_subtle_hover,
            ButtonVariant::Link => gpui::transparent_black(),
        };
        let active_bg = match self.variant {
            ButtonVariant::Primary => theme.accent_pressed,
            ButtonVariant::Secondary | ButtonVariant::Ghost => theme.element_pressed,
            ButtonVariant::Danger => theme.error_subtle_hover,
            ButtonVariant::Link => gpui::transparent_black(),
        };
        let is_link = self.variant == ButtonVariant::Link;
        let h = if self.compact { 20. } else { CONTROL_H };
        let text_size = if self.compact { type_scale::SM } else { type_scale::BASE };
        let mut el = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .h(px(h))
            .rounded(px(4.))
            .text_size(text_size)
            .line_height(px(h))
            .text_color(fg)
            .bg(bg)
            .when(!is_link, |d| d.px(px(if self.compact { 8. } else { 10. })))
            .when_some(border, |d, b| d.border_1().border_color(b))
            .when(!disabled, |d| {
                d.cursor_pointer().hover(move |s| if is_link { s.underline() } else { s.bg(hover_bg) }).active(move |s| if is_link { s.underline() } else { s.bg(active_bg) })
            });
        if self.loading {
            el = el.child(spinner(14., fg));
        } else if let Some(icon) = self.icon {
            el = el.child(icon_el(icon, 14., fg));
        }
        el = el.child(div().whitespace_nowrap().child(self.label));
        if let Some(keys) = self.kbd {
            let chip_bg = if self.variant == ButtonVariant::Primary { theme.accent_fg.alpha(0.12) } else { theme.element };
            el = el.child(kbd(&theme, keys).bg(chip_bg).text_color(if self.variant == ButtonVariant::Primary { theme.accent_fg } else { theme.text_muted }));
        }
        if let Some(on_click) = self.on_click
            && !disabled {
                el = el.on_click(move |ev, window, cx| on_click(ev, window, cx));
            }
        el
    }
}

// ------------------------------------------------------------------ icon button

#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: &'static str,
    toggled: bool,
    disabled: bool,
    size: f32,
    icon_size: f32,
    color: Option<Hsla>,
    tooltip: Option<(SharedString, Option<SharedString>)>,
    on_click: Option<ClickHandler>,
}

pub fn icon_button(id: impl Into<ElementId>, icon: &'static str) -> IconButton {
    IconButton { id: id.into(), icon, toggled: false, disabled: false, size: 24., icon_size: 14., color: None, tooltip: None, on_click: None }
}

impl IconButton {
    pub fn toggled(mut self, t: bool) -> Self {
        self.toggled = t;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    pub fn size(mut self, size: f32, icon_size: f32) -> Self {
        self.size = size;
        self.icon_size = icon_size;
        self
    }
    pub fn color(mut self, c: Hsla) -> Self {
        self.color = Some(c);
        self
    }
    pub fn tooltip(mut self, text: impl Into<SharedString>, keys: Option<&str>) -> Self {
        self.tooltip = Some((text.into(), keys.map(|k| k.to_string().into())));
        self
    }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let icon_color = if self.disabled {
            theme.text_disabled
        } else if self.toggled {
            theme.accent
        } else {
            self.color.unwrap_or(theme.text_muted)
        };
        let hover_fg = theme.text;
        let mut el = div()
            .id(self.id)
            .group("iconbtn")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(self.size))
            .rounded(px(4.))
            .when(self.toggled, |d| d.bg(theme.accent_subtle).border_1().border_color(theme.border_selected))
            .when(!self.disabled, |d| d.cursor_pointer().hover(move |s| s.bg(theme.element_hover)).active(move |s| s.bg(theme.element_pressed)))
            .child(icon_el(self.icon, self.icon_size, icon_color).when(!self.disabled && !self.toggled, |s| s.group_hover("iconbtn", move |s| s.text_color(hover_fg))));
        if let Some((text, keys)) = self.tooltip {
            el = el.tooltip(move |_, cx| tooltip_view(cx, text.clone(), keys.clone()));
        }
        if let Some(on_click) = self.on_click
            && !self.disabled {
                el = el.on_click(move |ev, window, cx| on_click(ev, window, cx));
            }
        el
    }
}

// ------------------------------------------------------------------ tooltip

pub struct TooltipView {
    text: SharedString,
    keys: Option<SharedString>,
}

impl gpui::Render for TooltipView {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(4.))
            .bg(theme.elevated)
            .border_1()
            .border_color(theme.border)
            .shadow(vec![BoxShadow { color: theme.shadow, offset: point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.) }])
            .max_w(px(280.))
            .font(font(&theme.ui_font, FontWeight::NORMAL))
            .text_size(type_scale::SM)
            .line_height(type_scale::LH_SM)
            .text_color(theme.text)
            .child(self.text.clone())
            .when_some(self.keys.clone(), |d, k| d.child(kbd(&theme, k)))
    }
}

pub fn tooltip_view(cx: &mut App, text: SharedString, keys: Option<SharedString>) -> AnyView {
    cx.new(|_| TooltipView { text, keys }).into()
}

/// Attach a tooltip to any stateful element.
pub fn with_tooltip<E: StatefulInteractiveElement>(el: E, text: impl Into<SharedString>, keys: Option<&str>) -> E {
    let text: SharedString = text.into();
    let keys: Option<SharedString> = keys.map(|k| k.to_string().into());
    el.tooltip(move |_, cx| tooltip_view(cx, text.clone(), keys.clone()))
}

// ------------------------------------------------------------------ toggle

pub fn toggle(theme: &Theme, id: impl Into<ElementId>, on: bool, disabled: bool, on_change: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    let (track, knob) = match (on, disabled) {
        (true, false) => (theme.accent, theme.accent_fg),
        (false, false) => (theme.element_pressed, theme.text_muted),
        (true, true) => (theme.element, theme.text_disabled),
        (false, true) => (theme.element, theme.text_disabled),
    };
    let hover_track = if on { theme.accent_hover } else { theme.text.alpha(0.18) };
    div()
        .id(id)
        .flex_none()
        .w(px(28.))
        .h(px(16.))
        .rounded(px(8.))
        .bg(track)
        .when(!on && !disabled, |d| d.border_1().border_color(theme.border))
        .relative()
        .when(!disabled, |d| d.cursor_pointer().hover(move |s| s.bg(hover_track)))
        .child(
            div()
                .absolute()
                .top(px(if on || disabled { 2. } else { 1. }))
                .left(px(if on { 14. } else if disabled { 2. } else { 1. }))
                .size(px(12.))
                .rounded(px(6.))
                .bg(knob),
        )
        .when(!disabled, |d| {
            d.on_click(move |_, window, cx| {
                on_change(&!on, window, cx);
            })
        })
}

// ------------------------------------------------------------------ segmented control

pub fn segmented(theme: &Theme, id: impl Into<SharedString>, options: &[&str], selected: usize, disabled: bool, on_select: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Div {
    let id: SharedString = id.into();
    let on_select = std::rc::Rc::new(on_select);
    let mut el = div().flex().flex_none().h(px(CONTROL_H)).rounded(px(4.)).border_1().border_color(theme.border).bg(theme.bg).overflow_hidden();
    for (i, label) in options.iter().enumerate() {
        let is_selected = i == selected;
        let on_select = on_select.clone();
        let fg = if disabled { theme.text_disabled } else if is_selected { theme.accent } else { theme.text_muted };
        let hover_fg = theme.text;
        let hover_bg = theme.element_hover;
        el = el.child(
            div()
                .id(ElementId::NamedInteger(id.clone(), i as u64))
                .flex()
                .items_center()
                .h_full()
                .px(px(10.))
                .text_size(type_scale::BASE)
                .line_height(px(CONTROL_H - 2.))
                .text_color(fg)
                .when(i > 0, |d| d.border_l_1().border_color(theme.border))
                .when(is_selected, |d| d.bg(theme.accent_subtle))
                .when(!disabled && !is_selected, |d| d.cursor_pointer().hover(move |s| s.bg(hover_bg).text_color(hover_fg)))
                .child(div().whitespace_nowrap().child(label.to_string()))
                .when(!disabled, |d| {
                    d.on_click(move |_, window, cx| {
                        on_select(&i, window, cx);
                    })
                }),
        );
    }
    el
}

// ------------------------------------------------------------------ badge

pub fn badge(theme: &Theme, text: impl Into<SharedString>, fg: Hsla, bg: Hsla) -> Div {
    let _ = theme;
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(2.))
        .h(px(16.))
        .px(px(5.))
        .rounded(px(3.))
        .bg(bg)
        .text_size(type_scale::XS)
        .line_height(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .whitespace_nowrap()
        .child(text.into())
}

pub fn badge_icon(theme: &Theme, icon: &'static str, text: impl Into<SharedString>, fg: Hsla, bg: Hsla) -> Div {
    badge(theme, text, fg, bg).child(icon_el(icon, 10., fg))
}

// ------------------------------------------------------------------ status dot

pub fn status_dot(color: Hsla, ring: bool) -> Div {
    let d = div().flex_none().size(px(8.)).rounded(px(4.));
    if ring { d.border_1().border_color(color) } else { d.bg(color) }
}

// ------------------------------------------------------------------ section header

pub fn section_header(theme: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .h(px(28.))
        .px(px(8.))
        .font_weight(FontWeight::MEDIUM)
        .text_size(type_scale::BASE)
        .line_height(type_scale::LH_BASE)
        .text_color(theme.text_muted)
        .child(text.into())
}

// ------------------------------------------------------------------ key/value row

pub fn kv_row(theme: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>, value: impl Into<SharedString>, on_copy: Option<ClickHandler>) -> Stateful<Div> {
    let value: SharedString = value.into();
    let copyable = on_copy.is_some() && value != "—";
    let hover_bg = theme.element_hover;
    div()
        .id(id)
        .group("kv")
        .flex()
        .items_center()
        .h(px(22.))
        .px(px(8.))
        .rounded(px(3.))
        .gap(px(4.))
        .when(copyable, |d| d.cursor_pointer().hover(move |s| s.bg(hover_bg)))
        .child(div().w(px(100.)).flex_none().text_size(type_scale::SM).line_height(type_scale::LH_SM).text_color(theme.text_muted).child(label.into()))
        .child(mono_sm(theme, value).flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis())
        .when(copyable, |d| d.child(div().flex_none().invisible().group_hover("kv", |s| s.visible()).child(icon_el(icon::COPY, 12., theme.text_muted))))
        .when_some(on_copy.filter(|_| copyable), |d, f| d.on_click(move |ev, window, cx| f(ev, window, cx)))
}

// ------------------------------------------------------------------ callout

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CalloutKind {
    Info,
    Warning,
    Error,
}

pub fn callout(theme: &Theme, kind: CalloutKind, text: impl Into<SharedString>) -> Div {
    let (icon, color, bg, border) = match kind {
        CalloutKind::Info => (icon::INFO, theme.accent, theme.accent_subtle, theme.border_selected),
        CalloutKind::Warning => (icon::TRIANGLE_ALERT, theme.warning, theme.warning_subtle, theme.warning.alpha(0.24)),
        CalloutKind::Error => (icon::CIRCLE_ALERT, theme.error, theme.error_subtle, theme.error.alpha(0.24)),
    };
    div()
        .flex()
        .items_start()
        .gap(px(8.))
        .px(px(12.))
        .py(px(8.))
        .rounded(px(6.))
        .bg(bg)
        .border_1()
        .border_color(border)
        .child(div().flex_none().pt(px(1.)).child(icon_el(icon, 14., color)))
        .child(div().flex_1().text_size(type_scale::SM).line_height(type_scale::LH_SM).text_color(theme.text).child(text.into()))
}

// ------------------------------------------------------------------ menu

pub struct MenuEntry {
    pub label: SharedString,
    pub shortcut: Option<SharedString>,
    pub checked: Option<bool>,
    pub icon: Option<&'static str>,
    pub disabled: bool,
    pub destructive: bool,
    pub separator_before: bool,
    pub on_select: Option<ClickHandler>,
}

impl MenuEntry {
    pub fn new(label: impl Into<SharedString>, on_select: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        MenuEntry { label: label.into(), shortcut: None, checked: None, icon: None, disabled: false, destructive: false, separator_before: false, on_select: Some(Box::new(on_select)) }
    }
    pub fn shortcut(mut self, s: impl Into<SharedString>) -> Self {
        self.shortcut = Some(s.into());
        self
    }
    pub fn checked(mut self, c: bool) -> Self {
        self.checked = Some(c);
        self
    }
    pub fn icon(mut self, i: &'static str) -> Self {
        self.icon = Some(i);
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }
    pub fn separator(mut self) -> Self {
        self.separator_before = true;
        self
    }
}

/// Renders a menu container. `highlighted` is the keyboard-selected index.
pub fn menu(theme: &Theme, id: impl Into<SharedString>, entries: Vec<MenuEntry>, highlighted: Option<usize>, min_width: f32) -> Div {
    let id: SharedString = id.into();
    let has_slot = entries.iter().any(|e| e.checked.is_some() || e.icon.is_some());
    let mut container = div()
        .flex()
        .flex_col()
        .min_w(px(min_width))
        .max_h(px(360.))
        .p(px(4.))
        .rounded(px(6.))
        .bg(theme.elevated)
        .border_1()
        .border_color(theme.border)
        .shadow(vec![BoxShadow { color: theme.shadow, offset: point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.) }])
        .font(font(&theme.ui_font, FontWeight::NORMAL))
        .text_size(type_scale::BASE)
        .line_height(type_scale::LH_BASE);
    for (i, entry) in entries.into_iter().enumerate() {
        if entry.separator_before {
            container = container.child(div().h(px(1.)).my(px(4.)).bg(theme.border));
        }
        let fg = if entry.disabled {
            theme.text_disabled
        } else if entry.destructive {
            theme.error
        } else {
            theme.text
        };
        let hover_bg = theme.element_hover;
        let is_hl = highlighted == Some(i);
        let mut row = div()
            .id(ElementId::NamedInteger(id.clone(), i as u64))
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(24.))
            .px(px(8.))
            .rounded(px(4.))
            .text_color(fg)
            .when(is_hl, |d| d.bg(theme.selection))
            .when(!entry.disabled, |d| d.cursor_pointer().hover(move |s| s.bg(hover_bg)));
        if has_slot {
            let slot = div().flex_none().size(px(14.)).flex().items_center().justify_center();
            row = row.child(match (entry.checked, entry.icon) {
                (Some(true), _) => slot.child(icon_el(icon::CHECK, 14., theme.accent)),
                (_, Some(ic)) => slot.child(icon_el(ic, 14., fg)),
                _ => slot,
            });
        }
        row = row.child(div().flex_1().whitespace_nowrap().child(entry.label));
        if let Some(sc) = entry.shortcut {
            row = row.child(div().flex_none().text_size(type_scale::SM).text_color(theme.text_muted).child(sc));
        }
        if let Some(on_select) = entry.on_select
            && !entry.disabled {
                row = row.on_click(move |ev, window, cx| on_select(ev, window, cx));
            }
        container = container.child(row);
    }
    container
}

// ------------------------------------------------------------------ dialog shell

pub fn dialog_shell(theme: &Theme, width: f32, title: impl Into<SharedString>, title_icon: Option<(&'static str, Hsla)>, body: Vec<AnyElement>, footer: Vec<AnyElement>) -> Div {
    div()
        .flex()
        .flex_col()
        .w(px(width))
        .max_w_full()
        .p(px(16.))
        .rounded(px(8.))
        .bg(theme.elevated)
        .border_1()
        .border_color(theme.border)
        .shadow(vec![BoxShadow { color: theme.shadow, offset: point(px(0.), px(4.)), blur_radius: px(16.), spread_radius: px(0.) }])
        .font(font(&theme.ui_font, FontWeight::NORMAL))
        .text_size(type_scale::BASE)
        .line_height(type_scale::LH_BASE)
        .text_color(theme.text)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .when_some(title_icon, |d, (ic, color)| d.child(icon_el(ic, 16., color)))
                .child(text_lg(theme, title)),
        )
        .child(div().flex().flex_col().gap(px(12.)).mt(px(12.)).children(body))
        .when(!footer.is_empty(), |d| d.child(div().flex().items_center().justify_end().gap(px(8.)).mt(px(16.)).children(footer)))
}

// ------------------------------------------------------------------ toast

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToastKind {
    Success,
    Info,
    Warning,
    Error,
}

pub fn toast_view(theme: &Theme, id: impl Into<ElementId>, kind: ToastKind, text: impl Into<SharedString>, on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Div {
    let (ic, color) = match kind {
        ToastKind::Success => (icon::CIRCLE_CHECK, theme.success),
        ToastKind::Info => (icon::INFO, theme.accent),
        ToastKind::Warning => (icon::TRIANGLE_ALERT, theme.warning),
        ToastKind::Error => (icon::CIRCLE_ALERT, theme.error),
    };
    div()
        .flex()
        .items_start()
        .gap(px(8.))
        .w(px(320.))
        .px(px(12.))
        .py(px(8.))
        .rounded(px(6.))
        .bg(theme.elevated)
        .border_1()
        .border_color(theme.border)
        .shadow(vec![BoxShadow { color: theme.shadow, offset: point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.) }])
        .child(div().flex_none().pt(px(2.)).child(icon_el(ic, 14., color)))
        .child(div().flex_1().text_size(type_scale::BASE).line_height(type_scale::LH_BASE).text_color(theme.text).child(text.into()))
        .child(icon_button(id, icon::X).size(20., 12.).on_click(on_close))
}

// ------------------------------------------------------------------ misc

pub fn hairline_h(theme: &Theme) -> Div {
    div().h(px(1.)).w_full().flex_none().bg(theme.border)
}

pub fn hairline_v(theme: &Theme) -> Div {
    div().w(px(1.)).h_full().flex_none().bg(theme.border)
}

