use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, Hsla, IntoElement, Radians, Render, RenderOnce,
    SharedString, StyleRefinement, Styled, Svg, Transformation, Window, svg,
};
use theme::ActiveTheme;

use crate::{Sizable, Size};

pub trait IconNamed {
    /// Returns the embedded path of the icon.
    fn path(self) -> SharedString;
}

impl<T: IconNamed> From<T> for Icon {
    fn from(value: T) -> Self {
        Icon::build(value)
    }
}

#[derive(IntoElement, Clone)]
pub enum IconName {
    Activity,
    Archive,
    Block,
    Mute,
    Pin,
    ArrowLeft,
    ArrowRight,
    Boom,
    Bug,
    Book,
    ChevronDown,
    CaretDown,
    CaretRight,
    CaretUp,
    Chat,
    Check,
    CheckDouble,
    CheckCircle,
    Close,
    CloseCircle,
    CloseCircleFill,
    Copy,
    Device,
    Door,
    Ellipsis,
    EllipsisVertical,
    Emoji,
    Eye,
    Input,
    Keyboard,
    History,
    Help,
    Info,
    Invite,
    Inbox,
    InboxFill,
    Link,
    Loader,
    Moon,
    Plus,
    PlusCircle,
    Profile,
    Reset,
    Relay,
    Reply,
    Robot,
    Refresh,
    Scan,
    Search,
    Settings,
    Settings2,
    Sun,
    Ship,
    Flag,
    Shield,
    Group,
    User,
    UserKey,
    Download,
    Upload,
    Usb,
    PanelLeft,
    PanelLeftOpen,
    PanelRight,
    PanelRightOpen,
    PanelBottom,
    PanelBottomOpen,
    PaperPlaneFill,
    Warning,
    WarningTriangle,
    WindowClose,
    WindowMaximize,
    WindowMinimize,
    WindowRestore,
    Fistbump,
    FistbumpFill,
    Zoom,
}

impl IconName {
    /// Return the icon as a Entity<Icon>
    pub fn view(self, cx: &mut App) -> Entity<Icon> {
        Icon::build(self).view(cx)
    }
}

impl IconNamed for IconName {
    fn path(self) -> SharedString {
        match self {
            Self::ArrowLeft => "icons/arrow-left.svg",
            Self::ArrowRight => "icons/arrow-right.svg",
            Self::Boom => "icons/boom.svg",
            Self::Bug => "icons/bug.svg",
            Self::Book => "icons/book.svg",
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::CaretDown => "icons/caret-down.svg",
            Self::CaretRight => "icons/caret-right.svg",
            Self::CaretUp => "icons/caret-up.svg",
            Self::Chat => "icons/chat.svg",
            Self::Check => "icons/check.svg",
            Self::CheckDouble => "icons/check-double.svg",
            Self::CheckCircle => "icons/check-circle.svg",
            Self::Close => "icons/close.svg",
            Self::CloseCircle => "icons/close-circle.svg",
            Self::CloseCircleFill => "icons/close-circle-fill.svg",
            Self::Copy => "icons/copy.svg",
            Self::Device => "icons/device.svg",
            Self::Door => "icons/door.svg",
            Self::Ellipsis => "icons/ellipsis.svg",
            Self::EllipsisVertical => "icons/ellipsis-vertical.svg",
            Self::Emoji => "icons/emoji.svg",
            Self::Eye => "icons/eye.svg",
            Self::Input => "icons/input.svg",
            Self::Keyboard => "icons/keyboard.svg",
            Self::Help => "icons/help.svg",
            Self::History => "icons/history.svg",
            Self::Info => "icons/info.svg",
            Self::Invite => "icons/invite.svg",
            Self::Pin => "icons/pin.svg",
            Self::Block => "icons/block.svg",
            Self::Mute => "icons/mute.svg",
            Self::Activity => "icons/activity.svg",
            Self::Archive => "icons/archive.svg",
            Self::Inbox => "icons/inbox.svg",
            Self::InboxFill => "icons/inbox-fill.svg",
            Self::Link => "icons/link.svg",
            Self::Loader => "icons/loader.svg",
            Self::Moon => "icons/moon.svg",
            Self::Plus => "icons/plus.svg",
            Self::PlusCircle => "icons/plus-circle.svg",
            Self::Profile => "icons/profile.svg",
            Self::Reset => "icons/reset.svg",
            Self::Relay => "icons/relay.svg",
            Self::Reply => "icons/reply.svg",
            Self::Robot => "icons/robot.svg",
            Self::Refresh => "icons/refresh.svg",
            Self::Scan => "icons/scan.svg",
            Self::Search => "icons/search.svg",
            Self::Settings => "icons/settings.svg",
            Self::Settings2 => "icons/settings2.svg",
            Self::Sun => "icons/sun.svg",
            Self::Ship => "icons/ship.svg",
            Self::Flag => "icons/flag.svg",
            Self::Shield => "icons/shield.svg",
            Self::User => "icons/user.svg",
            Self::UserKey => "icons/user-key.svg",
            Self::Download => "icons/download.svg",
            Self::Upload => "icons/upload.svg",
            Self::Usb => "icons/usb.svg",
            Self::Group => "icons/group.svg",
            Self::PanelLeft => "icons/panel-left.svg",
            Self::PanelLeftOpen => "icons/panel-left-open.svg",
            Self::PanelRight => "icons/panel-right.svg",
            Self::PanelRightOpen => "icons/panel-right-open.svg",
            Self::PanelBottom => "icons/panel-bottom.svg",
            Self::PanelBottomOpen => "icons/panel-bottom-open.svg",
            Self::PaperPlaneFill => "icons/paper-plane-fill.svg",
            Self::WarningTriangle => "icons/warning-triangle.svg",
            Self::Warning => "icons/warning.svg",
            Self::WindowClose => "icons/window-close.svg",
            Self::WindowMaximize => "icons/window-maximize.svg",
            Self::WindowMinimize => "icons/window-minimize.svg",
            Self::WindowRestore => "icons/window-restore.svg",
            Self::Fistbump => "icons/fistbump.svg",
            Self::FistbumpFill => "icons/fistbump-fill.svg",
            Self::Zoom => "icons/zoom.svg",
        }
        .into()
    }
}

