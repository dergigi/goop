use gpui::prelude::FluentBuilder;
use gpui::{
    AbsoluteLength, App, Div, Hsla, ImageSource, Img, InteractiveElement, Interactivity,
    IntoElement, ObjectFit, ParentElement, RenderOnce, StyleRefinement, Styled, StyledImage,
    Window, div, img, px,
};
use theme::ActiveTheme;

use crate::{Selectable, Sizable, Size};

/// Returns the size of the avatar based on the given [`Size`].
pub(super) fn avatar_size(size: Size) -> AbsoluteLength {
    match size {
        Size::Large => px(64.).into(),
        Size::Medium => px(32.).into(),
        Size::Small => px(24.).into(),
        Size::XSmall => px(20.).into(),
        Size::Size(size) => size.into(),
    }
}

/// An element that renders a user avatar with customizable appearance options.
///
/// # Examples
///
/// ```
/// use ui::{Avatar};
///
/// Avatar::new("path/to/image.png").grayscale(true).border_color(gpui::red());
/// ```
#[derive(IntoElement)]
pub struct Avatar {
    base: Div,
    image: Img,
    style: StyleRefinement,
    size: Size,
    border_color: Option<Hsla>,
    selected: bool,
}

impl Avatar {
    /// Creates a new avatar element with the specified image source.
    pub fn new(src: impl Into<ImageSource>) -> Self {
        Avatar {
            base: div(),
            image: img(src),
            style: StyleRefinement::default(),
            size: Size::Medium,
            border_color: None,
            selected: false,
        }
    }

    /// Applies a grayscale filter to the avatar image.
    ///
    /// # Examples
    ///
    /// ```
    /// use ui::{Avatar, AvatarShape};
    ///
    /// let avatar = Avatar::new("path/to/image.png").grayscale(true);
    /// ```
    pub fn grayscale(mut self, grayscale: bool) -> Self {
        self.image = self.image.grayscale(grayscale);
        self
    }

    /// Sets the border color of the avatar.
    ///
    /// This might be used to match the border to the background color of
    /// the parent element to create the illusion of cropping another
    /// shape underneath (for example in face piles.)
    pub fn border_color(mut self, color: impl Into<Hsla>) -> Self {
        self.border_color = Some(color.into());
        self
    }
}

impl Sizable for Avatar {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl Styled for Avatar {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Selectable for Avatar {
    fn is_selected(&self) -> bool {
        self.selected
    }

    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl InteractiveElement for Avatar {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl RenderOnce for Avatar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border_width = if self.border_color.is_some() {
            px(2.)
        } else {
            px(0.)
        };
        let image_size = avatar_size(self.size);
        let container_size = image_size.to_pixels(window.rem_size()) + border_width * 2.;

        div()
            .image_cache(common::avatar_cache(
                image_size.to_pixels(window.rem_size()).into(), window.scale_factor(), cx,
            ))
            .flex_shrink_0()
            .size(container_size)
            .rounded_full()
            .overflow_hidden()
            .when_some(self.border_color, |this, color| {
                this.border(border_width).border_color(color)
            })
            .child(
                self.image
                    .size(image_size)
                    .aspect_square()
                    .rounded_full()
                    .object_fit(ObjectFit::Cover)
                    .bg(cx.theme().ghost_element_background)
                    .with_fallback(move || {
                        img("brand/avatar.png")
                            .size(image_size)
                            .aspect_square()
                            .object_fit(ObjectFit::Cover)
                            .rounded_full()
                            .into_any_element()
                    }),
            )
    }
}
