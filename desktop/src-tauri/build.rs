fn main() {
    // Sharing your Mac's sound uses Core Audio's process taps (src/share_sound.m).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/share_sound.m");
        cc::Build::new()
            .file("src/share_sound.m")
            .flag("-fobjc-arc")
            .flag("-Wno-unguarded-availability-new")
            .flag("-Wno-unused-parameter")
            .compile("share_sound");
        println!("cargo:rustc-link-lib=framework=CoreAudio");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
    tauri_build::build()
}
