fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("apple-darwin") {
        build_macos_microphone_permission();
    }
}

fn build_macos_microphone_permission() {
    println!("cargo:rerun-if-changed=src/macos_microphone_permission.m");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=Foundation");
    cc::Build::new()
        .file("src/macos_microphone_permission.m")
        .flag("-fblocks")
        .compile("dicta_audio_macos_microphone_permission");
}
