#[cfg(desktop)]
use tauri::{menu::{Menu, MenuItem}, tray::TrayIconBuilder, Manager};
#[cfg(desktop)]
use tauri_plugin_deep_link::DeepLinkExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_call_service::init())
        .setup(|_app| {
            #[cfg(desktop)] let app = _app;
            #[cfg(desktop)]
            {
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    if event.urls().iter().any(|url| url.scheme() == "silicon-ring" && url.host_str() == Some("login") && url.path() == "/callback") {
                        if let Some(window) = handle.get_webview_window("main") {
                            let _ = window.show(); let _ = window.unminimize(); let _ = window.set_focus();
                        }
                    }
                });
                let show = MenuItem::with_id(app, "show", "Open Ring", true, None::<&str>)?;
                let quit = MenuItem::with_id(app, "quit", "Quit Ring", true, None::<&str>)?;
                let menu = Menu::with_items(app, &[&show, &quit])?;
                TrayIconBuilder::new()
                    .icon(app.default_window_icon().expect("bundled application icon").clone())
                    .tooltip("Silicon Ring")
                    .menu(&menu)
                    .on_menu_event(|app, event| match event.id.as_ref() {
                        "show" => if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show(); let _ = window.set_focus();
                        },
                        "quit" => app.exit(0),
                        _ => {},
                    })
                    .build(app)?;
            }
            Ok(())
        })
        .on_window_event(|_window, _event| {
            #[cfg(desktop)]
            if let tauri::WindowEvent::CloseRequested { api, .. } = _event {
                api.prevent_close();
                let _ = _window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("Silicon Ring could not start");
}
