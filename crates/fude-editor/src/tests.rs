//! Editor tests. Documents are written with markers: `|` is a cursor (a selection's
//! head), `[` before it or `]` after it marks the selection's anchor.

use std::time::{Duration, Instant};

use fude_buffer::{Format, LineEnding, Rope};
use proptest::prelude::*;

use crate::{Editor, MonoLayout, Motion, Selection, Selections};

fn lf() -> Format {
    Format {
        line_ending: LineEnding::Lf,
        ..Format::default()
    }
}

fn editor_with(marked: &str, format: Format) -> Editor {
    let mut text = String::new();
    let mut marks = vec![];
    for c in marked.chars() {
        match c {
            '|' | '[' | ']' => marks.push((c, text.len())),
            _ => text.push(c),
        }
    }
    let mut sels = vec![];
    let mut i = 0;
    while i < marks.len() {
        match marks[i] {
            ('[', anchor) => {
                sels.push(Selection::new(anchor, marks[i + 1].1));
                i += 2;
            }
            ('|', head) if marks.get(i + 1).is_some_and(|m| m.0 == ']') => {
                sels.push(Selection::new(marks[i + 1].1, head));
                i += 2;
            }
            (_, head) => {
                sels.push(Selection::cursor(head));
                i += 1;
            }
        }
    }
    let mut e = Editor::new(Rope::from(text.as_str()), format);
    e.set_selections(Selections::new(sels, 0));
    e
}

fn editor(marked: &str) -> Editor {
    editor_with(marked, lf())
}

fn show(e: &Editor) -> String {
    let mut s = String::from_utf8(e.text().to_vec()).unwrap();
    let mut marks: Vec<(usize, char)> = vec![];
    for sel in e.selections().iter() {
        marks.push((sel.head, '|'));
        if sel.anchor < sel.head {
            marks.push((sel.anchor, '['));
        } else if sel.anchor > sel.head {
            marks.push((sel.anchor, ']'));
        }
    }
    marks.sort_by(|a, b| b.cmp(a));
    for (at, c) in marks {
        s.insert(at, c);
    }
    s
}

/// A clock for undo grouping.
struct Clock(Instant);

impl Clock {
    fn new() -> Clock {
        Clock(Instant::now())
    }
    fn tick(&mut self, ms: u64) -> Instant {
        self.0 += Duration::from_millis(ms);
        self.0
    }
}

fn type_str(e: &mut Editor, s: &str, clock: &mut Clock) {
    for c in s.chars() {
        e.insert(&c.to_string(), clock.tick(50));
    }
}

fn mv(e: &mut Editor, m: Motion, extend: bool) {
    e.move_cursors(m, extend, &mut MonoLayout { tab_width: 4 });
}

#[test]
fn typing_groups_by_word_and_pause() {
    let mut clock = Clock::new();
    let mut e = editor("|");
    type_str(&mut e, "hello world", &mut clock);
    assert_eq!(show(&e), "hello world|");
    assert!(e.undo());
    assert_eq!(show(&e), "hello |");
    assert!(e.undo());
    assert_eq!(show(&e), "|");

    let mut e = editor("|");
    type_str(&mut e, "ab", &mut clock);
    clock.tick(2000);
    type_str(&mut e, "c", &mut clock);
    e.undo();
    assert_eq!(show(&e), "ab|", "a pause starts a new step");
}

#[test]
fn moving_the_cursor_ends_the_undo_step() {
    let mut clock = Clock::new();
    let mut e = editor("|");
    type_str(&mut e, "ab", &mut clock);
    mv(&mut e, Motion::Left, false);
    type_str(&mut e, "X", &mut clock);
    assert_eq!(show(&e), "aX|b");
    e.undo();
    assert_eq!(show(&e), "a|b", "undo restores the cursor too");
    e.undo();
    assert_eq!(show(&e), "|");
}

#[test]
fn deleting_groups_and_handles_clusters() {
    let mut clock = Clock::new();
    let mut e = editor("a\r\nb👍🏽c|");
    e.backspace(clock.tick(50));
    e.backspace(clock.tick(50));
    assert_eq!(show(&e), "a\r\nb|", "an emoji with a skin tone is one step");
    e.backspace(clock.tick(50));
    e.backspace(clock.tick(50));
    assert_eq!(show(&e), "a|", "CRLF is one step");
    e.undo();
    assert_eq!(
        show(&e),
        "a\r\nb👍🏽c|",
        "consecutive backspaces are one step"
    );

    let mut e = Editor::new(Rope::from(&b"a\xFFb"[..]), lf());
    e.set_selections(Selections::single(Selection::cursor(2)));
    e.backspace(clock.tick(50));
    assert_eq!(e.text().to_vec(), b"ab", "an invalid byte is one step");
}

