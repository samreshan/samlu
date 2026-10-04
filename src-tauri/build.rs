use std::path::PathBuf;
use std::process::Command;

/// Compiles the SpeechAnalyzer bridge with `swiftc` for the target slice.
/// Speech and Swift concurrency end up weakly linked (the deployment target
/// is macOS 11), and `/usr/lib/swift` is added as an rpath so macOS 26 finds
/// `libswift_Concurrency.dylib` at runtime.
fn build_swift_bridge() {
    let source = "native/SamluSpeech.swift";
    println!("cargo:rerun-if-changed={source}");
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
        .arg(source)
        .arg("-o")
        .arg(out_dir.join("libsamlu_speech.a"))
        .status()
        .expect("failed to run swiftc (install Xcode 26 or its command line tools)");
    assert!(status.success(), "swiftc failed to compile {source}");

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
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}

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
    build_swift_bridge();
    tauri_build::build()
}
