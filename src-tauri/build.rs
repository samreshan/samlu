use std::path::PathBuf;
use std::process::Command;

/// Compiles the SpeechAnalyzer and Core ML bridges with `swiftc` for the
/// target slice. Speech, Core ML's newer API and Swift concurrency end up
/// weakly linked (the deployment target is macOS 11), and `/usr/lib/swift` is
/// added as an rpath so macOS 26 finds `libswift_Concurrency.dylib` at
/// runtime.
fn build_swift_bridge() {
    let sources = ["native/SamluSpeech.swift", "native/SamluBark.swift"];
    for source in sources {
        println!("cargo:rerun-if-changed={source}");
    }
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        other => panic!("unsupported macOS architecture: {other}"),
    };
    let status = Command::new("xcrun")
        .args([
            "swiftc",
            "-parse-as-library",
            "-emit-library",
            "-static",
            "-O",
        ])
        .args(["-module-name", "SamluSpeech"])
        .args(["-target", &format!("{arch}-apple-macos11.0")])
        .args(sources)
        .arg("-o")
        .arg(out_dir.join("libsamlu_speech.a"))
        .status()
        .expect("failed to run swiftc (install Xcode 26 or its command line tools)");
    assert!(
        status.success(),
        "swiftc failed to compile the Swift bridges"
    );

    let swiftc = Command::new("xcrun")
        .args(["--find", "swiftc"])
        .output()
        .expect("failed to locate swiftc");
    let swiftc = PathBuf::from(String::from_utf8(swiftc.stdout).unwrap().trim());
    let toolchain_lib = swiftc
        .parent()
        .and_then(|bin| bin.parent())
        .unwrap()
        .join("lib/swift/macosx");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=samlu_speech");
    println!("cargo:rustc-link-search=native={}", toolchain_lib.display());
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    println!("cargo:rustc-link-lib=framework=Speech");
    println!("cargo:rustc-link-lib=framework=CoreML");
    println!("cargo:rustc-link-lib=framework=Accelerate");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}

/// Compiles the Objective-C helpers and the Swift speech bridge. Only macOS
/// builds need them; every other platform has its own implementations in Rust.
fn build_macos_native() {
    println!("cargo:rerun-if-changed=native/color_sampler.m");
    println!("cargo:rerun-if-changed=native/modifier_keys.m");
    println!("cargo:rerun-if-changed=native/permissions.m");
    println!("cargo:rerun-if-changed=native/signing.m");
    cc::Build::new()
        .file("native/color_sampler.m")
        .file("native/modifier_keys.m")
        .file("native/permissions.m")
        .file("native/signing.m")
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .compile("samlu_native");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=Security");
    build_swift_bridge();
}

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_macos_native();
    }
    tauri_build::build()
}
