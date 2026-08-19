//! Page-title lookup for pasted URLs: a pasted bare link upgrades in place
//! to `[Page Title](url)` once its title arrives, like other note apps.
//! Fetching is blocking (`ureq`) and runs on the background executor; the
//! editor applies the result only if the URL still sits untouched where it
//! was pasted.

use std::io::Read as _;
use std::time::Duration;

/// Hard ceiling on bytes read while hunting for `</title>`. Reading stops as
/// soon as the closing tag arrives, so this only bounds the worst case:
/// script-heavy pages (YouTube pushes `<title>` ~690 KB into `<head>`) need
/// far more than a small fixed prefix, but a page that never closes a title
/// must still not be read without limit.
const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
/// The closing tag that ends the title; the read stops once it appears.
const TITLE_CLOSE: &[u8; 8] = b"</title>";
/// Longest title kept; page titles beyond this are marketing suffixes.
const MAX_TITLE_CHARS: usize = 120;

/// The single bare web URL in pasted clipboard text, if that is all the
/// paste is. Anything with whitespace or extra content stays untouched.
pub fn pasted_url(text: &str) -> Option<&str> {
    let t = text.trim();
    let rest = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://"))?;
    (!rest.is_empty() && !t.contains(char::is_whitespace)).then_some(t)
}

/// Fetch `url` and return its page title, `None` on any failure: a paste
/// never fails because a title couldn't be had. Blocking; call off the UI
/// thread.
pub fn fetch_title(url: &str) -> Option<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(4))
        .timeout(Duration::from_secs(8))
        .build();
    let resp = agent
        .get(url)
        .set("User-Agent", concat!("kairn/", env!("CARGO_PKG_VERSION")))
        .set("Accept", "text/html")
        .call()
        .ok()?;
    let content_type = resp.header("content-type").unwrap_or("");
    if !content_type.is_empty() && !content_type.contains("html") {
        return None;
    }
    let html = read_until_title(resp.into_reader().take(MAX_HTML_BYTES as u64))?;
    extract_title(&String::from_utf8_lossy(&html))
}

/// Read from `reader` until the closing `</title>` tag appears, returning the
/// bytes gathered so far (the caller extracts the title from them). A tiny
/// page costs a tiny read; a page whose title sits deep in a large `<head>`
/// is read only as far as that tag, and the caller's `.take` caps the total.
fn read_until_title(mut reader: impl std::io::Read) -> Option<Vec<u8>> {
    let mut html = Vec::with_capacity(64 * 1024);
    let mut buf = [0u8; 16 * 1024];
    let mut scanned = 0usize;
    loop {
        let n = reader.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        html.extend_from_slice(&buf[..n]);
        // Re-scan only the fresh tail plus a 7-byte overlap, so a `</title>`
        // straddling two reads is still caught without rescanning the whole
        // buffer each time.
        let from = scanned.saturating_sub(TITLE_CLOSE.len() - 1);
        if html[from..]
            .windows(TITLE_CLOSE.len())
            .any(|w| w.eq_ignore_ascii_case(TITLE_CLOSE))
        {
            break;
        }
        scanned = html.len();
    }
    Some(html)
}

/// The `<title>` element's text, entity-decoded, whitespace-collapsed, and
/// sanitised for use as markdown link text.
fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let open = lower.find("<title")?;
    let start = open + html[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let raw = decode_entities(&html[start..end]);
    let mut title: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    // Square brackets would end the link text early; parentheses read fine.
    title = title.replace('[', "(").replace(']', ")");
    if title.chars().count() > MAX_TITLE_CHARS {
        title = title.chars().take(MAX_TITLE_CHARS - 1).collect();
        title.push('…');
    }
    (!title.is_empty()).then_some(title)
}

/// Decode the HTML entities that actually appear in titles: the named
/// handful plus numeric references.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let semi = tail.find(';').filter(|&i| i <= 12);
        let Some(semi) = semi else {
            out.push('&');
            rest = &rest[amp + 1..];
            continue;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[amp + semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[amp + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::{extract_title, pasted_url, read_until_title};

    #[test]
    fn reads_past_a_large_head_to_reach_the_title() {
        // A YouTube-shaped page: hundreds of KB of head script before the
        // title, far past any small fixed-prefix cap.
        let mut page = String::from("<head>");
        page.push_str(&"<script>padding</script>".repeat(30_000)); // ~700 KB
        page.push_str("<title>Deep Title</title></head><body>...");
        assert!(page.find("<title").unwrap() > 512 * 1024);
        let bytes = read_until_title(std::io::Cursor::new(page.into_bytes())).unwrap();
        assert_eq!(
            extract_title(&String::from_utf8_lossy(&bytes)),
            Some("Deep Title".into())
        );
    }

    #[test]
    fn stops_early_once_the_title_closes() {
        // The gigabytes after `</title>` are never read: only the head is.
        let page = "<title>Short</title>".to_string() + &"x".repeat(4 * 1024 * 1024);
        let bytes = read_until_title(std::io::Cursor::new(page.into_bytes())).unwrap();
        assert!(
            bytes.len() < 64 * 1024,
            "read {} bytes, expected an early stop",
            bytes.len()
        );
        assert_eq!(
            extract_title(&String::from_utf8_lossy(&bytes)),
            Some("Short".into())
        );
    }

    #[test]
    fn pasted_url_accepts_only_a_single_bare_link() {
        assert_eq!(pasted_url("  https://example.com/x  "), Some("https://example.com/x"));
        assert_eq!(pasted_url("http://a.io"), Some("http://a.io"));
        assert_eq!(pasted_url("see https://example.com"), None);
        assert_eq!(pasted_url("https://a.io and more"), None);
        assert_eq!(pasted_url("plain text"), None);
        assert_eq!(pasted_url("https://"), None);
    }

    #[test]
    fn titles_extract_decode_and_collapse() {
        assert_eq!(
            extract_title("<head><title>Rust &amp; GPUI\n  notes</title></head>"),
            Some("Rust & GPUI notes".into())
        );
        assert_eq!(
            extract_title("<TITLE lang=\"en\">A [b] &#8212; c</TITLE>"),
            Some("A (b) — c".into())
        );
        assert_eq!(extract_title("<title></title>"), None);
        assert_eq!(extract_title("no title here"), None);
    }
}
