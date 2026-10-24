//! Modal dialogs: confirmations and the stream error dialog.

use gpui::{AnyElement, ClipboardItem, Context, IntoElement, ParentElement, Styled, div, prelude::*, px};

use crate::app::{Dialog, Root};
use crate::assets::icon;
use crate::theme::Theme;
use crate::ui::*;

pub fn render(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> Option<AnyElement> {
    let dialog = root.dialog.as_ref()?;
    let content: AnyElement = match dialog {
        Dialog::Confirm { title, body, primary, danger, action } => {
            let action = action.clone();
            let footer = vec![
                button("confirm-cancel", "Cancel").on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))).into_any_element(),
                button("confirm-ok", primary.clone())
                    .variant(if *danger { ButtonVariant::Danger } else { ButtonVariant::Primary })
                    .kbd("⏎")
                    .on_click(cx.listener(move |this, _, _, cx| this.confirm(action.clone(), cx)))
                    .into_any_element(),
            ];
            dialog_shell(theme, 360., title.clone(), None, vec![text_ui(theme, body.clone()).into_any_element()], footer).into_any_element()
        }
        Dialog::Error { title, body, details, permission, host_uuid } => {
            let mut body_els: Vec<AnyElement> = body.iter().map(|line| text_ui(theme, line.clone()).into_any_element()).collect();
            if !details.is_empty() && !*permission {
                let d = details.clone();
                body_els.push(
                    button("copy-details", "Copy details")
                        .link()
                        .icon(icon::COPY)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(d.clone()));
                            this.toast(ToastKind::Success, "Details copied", cx);
                        }))
                        .into_any_element(),
                );
            }
            let mut footer: Vec<AnyElement> = Vec::new();
            if host_uuid.is_some() {
                footer.push(
                    button("err-details", "Host info")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_dialog(cx);
                            this.open_host_info(cx);
                        }))
                        .into_any_element(),
                );
            }
            if !*permission {
                footer.push(button("err-help", "Troubleshooting").icon(icon::EXTERNAL_LINK).on_click(|_, _, cx| cx.open_url("https://github.com/moonlight-stream/moonlight-docs/wiki/Troubleshooting")).into_any_element());
            }
            footer.push(button("err-ok", "OK").primary().kbd("⏎").on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))).into_any_element());
            let ic = if *permission { (icon::SHIELD_ALERT, theme.warning) } else { (icon::CIRCLE_X, theme.error) };
            dialog_shell(theme, 440., title.clone(), Some(ic), body_els, footer).into_any_element()
        }
        Dialog::Input { purpose: _, title, help, primary, input, in_progress, error, blocked_by_os } => {
            let mut body_els: Vec<AnyElement> = Vec::new();
            if let Some(h) = help {
                body_els.push(text_sm(theme, h.clone()).into_any_element());
            }
            body_els.push(div().w_full().child(input.clone()).into_any_element());
            if let Some(e) = error {
                body_els.push(div().flex().items_start().gap(px(6.)).child(icon_el(icon::CIRCLE_ALERT, 14., theme.error)).child(div().flex_1().text_color(theme.error).child(e.clone())).into_any_element());
            }
            if *blocked_by_os {
                #[cfg(target_os = "macos")]
                {
                    body_els.push(text_sm(theme, crate::backend::platform::LOCAL_NETWORK_HINT).into_any_element());
                }
            }
            let mut footer: Vec<AnyElement> = Vec::new();
            #[cfg(target_os = "macos")]
            if *blocked_by_os {
                footer.push(button("input-lan-settings", "Local Network settings").ghost().icon(icon::EXTERNAL_LINK).on_click(|_, _, cx| cx.open_url(crate::backend::platform::LOCAL_NETWORK_SETTINGS_URL)).into_any_element());
            }
            footer.push(button("input-cancel", "Cancel").on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))).into_any_element());
            let label = if *in_progress { "Adding…".to_string() } else { primary.clone() };
            footer.push(button("input-ok", label).primary().kbd("⏎").disabled(*in_progress).on_click(cx.listener(|this, _, _, cx| this.submit_input_dialog(cx))).into_any_element());
            dialog_shell(theme, 400., title.clone(), None, body_els, footer).into_any_element()
        }
    };
    Some(div().id("dialog-scrim").absolute().inset_0().bg(theme.scrim).flex().justify_center().items_center().occlude().child(content).into_any_element())
}
