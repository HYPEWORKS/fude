//! Find and replace over a rope, with the `regex` engine (linear time, so no pattern can
//! hang the editor) running directly over the text through `regex-cursor`.

use std::cell::Cell;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use fude_buffer::Rope;
use regex_cursor::engines::meta::Regex;
use regex_cursor::regex_automata::Anchored;
use regex_cursor::regex_automata::util::captures::Captures;
use regex_cursor::{Cursor, Input};

/// What to look for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    /// `text` is a regular expression (otherwise it matches literally).
    pub regex: bool,
    pub case_sensitive: bool,
    /// Only matches that start and end at word boundaries.
    pub whole_word: bool,
}

/// When a search should give up before the end of the text: it was cancelled, or it has
/// had its time (the UI thread searches for a few milliseconds, then hands over to a
/// worker). Checked once per 64 KB window.
#[derive(Debug, Default)]
pub struct Stop<'a> {
    cancel: Option<&'a AtomicBool>,
    deadline: Option<Instant>,
    /// Set when the search was cut short, which makes its result meaningless.
    hit: Cell<bool>,
}

/// A search gave up before it knew the answer (see [`Stop`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stopped;

impl<'a> Stop<'a> {
    /// Never: the search runs to the end.
    pub fn never() -> Stop<'static> {
        Stop::default()
    }

    /// Once `cancel` is set.
    pub fn when(cancel: &'a AtomicBool) -> Stop<'a> {
        Stop {
            cancel: Some(cancel),
            ..Stop::default()
        }
    }

    /// At `deadline`.
    pub fn at(deadline: Instant) -> Stop<'static> {
        Stop {
            deadline: Some(deadline),
            ..Stop::default()
        }
    }

    /// Whether to stop now; remembers having said yes.
    fn now(&self) -> bool {
        let stop = self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
            || self.deadline.is_some_and(|d| Instant::now() >= d);
        if stop {
            self.hit.set(true);
        }
        stop
    }

    /// `result`, unless the search behind it was cut short: the engine took the cut for
    /// the end of the text, so even a match it reported may be wrong.
    fn check<T>(&self, result: T) -> Result<T, Stopped> {
        if self.hit.get() {
            Err(Stopped)
        } else {
            Ok(result)
        }
    }
}

/// A compiled query.
#[derive(Clone, Debug)]
pub struct Searcher {
    regex: Regex,
    /// Replacement text understands `$1`, `${name}` and `$$` (regex queries only).
    expand: bool,
}

/// How far back [`Searcher::find_before`] looks at first; it doubles until it finds one.
const BACK_WINDOW: usize = 64 * 1024;

impl Searcher {
    /// Compiles `query`. `Ok(None)` for an empty query; `Err` with the regex error.
    pub fn new(query: &Query) -> Result<Option<Searcher>, String> {
        if query.text.is_empty() {
            return Ok(None);
        }
        let mut pattern = if query.regex {
            query.text.clone()
        } else {
            regex_syntax::escape(&query.text)
        };
        if query.whole_word {
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            if word(query.text.chars().next()) {
                pattern = format!(r"\b(?:{pattern})");
            }
            if word(query.text.chars().last()) {
                pattern = format!(r"(?:{pattern})\b");
            }
        }
        // m: ^ and $ match at line breaks; R: and treat \r\n as one.
        let flags = if query.case_sensitive {
            "(?mR)"
        } else {
            "(?mRi)"
        };
        let regex = Regex::new(&format!("{flags}{pattern}")).map_err(|e| {
            // regex errors are multi-line with a caret diagram; keep the last line.
            e.to_string()
                .lines()
                .last()
                .unwrap_or("invalid pattern")
                .trim_start_matches("error: ")
                .to_owned()
        })?;
        Ok(Some(Searcher {
            regex,
            expand: query.regex,
        }))
    }

    /// The first non-empty match starting at or after `from`.
    pub fn find_from(&self, text: &Rope, from: usize) -> Option<Range<usize>> {
        self.find_from_until(text, from, &Stop::never())
            .unwrap_or(None)
    }

    /// [`Searcher::find_from`], giving up when `stop` says.
    pub fn find_from_until(
        &self,
        text: &Rope,
        from: usize,
        stop: &Stop<'_>,
    ) -> Result<Option<Range<usize>>, Stopped> {
        let mut cursor = RopeCursor::at(text, from);
        cursor.stop = Some(stop);
        let input = Input::new(cursor).range(from..);
        let found = self
            .regex
            .find_iter(input)
            .map(|m| m.range())
            .find(|r| !r.is_empty());
        stop.check(found)
    }

