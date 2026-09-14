use gpui::*;
use ui::Root;
use universal_time::{Instant, MonotonicClock, SystemTime, WallClock, define_time_provider};
use wasm_bindgen::prelude::*;

struct CustomTimeProvider;

impl WallClock for CustomTimeProvider {
    fn system_time(&self) -> SystemTime {
        SystemTime::from_unix_duration(instant::Duration::from_secs(0))
    }
}

impl MonotonicClock for CustomTimeProvider {
    fn instant(&self) -> Instant {
        Instant::from_ticks(instant::Duration::from_secs(0))
    }
}

define_time_provider!(CustomTimeProvider);

#[wasm_bindgen]
pub fn run() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    // Initialize logging to browser console
    console_log::init_with_level(log::Level::Info).expect("Failed to initialize logger");

    // Also initialize tracing for WASM
    tracing_wasm::set_as_global_default();

    #[cfg(target_family = "wasm")]
    gpui_platform::web_init();

    #[cfg(not(target_family = "wasm"))]
    let app = gpui_platform::application();

    #[cfg(target_family = "wasm")]
    let app = {
        let app = gpui_platform::single_threaded_web();

        // Temporary fix: intentionally leak the `Rc<AppCell>` to keep the application alive
        struct WasmApplication(std::rc::Rc<AppCell>);
        let wasm_app = unsafe { std::mem::transmute::<Application, WasmApplication>(app) };
        std::mem::forget(wasm_app.0.clone());
        unsafe { std::mem::transmute::<WasmApplication, Application>(wasm_app) }
    };

    app.run(|cx| {
        // Open the root window
        cx.open_window(WindowOptions::default(), |window, cx| {
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

            // Root view
            cx.new(|cx| Root::new(workspace::init(window, cx).into(), window, cx))
        })
        .expect("Failed to open window. Please restart the application.");

        cx.activate(true);
    });

    Ok(())
}
