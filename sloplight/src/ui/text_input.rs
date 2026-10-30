//! Single-line text field entity. Adapted from gpui's `input.rs` example with
//! theming, placeholder, password masking, numeric filtering and events.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, GlobalElementId, InspectorElementId, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle,
    Window, actions, div, fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::{Theme, font, type_scale};

actions!(
    text_input,
    [Backspace, Delete, Left, Right, SelectLeft, SelectRight, SelectAll, Home, End, ShowCharacterPalette, Paste, Cut, Copy, WordLeft, WordRight, DeleteWord]
);

pub fn bind_keys(cx: &mut App) {
    use gpui::KeyBinding;
    let ctx = Some("TextInput");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("alt-left", WordLeft, ctx),
        KeyBinding::new("alt-right", WordRight, ctx),
        KeyBinding::new("alt-backspace", DeleteWord, ctx),
        KeyBinding::new("cmd-a", SelectAll, ctx),
        KeyBinding::new("cmd-v", Paste, ctx),
        KeyBinding::new("cmd-c", Copy, ctx),
        KeyBinding::new("cmd-x", Cut, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("cmd-left", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("cmd-right", End, ctx),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, ctx),
    ]);
}

#[derive(Clone, Debug)]
pub enum TextInputEvent {
    Changed,
    Submit,
    Escape,
    /// Up/Down arrows, for list navigation from a filter field (+1 = down)
    Arrow(i32),
    Blur,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Text,
    Mono,
    Password,
    /// Digits only (plus '.' when `decimal`)
    Numeric { decimal: bool, negative: bool },
}