    /// The last non-empty match starting before `before`.
    pub fn find_before(&self, text: &Rope, before: usize) -> Option<Range<usize>> {
        self.find_before_until(text, before, &Stop::never())
            .unwrap_or(None)
    }

    /// [`Searcher::find_before`], giving up when `stop` says.
    pub fn find_before_until(
        &self,
        text: &Rope,
        before: usize,
        stop: &Stop<'_>,
    ) -> Result<Option<Range<usize>>, Stopped> {
        let mut window = BACK_WINDOW;
        loop {
            let start = before.saturating_sub(window);
            let found = self
                .matches_in_until(text, start..before, stop)?
                .into_iter()
                .rev()
                .find(|r| r.start < before);
            if found.is_some() || start == 0 {
                return Ok(found);
            }
            window *= 2;
        }
    }

    /// The next match after `from`, wrapping around to the start.
    pub fn next(&self, text: &Rope, from: usize) -> Option<Range<usize>> {
        self.next_until(text, from, &Stop::never()).unwrap_or(None)
    }

    /// [`Searcher::next`], giving up when `stop` says.
    pub fn next_until(
        &self,
        text: &Rope,
        from: usize,
        stop: &Stop<'_>,
    ) -> Result<Option<Range<usize>>, Stopped> {
        match self.find_from_until(text, from, stop)? {
            Some(found) => Ok(Some(found)),
            None => self.find_from_until(text, 0, stop),
        }
    }

    /// The match before `before`, wrapping around to the end.
    pub fn previous(&self, text: &Rope, before: usize) -> Option<Range<usize>> {
        self.previous_until(text, before, &Stop::never())
            .unwrap_or(None)
    }

    /// [`Searcher::previous`], giving up when `stop` says.
    pub fn previous_until(
        &self,
        text: &Rope,
        before: usize,
        stop: &Stop<'_>,
    ) -> Result<Option<Range<usize>>, Stopped> {
        match self.find_before_until(text, before, stop)? {
            Some(found) => Ok(Some(found)),
            None => self.find_before_until(text, text.len(), stop),
        }
    }

    /// Non-empty matches that start within `range` (they may end after it).
    pub fn matches_in(&self, text: &Rope, range: Range<usize>) -> Vec<Range<usize>> {
        self.matches_in_until(text, range, &Stop::never())
            .unwrap_or_default()
    }

    /// [`Searcher::matches_in`], giving up when `stop` says.
    pub fn matches_in_until(
        &self,
        text: &Rope,
        range: Range<usize>,
        stop: &Stop<'_>,
    ) -> Result<Vec<Range<usize>>, Stopped> {
        let mut cursor = RopeCursor::at(text, range.start);
        cursor.stop = Some(stop);
        let input = Input::new(cursor).range(range.start..);
        let mut out = Vec::new();
        for m in self.regex.find_iter(input) {
            if m.start() >= range.end {
                break;
            }
            if !m.is_empty() {
                out.push(m.range());
            }
        }
        stop.check(out)
    }

    /// Counts matches in the whole text: the total, and their starts if there are at most
    /// `keep` (so the UI can say "3 of 120"). Linear in the text: run it on a worker.
    pub fn count(&self, text: &Rope, keep: usize) -> (usize, Option<Vec<usize>>) {
        self.count_until(text, keep, &AtomicBool::new(false))
    }

    /// [`Searcher::count`], stopping early (with a partial count) once `cancel` is set.
    pub fn count_until(
        &self,
        text: &Rope,
        keep: usize,
        cancel: &AtomicBool,
    ) -> (usize, Option<Vec<usize>>) {
        let mut starts = Some(Vec::new());
        let mut total = 0;
        let stop = Stop::when(cancel);
        let mut cursor = RopeCursor::at(text, 0);
        cursor.stop = Some(&stop);
        for m in self.regex.find_iter(Input::new(cursor)) {
            if m.is_empty() {
                continue;
            }
            total += 1;
            if let Some(s) = &mut starts {
                if s.len() == keep {
                    starts = None;
                } else {
                    s.push(m.start());
                }
            }
        }
        (total, starts)
    }

