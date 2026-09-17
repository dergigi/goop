use anyhow::Error;
use common::config_dir;
use gpui::{App, AppContext, Context, Entity, Global, Subscription, Task, Window};
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};
use theme::{Appearance, Theme};

pub mod emoji_history;

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
    pub render_markdown: bool,
    pub screening: bool,
    pub auto_block_reports: bool,
    pub trusted_relays: Vec<String>,
    pub file_server: Url,
}

/// Room configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct RoomConfig {
    backup: bool,
}

impl RoomConfig {
    pub fn new() -> Self {
        Self {
            backup: true,
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

}

/// Settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub emoji_history: emoji_history::EmojiHistory,

    /// Missing in legacy settings: migrate to following the system.
    #[serde(default)]
    pub appearance: Appearance,

    /// Render Markdown formatting in chat bubbles
    #[serde(default = "default_render_markdown")]
    pub render_markdown: bool,

    /// Enable screening for unknown chat requests
    pub screening: bool,

    /// Block a reported user after a relay acknowledges the report.
    #[serde(default = "default_auto_block_reports")]
    pub auto_block_reports: bool,

    /// Trusted relays; Goop will automatically authenticate with these relays
    pub trusted_relays: Vec<String>,

    /// Server for blossom media attachments
    pub file_server: Url,
}

fn default_auto_block_reports() -> bool { true }

fn default_render_markdown() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            emoji_history: Default::default(),
            appearance: Appearance::System,
            render_markdown: default_render_markdown(),
            screening: true,
            auto_block_reports: default_auto_block_reports(),
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
    pub fn emoji_history(cx: &App) -> emoji_history::EmojiHistory {
        cx.try_global::<GlobalAppSettings>()
            .map(|settings| settings.0.read(cx).inner.read(cx).emoji_history.clone())
            .unwrap_or_default()
    }

    pub fn record_emoji(emoji: &str, reaction: bool, cx: &mut App) {
        let Some(settings) = cx.try_global::<GlobalAppSettings>().map(|s| s.0.clone()) else { return; };
        settings.update(cx, |settings, cx| {
            settings.inner.update(cx, |inner, cx| {
                inner.emoji_history.record(emoji, reaction);
                cx.notify();
            });
        });
    }

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
            Ok(common::persistence::global().load(&config_dir().join(".settings"))?)
        });

        cx.spawn_in(window, async move |this, cx| {
            let settings = task.await;

            // Update settings
            this.update_in(cx, |this, window, cx| {
                match settings {
                    Ok(settings) => this.set_settings(settings, cx),
                    Err(error) => log::error!("Could not load settings; keeping the existing file: {error}"),
                }
                this.apply_theme(window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Save settings
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let settings = self.inner.read(cx).clone();
        if let Err(error) = common::persistence::global().save(&config_dir().join(".settings"), settings) {
            log::error!("Could not queue settings save: {error}");
        }
    }

    /// Apply the fixed light/dark palette using the saved appearance preference.
    pub fn apply_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.inner.read(cx).appearance.resolve(window.appearance());
        Theme::change(mode, Some(window), cx);
    }

    /// Check if decoupling encryption key is enabled

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
    fn legacy_experiments_do_not_change_remaining_preferences() {
        let mut saved = serde_json::to_value(Settings::default()).unwrap();
        saved["nip4e"] = true.into();
        saved["hide_avatar"] = true.into();
        let loaded: Settings = serde_json::from_value(saved).unwrap();
        assert!(serde_json::to_value(&loaded).unwrap().get("hide_avatar").is_none());
        assert!(serde_json::to_value(loaded).unwrap().get("nip4e").is_none());
        for kind in ["Auto", "Encryption", "User"] {
            let config: RoomConfig = serde_json::from_value(serde_json::json!({"backup": false, "signer_kind": kind})).unwrap();
            assert!(!config.backup());
            assert_eq!(serde_json::to_value(config).unwrap(), serde_json::json!({"backup": false}));
        }
    }

    #[test]
    fn auto_block_defaults_on_for_existing_settings_and_preserves_opt_out() {
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("auto_block_reports");
        legacy["screening"] = false.into();
        let migrated: Settings = serde_json::from_value(legacy).unwrap();
        assert!(migrated.auto_block_reports);
        assert!(!migrated.screening);
        let opted_out = Settings { auto_block_reports: false, ..migrated };
        let restored: Settings = serde_json::from_str(&serde_json::to_string(&opted_out).unwrap()).unwrap();
        assert!(!restored.auto_block_reports);
    }

    #[test]
    fn old_settings_keep_preferences_and_enable_markdown() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().remove("render_markdown");
        value["screening"] = false.into();
        let settings: Settings = serde_json::from_value(value).unwrap();
        assert!(settings.render_markdown);
        assert!(!settings.screening);
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
        object.insert("screening".into(), serde_json::json!(false));
        let settings: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(settings.appearance, Appearance::System);
        assert!(!settings.screening);
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
