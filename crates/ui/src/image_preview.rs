//! Shared, title-free image gallery used by chats and profile pictures.
use gpui::{App, AppContext, Context, FocusHandle, ImageSource, InteractiveElement, KeyDownEvent,
    ObjectFit, ParentElement, Render, Styled, StyledImage, Window, div, img, px};
use gpui::prelude::FluentBuilder;
use crate::button::{ButtonCustomVariant, ButtonVariants};
use crate::{Sizable, Disableable, IconName, WindowExtension, button::Button, h_flex};

type Footer = Box<dyn Fn(usize, &mut Window, &mut App) -> Vec<Button>>;

pub fn open(
    image: impl Into<ImageSource>,
    footer: impl Fn(&mut Window, &mut App) -> Vec<Button> + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    open_gallery(vec![image.into()], 0, move |_, window, cx| footer(window, cx), window, cx);
}

pub fn open_gallery(
    images: Vec<ImageSource>,
    selected: usize,
    footer: impl Fn(usize, &mut Window, &mut App) -> Vec<Button> + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    if images.is_empty() { return; }
    let selected = selected.min(images.len() - 1);
    let gallery = cx.new(|cx| Gallery { images, selected, footer: Box::new(footer), keyboard_controls: false, focus: cx.focus_handle() });
    let focus = gallery.read(cx).focus.clone();
    window.open_modal(cx, move |modal, window, _| {
        modal.show_close(false).margin_top(px(24.)).p_0()
            .overlay_color(gpui::black().opacity(0.85))
            .bg(gpui::transparent_black()).border_0().rounded_none().shadow(Vec::new())
            .width((window.viewport_size().width - px(48.)).min(px(1400.)))
            .child(gallery.clone())
    });
    focus.focus(window, cx);
}

struct Gallery {
    images: Vec<ImageSource>,
    selected: usize,
    footer: Footer,
    focus: FocusHandle,
    keyboard_controls: bool,
}

fn adjacent(index: usize, count: usize, forward: bool) -> usize {
    if forward { index.saturating_add(1).min(count.saturating_sub(1)) }
    else { index.saturating_sub(1) }
}

impl Gallery {
    fn navigate(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.selected = adjacent(self.selected, self.images.len(), forward);
        cx.notify();
    }
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let size = window.viewport_size();
        let control_style = ButtonCustomVariant::new(window, cx)
            .color(gpui::transparent_black()).foreground(gpui::white())
            .hover(gpui::white().opacity(0.16)).active(gpui::white().opacity(0.24));
        let downloads = (self.footer)(self.selected, window, cx);
        div().relative().group("gallery").track_focus(&self.focus)
            .on_mouse_move(cx.listener(|this, _, _, cx| {
                if this.keyboard_controls { this.keyboard_controls = false; cx.notify(); }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.keyboard_controls = true;
                cx.notify();
                match event.keystroke.key.as_str() {
                    "left" => this.navigate(false, cx),
                    "right" => this.navigate(true, cx),
                    "escape" => window.close_modal(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(img(self.images[self.selected].clone()).w_full()
                .h((size.height - px(64.)).max(px(64.)))
                .object_fit(ObjectFit::Contain))
            .child(h_flex().absolute().bottom_4().left_0().w_full().justify_center()
                .when(!self.keyboard_controls, |view| view.invisible().group_hover("gallery", |style| style.visible()))
                .child(h_flex().items_center().gap_1().p_1().rounded_full()
                    .bg(gpui::black().opacity(0.72)).text_color(gpui::white()).text_sm()
                    .when(self.images.len() > 1, |view| view
                        .child(Button::new("previous-image").icon(IconName::ArrowLeft).small()
                            .custom(control_style).rounded_full()
                            .tooltip("Previous image (←)").disabled(self.selected == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.navigate(false, cx))))
                        .child(div().px_2().child(format!("{} / {}", self.selected + 1, self.images.len())))
                        .child(Button::new("next-image").icon(IconName::ArrowRight).small()
                            .custom(control_style).rounded_full()
                            .tooltip("Next image (→)").disabled(self.selected + 1 == self.images.len())
                            .on_click(cx.listener(|this, _, _, cx| this.navigate(true, cx)))))
                    .children(downloads.into_iter().map(|button| button.small().custom(control_style).rounded_full()))
                    .child(Button::new("close-gallery").icon(IconName::Close).small()
                        .custom(control_style).rounded_full().tooltip("Close (Esc)")
                        .on_click(|_, window, cx| window.close_modal(cx)))))
    }
}

#[cfg(test)]
mod tests {
    use super::adjacent;
    #[test]
    fn navigation_stops_at_gallery_edges() {
        assert_eq!(adjacent(0, 3, false), 0);
        assert_eq!(adjacent(0, 3, true), 1);
        assert_eq!(adjacent(2, 3, true), 2);
        assert_eq!(adjacent(2, 3, false), 1);
        assert_eq!(adjacent(0, 1, true), 0);
    }
}

#[cfg(all(test, feature = "test-support"))]
mod interaction_tests {
    use super::*;
    use crate::{Root, v_flex};
    use std::{cell::Cell, rc::Rc};

    struct Harness;
    impl Render for Harness {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
            v_flex().size_full().children(Root::render_modal_layer(window, cx))
        }
    }

    #[gpui::test]
    fn gallery_keyboard_updates_selected_footer_and_escape_closes(cx: &mut gpui::TestAppContext) {
        let (_, cx) = cx.add_window_view(|window, cx| {
            theme::init(cx);
            crate::init(cx);
            let harness = cx.new(|_| Harness);
            Root::new(harness.into(), window, cx)
        });
        let selected = Rc::new(Cell::new(usize::MAX));
        cx.update(|window, cx| {
            let selected = selected.clone();
            let images = (0..3).map(|_| gpui::SharedString::from("brand/avatar.png").into()).collect();
            open_gallery(images, 1, move |index, _, _| {
                selected.set(index);
                Vec::new()
            }, window, cx);
            window.draw(cx).clear(cx);
        });
        assert_eq!(selected.get(), 1);
        for (key, expected) in [("right", 2), ("right", 2), ("left", 1), ("left", 0), ("left", 0)] {
            cx.simulate_keystrokes(key);
            cx.update(|window, cx| { window.draw(cx).clear(cx); });
            assert_eq!(selected.get(), expected);
        }
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| assert!(!Root::read(window, cx).has_active_modals()));
        cx.update(|window, cx| {
            open_gallery(Vec::new(), 0, |_, _, _| Vec::new(), window, cx);
            assert!(!Root::read(window, cx).has_active_modals());
        });
    }
}