    /// The replacement for the match at `range`: `replacement` as is, or for regex
    /// queries with `$1`-style groups filled in from the match.
    pub fn replacement(&self, text: &Rope, range: Range<usize>, replacement: &str) -> Vec<u8> {
        if !self.expand || !replacement.contains('$') {
            return replacement.as_bytes().to_vec();
        }
        // Capture with some context around the match, so anchors and word boundaries at
        // its edges see the same text as the original search did.
        let context = 256;
        let start = range.start.saturating_sub(context);
        let end = (range.end + context).min(text.len());
        let mut haystack = Vec::with_capacity(end - start);
        text.copy_range(start..end, &mut haystack);
        let mut caps: Captures = self.regex.create_captures();
        let mut input = Input::new(&haystack[..]).range(range.start - start..end - start);
        input.anchored(Anchored::Yes);
        self.regex.search_captures(input, &mut caps);
        let mut out = Vec::new();
        if caps.is_match() {
            caps.interpolate_bytes_into(&haystack, replacement.as_bytes(), &mut out);
        } else {
            out.extend_from_slice(replacement.as_bytes());
        }
        out
    }
}

/// Serves a rope to the regex engine in windows that never split a UTF-8 character.
///
/// The engine needs chunk boundaries on character boundaries (for Unicode word
/// boundaries and UTF-8 mode), which rope leaves don't promise, so this copies windows
/// of up to 64 KB whose edges are moved back onto character starts. The edges are a
/// function of the window index, so moving forward and backward agree.
struct RopeCursor<'a> {
    text: &'a Rope,
    index: usize,
    start: usize,
    buf: Vec<u8>,
    /// When it says so, the cursor ends early, which ends the search.
    stop: Option<&'a Stop<'a>>,
    /// The furthest window served. Only going past it can stop the search: the engine
    /// also steps back a window for context and forward again, which is no progress.
    reached: usize,
}

const WINDOW: usize = 64 * 1024;

impl<'a> RopeCursor<'a> {
    /// A cursor on the window containing `at`.
    fn at(text: &'a Rope, at: usize) -> RopeCursor<'a> {
        let mut c = RopeCursor {
            text,
            index: at / WINDOW,
            start: 0,
            buf: Vec::new(),
            stop: None,
            reached: 0,
        };
        if c.index > 0 && c.boundary(c.index) > at {
            c.index -= 1;
        }
        c.reached = c.index;
        c.load();
        c
    }

    /// Where window `i` starts: `i * WINDOW`, moved back off UTF-8 continuation bytes.
    fn boundary(&self, i: usize) -> usize {
        let len = self.text.len();
        let mut b = (i * WINDOW).min(len);
        let mut back = 0;
        while b > 0 && b < len && back < 3 && self.text.byte(b) & 0xC0 == 0x80 {
            b -= 1;
            back += 1;
        }
        b
    }

    fn load(&mut self) {
        self.start = self.boundary(self.index);
        let end = self.boundary(self.index + 1);
        self.buf.clear();
        self.text.copy_range(self.start..end, &mut self.buf);
    }
}

impl Cursor for RopeCursor<'_> {
    fn chunk(&self) -> &[u8] {
        &self.buf
    }

    fn advance(&mut self) -> bool {
        if self.start + self.buf.len() >= self.text.len()
            || (self.index >= self.reached && self.stop.is_some_and(Stop::now))
        {
            return false;
        }
        self.index += 1;
        self.reached = self.reached.max(self.index);
        self.load();
        true
    }

    fn backtrack(&mut self) -> bool {
        if self.index == 0 {
            return false;
        }
        self.index -= 1;
        self.load();
        true
    }

    fn total_bytes(&self) -> Option<usize> {
        Some(self.text.len())
    }

    fn offset(&self) -> usize {
        self.start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str) -> Query {
        Query {
            text: text.into(),
            ..Query::default()
        }
    }

    fn searcher(query: Query) -> Searcher {
        Searcher::new(&query).unwrap().unwrap()
    }

    #[test]
    fn literal_case_and_whole_word() {
        let text = Rope::from("Foo foo food (foo) foo.");
        let s = searcher(q("foo"));
        assert_eq!(
            s.matches_in(&text, 0..text.len()).len(),
            5,
            "case-insensitive by default"
        );
        let s = searcher(Query {
            case_sensitive: true,
            ..q("foo")
        });
        assert_eq!(s.matches_in(&text, 0..text.len()).len(), 4);
        let s = searcher(Query {
            whole_word: true,
            ..q("foo")
        });
        let starts: Vec<usize> = s
            .matches_in(&text, 0..text.len())
            .iter()
            .map(|r| r.start)
            .collect();
        assert_eq!(starts, [0, 4, 14, 19]);
        let s = searcher(q("(foo)"));
        assert_eq!(s.find_from(&text, 0), Some(13..18), "literal by default");
    }

