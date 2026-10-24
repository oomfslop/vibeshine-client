pub mod dialogs;
pub mod host_info;
pub mod host_state;
pub mod main_view;
pub mod overlays;
pub mod palette;
pub mod settings;

use gpui::{Context, FontWeight, IntoElement, Window, div, prelude::*};

use crate::app::Root;
use crate::theme::{Theme, font, type_scale};

impl gpui::Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();

        // Deferred focus changes (set by event handlers)
        if let Some(handle) = self.pending_focus.take() {
            window.focus(&handle);
            self.needs_focus = false;
        } else if self.needs_focus {
            window.focus(&self.focus_handle);
            self.needs_focus = false;
        }

        if self.pending_add_host {
            self.pending_add_host = false;
            self.open_add_host(window, cx);
        }
        if let Some(state) = self.debug_state.take() {
            self.apply_debug_state(&state, window, cx);
        }

        let bounds = window.bounds();
        let width = f32::from(bounds.size.width);

        div()
            .id("root")
            .key_context("Root")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key))
            .on_action(cx.listener(Self::action_settings))
            .on_action(cx.listener(Self::action_add_host))
            .on_action(cx.listener(Self::action_refresh))
            .on_action(cx.listener(Self::action_focus_filter))
            .on_action(cx.listener(Self::action_toggle_host_info))
            .on_action(cx.listener(Self::action_copy_host_address))
            .on_action(cx.listener(Self::action_toggle_hidden))
            .on_action(cx.listener(Self::action_back))
            .on_action(cx.listener(Self::action_quit))
            .on_action(cx.listener(Self::action_quick_launch))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg)
            .font(font(&theme.ui_font, FontWeight::NORMAL))
            .text_size(type_scale::BASE)
            .line_height(type_scale::LH_BASE)
            .text_color(theme.text)
            .child(self.render_shell(width, window, cx))
            .children(self.render_overlays(window, cx))
    }
}