impl From<IconName> for AnyElement {
    fn from(val: IconName) -> Self {
        Icon::build(val).into_any_element()
    }
}

impl RenderOnce for IconName {
    fn render(self, _: &mut Window, _cx: &mut App) -> impl IntoElement {
        Icon::build(self)
    }
}

#[derive(IntoElement)]
pub struct Icon {
    base: Svg,
    style: StyleRefinement,
    path: SharedString,
    text_color: Option<Hsla>,
    size: Option<Size>,
    rotation: Option<Radians>,
}

impl Default for Icon {
    fn default() -> Self {
        Self {
            base: svg().flex_none().size_4(),
            style: StyleRefinement::default(),
            path: "".into(),
            text_color: None,
            size: None,
            rotation: None,
        }
    }
}

impl Clone for Icon {
    fn clone(&self) -> Self {
        let mut this = Self::default().path(self.path.clone());
        this.style = self.style.clone();
        this.rotation = self.rotation;
        this.size = self.size;
        this.text_color = self.text_color;
        this
    }
}

impl Icon {
    pub fn new(icon: impl Into<Icon>) -> Self {
        icon.into()
    }

    fn build(name: impl IconNamed) -> Self {
        Self::default().path(name.path())
    }

    /// Set the icon path of the Assets bundle
    ///
    /// For example: `icons/foo.svg`
    pub fn path(mut self, path: impl Into<SharedString>) -> Self {
        self.path = path.into();
        self
    }

    /// Create a new view for the icon
    pub fn view(self, cx: &mut App) -> Entity<Icon> {
        cx.new(|_| self)
    }

    pub fn transform(mut self, transformation: gpui::Transformation) -> Self {
        self.base = self.base.with_transformation(transformation);
        self
    }

    pub fn empty() -> Self {
        Self::default()
    }

    /// Rotate the icon by the given angle
    pub fn rotate(mut self, radians: impl Into<Radians>) -> Self {
        self.base = self
            .base
            .with_transformation(Transformation::rotate(radians));
        self
    }
}

impl Styled for Icon {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }

    fn text_color(mut self, color: impl Into<Hsla>) -> Self {
        self.text_color = Some(color.into());
        self
    }
}

impl Sizable for Icon {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = Some(size.into());
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let text_color = self.text_color.unwrap_or_else(|| window.text_style().color);
        let text_size = window.text_style().font_size.to_pixels(window.rem_size());
        let has_base_size = self.style.size.width.is_some() || self.style.size.height.is_some();

        let mut base = self.base;
        *base.style() = self.style;

        base.flex_shrink_0()
            .text_color(text_color)
            .when(!has_base_size, |this| this.size(text_size))
            .when_some(self.size, |this, size| match size {
                Size::Size(px) => this.size(px),
                Size::XSmall => this.size_3(),
                Size::Small => this.size_4(),
                Size::Medium => this.size_5(),
                Size::Large => this.size_6(),
            })
            .path(self.path)
    }
}

impl From<Icon> for AnyElement {
    fn from(val: Icon) -> Self {
        val.into_any_element()
    }
}

impl Render for Icon {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text_color = self.text_color.unwrap_or_else(|| cx.theme().icon);
        let text_size = window.text_style().font_size.to_pixels(window.rem_size());
        let has_base_size = self.style.size.width.is_some() || self.style.size.height.is_some();

        let mut base = svg().flex_none();
        *base.style() = self.style.clone();

        base.flex_shrink_0()
            .text_color(text_color)
            .when(!has_base_size, |this| this.size(text_size))
            .when_some(self.size, |this, size| match size {
                Size::Size(px) => this.size(px),
                Size::XSmall => this.size_3(),
                Size::Small => this.size_4(),
                Size::Medium => this.size_5(),
                Size::Large => this.size_6(),
            })
            .path(self.path.clone())
            .when_some(self.rotation, |this, rotation| {
                this.with_transformation(Transformation::rotate(rotation))
            })
    }
}