#[test]
fn selections_are_replaced_and_collapse() {
    let mut clock = Clock::new();
    let mut e = editor("one [two| three");
    type_str(&mut e, "2", &mut clock);
    assert_eq!(show(&e), "one 2| three");

    let mut e = editor("one [two| three");
    mv(&mut e, Motion::Left, false);
    assert_eq!(show(&e), "one |two three", "left collapses to the start");
    let mut e = editor("one [two| three");
    mv(&mut e, Motion::Right, false);
    assert_eq!(show(&e), "one two| three");

    let mut e = editor("ab|cd");
    mv(&mut e, Motion::Right, true);
    mv(&mut e, Motion::Right, true);
    assert_eq!(show(&e), "ab[cd|");
    e.backspace(clock.tick(50));
    assert_eq!(show(&e), "ab|");
}

#[test]
fn vertical_movement_keeps_the_goal_column() {
    let mut e = editor("abcde|f\nab\nabcdef");
    mv(&mut e, Motion::Down, false);
    assert_eq!(show(&e), "abcdef\nab|\nabcdef");
    mv(&mut e, Motion::Down, false);
    assert_eq!(show(&e), "abcdef\nab\nabcde|f");
    mv(&mut e, Motion::Down, false);
    assert_eq!(
        show(&e),
        "abcdef\nab\nabcdef|",
        "past the last line goes to the end"
    );

    let mut e = editor("ab|c\nx");
    mv(&mut e, Motion::Up, false);
    assert_eq!(
        show(&e),
        "|abc\nx",
        "above the first line goes to the start"
    );

    let mut e = editor("\tx|\n12345678");
    mv(&mut e, Motion::Down, false);
    assert_eq!(show(&e), "\tx\n12345|678", "tabs count to their stop");
}

#[test]
fn smart_home_and_end() {
    let mut e = editor("    foo|bar");
    mv(&mut e, Motion::LineStart, false);
    assert_eq!(show(&e), "    |foobar");
    mv(&mut e, Motion::LineStart, false);
    assert_eq!(show(&e), "|    foobar");
    mv(&mut e, Motion::LineEnd, true);
    assert_eq!(show(&e), "[    foobar|");
}

#[test]
fn word_motion_and_deletion() {
    let mut clock = Clock::new();
    let mut e = editor("foo.bar baz|");
    mv(&mut e, Motion::WordLeft, false);
    assert_eq!(show(&e), "foo.bar |baz");
    e.delete_word_left(clock.tick(50));
    assert_eq!(show(&e), "foo.|baz");
}

#[test]
fn multiple_cursors_edit_together() {
    let mut clock = Clock::new();
    let mut e = editor("a|b\nc|d\ne|f");
    type_str(&mut e, "XY", &mut clock);
    assert_eq!(show(&e), "aXY|b\ncXY|d\neXY|f");
    e.backspace(clock.tick(50));
    assert_eq!(show(&e), "aX|b\ncX|d\neX|f");
    e.undo();
    assert_eq!(show(&e), "aXY|b\ncXY|d\neXY|f");
    e.undo();
    assert_eq!(show(&e), "a|b\nc|d\ne|f");
}

#[test]
fn cursors_that_meet_merge() {
    let mut clock = Clock::new();
    let mut e = editor("a|b|c");
    e.backspace(clock.tick(50));
    assert_eq!(show(&e), "|c");
    assert_eq!(e.selections().len(), 1);
}

#[test]
fn newline_keeps_indentation_and_line_ending_style() {
    let mut clock = Clock::new();
    let mut e = editor("\t  foo|");
    e.newline(clock.tick(50));
    assert_eq!(show(&e), "\t  foo\n\t  |");

    let crlf = Format {
        line_ending: LineEnding::CrLf,
        ..Format::default()
    };
    let mut e = editor_with("a|", crlf);
    e.newline(clock.tick(50));
    assert_eq!(show(&e), "a\r\n|");
}

#[test]
fn copy_cut_and_paste() {
    let mut clock = Clock::new();
    let mut e = editor("one [two| three");
    assert_eq!(e.copy(), "two");
    let copied = e.cut(clock.tick(50));
    assert_eq!(copied, "two");
    assert_eq!(show(&e), "one | three");
    e.paste("2", clock.tick(50));
    assert_eq!(show(&e), "one 2| three");

    // Nothing selected: whole lines, pasted above.
    let mut e = editor("first\nsec|ond\nthird");
    assert_eq!(e.copy(), "second\n");
    mv(&mut e, Motion::Down, false);
    e.paste("second\n", clock.tick(50));
    assert_eq!(show(&e), "first\nsecond\nsecond\nthi|rd");

    let mut e = editor("first\nsec|ond\nthird");
    e.cut(clock.tick(50));
    assert_eq!(show(&e), "first\n|third");
    let mut e = editor("first\nla|st");
    assert_eq!(e.cut(clock.tick(50)), "last\n");
    assert_eq!(
        show(&e),
        "first|",
        "cutting the last line takes the break before it"
    );
}

