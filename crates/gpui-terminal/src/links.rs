//! Link detection in terminal content: OSC 8 hyperlinks carried by cells,
//! and plain URLs recognised in the visible text.

use crate::event::GpuiEventProxy;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Line, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::search::{RegexIter, RegexSearch};

/// Alacritty's default hint pattern: a known URL scheme followed by anything
/// that is not whitespace, a control character, or a common URL delimiter.
pub(crate) const URL_REGEX: &str = "(ipfs:|ipns:|magnet:|mailto:|gemini://|gopher://|https://|http://|news:|file://|git://|ssh:|ftp://)[^\\x{00}-\\x{1F}\\x{7F}-\\x{9F}<>\"\\s{-}\\^⟨⟩`]+";

/// The link at buffer-coordinate `point` in `term`, if any: an OSC 8
/// hyperlink carried by the cell, or else a plain URL in the visible text
/// matched by `url_regex`. Returns the URI and the inclusive
/// buffer-coordinate span to underline.
pub(crate) fn link_at_point(
    term: &Term<GpuiEventProxy>,
    point: Point,
    url_regex: &mut RegexSearch,
) -> Option<(String, Point, Point)> {
    let grid = term.grid();
    let display_offset = grid.display_offset() as i32;
    let last_col = grid.last_column();
    let visible_top = Line(-display_offset);
    let visible_bottom = Line(grid.screen_lines() as i32 - 1 - display_offset);

    // An OSC 8 hyperlink on the cell wins: the URI is explicit and the span
    // is every adjacent cell carrying the same link, walked across wrapped
    // rows within the visible window.
    if let Some(link) = grid[point].hyperlink() {
        let mut start = point;
        let mut end = point;
        loop {
            let prev = if start.column.0 > 0 {
                Point::new(start.line, Column(start.column.0 - 1))
            } else if start.line > visible_top {
                Point::new(start.line - 1, last_col)
            } else {
                break;
            };
            if grid[prev].hyperlink().as_ref() == Some(&link) {
                start = prev;
            } else {
                break;
            }
        }
        loop {
            let next = if end.column < last_col {
                Point::new(end.line, Column(end.column.0 + 1))
            } else if end.line < visible_bottom {
                Point::new(end.line + 1, Column(0))
            } else {
                break;
            };
            if grid[next].hyperlink().as_ref() == Some(&link) {
                end = next;
            } else {
                break;
            }
        }
        return Some((link.uri().to_string(), start, end));
    }

    // Plain text: find a visible URL match containing the point. The search
    // region pads beyond the viewport so a URL wrapped across its edges
    // still matches in full.
    let pad = 100;
    let search_start =
        Point::new(Line((visible_top.0 - pad).max(grid.topmost_line().0)), Column(0));
    let search_end =
        Point::new(Line((visible_bottom.0 + pad).min(grid.bottommost_line().0)), last_col);
    RegexIter::new(search_start, search_end, Direction::Right, term, url_regex)
        .take(1000)
        .find(|m| m.contains(&point))
        .map(|m| (term.bounds_to_string(*m.start(), *m.end()), *m.start(), *m.end()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::TerminalState;
    use std::sync::mpsc::channel;

    fn term_with(bytes: &[u8], cols: usize, rows: usize) -> TerminalState {
        let (tx, _rx) = channel();
        let mut state = TerminalState::new(cols, rows, GpuiEventProxy::new(tx));
        state.process_bytes(bytes);
        state
    }

    fn link_at(state: &TerminalState, line: i32, col: usize) -> Option<(String, Point, Point)> {
        let mut regex = RegexSearch::new(URL_REGEX).expect("URL_REGEX compiles");
        state
            .with_term(|term| link_at_point(term, Point::new(Line(line), Column(col)), &mut regex))
    }

    #[test]
    fn plain_url_under_point() {
        let state = term_with(b"see https://example.com/docs for more", 80, 24);
        let (uri, start, end) = link_at(&state, 0, 10).expect("URL under the point");
        assert_eq!(uri, "https://example.com/docs");
        assert_eq!(start, Point::new(Line(0), Column(4)));
        assert_eq!(end, Point::new(Line(0), Column(27)));
    }

    #[test]
    fn plain_text_is_not_a_link() {
        let state = term_with(b"see https://example.com/docs for more", 80, 24);
        assert_eq!(link_at(&state, 0, 1), None);
        assert_eq!(link_at(&state, 0, 31), None);
        assert_eq!(link_at(&state, 5, 0), None);
    }

    #[test]
    fn wrapped_url_matches_in_full() {
        // 20 columns: the 29-char URL starting at column 2 wraps onto row 1.
        let state = term_with(b"x https://example.com/aaaa/bbbb", 20, 24);
        let (uri, start, end) = link_at(&state, 1, 3).expect("URL under the point");
        assert_eq!(uri, "https://example.com/aaaa/bbbb");
        assert_eq!(start, Point::new(Line(0), Column(2)));
        assert_eq!(end, Point::new(Line(1), Column(10)));
        let (same_uri, ..) = link_at(&state, 0, 5).expect("same URL from the first row");
        assert_eq!(same_uri, uri);
    }

    #[test]
    fn osc8_hyperlink_uri_and_span() {
        let state = term_with(
            b"\x1b]8;;https://example.com/osc\x1b\\click me\x1b]8;;\x1b\\ after",
            80,
            24,
        );
        let (uri, start, end) = link_at(&state, 0, 4).expect("hyperlink under the point");
        assert_eq!(uri, "https://example.com/osc");
        assert_eq!(start, Point::new(Line(0), Column(0)));
        assert_eq!(end, Point::new(Line(0), Column(7)));
        assert_eq!(link_at(&state, 0, 12), None);
    }
}
