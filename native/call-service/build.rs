fn main() {
    tauri_plugin::Builder::new(&["configure", "control"])
        .android_path("android").ios_path("ios").build();
}
