use serde_json::Value;
#[cfg(desktop)]
use serde_json::json;
use tauri::{plugin::{Builder, TauriPlugin}, AppHandle, Runtime};
#[cfg(mobile)]
use tauri::{plugin::PluginHandle, Manager};
#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_call_service);

#[cfg(mobile)]
struct NativeCalls<R: Runtime>(PluginHandle<R>);

#[tauri::command]
async fn configure<R: Runtime>(app: AppHandle<R>, payload: Value) -> Result<Value, String> {
    #[cfg(mobile)]
    return app.state::<NativeCalls<R>>().0.run_mobile_plugin("configure", payload).map_err(|e| e.to_string());
    #[cfg(desktop)]
    { let _ = (app, payload); Ok(json!({"mobile": false, "native_audio": false})) }
}
#[tauri::command]
async fn control<R: Runtime>(app: AppHandle<R>, payload: Value) -> Result<Value, String> {
    let action = payload["action"].as_str().unwrap_or("");
    if !matches!(action, "start" | "stop" | "mute" | "logout" | "status" | "restore") { return Err("Unknown native call action".into()); }
    #[cfg(mobile)]
    return app.state::<NativeCalls<R>>().0.run_mobile_plugin("control", payload).map_err(|e| e.to_string());
    #[cfg(desktop)]
    { let _ = app; Ok(json!({"mobile": false})) }
}
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("call-service")
        .invoke_handler(tauri::generate_handler![configure, control])
        .setup(|app, api| {
            #[cfg(target_os = "ios")]
            app.manage(NativeCalls(api.register_ios_plugin(init_plugin_call_service)?));
            #[cfg(target_os = "android")]
            app.manage(NativeCalls(api.register_android_plugin("com.teamofsilicons.callservice", "CallServicePlugin")?));
            #[cfg(desktop)]
            let _ = (app, api);
            Ok(())
        }).build()
}
