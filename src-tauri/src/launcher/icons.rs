//! App icon extraction for the launcher's result list. macOS app icons live
//! as `.icns` files inside the bundle; converted to PNG via `sips` (built
//! into macOS, same "shell out to the OS tool" approach as `mdfind`/`open`)
//! and cached on disk under the app's data dir — each app's icon is only
//! ever extracted once, not on every search.

use base64::{engine::general_purpose::STANDARD, Engine};
use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;
#[cfg(not(windows))]
use std::process::{Command, Stdio};

/// Returns a `data:image/png;base64,...` URL for `app_path`'s icon, or
/// `None` if it can't be found/converted (the frontend just shows no icon).
pub fn icon_data_url(app_path: &str, cache_dir: &Path) -> Option<String> {
    let cache_path = cache_dir.join(cache_key(app_path));

    if !cache_path.exists() {
        extract_icon(app_path, &cache_path)?;
    }

    let bytes = std::fs::read(&cache_path).ok()?;
    Some(format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
}

/// Stable, filesystem-safe cache filename for an app path — content doesn't
/// matter, only that the same app always maps to the same file.
fn cache_key(app_path: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for byte in app_path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}.png")
}

#[cfg(not(windows))]
fn extract_icon(app_path: &str, out_path: &Path) -> Option<()> {
    let icon_file = bundle_icon_file(app_path)?;
    let icns_path = PathBuf::from(app_path)
        .join("Contents/Resources")
        .join(&icon_file);
    if !icns_path.exists() {
        return None;
    }

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }

    let status = Command::new("/usr/bin/sips")
        .arg("-s")
        .arg("format")
        .arg("png")
        .arg(&icns_path)
        .arg("--out")
        .arg(out_path)
        .arg("--resampleHeightWidthMax")
        .arg("64")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;

    status.success().then_some(())
}

/// Reads `CFBundleIconFile` out of the app's Info.plist via `defaults` —
/// avoids pulling in a plist-parsing crate for one field.
#[cfg(not(windows))]
fn bundle_icon_file(app_path: &str) -> Option<String> {
    let info_plist = format!("{app_path}/Contents/Info");
    let output = Command::new("/usr/bin/defaults")
        .arg("read")
        .arg(&info_plist)
        .arg("CFBundleIconFile")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        return None;
    }
    Some(if name.ends_with(".icns") {
        name
    } else {
        format!("{name}.icns")
    })
}

/// Windows reads an app's icon through the shell, the same icon Explorer
/// shows, and Samlu writes it out as a PNG. Store apps (`shell:AppsFolder`)
/// have no file to read from and go without.
#[cfg(windows)]
fn extract_icon(app_path: &str, out_path: &Path) -> Option<()> {
    if app_path.starts_with("shell:") {
        return None;
    }
    let (width, height, rgba) = crate::platform::file_icon_rgba(app_path)?;
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    std::fs::write(out_path, png(width, height, &rgba)).ok()
}

/// A minimal PNG: one IDAT of unfiltered RGBA rows.
#[cfg(windows)]
fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let row = width as usize * 4;
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for line in rgba.chunks(row) {
        raw.push(0);
        raw.extend_from_slice(line);
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = encoder.write_all(&raw);
    let data = encoder.finish().unwrap_or_default();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    // 8-bit RGBA, deflate, no filter, no interlace.
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &data);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(windows)]
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = 0xFFFF_FFFFu32;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    out.extend_from_slice(&(!crc).to_be_bytes());
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn pngs_have_the_standard_chunks() {
        let image = png(2, 1, &[255, 0, 0, 255, 0, 0, 255, 128]);
        assert_eq!(&image[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&image[12..16], b"IHDR");
        assert_eq!(&image[16..24], &[0, 0, 0, 2, 0, 0, 0, 1]);
        // IEND with no data always ends in the same checksum.
        assert_eq!(
            &image[image.len() - 8..],
            &[b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]
        );
    }

    #[test]
    fn explorer_has_an_icon() {
        let explorer = crate::platform::expand_variables(r"%SystemRoot%\explorer.exe");
        let cache = std::env::temp_dir().join(format!("samlu-icons-{}", std::process::id()));
        let url = icon_data_url(&explorer, &cache).expect("an icon for explorer.exe");
        assert!(url.starts_with("data:image/png;base64,"));
        assert!(icon_data_url(r"shell:AppsFolder\Some.App!App", &cache).is_none());
        let _ = std::fs::remove_dir_all(cache);
    }
}
