//! What the detail pane may hand to Windows: attachment file names and link targets.
//!
//! Attachments and links come from email and imports, so they are untrusted. An attachment is saved by
//! default; only documents and images whose final name ends in an allowlisted extension may also be
//! opened, and every downloaded file carries the Mark of the Web (see `session`). Only `http` and
//! `https` links are clickable; anything else is shown as text.

/// Extensions an attachment may be opened with its app. Everything else can only be saved.
pub const OPENABLE: &[&str] = &[
    "pdf", "png", "jpg", "jpeg", "gif", "webp", "txt", "md", "csv", "docx", "xlsx", "pptx", "eml",
    "msg",
];

/// The longest file name created, in characters (well inside Windows' 255).
const MAX_NAME: usize = 120;
/// An extension longer than this isn't kept whole when a name is shortened.
const MAX_EXTENSION: usize = 16;

/// Characters that can make a name read differently from what it is: bidirectional overrides and
/// isolates (an override before `fdp.exe` makes `invoice` + it display as `invoiceexe.pdf`) and zero-width marks.
fn deceptive(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// A file name safe to create on Windows, and the one every decision is made on: no directories, no
/// reserved or deceptive characters, no trailing dots or spaces (Windows drops them, which would change
/// the extension), not a reserved device name, not empty, and at most [`MAX_NAME`] characters with the
/// extension kept when it is shortened.
pub fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut clean: String = base
        .chars()
        .filter(|c| !deceptive(*c))
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    clean = clean.trim().trim_end_matches(['.', ' ']).to_string();
    if clean.is_empty() || clean.chars().all(|c| c == '.') {
        return "attachment".into();
    }
    let stem = clean
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|n| {
            matches!(
                n,
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    };
    let reserved = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || numbered("COM")
        || numbered("LPT");
    if reserved {
        clean = format!("attachment-{clean}");
    }
    shorten(&clean)
}

/// Cuts a name to [`MAX_NAME`] characters, keeping a short extension whole.
fn shorten(name: &str) -> String {
    if name.chars().count() <= MAX_NAME {
        return name.to_string();
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e)
        .filter(|e| !e.is_empty() && e.chars().count() <= MAX_EXTENSION);
    match ext {
        Some(ext) => {
            let keep = MAX_NAME - ext.chars().count() - 1;
            let stem: String = name.chars().take(keep).collect();
            format!("{}.{ext}", stem.trim_end_matches(['.', ' ']))
        }
        None => name
            .chars()
            .take(MAX_NAME)
            .collect::<String>()
            .trim_end_matches(['.', ' '])
            .to_string(),
    }
}

/// Whether an attachment may be opened with its app (decided on the name it is saved under).
pub fn can_open(name: &str) -> bool {
    let name = safe_file_name(name);
    name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && OPENABLE.contains(&ext.to_ascii_lowercase().as_str())
    })
}

/// The address to open for a link, if it may be opened: an absolute `http` or `https` URL with a host,
/// normalised by the URL parser. `file:`, UNC paths, `javascript:`, custom protocol handlers and
/// anything that doesn't parse are shown but never opened.
pub fn openable_link(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw.trim()).ok()?;
    let web = matches!(url.scheme(), "http" | "https");
    (web && url.host_str().is_some_and(|h| !h.is_empty())).then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_normalised_before_anything_is_decided() {
        assert_eq!(safe_file_name("Report.PDF"), "Report.PDF");
        assert_eq!(safe_file_name("../../evil.txt"), "evil.txt");
        assert_eq!(safe_file_name(r"C:\Windows\x.txt"), "x.txt");
        assert_eq!(safe_file_name("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(
            safe_file_name("notes.txt:Zone.Identifier"),
            "notes.txt_Zone.Identifier"
        );
        // Windows drops trailing dots and spaces, so they never hide the real extension.
        assert_eq!(safe_file_name("x.exe. . "), "x.exe");
        assert_eq!(safe_file_name("  .. "), "attachment");
        assert_eq!(safe_file_name(""), "attachment");
        assert_eq!(safe_file_name(".."), "attachment");
        // Reserved device names, with or without an extension, in any case.
        assert_eq!(safe_file_name("CON"), "attachment-CON");
        assert_eq!(safe_file_name("nul.txt"), "attachment-nul.txt");
        assert_eq!(safe_file_name("Com1.pdf"), "attachment-Com1.pdf");
        assert_eq!(safe_file_name("lpt9"), "attachment-lpt9");
        assert_eq!(safe_file_name("com¹.txt"), "attachment-com¹.txt");
        assert_eq!(safe_file_name("conout$"), "attachment-conout$");
        assert_eq!(safe_file_name("COM10.txt"), "COM10.txt");
        assert_eq!(safe_file_name("console.txt"), "console.txt");
        // A right-to-left override can't disguise the extension.
        assert_eq!(safe_file_name("invoice\u{202E}fdp.exe"), "invoicefdp.exe");
    }

    #[test]
    fn long_names_keep_their_extension() {
        let long = format!("{}.pdf", "a".repeat(300));
        let short = safe_file_name(&long);
        assert_eq!(short.chars().count(), MAX_NAME);
        assert!(short.ends_with(".pdf"));
        // Shortening can't drop the real extension and leave an earlier one at the end.
        let sneaky = format!("{}.pdf{}.exe", "a".repeat(100), "b".repeat(100));
        let short = safe_file_name(&sneaky);
        assert!(short.ends_with(".exe"), "{short}");
        assert!(!can_open(&sneaky));
        let no_ext = "z".repeat(300);
        assert_eq!(safe_file_name(&no_ext).chars().count(), MAX_NAME);
    }

    #[test]
    fn only_documents_and_images_open() {
        for ok in [
            "a.pdf",
            "A.PDF",
            "photo.JpEg",
            "notes.md",
            "mail.eml",
            "x.docx",
            "y.msg",
        ] {
            assert!(can_open(ok), "{ok}");
        }
        for no in [
            "x.pdf.exe",
            "setup.exe",
            "run.bat",
            "x.lnk",
            "x.url",
            "x.js",
            "x.hta",
            "x.ps1",
            "x.docm",
            "x.html",
            "x.iso",
            "x.zip",
            "x.exe.",
            "pdf",
            ".pdf",
            "invoice\u{202E}fdp.exe",
            "x.pdf:evil.exe",
        ] {
            assert!(!can_open(no), "{no}");
        }
        // Trailing dots and spaces are dropped before deciding.
        assert!(can_open("report.pdf. "));
    }

    #[test]
    fn only_web_links_open() {
        assert_eq!(
            openable_link("https://todo.example.com/a?b=c").as_deref(),
            Some("https://todo.example.com/a?b=c")
        );
        assert_eq!(
            openable_link(" HTTP://Example.com ").as_deref(),
            Some("http://example.com/")
        );
        for no in [
            "file:///C:/Windows/System32/calc.exe",
            "file://server/share/x.exe",
            r"\\server\share\x.exe",
            "//server/share",
            "javascript:alert(1)",
            "ms-settings:privacy",
            "search-ms:query=x",
            "vscode://file/c:/x",
            "mailto:you@example.com",
            "data:text/html,<script>",
            "C:\\Windows\\notepad.exe",
            "example.com",
            "https://",
            "",
            "not a url",
        ] {
            assert_eq!(openable_link(no), None, "{no}");
        }
    }
}
