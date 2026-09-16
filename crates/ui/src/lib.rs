pub use element_ext::ElementExt;
pub use event::InteractiveElementExt;
pub use focusable::FocusableCycle;
pub use icon::*;
pub use index_path::IndexPath;
pub use kbd::*;
pub use root::{Root, window_paddings};
pub use styled::*;
pub use title_bar::*;
pub use window_ext::*;

pub use crate::Disableable;

pub mod actions;
pub mod animation;
pub mod avatar;
pub mod button;
pub mod checkbox;
pub mod divider;
pub mod dock;
pub mod group_box;
pub mod history;
pub mod indicator;
pub mod image_preview;
pub mod input;
pub mod list;
pub mod menu;
pub mod modal;
pub mod notification;
pub mod popover;
pub mod resizable;
pub mod scroll;
pub mod skeleton;
pub mod switch;
pub mod tab;
pub mod tooltip;

mod element_ext;
mod event;
mod focusable;
mod icon;
mod index_path;
mod kbd;
mod root;
mod styled;
mod title_bar;
mod window_ext;

/// Initialize the UI module.
///
/// This must be called before using any of the UI components.
/// You can initialize the UI module at your application's entry point.
pub fn init(cx: &mut gpui::App) {
    input::init(cx);
    list::init(cx);
    modal::init(cx);
    popover::init(cx);
    menu::init(cx);
}
