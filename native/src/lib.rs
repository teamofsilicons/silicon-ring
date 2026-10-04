#[cfg(desktop)]
use tauri::{menu::{Menu, MenuItem}, tray::TrayIconBuilder, Manager};
#[cfg(desktop)]
use tauri_plugin_deep_link::DeepLinkExt;

#[cfg(desktop)]
#[tauri::command]
async fn save_recording(app: tauri::AppHandle, request: tauri::ipc::Request<'_>) -> Result<String, String> {
    let filename = request.headers().get("x-ring-filename").and_then(|value| value.to_str().ok()).ok_or("A recording filename is required.")?;
    let stem = filename.strip_suffix(".wav").filter(|stem| stem.starts_with("ring_") && stem.len() <= 128 && stem.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))).ok_or("The recording filename is invalid.")?.to_owned();
    let bytes = match request.body() { tauri::ipc::InvokeBody::Raw(bytes) if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" => bytes.clone(), _ => return Err("A WAV recording is required.".into()) };
    let directory = app.path().download_dir().map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || save_recording_file(&directory, &stem, &bytes)).await.map_err(|error| error.to_string())?
}

#[cfg(desktop)]
fn save_recording_file(directory: &std::path::Path, stem: &str, bytes: &[u8]) -> Result<String, String> {
    use std::io::Write;
    for suffix in 0..1000 {
        let filename = if suffix == 0 { format!("{stem}.wav") } else { format!("{stem} ({suffix}).wav") };
        let path = directory.join(filename);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_data()) { let _ = std::fs::remove_file(&path); return Err(error.to_string()); }
                return Ok(path.to_string_lossy().into_owned());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("Too many copies of this recording are already in Downloads.".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_call_service::init());
    #[cfg(desktop)]
    let builder = builder.invoke_handler(tauri::generate_handler![save_recording]);
    builder
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
