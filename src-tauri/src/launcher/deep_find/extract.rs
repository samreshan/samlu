//! Reads a file's text and cuts it into passages: plain text and code
//! directly, HTML and RTF by stripping their markup, DOCX and ODT from their
//! XML, PDFs page by page. Pure Rust, so it reads the same on macOS and
//! Windows.

use super::rules::{Kind, MAX_DOCUMENT_BYTES, MAX_TEXT_BYTES};
use super::text;
use std::io::Read;
use std::path::Path;

/// A long book or a big log is read this far; the rest stays findable by
/// name only.
const MAX_PASSAGES: usize = 600;
const MAX_PDF_PAGES: usize = 2000;

/// A passage of a file and where it is: the line it starts on for text and
/// code, the page for a PDF.
pub struct Passage {
    pub text: String,
    pub loc: Option<u32>,
}

pub fn extract(path: &Path, kind: Kind, size: u64) -> Result<Vec<Passage>, String> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    match (kind, ext.as_str()) {
        (Kind::Pdf, _) => pdf(path, size),
        (_, "docx") => office(path, size, "word/document.xml", &["w:p", "w:tr"]),
        (_, "odt") => office(path, size, "content.xml", &["text:p", "text:h"]),
        (_, "html" | "htm") => Ok(unlocated(&strip_html(&read_text(path, size)?))),
        (_, "rtf") => Ok(unlocated(&strip_rtf(&read_text(path, size)?))),
        _ => Ok(located(&read_text(path, size)?)),
    }
}

/// Passages that remember their line.
fn located(text: &str) -> Vec<Passage> {
    text::chunks(text, MAX_PASSAGES)
        .into_iter()
        .map(|chunk| Passage {
            text: chunk.text,
            loc: Some(chunk.line as u32),
        })
        .collect()
}

/// Passages of text whose lines don't match the file's (markup stripped).
fn unlocated(text: &str) -> Vec<Passage> {
    text::chunks(text, MAX_PASSAGES)
        .into_iter()
        .map(|chunk| Passage {
            text: chunk.text,
            loc: None,
        })
        .collect()
}

/// UTF-8 (with a BOM or without), UTF-16 with a BOM, or Latin-1. A file with
/// NUL bytes in its first 8 KB is binary and refused.
fn read_text(path: &Path, size: u64) -> Result<String, String> {
    if size > MAX_TEXT_BYTES {
        return Err("too large to read as text".into());
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return Ok(String::from_utf8_lossy(rest).into_owned());
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let little = bytes[0] == 0xFF;
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                if little {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect();
        return Ok(String::from_utf16_lossy(&units));
    }
    if bytes.iter().take(8192).any(|&byte| byte == 0) {
        return Err("binary file".into());
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(error) => Ok(error.into_bytes().iter().map(|&b| b as char).collect()),
    }
}

/// DOCX and ODT are zip files holding the text as XML. Each paragraph (and
/// table row) ends a line.
fn office(path: &Path, size: u64, entry: &str, blocks: &[&str]) -> Result<Vec<Passage>, String> {
    if size > MAX_DOCUMENT_BYTES {
        return Err("too large".into());
    }
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    let mut xml = String::new();
    archive
        .by_name(entry)
        .map_err(|error| error.to_string())?
        .take(MAX_TEXT_BYTES * 4)
        .read_to_string(&mut xml)
        .map_err(|error| error.to_string())?;
    Ok(unlocated(&xml_text(&xml, blocks)))
}

/// The text of an XML document: tags dropped, the end of each `blocks`
/// element a line break, tabs and breaks kept, entities decoded.
fn xml_text(xml: &str, blocks: &[&str]) -> String {
    let mut out = String::with_capacity(xml.len() / 4);
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..open]));
        let Some(close) = rest[open..].find('>') else {
            break;
        };
        let tag = &rest[open + 1..open + close];
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        if tag.starts_with('/') && blocks.contains(&name) {
            out.push('\n');
        } else if matches!(name, "w:tab" | "text:tab") {
            out.push('\t');
        } else if matches!(name, "w:br" | "w:cr" | "text:line-break") {
            out.push('\n');
        } else if name == "text:s" {
            out.push(' ');
        }
        rest = &rest[open + close + 1..];
    }
    out
}