#[test]
fn paste_converts_line_breaks() {
    let crlf = Format {
        line_ending: LineEnding::CrLf,
        ..Format::default()
    };
    let mut e = editor_with("|", crlf);
    e.paste("a\nb\r\nc\rd", Instant::now());
    assert_eq!(e.text().to_vec(), b"a\r\nb\r\nc\r\nd");
}

#[test]
fn applying_many_edits_is_one_step() {
    let mut e = editor("a-b-c|");
    e.apply_edits(&[(1..2, b"+".to_vec()), (3..4, b"+".to_vec())]);
    assert_eq!(show(&e), "a+b+c|");
    e.undo();
    assert_eq!(show(&e), "a-b-c|");
}

#[test]
fn line_and_column() {
    let e = editor("ab\n\tc|d");
    assert_eq!(e.cursor_line_col(), (2, 6));
}

#[test]
fn escape_drops_extra_cursors_then_collapses() {
    let mut e = editor("[ab| c|d");
    e.collapse();
    assert_eq!(show(&e), "[ab| cd");
    e.collapse();
    assert_eq!(show(&e), "ab| cd");
}

#[derive(Debug, Clone)]
enum Op {
    Type(String),
    Newline,
    Backspace,
    Delete,
    DeleteWordLeft,
    Move(u8, bool),
    Select(usize, usize),
    Undo,
    Redo,
    Cut,
    Paste(String),
}

fn op() -> impl Strategy<Value = Op> {
    let text = prop::collection::vec(
        prop_oneof![
            Just('a'),
            Just(' '),
            Just('\n'),
            Just('\r'),
            Just('é'),
            Just('😀'),
            Just('\u{301}')
        ],
        1..4,
    )
    .prop_map(|v| v.into_iter().collect::<String>());
    prop_oneof![
        4 => text.clone().prop_map(Op::Type),
        1 => Just(Op::Newline),
        2 => Just(Op::Backspace),
        1 => Just(Op::Delete),
        1 => Just(Op::DeleteWordLeft),
        3 => (0u8..12, any::<bool>()).prop_map(|(m, x)| Op::Move(m, x)),
        1 => (any::<usize>(), any::<usize>()).prop_map(|(a, b)| Op::Select(a, b)),
        2 => Just(Op::Undo),
        1 => Just(Op::Redo),
        1 => Just(Op::Cut),
        1 => text.prop_map(Op::Paste),
    ]
}

proptest! {
    /// Any sequence of commands keeps selections valid, and undoing everything gives back
    /// the original text.
    #[test]
    fn commands_keep_selections_valid(initial in "[a é\n\r]{0,30}", ops in prop::collection::vec(op(), 0..60)) {
        let mut clock = Clock::new();
        let mut e = Editor::new(Rope::from(initial.as_str()), lf());
        for op in ops {
            let now = clock.tick(30);
            match op {
                Op::Type(s) => e.insert(&s, now),
                Op::Newline => e.newline(now),
                Op::Backspace => e.backspace(now),
                Op::Delete => e.delete(now),
                Op::DeleteWordLeft => e.delete_word_left(now),
                Op::Move(m, extend) => {
                    let motion = [
                        Motion::Left, Motion::Right, Motion::WordLeft, Motion::WordRight,
                        Motion::LineStart, Motion::LineEnd, Motion::Up, Motion::Down,
                        Motion::PageUp(3), Motion::PageDown(3), Motion::DocStart, Motion::DocEnd,
                    ][m as usize];
                    mv(&mut e, motion, extend);
                }
                Op::Select(a, b) => {
                    let len = e.text().len() + 1;
                    let (a, b) = (a % len, b % len);
                    let t = e.text().clone();
                    let (a, b) = (crate::text::floor_grapheme(&t, a), crate::text::floor_grapheme(&t, b));
                    e.set_selections(Selections::new(vec![Selection::new(a, b), Selection::cursor(b / 2)], 0));
                }
                Op::Undo => { e.undo(); }
                Op::Redo => { e.redo(); }
                Op::Cut => { e.cut(now); }
                Op::Paste(s) => e.paste(&s, now),
            }
            let len = e.text().len();
            let mut prev_end = None;
            for s in e.selections().iter() {
                prop_assert!(s.anchor <= len && s.head <= len);
                if let Some(end) = prev_end {
                    prop_assert!(s.start() >= end, "sorted and non-overlapping");
                }
                prev_end = Some(s.end());
            }
            e.text().check_invariants();
        }
        while e.undo() {}
        prop_assert_eq!(e.text().to_vec(), initial.as_bytes());
    }
}
