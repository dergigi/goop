//! Shared, title-free image gallery used by chats and profile pictures.
use gpui::{App, AppContext, Context, FocusHandle, ImageSource, InteractiveElement, KeyDownEvent,
    ObjectFit, ParentElement, Render, Resource, Styled, StyledImage, Window, div, img, px};
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
    let gallery = cx.new(|cx| Gallery { images, selected, footer: Box::new(footer), downloading: false, keyboard_controls: false, focus: cx.focus_handle() });
    let focus = gallery.read(cx).focus.clone();
    window.open_modal(cx, move |modal, window, cx| {
        modal.show_close(false).margin_top(px(24.)).p_0()
            .overlay_color(gpui::black().opacity(0.85))
            .bg(gpui::transparent_black()).border_0().rounded_none().shadow(Vec::new())
            .width(gallery_size(window, cx).width)
            .child(gallery.clone())
    });
    focus.focus(window, cx);
}

struct Gallery {
    images: Vec<ImageSource>,
    selected: usize,
    footer: Footer,
    downloading: bool,
    focus: FocusHandle,
    keyboard_controls: bool,
}

fn gallery_size(window: &mut Window, cx: &mut App) -> gpui::Size<gpui::Pixels> {
    let padding = crate::root::window_paddings(window, cx);
    let layers = crate::Root::read(window, cx).active_modals.len().saturating_sub(1);
    gpui::size(
        (window.viewport_size().width - padding.left - padding.right - px(48.)).max(px(1.)),
        (window.viewport_size().height - padding.top - padding.bottom - px(49. + layers as f32 * 16.)).max(px(1.)),
    )
}

fn downloadable_url(image: &ImageSource) -> Option<gpui::http_client::Url> {
    let ImageSource::Resource(Resource::Uri(uri)) = image else { return None; };
    let url = gpui::http_client::Url::parse(uri.as_ref()).ok()?;
    matches!(url.scheme(), "https" | "http").then_some(url)
}

