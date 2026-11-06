//! What can be clicked in a terminal's text: web addresses, file links and Windows paths that end in
//! a file extension, and "[Image #N]", the placeholder agents show for a pasted image.

use std::sync::LazyLock;

use regex::Regex;

static WEB: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(https?|HTTPS?)://[^\s"'!*(){}|\\^<>`]*[^\s"':,.!?{}|\\^~\[\]`()<>]"#).unwrap());
// file:///… links, and Windows paths that end in a file extension ("C:\dir\out.png").
static FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"file:///[^\s"'<>`]+|[A-Za-z]:[\\/](?:[^\s"'<>`|*?\\/]+[\\/])*[^\s"'<>`|*?\\/]+\.[A-Za-z0-9]{1,8}"#).unwrap());
static IMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[Image #(\d+)\]").unwrap());

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Web(String),
    /// A path, or a file:/// link.
    File(String),
    /// The image's number, and the text around it, which tells apart messages with the same number.
    Image { n: u32, context: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextLink {
    /// In characters of the text.
    pub start: usize,
    pub end: usize,
    pub target: LinkTarget,
}

/// The links in a line of text, most specific kind first: a web address wins over what looks like a
/// path inside it.
pub fn find_links(text: &str) -> Vec<TextLink> {
    let chars_to = |byte: usize| text[..byte].chars().count();
    let mut links = Vec::new();
    for m in WEB.find_iter(text) {
        links.push(TextLink { start: chars_to(m.start()), end: chars_to(m.end()), target: LinkTarget::Web(m.as_str().to_string()) });
    }
    for m in FILE.find_iter(text) {
        // Trailing punctuation isn't part of the path.
        let raw = m.as_str().trim_end_matches(['.', ',', ';', ':', ')', ']', '}', '\'', '"', '`']);
        links.push(TextLink { start: chars_to(m.start()), end: chars_to(m.start() + raw.len()), target: LinkTarget::File(raw.to_string()) });
    }
    let mut context = None;
    for c in IMAGE.captures_iter(text) {
        let m = c.get(0).unwrap();
        let Ok(n) = c[1].parse() else { continue };
        let context = context.get_or_insert_with(|| IMAGE.replace_all(text, " ").into_owned()).clone();
        links.push(TextLink { start: chars_to(m.start()), end: chars_to(m.end()), target: LinkTarget::Image { n, context } });
    }
    links
}

/// What a link a program printed (OSC 8) opens; other kinds of address do nothing.
pub fn target_of_uri(uri: &str) -> Option<LinkTarget> {
    let lower = uri.to_lowercase();
    if lower.starts_with("file:") {
        Some(LinkTarget::File(uri.to_string()))
    } else if lower.starts_with("http:") || lower.starts_with("https:") {
        Some(LinkTarget::Web(uri.to_string()))
    } else {
        None
    }
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| text.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// file:///C:/x/y.png -> C:\x\y.png; a plain path stays as it is.
pub fn file_path_of(link: &str) -> String {
    if !link.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:")) {
        return link.to_string();
    }
    let p = percent_decoded(link[5..].trim_start_matches('/'));
    let drive = p.len() >= 2 && p.as_bytes()[0].is_ascii_alphabetic() && p.as_bytes()[1] == b':';
    if drive { p.replace('/', "\\") } else { format!("/{p}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(text: &str) -> Vec<LinkTarget> {
        find_links(text).into_iter().map(|l| l.target).collect()
    }

    #[test]
    fn finds_paths() {
        assert_eq!(targets(r"- Zip: C:\Users\a\release\v2.6.18\Crew-Rust-2.6.18-win-x64.zip (16 MB)"), [LinkTarget::File(r"C:\Users\a\release\v2.6.18\Crew-Rust-2.6.18-win-x64.zip".into())]);
        assert_eq!(targets("see D:/x/y.txt."), [LinkTarget::File("D:/x/y.txt".into())]);
        assert_eq!(targets("(C:\\a\\b.png)"), [LinkTarget::File("C:\\a\\b.png".into())]);
        assert!(targets(r"C:\no\extension").is_empty());
        assert_eq!(targets("file:///C:/a%20b/c.png, next"), [LinkTarget::File("file:///C:/a%20b/c.png".into())]);
        let l = &find_links(r"好的 C:\a.txt")[0];
        assert_eq!((l.start, l.end), (3, 11), "positions count characters, not bytes");
    }

    #[test]
    fn finds_web_addresses_and_images() {
        assert_eq!(targets("go to https://a.com/x?y=1, then"), [LinkTarget::Web("https://a.com/x?y=1".into())]);
        assert_eq!(targets("(http://127.0.0.1:7788/)"), [LinkTarget::Web("http://127.0.0.1:7788/".into())]);
        assert_eq!(targets("[Image #3] look"), [LinkTarget::Image { n: 3, context: "  look".into() }]);
    }

    #[test]
    fn reads_file_links_and_uris() {
        assert_eq!(file_path_of("file:///C:/a%20b/%E5%A5%BD.png"), "C:\\a b\\好.png");
        assert_eq!(file_path_of("file:///tmp/x.png"), "/tmp/x.png");
        assert_eq!(file_path_of(r"C:\a\b.png"), r"C:\a\b.png");
        assert_eq!(target_of_uri("HTTPS://a"), Some(LinkTarget::Web("HTTPS://a".into())));
        assert_eq!(target_of_uri("file:///a"), Some(LinkTarget::File("file:///a".into())));
        assert_eq!(target_of_uri("vscode://a"), None);
    }
}
