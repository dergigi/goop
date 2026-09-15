use ui::button::{Button, ButtonVariants};
use ui::{Disableable, Sizable};

const REPOSITORY: &str = "https://github.com/dergigi/goop";

pub fn version_link() -> Button {
    Button::new("version-link")
        .label(format!("Goop {}", env!("CARGO_PKG_VERSION")))
        .xsmall().ghost()
        .tooltip("View release on GitHub")
        .on_click(|_, _, cx| {
            cx.open_url(&format!("{REPOSITORY}/releases/tag/v{}", env!("CARGO_PKG_VERSION")));
        })
}

pub fn build_link() -> Button {
    let known = env!("GOOP_BUILD_COMMIT") != "unknown";
    Button::new("build-link")
        .label(format!("Build {}", env!("GOOP_BUILD_REVISION")))
        .xsmall().ghost().disabled(!known)
        .tooltip(if known { "View commit on GitHub" } else { "Build revision unavailable" })
        .on_click(|_, _, cx| {
            cx.open_url(&format!("{REPOSITORY}/commit/{}", env!("GOOP_BUILD_COMMIT")));
        })
}
