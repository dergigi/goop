//! Shared image viewer used by chat attachments and profile pictures.
use gpui::{App, ImageSource, ObjectFit, ParentElement, SharedString, Styled, StyledImage, Window, img, px};
use crate::{WindowExtension, button::Button, v_flex};

pub fn open(
    title: impl Into<SharedString>,
    image: impl Into<ImageSource>,
    footer: impl Fn(&mut Window, &mut App) -> Vec<Button> + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let title = title.into();
    let image = image.into();
    let footer = std::rc::Rc::new(footer);
    window.open_modal(cx, move |modal, window, _| {
        let size = window.viewport_size();
        let footer = footer.clone();
        modal.title(title.clone()).show_close(true).margin_top(px(16.))
            .width((size.width - px(64.)).min(px(1000.)))
            .child(v_flex().items_center().justify_center()
                .child(img(image.clone()).w_full()
                    .h((size.height - px(160.)).max(px(64.)).min(size.height * 0.7))
                    .object_fit(ObjectFit::Contain)))
            .footer(move |_, _, window, cx| footer(window, cx))
    });
}
