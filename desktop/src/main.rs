#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};

use assets::Assets;
use gpui::{
    App, AppContext, Bounds, KeyBinding, SharedString, TitlebarOptions, WindowBackgroundAppearance,
    WindowBounds, WindowDecorations, WindowKind, WindowOptions, actions, point, px, size,
};
use gpui_platform::application;
use state::{APP_ID, CLIENT_NAME};
use ui::{Root, WindowExtension, notification::Notification};

actions!(goop, [Quit]);

#[derive(Default)]
struct QuitPending(bool);
impl gpui::Global for QuitPending {}

mod menus;

fn main() {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    if workspace::qr_camera::run_if_requested() { return; }
    let _crash_guard = common::crash_report::install(env!("CARGO_PKG_VERSION"), workspace::build_revision());
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
            cx.set_global(QuitPending::default());
            cx.on_action(quit);
            init_persistence(cx);

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

fn save_error(message: String, cx: &mut App) {
    log::error!("{message}");
    for handle in cx.windows() {
        let _ = handle.update(cx, |_, window, cx| {
            window.push_notification(Notification::error(message.clone()).autohide(false), cx);
        });
    }
}

fn init_persistence(cx: &mut App) {
    // GPUI gives quit futures only 200 ms. Normal Quit flushes asynchronously
    // below; this synchronous barrier also covers platform-initiated shutdown.
    // Disk operations still run on the dedicated writer, never on this thread.
    cx.on_app_quit(|_| {
        if let Err(error) = common::persistence::global().shutdown_blocking() {
            log::error!("Could not finish saving local state during shutdown: {error}");
        }
        async {}
    }).detach();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
            let errors = common::persistence::global().take_errors();
            if errors.is_empty() { continue; }
            cx.update(|cx| {
                for error in errors {
                    save_error(format!("Could not save local state: {error}. Changes remain in memory; Goop will retry."), cx);
                }
            });
        }
    }).detach();
}

fn quit(_ev: &Quit, cx: &mut App) {
    if cx.global::<QuitPending>().0 { return; }
    cx.global_mut::<QuitPending>().0 = true;
    cx.spawn(async move |cx| {
        let result = common::persistence::global().flush().await;
        cx.update(|cx| {
            cx.global_mut::<QuitPending>().0 = false;
            match result {
                Ok(()) => cx.quit(),
                Err(error) => save_error(format!("Could not save local state: {error}. Goop stayed open. Try quitting again after resolving the storage error."), cx),
            }
        });
    }).detach();
}