/// Good enough for search: drops scripts, styles, comments and the head,
/// turns block ends into line breaks and strips the remaining tags.
pub fn strip_html(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len() / 2);
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < html.len() {
        if bytes[i] != b'<' {
            let next = html[i..].find('<').map_or(html.len(), |offset| i + offset);
            out.push_str(&decode_entities(&html[i..next]));
            i = next;
            continue;
        }
        if lower[i..].starts_with("<!--") {
            i = lower[i..].find("-->").map_or(html.len(), |end| i + end + 3);
            continue;
        }
        let skipped = ["script", "style", "head", "noscript", "svg"]
            .into_iter()
            .find(|name| {
                lower[i + 1..].starts_with(name)
                    && lower[i + 1 + name.len()..]
                        .starts_with(|c: char| c == '>' || c.is_whitespace())
            });
        if let Some(name) = skipped {
            let end = format!("</{name}");
            i = lower[i..].find(&end).map_or(html.len(), |offset| {
                let close = i + offset;
                lower[close..]
                    .find('>')
                    .map_or(html.len(), |gt| close + gt + 1)
            });
            out.push(' ');
            continue;
        }
        let end = lower[i..].find('>').map_or(html.len(), |gt| i + gt + 1);
        let tag = &lower[i..end];
        let block = [
            "<br",
            "</p",
            "</div",
            "</li",
            "</h1",
            "</h2",
            "</h3",
            "</h4",
            "</h5",
            "</h6",
            "</tr",
            "</section",
            "</article",
            "</blockquote",
            "</pre",
        ]
        .iter()
        .any(|prefix| tag.starts_with(prefix));
        out.push(if block { '\n' } else { ' ' });
        i = end;
    }
    out
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        let Some(semi) = after.find(';').filter(|&semi| semi <= 10) else {
            out.push('&');
            rest = &after[1..];
            continue;
        };
        let entity = &after[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &after[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// RTF's text: control words dropped, destinations like font tables and
/// pictures skipped, `\par` a line break, `\'hh` and `\uN` decoded.
pub fn strip_rtf(rtf: &str) -> String {
    let chars: Vec<char> = rtf.chars().collect();
    let mut out = String::new();
    // For each open group, whether its text is skipped.
    let mut skip_stack: Vec<bool> = vec![false];
    let mut i = 0;
    // Characters to drop after a \uN (its ANSI fallback).
    let mut fallback = 0usize;
    while i < chars.len() {
        let skipping = *skip_stack.last().unwrap_or(&false);
        match chars[i] {
            '{' => {
                skip_stack.push(skipping);
                i += 1;
            }
            '}' => {
                if skip_stack.len() > 1 {
                    skip_stack.pop();
                }
                i += 1;
            }
            '\\' => {
                i += 1;
                let Some(&next) = chars.get(i) else { break };
                if next == '*' {
                    if let Some(top) = skip_stack.last_mut() {
                        *top = true;
                    }
                    i += 1;
                } else if next == '\'' {
                    let hex: String = chars.iter().skip(i + 1).take(2).collect();
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        if !skipping && fallback == 0 {
                            out.push(byte as char);
                        }
                    }
                    fallback = fallback.saturating_sub(1);
                    i += 3;
                } else if next.is_ascii_alphabetic() {
                    let start = i;
                    while i < chars.len() && chars[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word: String = chars[start..i].iter().collect();
                    let num_start = i;
                    if i < chars.len() && (chars[i] == '-' || chars[i].is_ascii_digit()) {
                        i += 1;
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    let number: Option<i32> =
                        chars[num_start..i].iter().collect::<String>().parse().ok();
                    if i < chars.len() && chars[i] == ' ' {
                        i += 1;
                    }
                    match word.as_str() {
                        "fonttbl" | "colortbl" | "stylesheet" | "info" | "pict" | "header"
                        | "footer" | "listtable" | "listoverridetable" | "themedata"
                        | "datastore" | "latentstyles" | "rsidtbl" | "generator" => {
                            if let Some(top) = skip_stack.last_mut() {
                                *top = true;
                            }
                        }
                        "par" | "line" | "row" | "sect" | "page" if !skipping => out.push('\n'),
                        "tab" | "cell" if !skipping => out.push('\t'),
                        "u" => {
                            if let Some(code) = number {
                                let code = if code < 0 { code + 65536 } else { code } as u32;
                                if !skipping {
                                    if let Some(c) = char::from_u32(code) {
                                        out.push(c);
                                    }
                                }
                                fallback = 1;
                            }
                        }
                        _ => {}
                    }
                } else {
                    // An escaped symbol: \\ \{ \} and \~ (a no-break space).
                    if !skipping {
                        match next {
                            '~' => out.push(' '),
                            '\n' | '\r' => out.push('\n'),
                            '-' | '_' => {}
                            other => out.push(other),
                        }
                    }
                    i += 1;
                }
            }
            '\r' | '\n' => i += 1,
            c => {
                if !skipping {
                    if fallback > 0 {
                        fallback -= 1;
                    } else {
                        out.push(c);
                    }
                }
                i += 1;
            }
        }
    }
    out
}

/// PDF text page by page; each page is one passage, or several when it is
/// long. pdf-extract panics on some malformed files, so that is caught and
/// reported as unreadable.
fn pdf(path: &Path, size: u64) -> Result<Vec<Passage>, String> {
    if size > MAX_DOCUMENT_BYTES {
        return Err("too large".into());
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let pages = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem_by_pages(&bytes))
        .map_err(|_| "unreadable PDF".to_string())?
        .map_err(|error| error.to_string())?;
    let mut passages = Vec::new();
    for (index, page) in pages.iter().take(MAX_PDF_PAGES).enumerate() {
        for chunk in text::chunks(page, 8) {
            passages.push(Passage {
                text: chunk.text,
                loc: Some(index as u32 + 1),
            });
            if passages.len() >= MAX_PASSAGES {
                return Ok(passages);
            }
        }
    }
    Ok(passages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_keeps_text_and_drops_scripts() {
        let html = "<html><head><title>x</title></head><body><script>var a=1;</script>\
                    <h1>Lease</h1><p>Pets &amp; cats are welcome&#33;</p></body></html>";
        let text = strip_html(html);
        assert!(text.contains("Lease"));
        assert!(text.contains("Pets & cats are welcome!"));
        assert!(!text.contains("var a"));
    }

    #[test]
    fn rtf_keeps_text_and_drops_tables() {
        let rtf = r"{\rtf1\ansi{\fonttbl\f0\fswiss Helvetica;}\f0\pard The pet clause\par Caf\'e9 \u8364? price}";
        let text = strip_rtf(rtf);
        assert!(text.contains("The pet clause"));
        assert!(text.contains("Café"));
        assert!(text.contains("€ price"));
        assert!(!text.contains("Helvetica"));
    }

    #[test]
    fn office_xml_breaks_at_paragraphs() {
        let xml = r#"<w:document><w:body><w:p><w:r><w:t>First</w:t></w:r><w:r><w:tab/><w:t xml:space="preserve">para &amp; more</w:t></w:r></w:p><w:p><w:r><w:t>Second</w:t></w:r></w:p></w:body></w:document>"#;
        assert_eq!(xml_text(xml, &["w:p"]).trim(), "First\tpara & more\nSecond");
    }
}
