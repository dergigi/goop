use std::sync::Arc;
use gpui::prelude::FluentBuilder;
use gpui::{App, AppContext, Context, Image, ImageFormat, InteractiveElement, IntoElement,
    ObjectFit, ParentElement, Render, StatefulInteractiveElement, Styled, StyledImage, Task, Window, div, img, px};
use state::encrypted_file::EncryptedFile;
use theme::ActiveTheme;
use ui::{Icon, IconName, Sizable, WindowExtension, v_flex};
use ui::button::{Button, ButtonVariants};
use ui::notification::Notification;

#[derive(Clone)]
pub(super) struct Attachment {
    pub file: EncryptedFile,
    pub bytes: Arc<Vec<u8>>,
    pub image: Option<Arc<Image>>,
}

impl Attachment {
    pub fn new(file: EncryptedFile, bytes: Vec<u8>) -> Self {
        let image = ImageFormat::from_mime_type(&file.mime)
            .map(|format| Arc::new(Image::from_bytes(format, bytes.clone())));
        Self { file, bytes: Arc::new(bytes), image }
    }
}

fn save(attachment: Attachment, window: &mut Window, cx: &mut App) {
    let extension = ImageFormat::from_mime_type(&attachment.file.mime)
        .map(|format| format.extension()).unwrap_or("bin");
    let filename = format!("attachment.{extension}");
    let prompt = cx.prompt_for_new_path(common::download_dir(), Some(&filename));
    window.spawn(cx, async move |cx| {
        let result = async {
            let Some(path) = prompt.await?? else { return Ok::<(), anyhow::Error>(()) };
            cx.background_spawn(async move { std::fs::write(path, &*attachment.bytes) }).await?;
            Ok(())
        }.await;
        if let Err(error) = result {
            let _ = cx.update(|window, cx| {
                window.push_notification(Notification::error(format!("Could not save attachment: {error}")), cx);
            });
        }
    }).detach();
}

pub(super) fn preview(attachment: Attachment, window: &mut Window, cx: &mut App) {
    window.open_modal(cx, move |modal, window, _| {
        let size = window.viewport_size();
        let save_attachment = attachment.clone();
        modal.title("Attachment preview").show_close(true)
            .width((size.width - px(64.)).min(px(1000.)))
            .child(v_flex().items_center().justify_center()
                .when_some(attachment.image.clone(), |view, image| {
                    view.child(img(image).w_full().h(size.height * 0.7).object_fit(ObjectFit::Contain))
                })
                .when(attachment.image.is_none(), |view| {
                    view.child("Encrypted attachment").child(attachment.file.mime.clone())
                }))
            .footer(move |_, _, _, _| {
                let attachment = save_attachment.clone();
                vec![Button::new("save-attachment").label("Save decrypted file…")
                    .on_click(move |_, window, cx| save(attachment.clone(), window, cx))]
            })
    });
}

pub(super) struct EncryptedMedia {
    file: EncryptedFile,
    attachment: Option<Attachment>,
    error: Option<String>,
    task: Option<Task<()>>,
}

impl EncryptedMedia {
    pub fn new(file: EncryptedFile, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut view = Self { file, attachment: None, error: None, task: None };
        view.load(window, cx);
        view
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        let file = self.file.clone();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = state::encrypted_file::download(file.clone(), cx).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(bytes) => this.attachment = Some(Attachment::new(file, bytes)),
                    Err(_) => this.error = Some("Could not download or decrypt attachment".into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for EncryptedMedia {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().gap_2()
            .when_some(self.attachment.clone(), |view, attachment| {
                let click_attachment = attachment.clone();
                view.child(div().id("encrypted-media-preview").cursor_pointer()
                    .when_some(attachment.image.clone(), |view, image| {
                        view.child(img(image).max_w_full().h(px(250.)).object_fit(ObjectFit::Contain))
                    })
                    .when(attachment.image.is_none(), |view| {
                        view.child(Icon::new(IconName::Upload)).child("Open attachment")
                    })
                    .on_click(move |_, window, cx| preview(click_attachment.clone(), window, cx)))
            })
            .when(self.attachment.is_none() && self.error.is_none(), |view| {
                view.child(ui::indicator::Indicator::new().small()).child("Decrypting attachment…")
            })
            .when_some(self.error.clone(), |view, error| {
                view.child(div().text_sm().text_color(cx.theme().text_muted).child(error))
                    .child(Button::new("retry-attachment").label("Retry").small().ghost()
                        .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))))
            })
    }
}
