use crate::qr_camera::{Capture, CaptureEvent, bunker_url};
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, EventEmitter, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render,
    RenderImage, Styled, StyledImage, Task, Window, div, img, px,
};
use std::{sync::Arc, time::Duration};
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::menu::{DropdownMenu, PopupMenuItem};
use ui::scroll::ScrollableElement;
use ui::{IconName, Sizable, StyledExt, h_flex, v_flex};

pub enum ScanEvent {
    Scanned(String),
    Cancel,
}

pub struct QrScanner {
    focus: FocusHandle,
    capture: Option<Capture>,
    task: Option<Task<()>>,
    preview: Option<Arc<RenderImage>>,
    devices: Vec<String>,
    selected: usize,
    error: Option<String>,
}
impl EventEmitter<ScanEvent> for QrScanner {}
impl QrScanner {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.on_release_in(window, |this, window, _| {
            this.stop(window);
        })
        .detach();
        cx.defer_in(window, |this, window, cx| this.start(0, window, cx));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            focus,
            capture: None,
            task: None,
            preview: None,
            devices: vec![],
            selected: 0,
            error: None,
        }
    }
    fn clear_preview(&mut self, window: &mut Window) {
        if let Some(image) = self.preview.take() {
            let _ = window.drop_image(image);
        }
    }
    fn stop(&mut self, window: &mut Window) {
        self.capture = None;
        self.clear_preview(window);
    }
    fn start(&mut self, selected: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.task = None;
        self.stop(window);
        self.selected = selected;
        self.error = None;
        match Capture::start(selected) {
            Err(_) => {
                self.error =
                    Some("Could not start the scanner. Try again or paste the bunker URL.".into())
            }
            Ok((capture, receiver)) => {
                self.capture = Some(capture);
                self.task = Some(cx.spawn_in(window, async move |this, cx| {
                    use futures::{
                        FutureExt,
                        future::{Either, select},
                    };
                    let deadline = cx
                        .background_executor()
                        .timer(Duration::from_secs(120))
                        .fuse();
                    futures::pin_mut!(deadline);
                    loop {
                        let event = match select(receiver.recv_async().boxed(), &mut deadline).await
                        {
                            Either::Left((Ok(event), _)) => event,
                            Either::Left((Err(_), _)) => CaptureEvent::Error(
                                "The camera stopped. Try scanning again.".into(),
                            ),
                            Either::Right(_) => CaptureEvent::Error(
                                "Scanning timed out. Try again or paste the bunker URL.".into(),
                            ),
                        };
                        let finished = this
                            .update_in(cx, |this, window, cx| {
                                match event {
                                    CaptureEvent::Devices(devices) => this.devices = devices,
                                    CaptureEvent::Frame(mut image) => {
                                        // GPUI's render images use BGRA rather than RGBA.
                                        for pixel in image.pixels_mut() {
                                            pixel.0.swap(0, 2);
                                        }
                                        this.clear_preview(window);
                                        this.preview =
                                            Some(Arc::new(RenderImage::new(smallvec::smallvec![
                                                image::Frame::new(image)
                                            ])));
                                    }
                                    CaptureEvent::Decoded(text) => {
                                        this.stop(window);
                                        match bunker_url(&text) {
                                            Ok(url) => cx.emit(ScanEvent::Scanned(url)),
                                            Err(error) => this.error = Some(error.to_string()),
                                        }
                                        cx.notify();
                                        return true;
                                    }
                                    CaptureEvent::Error(error) => {
                                        this.stop(window);
                                        this.error = Some(error);
                                        cx.notify();
                                        return true;
                                    }
                                }
                                cx.notify();
                                false
                            })
                            .unwrap_or(true);
                        if finished {
                            break;
                        }
                    }
                }));
            }
        }
        cx.notify();
    }
}
impl Render for QrScanner {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let devices = self.devices.clone();
        let view = cx.entity().downgrade();
        v_flex().track_focus(&self.focus).on_key_down(cx.listener(|this,event: &gpui::KeyDownEvent,window,cx| {
            if event.keystroke.key == "escape" { cx.stop_propagation(); this.task=None; this.stop(window); cx.emit(ScanEvent::Cancel); }
        })).gap_3().w_full().max_h((window.viewport_size().height-px(160.)).max(px(120.))).overflow_y_scrollbar()
            .child(div().font_semibold().child("Scan your signer’s QR code"))
            .child(div().text_sm().text_color(cx.theme().text_muted).child("Show the bunker QR code from your signer app to the camera. Scanning happens on this device."))
            .when(!devices.is_empty(), |column| column.child(Button::new("camera-selector")
                .label(devices.get(self.selected).cloned().unwrap_or_else(|| "Select camera".into())).ghost_alt().small()
                .dropdown_menu(move |mut menu,_,_| {
                    for (index,name) in devices.iter().enumerate() {
                        let view = view.clone();
                        menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_,window,cx| { let _ = view.update(cx,|this,cx| this.start(index,window,cx)); }));
                    }
                    menu
                })))
            .child(div().w_full().h(px(240.).min((window.viewport_size().height-px(360.)).max(px(100.))))
                .rounded_lg().overflow_hidden().bg(cx.theme().elevated_surface_background).flex().items_center().justify_center()
                .when_some(self.preview.clone(), |view,image| view.child(img(image).size_full().object_fit(gpui::ObjectFit::Contain)))
                .when(self.preview.is_none(), |view| view.child(if self.capture.is_some() { "Waiting for camera… Allow camera access if prompted." } else { "Camera off" })))
            .when_some(self.error.clone(), |view,error| view.child(div().text_sm().text_color(cx.theme().text_danger).child(error)))
            .child(h_flex().gap_2()
                .child(Button::new("cancel-qr-scan").icon(IconName::ArrowLeft).label("Paste URL instead").ghost().on_click(cx.listener(|this,_,window,cx| { this.task=None; this.stop(window); cx.emit(ScanEvent::Cancel); })))
                .when(self.capture.is_none(), |view| view.child(Button::new("retry-qr-scan").label("Try again").primary().on_click(cx.listener(|this,_,window,cx| this.start(this.selected,window,cx))))))
    }
}