pub struct TextInput {
    pub focus_handle: FocusHandle,
    content: SharedString,
    pub placeholder: SharedString,
    pub kind: InputKind,
    pub max_len: Option<usize>,
    pub reveal_password: bool,
    pub invalid: bool,
    pub disabled: bool,
    pub height: f32,
    pub text_size: Pixels,
    pub align_right: bool,
    /// Draw no border/background until focused or non-empty
    pub quiet: bool,
    /// Never draw a border/background: the surrounding row is the field
    pub bare: bool,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    was_focused: bool,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>, kind: InputKind) -> Self {
        TextInput {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: placeholder.into(),
            kind,
            max_len: None,
            reveal_password: false,
            invalid: false,
            disabled: false,
            height: 24.,
            text_size: type_scale::BASE,
            align_right: false,
            quiet: false,
            bare: false,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            was_focused: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let len = self.content.len();
        self.selected_range = len..len;
        self.marked_range = None;
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_text("", cx);
    }

    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    fn is_mono(&self) -> bool {
        matches!(self.kind, InputKind::Mono | InputKind::Numeric { .. })
    }

    fn accepts(&self, text: &str) -> bool {
        match self.kind {
            InputKind::Numeric { decimal, negative } => text.chars().all(|c| c.is_ascii_digit() || (decimal && c == '.') || (negative && c == '-')),
            _ => true,
        }
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let off = self.cursor_offset();
        let target = self.content[..off].unicode_word_indices().next_back().map(|(i, _)| i).unwrap_or(0);
        self.move_to(target, cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let off = self.cursor_offset();
        let target = self.content[off..].unicode_word_indices().nth(0).map(|(i, w)| off + i + w.len()).unwrap_or(self.content.len());
        self.move_to(target, cx);
    }

    fn delete_word(&mut self, _: &DeleteWord, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let off = self.cursor_offset();
            let target = self.content[..off].unicode_word_indices().next_back().map(|(i, _)| i).unwrap_or(0);
            self.select_to(target, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else if event.click_count >= 2 {
            self.select_all_text(cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(&mut self, _: &ShowCharacterPalette, window: &mut Window, _: &mut Context<Self>) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = text.replace('\n', " ").trim().to_string();
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && self.kind != InputKind::Password {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            if self.kind != InputKind::Password {
                cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
            }
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => {
                cx.emit(TextInputEvent::Submit);
                cx.stop_propagation();
            }
            "escape" => {
                cx.emit(TextInputEvent::Escape);
                cx.stop_propagation();
            }
            "up" => {
                cx.emit(TextInputEvent::Arrow(-1));
                cx.stop_propagation();
            }
            "down" => {
                cx.emit(TextInputEvent::Arrow(1));
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed { self.selected_range.start } else { self.selected_range.end }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        let idx = line.closest_index_for_x(position.x - bounds.left());
        // Map display index back for masked passwords (1 bullet per char)
        if self.kind == InputKind::Password && !self.reveal_password {
            let display = self.display_text();
            let n = display[..idx.min(display.len())].chars().count();
            return self.content.char_indices().nth(n).map(|(i, _)| i).unwrap_or(self.content.len());
        }
        idx
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content.grapheme_indices(true).rev().find_map(|(idx, _)| (idx < offset).then_some(idx)).unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content.grapheme_indices(true).find_map(|(idx, _)| (idx > offset).then_some(idx)).unwrap_or(self.content.len())
    }

    fn display_text(&self) -> SharedString {
        if self.kind == InputKind::Password && !self.reveal_password {
            self.content.chars().map(|_| '•').collect::<String>().into()
        } else {
            self.content.clone()
        }
    }

    /// Display-string offset for a content offset (differs for masked passwords).
    fn display_offset(&self, offset: usize) -> usize {
        if self.kind == InputKind::Password && !self.reveal_password {
            self.content[..offset.min(self.content.len())].chars().count() * '•'.len_utf8()
        } else {
            offset
        }
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(&mut self, range_utf16: Range<usize>, actual_range: &mut Option<Range<usize>>, _window: &mut Window, _cx: &mut Context<Self>) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(&mut self, _ignore_disabled_input: bool, _window: &mut Window, _cx: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected_range), reversed: self.selection_reversed })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(&mut self, range_utf16: Option<Range<usize>>, new_text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || !self.accepts(new_text) {
            return;
        }
        let range = range_utf16.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked_range.clone()).unwrap_or(self.selected_range.clone());
        let mut new_text = new_text.to_string();
        if let Some(max) = self.max_len {
            let remaining = max.saturating_sub(self.content.chars().count() - self.content[range.clone()].chars().count());
            new_text = new_text.chars().take(remaining).collect();
        }
        self.content = (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..]).into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(&mut self, range_utf16: Option<Range<usize>>, new_text: &str, new_selected_range_utf16: Option<Range<usize>>, _window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let range = range_utf16.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked_range.clone()).unwrap_or(self.selected_range.clone());
        self.content = (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        self.marked_range = if new_text.is_empty() { None } else { Some(range.start..range.start + new_text.len()) };
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .map(|r| r.start + range.start..r.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(&mut self, range_utf16: Range<usize>, bounds: Bounds<Pixels>, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(bounds.left() + last_layout.x_for_index(self.display_offset(range.start)), bounds.top()),
            point(bounds.left() + last_layout.x_for_index(self.display_offset(range.end)), bounds.bottom()),
        ))
    }

    fn character_index_for_point(&mut self, point: gpui::Point<Pixels>, _window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    offset_x: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _id: Option<&GlobalElementId>, _inspector_id: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _id: Option<&GlobalElementId>, _inspector_id: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _request_layout: &mut Self::RequestLayoutState, window: &mut Window, cx: &mut App) -> Self::PrepaintState {
        let theme = cx.global::<Theme>().clone();
        let input = self.input.read(cx);
        let content = input.display_text();
        let selected_range = input.display_offset(input.selected_range.start)..input.display_offset(input.selected_range.end);
        let cursor = input.display_offset(input.cursor_offset());
        let style = window.text_style();
        let align_right = input.align_right;
        let disabled = input.disabled;

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), theme.text_muted)
        } else {
            (content, if disabled { theme.text_disabled } else { style.color })
        };
        let placeholder = input.content.is_empty();

        let run = TextRun { len: display_text.len(), font: style.font(), color: text_color, background_color: None, underline: None, strikethrough: None };
        let runs = if let Some(marked_range) = input.marked_range.as_ref().filter(|_| !placeholder) {
            let m = input.display_offset(marked_range.start)..input.display_offset(marked_range.end);
            vec![
                TextRun { len: m.start, ..run.clone() },
                TextRun { len: m.end - m.start, underline: Some(UnderlineStyle { color: Some(run.color), thickness: px(1.0), wavy: false }), ..run.clone() },
                TextRun { len: display_text.len() - m.end, ..run },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(display_text, font_size, &runs, None);

        // Keep the cursor visible by scrolling the line horizontally
        let cursor_pos = line.x_for_index(cursor);
        let mut offset_x = px(0.);
        if align_right && line.width < bounds.size.width {
            offset_x = bounds.size.width - line.width;
        } else if cursor_pos > bounds.size.width - px(2.) {
            offset_x = bounds.size.width - px(2.) - cursor_pos;
        }
        let left = bounds.left() + offset_x;

        let (selection, cursor) = if selected_range.is_empty() || placeholder {
            (None, Some(fill(Bounds::new(point(left + cursor_pos, bounds.top() + px(1.)), size(px(1.), bounds.size.height - px(2.))), theme.accent)))
        } else {
            (
                Some(fill(
                    Bounds::from_corners(point(left + line.x_for_index(selected_range.start), bounds.top()), point(left + line.x_for_index(selected_range.end), bounds.bottom())),
                    theme.text_selection,
                )),
                None,
            )
        };
        PrepaintState { line: Some(line), cursor, selection, offset_x }
    }

    fn paint(&mut self, _id: Option<&GlobalElementId>, _inspector_id: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _request_layout: &mut Self::RequestLayoutState, prepaint: &mut Self::PrepaintState, window: &mut Window, cx: &mut App) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let line = prepaint.line.take().unwrap();
        let origin = point(bounds.origin.x + prepaint.offset_x, bounds.origin.y);
        line.paint(origin, window.line_height(), gpui::TextAlign::Left, None, window, cx).ok();

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(Bounds::new(origin, bounds.size));
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let focused = self.focus_handle.is_focused(window);
        if self.was_focused && !focused {
            cx.emit(TextInputEvent::Blur);
        }
        self.was_focused = focused;
        let quiet_idle = self.bare || (self.quiet && !focused && self.content.is_empty() && !self.invalid);
        let border = if self.invalid {
            theme.error
        } else if self.bare {
            gpui::transparent_black()
        } else if focused {
            theme.border_focused
        } else if quiet_idle {
            gpui::transparent_black()
        } else {
            theme.border
        };
        let family = if self.is_mono() { theme.mono_font.clone() } else { theme.ui_font.clone() };
        let line_h = if self.text_size == type_scale::BASE { px(18.) } else { self.text_size + px(5.) };
        div()
            .id("text-input")
            .key_context("TextInput")
            .track_focus(&self.focus_handle)
            .flex()
            .items_center()
            .w_full()
            .h(px(self.height))
            .px(px(if self.bare { 0. } else { 8. }))
            .rounded(px(4.))
            .bg(if quiet_idle { gpui::transparent_black() } else { theme.bg })
            .border_1()
            .border_color(border)
            .when(!focused && !self.disabled && !self.invalid && !self.bare, |d| d.hover(|s| s.border_color(theme.text_muted.alpha(0.5))))
            .cursor(if self.disabled { CursorStyle::Arrow } else { CursorStyle::IBeam })
            .font(font(&family, gpui::FontWeight::NORMAL))
            .text_size(self.text_size)
            .line_height(line_h)
            .text_color(theme.text)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::delete_word))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(div().flex_1().overflow_hidden().child(TextElement { input: cx.entity() }))
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
