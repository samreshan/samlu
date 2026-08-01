fn main() {
    println!("cargo:rerun-if-changed=native/color_sampler.m");
    println!("cargo:rerun-if-changed=native/permissions.m");
    cc::Build::new()
        .file("native/color_sampler.m")
        .file("native/permissions.m")
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .compile("samlu_native");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    tauri_build::build()
}