fn download_filename(url: &gpui::http_client::Url, content_type: &str) -> String {
    let mime = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let extension = gpui::ImageFormat::from_mime_type(&mime).map(|format| format.extension())
        .or_else(|| {
            let extension = url.path().rsplit('.').next()?;
            matches!(extension, "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "avif" | "bmp" | "tif" | "tiff" | "ico").then_some(extension)
        }).unwrap_or("bin");
    format!("image.{extension}")
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

    fn download(&mut self, url: gpui::http_client::Url, window: &mut Window, cx: &mut Context<Self>) {
        if self.downloading { return; }
        self.downloading = true;
        cx.notify();
        let client = cx.http_client();
        let executor = cx.background_executor().clone();
        let request = cx.background_spawn(async move {
            use smol::io::AsyncReadExt;
            smol::future::or(async move {
                let mut response = client.get(url.as_str(), ().into(), true).await?;
                anyhow::ensure!(response.status().is_success(), "Server returned {}", response.status());
                let content_type = response.headers().get("content-type").and_then(|value| value.to_str().ok()).unwrap_or("");
                let filename = download_filename(&url, content_type);
                let mut bytes = Vec::new();
                const MAX_BYTES: u64 = 64 * 1024 * 1024;
                response.body_mut().take(MAX_BYTES + 1).read_to_end(&mut bytes).await?;
                anyhow::ensure!(bytes.len() as u64 <= MAX_BYTES, "Image exceeds the 64 MB download limit");
                anyhow::ensure!(!bytes.is_empty(), "Server returned an empty image");
                Ok::<_, anyhow::Error>((filename, bytes))
            }, async move {
                executor.timer(std::time::Duration::from_secs(30)).await;
                anyhow::bail!("Image download timed out")
            }).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = async {
                let (filename, bytes) = request.await?;
                let prompt = cx.update(|_, cx| cx.prompt_for_new_path(common::download_dir(), Some(&filename)))?;
                if let Some(path) = prompt.await?? {
                    cx.background_spawn(async move { std::fs::write(path, bytes) }).await?;
                }
                Ok::<_, anyhow::Error>(())
            }.await;
            let _ = this.update(cx, |this, cx| { this.downloading = false; cx.notify(); });
            if let Err(error) = result {
                let _ = cx.update(|window, cx| window.push_notification(
                    crate::notification::Notification::error(format!("Could not save image: {error}")), cx));
            }
        }).detach();
    }
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let size = gallery_size(window, cx);
        let control_style = ButtonCustomVariant::new(window, cx)
            .color(gpui::transparent_black()).foreground(gpui::white())
            .hover(gpui::white().opacity(0.16)).active(gpui::white().opacity(0.24));
        let mut downloads = (self.footer)(self.selected, window, cx);
        if downloads.is_empty() {
            if let Some(url) = downloadable_url(&self.images[self.selected]) {
                downloads.push(Button::new("download-image").icon(IconName::Download).tooltip("Download")
                    .loading(self.downloading).disabled(self.downloading)
                    .on_click(cx.listener(move |this, _, window, cx| this.download(url.clone(), window, cx))));
            }
        }
        div().relative().w(size.width).h(size.height).min_w_0().min_h_0().flex_shrink_0().overflow_hidden()
            .debug_selector(|| "gallery-viewport".into())
            .group("gallery").track_focus(&self.focus)
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
            .child(img(self.images[self.selected].clone()).absolute().inset_0().size_full().min_w_0().min_h_0()
                .object_fit(ObjectFit::Contain).debug_selector(|| "gallery-image".into()))
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
    use super::*;
    #[test]
    fn navigation_stops_at_gallery_edges() {
        assert_eq!(adjacent(0, 3, false), 0);
        assert_eq!(adjacent(0, 3, true), 1);
        assert_eq!(adjacent(2, 3, true), 2);
        assert_eq!(adjacent(2, 3, false), 1);
        assert_eq!(adjacent(0, 1, true), 0);
    }

    #[test]
    fn profile_urls_are_downloadable_and_filenames_use_the_image_format() {
        let source: ImageSource = "https://example.com/avatar?id=123".into();
        let url = downloadable_url(&source).unwrap();
        assert_eq!(download_filename(&url, "image/webp; charset=binary"), "image.webp");
        let url = gpui::http_client::Url::parse("https://example.com/photo.jpg?token=secret").unwrap();
        assert_eq!(download_filename(&url, "image/png"), "image.png");
        assert_eq!(download_filename(&url, "application/octet-stream"), "image.jpg");
        assert!(downloadable_url(&ImageSource::from("brand/avatar.png")).is_none());
        assert!(downloadable_url(&ImageSource::from("file:///tmp/avatar.png")).is_none());
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
    fn oversized_images_stay_inside_the_window_after_resizing(cx: &mut gpui::TestAppContext) {
        let (_, cx) = cx.add_window_view(|window, cx| {
            theme::init(cx);
            crate::init(cx);
            let harness = cx.new(|_| Harness);
            Root::new(harness.into(), window, cx)
        });
        for (width, height) in [(8192, 64), (64, 8192)] {
            let image = std::sync::Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Svg,
                format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"><rect width="100%" height="100%" fill="red"/></svg>"#).into_bytes()));
            cx.update(|window, cx| open(image, |_, _| Vec::new(), window, cx));
            for (width, height) in [(900., 600.), (320., 200.), (600., 280.)] {
                cx.simulate_resize(gpui::size(px(width), px(height)));
                cx.update(|window, cx| { window.draw(cx).clear(cx); });
                cx.run_until_parked();
                cx.update(|window, cx| { window.draw(cx).clear(cx); });
                let bounds = cx.debug_bounds("gallery-image").expect("Gallery image must render");
                let viewport = cx.debug_bounds("gallery-viewport").unwrap();
                assert_eq!(bounds, viewport);
                assert!(bounds.origin.x >= px(0.) && bounds.origin.y >= px(0.));
                assert!(bounds.right() <= px(width) && bounds.bottom() <= px(height), "{bounds:?}");
            }
            cx.update(|window, cx| window.close_modal(cx));
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
