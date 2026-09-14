use std::sync::{Arc, Mutex};

use assets::Assets;
use gpui::{
    App, AppContext, Bounds, KeyBinding, SharedString, TitlebarOptions, WindowBackgroundAppearance,
    WindowBounds, WindowDecorations, WindowKind, WindowOptions, actions, point, px, size,
};
use gpui_platform::application;
use state::{APP_ID, CLIENT_NAME};
use ui::Root;

actions!(goop, [Quit]);

mod menus;

fn main() {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Run application
    application()
        .with_assets(Assets)
        .with_http_client(Arc::new(reqwest_client::ReqwestClient::new()))
        .run(move |cx| {
            // Load embedded fonts in assets/fonts
            load_embedded_fonts(cx);

            // Set app identity
            cx.set_app_identity(APP_ID, CLIENT_NAME);

            // Register the `quit` function
            cx.on_action(quit);

            // Register the `quit` function with CMD+Q (macOS)
            #[cfg(target_os = "macos")]
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);

            // Register the `quit` function with Super+Q (others)
            #[cfg(not(target_os = "macos"))]
            cx.bind_keys([KeyBinding::new("super-q", Quit, None)]);

            // Set up the window bounds
            let bounds = Bounds::centered(None, size(px(960.0), px(720.0)), cx);

            // Set up the window options
            let opts = WindowOptions {
                window_background: WindowBackgroundAppearance::Opaque,
                window_decorations: Some(WindowDecorations::Client),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                kind: WindowKind::Normal,
                app_id: Some(APP_ID.to_owned()),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::new_static(CLIENT_NAME)),
                    traffic_light_position: Some(point(px(9.0), px(9.0))),
                    appears_transparent: true,
                }),
                ..Default::default()
            };

            // Open a window with default options
            cx.open_window(opts, |window, cx| {
                // Initialize components
                ui::init(cx);

                // Initialize theme registry
                theme::init(cx);

                // Initialize settings
                settings::init(window, cx);

                // Initialize the nostr client
                state::init(window, cx);

                // Initialize person registry
                person::init(window, cx);

                // Initialize device signer
                //
                // NIP-4e: https://github.com/nostr-protocol/nips/blob/per-device-keys/4e.md
                device::init(window, cx);

                // Initialize app registry
                chat::init(window, cx);

                // Initialize auto update
                auto_update::init(window, cx);

                // Root view
                cx.new(|cx| Root::new(workspace::init(window, cx).into(), window, cx))
            })
            .expect("Failed to open window. Please restart the application.");

            menus::init(cx);

            // Bring the app to the foreground
            cx.activate(true);
        });
}

fn load_embedded_fonts(cx: &App) {
    let asset_source = cx.asset_source();
    let font_paths = asset_source.list("fonts").unwrap();
    let embedded_fonts = Mutex::new(vec![]);
    let executor = cx.background_executor();

    cx.foreground_executor().block_on(executor.scoped(|scope| {
        for font_path in &font_paths {
            if !font_path.ends_with(".ttf") {
                continue;
            }

            scope.spawn(async {
                let font_bytes = asset_source.load(font_path.as_str()).unwrap().unwrap();
                embedded_fonts.lock().unwrap().push(font_bytes);
            });
        }
    }));

    cx.text_system()
        .add_fonts(embedded_fonts.into_inner().unwrap())
        .unwrap();
}

fn quit(_ev: &Quit, cx: &mut App) {
    log::info!("Gracefully quitting the application . . .");
    cx.quit();
}
