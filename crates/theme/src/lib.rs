use std::ops::{Deref, DerefMut};

use gpui::{App, Global, Pixels, SharedString, Window, px};

mod colors;
mod geometry;
mod notification;
mod platform_kind;
mod scale;
mod scrollbar_mode;
mod theme;

pub use colors::*;
pub use geometry::*;
pub use notification::*;
pub use platform_kind::PlatformKind;
pub use scale::*;
pub use scrollbar_mode::*;
pub use theme::*;

/// Defines window border radius for platforms that use client side decorations.
pub const CLIENT_SIDE_DECORATION_ROUNDING: Pixels = px(10.0);

/// Defines window shadow size for platforms that use client side decorations.
pub const CLIENT_SIDE_DECORATION_SHADOW: Pixels = px(10.0);

/// Defines window border size for platforms that use client side decorations.
pub const CLIENT_SIDE_DECORATION_BORDER: Pixels = px(1.0);

/// Defines window titlebar height
pub const TITLEBAR_HEIGHT: Pixels = px(36.0);

/// Defines workspace tabbar height
pub const TABBAR_HEIGHT: Pixels = px(44.0);

/// Defines default sidebar width
pub const SIDEBAR_WIDTH: Pixels = px(280.);

pub fn init(cx: &mut App) {
    Theme::sync_system_appearance(None, cx);
    Theme::sync_scrollbar_appearance(cx);
}

pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    #[inline(always)]
    fn theme(&self) -> &Theme {
        Theme::global(self)
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    /// Theme colors
    pub colors: ThemeColors,

    /// The appearance of the theme (light or dark).
    pub mode: ThemeMode,

    /// The font family for the application.
    pub font_family: SharedString,

    /// The root font size for the application, default is 15px.
    pub font_size: Pixels,

    /// Radius for the general elements.
    pub radius: Pixels,

    /// Radius for the large elements, e.g.: modal, notification.
    pub radius_lg: Pixels,

    /// Enable shadow for the general elements. default is true
    pub shadow: bool,

    /// Show the scrollbar mode, default: scrolling
    pub scrollbar_mode: ScrollbarMode,

    /// Notification settings
    pub notification: NotificationSettings,

    /// Platform kind
    pub platform: PlatformKind,
}

impl Deref for Theme {
    type Target = ThemeColors;

    fn deref(&self) -> &Self::Target {
        &self.colors
    }
}

impl DerefMut for Theme {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.colors
    }
}

impl Global for Theme {}

impl Theme {
    /// Returns the global theme reference
    pub fn global(cx: &App) -> &Theme {
        cx.global::<Theme>()
    }

    /// Returns the global theme mutable reference
    pub fn global_mut(cx: &mut App) -> &mut Theme {
        cx.global_mut::<Theme>()
    }

    /// Returns true if the theme is dark.
    pub fn is_dark(&self) -> bool {
        self.mode.is_dark()
    }

    /// Sync the theme with the system appearance
    pub fn sync_system_appearance(window: Option<&mut Window>, cx: &mut App) {
        let appearance = window
            .as_ref()
            .map(|window| window.appearance())
            .unwrap_or_else(|| cx.window_appearance());

        Self::change(appearance, window, cx);
    }

    /// Sync the Scrollbar showing behavior with the system
    pub fn sync_scrollbar_appearance(cx: &mut App) {
        Theme::global_mut(cx).scrollbar_mode = if cx.should_auto_hide_scrollbars() {
            ScrollbarMode::Scrolling
        } else {
            ScrollbarMode::Hover
        };
    }

    /// Change the app's appearance
    pub fn change<M>(mode: M, window: Option<&mut Window>, cx: &mut App)
    where
        M: Into<ThemeMode>,
    {
        if !cx.has_global::<Theme>() {
            let theme = Theme::default();

            cx.set_global(theme);
        }

        let mode = mode.into();
        let theme = cx.global_mut::<Theme>();

        // Set the theme mode
        theme.mode = mode;

        // Set the theme colors
        if mode.is_dark() {
            theme.colors = ThemeColors::dark();
        } else {
            theme.colors = ThemeColors::light();
        }

        // Refresh the window if available
        if let Some(window) = window {
            window.refresh();
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        let platform = PlatformKind::platform();
        let mode = ThemeMode::default();

        // Define the font family based on the platform.
        let font_family = match platform {
            PlatformKind::Linux => "Inter",
            _ => ".SystemUIFont",
        };

        // Define the theme colors based on the appearance
        let colors = match mode {
            ThemeMode::Light => ThemeColors::light(),
            ThemeMode::Dark => ThemeColors::dark(),
        };

        Theme {
            font_size: px(15.),
            font_family: font_family.into(),
            radius: px(6.),
            radius_lg: px(10.),
            shadow: true,
            scrollbar_mode: ScrollbarMode::default(),
            notification: NotificationSettings::default(),
            mode,
            colors,
            platform,
        }
    }
}
