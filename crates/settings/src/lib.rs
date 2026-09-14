use anyhow::{Error, anyhow};
use common::config_dir;
use gpui::{App, AppContext, Context, Entity, Global, Subscription, Task, Window};
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};
use theme::{Appearance, Theme};

pub fn init(window: &mut Window, cx: &mut App) {
    AppSettings::set_global(cx.new(|cx| AppSettings::new(window, cx)), cx)
}

macro_rules! setting_accessors {
    ($(pub $field:ident: $type:ty),* $(,)?) => {
        impl AppSettings {
            $(
                paste::paste! {
                    pub fn [<get_ $field>](cx: &App) -> $type {
                        Self::global(cx).read(cx).inner.read(cx).$field.clone()
                    }

                    pub fn [<update_ $field>](value: $type, cx: &mut App) {
                        Self::global(cx).update(cx, |this, cx| {
                            this.inner.update(cx, |inner, cx| {
                                inner.$field = value;
                                cx.notify();
                            });
                        });
                    }
                }
            )*
        }
    };
}

setting_accessors! {
    pub appearance: Appearance,
    pub hide_avatar: bool,
    pub render_markdown: bool,
    pub screening: bool,
    pub nip4e: bool,
    pub trusted_relays: Vec<String>,
    pub file_server: Url,
}

/// Signer kind
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum SignerKind {
    Auto,
    Encryption,
    #[default]
    User,
}

impl SignerKind {
    pub fn auto(&self) -> bool {
        matches!(self, SignerKind::Auto)
    }

    pub fn user(&self) -> bool {
        matches!(self, SignerKind::User)
    }

    pub fn encryption(&self) -> bool {
        matches!(self, SignerKind::Encryption)
    }
}

/// Room configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct RoomConfig {
    backup: bool,
    signer_kind: SignerKind,
}

impl RoomConfig {
    pub fn new() -> Self {
        Self {
            backup: true,
            signer_kind: SignerKind::default(),
        }
    }

    /// Get backup config
    pub fn backup(&self) -> bool {
        self.backup
    }

    /// Set backup config
    pub fn toggle_backup(&mut self) {
        self.backup = !self.backup;
    }

    /// Get signer kind config
    pub fn signer_kind(&self) -> &SignerKind {
        &self.signer_kind
    }

    /// Set signer kind config
    pub fn set_signer_kind(&mut self, kind: &SignerKind) {
        self.signer_kind = kind.to_owned();
    }
}

/// Settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Missing in legacy settings: migrate to following the system.
    #[serde(default)]
    pub appearance: Appearance,

    /// Hide user avatars
    pub hide_avatar: bool,

    /// Render Markdown formatting in chat bubbles
    #[serde(default = "default_render_markdown")]
    pub render_markdown: bool,

    /// Enable screening for unknown chat requests
    pub screening: bool,

    /// Enable decoupling encryption key
    pub nip4e: bool,

    /// Trusted relays; Goop will automatically authenticate with these relays
    pub trusted_relays: Vec<String>,

    /// Server for blossom media attachments
    pub file_server: Url,
}

fn default_render_markdown() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: Appearance::System,
            hide_avatar: false,
            render_markdown: default_render_markdown(),
            screening: true,
            nip4e: false,
            trusted_relays: vec![],
            file_server: Url::parse("https://blossom.band/").unwrap(),
        }
    }
}

impl AsRef<Settings> for Settings {
    fn as_ref(&self) -> &Settings {
        self
    }
}

struct GlobalAppSettings(Entity<AppSettings>);

impl Global for GlobalAppSettings {}

/// Application settings
pub struct AppSettings {
    /// Settings
    inner: Entity<Settings>,

    /// Event subscriptions
    _subscriptions: SmallVec<[Subscription; 2]>,
}

