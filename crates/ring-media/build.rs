fn main() {
    // libwebrtc relies on Objective-C categories on Apple platforms. The linker only keeps them
    // with -ObjC; without it a libwebrtc static initializer throws before main().
    if std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple") {
        println!("cargo:rustc-link-arg=-ObjC");
    }
}