    #[test]
    fn next_and_previous_wrap() {
        let text = Rope::from("ab ab ab");
        let s = searcher(q("ab"));
        assert_eq!(s.next(&text, 1), Some(3..5));
        assert_eq!(s.next(&text, 7), Some(0..2), "wraps to the start");
        assert_eq!(s.previous(&text, 3), Some(0..2));
        assert_eq!(s.previous(&text, 0), Some(6..8), "wraps to the end");
    }

    #[test]
    fn regex_lines_and_crlf() {
        let text = Rope::from("one\r\ntwo\r\nthree");
        let s = searcher(Query {
            regex: true,
            ..q("^t.*$")
        });
        assert_eq!(
            s.matches_in(&text, 0..text.len()),
            [5..8, 10..15],
            "$ stops before \\r\\n"
        );
        assert!(
            Searcher::new(&Query {
                regex: true,
                ..q("(unclosed")
            })
            .is_err()
        );
        assert!(Searcher::new(&q("")).unwrap().is_none());
    }

    #[test]
    fn replacement_groups() {
        let text = Rope::from("name = value");
        let s = searcher(Query {
            regex: true,
            ..q(r"(\w+) = (\w+)")
        });
        let m = s.find_from(&text, 0).unwrap();
        assert_eq!(s.replacement(&text, m.clone(), "$2 = $1"), b"value = name");
        let literal = searcher(q("name"));
        assert_eq!(
            literal.replacement(&text, 0..4, "$1"),
            b"$1",
            "literal queries don't expand"
        );
    }

    #[test]
    fn across_windows_and_utf8() {
        // Matches straddle the 64 KB windows, with multi-byte characters at the edges.
        let unit = "日本語 needle ";
        let text_s = unit.repeat(20_000);
        let text = Rope::from(text_s.as_str());
        let s = searcher(q("needle"));
        let (total, starts) = s.count(&text, 100_000);
        assert_eq!(total, 20_000);
        let expected: Vec<usize> = text_s.match_indices("needle").map(|(i, _)| i).collect();
        assert_eq!(starts.unwrap(), expected);
        let word = searcher(Query {
            regex: true,
            ..q(r"\b日本語\b")
        });
        assert_eq!(
            word.count(&text, 0).0,
            20_000,
            "Unicode word boundaries work"
        );
        let far = text.len() - 10;
        let before_far = expected.iter().rev().find(|&&s| s < far).unwrap();
        assert_eq!(s.find_before(&text, far).unwrap().start, *before_far);
    }

    #[test]
    fn a_stopped_search_says_so_instead_of_answering() {
        // The only match is far past the first window.
        let mut bytes = vec![b'a'; 4 * WINDOW];
        bytes.extend_from_slice(b"needle");
        let text = Rope::from(&bytes[..]);
        let s = searcher(q("needle"));
        let at = 4 * WINDOW..4 * WINDOW + 6;

        let cancelled = AtomicBool::new(true);
        let stop = Stop::when(&cancelled);
        assert_eq!(s.find_from_until(&text, 0, &stop), Err(Stopped));
        let past = Stop::at(Instant::now());
        assert_eq!(s.next_until(&text, 0, &past), Err(Stopped));
        let past = Stop::at(Instant::now());
        assert_eq!(s.previous_until(&text, 10, &past), Err(Stopped));
        let past = Stop::at(Instant::now());
        assert_eq!(
            s.matches_in_until(&text, 0..text.len(), &past),
            Err(Stopped)
        );

        // A match in the window the search starts in needs no further window.
        let past = Stop::at(Instant::now());
        assert_eq!(
            s.find_from_until(&text, at.start, &past),
            Ok(Some(at.clone()))
        );
        // Left alone, they all find it.
        let never = Stop::never();
        assert_eq!(s.next_until(&text, 0, &never), Ok(Some(at.clone())));
        assert_eq!(s.previous_until(&text, 10, &never), Ok(Some(at.clone())));
        assert_eq!(s.next_until(&Rope::from("aaa"), 0, &never), Ok(None));
    }
}