impl AppSettings {
    /// Retrieve the global settings instance
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalAppSettings>().0.clone()
    }

    /// Set the global settings instance
    fn set_global(state: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalAppSettings(state));
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inner = cx.new(|_| Settings::default());
        let mut subscriptions = smallvec![];

        subscriptions.push(
            // Observe and automatically save settings on changes
            cx.observe(&inner, |this, _inner, cx| {
                this.save(cx);
                cx.notify();
            }),
        );

        // Run at the end of current cycle
        cx.defer_in(window, |this, window, cx| {
            this.load(window, cx);
        });

        Self {
            inner,
            _subscriptions: subscriptions,
        }
    }

    /// Update settings
    fn set_settings(&mut self, settings: Settings, cx: &mut Context<Self>) {
        self.inner.update(cx, |this, cx| {
            *this = settings;
            cx.notify();
        });
    }

    /// Load settings
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let task: Task<Result<Settings, Error>> = cx.background_spawn(async move {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let path = config_dir().join(".settings");
                if let Ok(content) = smol::fs::read_to_string(&path).await {
                    return Ok(serde_json::from_str(&content)?);
                }
            }
            Err(anyhow!("Not found"))
        });

        cx.spawn_in(window, async move |this, cx| {
            let settings = task.await.unwrap_or(Settings::default());

            // Update settings
            this.update_in(cx, |this, window, cx| {
                this.set_settings(settings, cx);
                this.apply_theme(window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Save settings
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let settings = self.inner.read(cx);
        if let Ok(content) = serde_json::to_string(&settings) {
            #[cfg(not(target_arch = "wasm32"))]
            cx.background_spawn(async move {
                let path = config_dir().join(".settings");
                smol::fs::write(&path, content).await.ok();
            })
            .detach();
        }
    }

    /// Apply the fixed light/dark palette using the saved appearance preference.
    pub fn apply_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.inner.read(cx).appearance.resolve(window.appearance());
        Theme::change(mode, Some(window), cx);
    }

    /// Check if decoupling encryption key is enabled
    pub fn is_nip4e_enabled(&self, cx: &App) -> bool {
        self.inner.read(cx).nip4e
    }

    /// Check if the given relay is already authenticated
    pub fn trusted_relay(&self, url: &RelayUrl, cx: &App) -> bool {
        self.inner
            .read(cx)
            .trusted_relays
            .iter()
            .any(|relay| relay == url.as_str_without_trailing_slash())
    }

    /// Add a relay to the trusted list
    pub fn add_trusted_relay(&mut self, url: &RelayUrl, cx: &mut Context<Self>) {
        self.inner.update(cx, |this, cx| {
            if !this
                .trusted_relays
                .iter()
                .any(|relay| relay == url.as_str_without_trailing_slash())
            {
                this.trusted_relays
                    .push(url.as_str_without_trailing_slash().to_string());
                cx.notify();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_keep_preferences_and_enable_markdown() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().remove("render_markdown");
        value["hide_avatar"] = true.into();
        let settings: Settings = serde_json::from_value(value).unwrap();
        assert!(settings.render_markdown);
        assert!(settings.hide_avatar);
    }

    #[test]
    fn markdown_opt_out_survives_serialization() {
        let settings = Settings {
            render_markdown: false,
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let restored: Settings = serde_json::from_str(&json).unwrap();
        assert!(!restored.render_markdown);
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::*;
    #[test]
    fn legacy_theme_settings_migrate_without_losing_other_preferences() {
        let mut json = serde_json::to_value(Settings::default()).unwrap();
        let object = json.as_object_mut().unwrap();
        object.remove("appearance");
        object.insert("theme".into(), serde_json::json!("themes/forest.json"));
        object.insert("theme_mode".into(), serde_json::json!("Dark"));
        object.insert("hide_avatar".into(), serde_json::json!(true));
        let settings: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(settings.appearance, Appearance::System);
        assert!(settings.hide_avatar);
        let saved = serde_json::to_value(settings).unwrap();
        assert!(saved.get("theme").is_none());
        assert!(saved.get("theme_mode").is_none());
    }
    #[test]
    fn manual_appearance_round_trips() {
        let mut settings = Settings::default();
        settings.appearance = Appearance::Dark;
        let saved = serde_json::to_string(&settings).unwrap();
        assert_eq!(
            serde_json::from_str::<Settings>(&saved).unwrap().appearance,
            Appearance::Dark
        );
    }
}
