//! Page-title lookup for pasted URLs: a pasted bare link upgrades in place
//! to `[Page Title](url)` once its title arrives, like other note apps.
//! Fetching is blocking (`ureq`) and runs on the background executor; the
//! editor applies the result only if the URL still sits untouched where it
//! was pasted.

use std::io::Read as _;
use std::time::Duration;

/// How much of the response body is read looking for `<title>`. Titles live
/// in `<head>`, well inside this.
const MAX_HTML_BYTES: usize = 256 * 1024;
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
    let mut html = Vec::with_capacity(64 * 1024);
    resp.into_reader()
        .take(MAX_HTML_BYTES as u64)
        .read_to_end(&mut html)
        .ok()?;
    extract_title(&String::from_utf8_lossy(&html))
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
    use super::{extract_title, pasted_url};

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
