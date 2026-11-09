//! Tests for the phase 5 commands: multiple cursors, line commands, indentation and
//! comments, bracket matching.

use std::time::Instant;

use fude_buffer::Rope;

use crate::tests::{editor, show};
use crate::text::matching_bracket;
use crate::{Editor, MonoLayout};

fn spaces(mut e: Editor) -> Editor {
    e.settings.insert_spaces = true;
    e.settings.tab_width = 4;
    e
}

#[test]
fn ctrl_click_adds_and_removes_cursors() {
    let mut e = editor("a|bc def");
    e.toggle_cursor(5);
    assert_eq!(show(&e), "a|bc d|ef");
    assert_eq!(e.selections().primary().head, 5);
    e.toggle_cursor(1);
    assert_eq!(show(&e), "abc d|ef", "clicking a cursor removes it");
    e.toggle_cursor(5);
    assert_eq!(show(&e), "abc d|ef", "the last one stays");
    e.insert("X", Instant::now());
    assert_eq!(show(&e), "abc dX|ef");
}

#[test]
fn cursors_above_and_below_keep_their_column() {
    let mut e = editor("abcd\nab\nabcdef\n|");
    let mut layout = MonoLayout { tab_width: 4 };
    let mut e2 = editor("ab|cd\nab\nabcdef");
    assert!(e2.add_cursor_vertical(1, &mut layout));
    assert!(e2.add_cursor_vertical(1, &mut layout));
    assert_eq!(show(&e2), "ab|cd\nab|\nab|cdef");
    assert!(
        !e2.add_cursor_vertical(1, &mut layout),
        "no row below the last"
    );
    e2.insert("-", Instant::now());
    assert_eq!(show(&e2), "ab-|cd\nab-|\nab-|cdef");
    assert!(e.add_cursor_vertical(-1, &mut layout));
    assert_eq!(e.selections().len(), 2);
}

#[test]
fn next_occurrence_selects_word_then_matches() {
    let mut e = editor("fo|o bar foo food foo");
    assert!(e.select_next_occurrence());
    assert_eq!(show(&e), "[foo| bar foo food foo");
    assert!(e.select_next_occurrence());
    assert!(e.select_next_occurrence());
    // "food" isn't a whole word "foo".
    assert_eq!(show(&e), "[foo| bar [foo| food [foo|");
    assert!(!e.select_next_occurrence(), "all found");
    e.insert("x", Instant::now());
    assert_eq!(show(&e), "x| bar x| food x|");

    // Starting from a selection, any match counts, and it wraps around.
    let mut e = editor("foo food [foo|");
    assert!(e.select_next_occurrence());
    assert_eq!(show(&e), "[foo| food [foo|");
    assert!(e.select_next_occurrence());
    assert_eq!(show(&e), "[foo| [foo|d [foo|");
}

#[test]
fn all_occurrences() {
    let mut e = editor("cat |dog cat dog\ndog");
    assert_eq!(e.select_all_occurrences(), 3);
    assert_eq!(show(&e), "cat [dog| cat [dog|\n[dog|");
    assert_eq!(e.selections().primary().start(), 4);
}

#[test]
fn split_selection_into_lines() {
    let mut e = editor("o[ne\ntwo\nthr|ee\nfour");
    e.split_into_lines();
    assert_eq!(show(&e), "o[ne|\n[two|\n[thr|ee\nfour");
    // A selection of whole lines doesn't take the line after.
    let mut e = editor("[a\nb\n|c");
    e.split_into_lines();
    assert_eq!(show(&e), "[a|\n[b|\nc");
}

#[test]
fn column_selection() {
    let mut e = editor("|abcdef\nab\nabcdef\nabc");
    let mut layout = MonoLayout { tab_width: 4 };
    e.select_column((0, 1.0), (3, 3.0), &mut layout);
    assert_eq!(show(&e), "a[bc|def\na[b|\na[bc|def\na[bc|");
    e.insert("_", Instant::now());
    assert_eq!(show(&e), "a_|def\na_|\na_|def\na_|");
    // Upwards: the head (and primary) is on the top line.
    let mut e = editor("|abc\nabc\nabc");
    e.select_column((2, 2.0), (0, 1.0), &mut layout);
    assert_eq!(show(&e), "a|b]c\na|b]c\na|b]c");
    assert_eq!(e.selections().primary().head, 1);
}

#[test]
fn multi_cursor_paste_spreads_lines() {
    let mut e = editor("|\n|\n|");
    e.paste("one\ntwo\nthree", Instant::now());
    assert_eq!(show(&e), "one|\ntwo|\nthree|");
    let mut e = editor("|\n|");
    e.paste("one\ntwo\nthree", Instant::now());
    assert_eq!(
        show(&e),
        "one\ntwo\nthree|\none\ntwo\nthree|",
        "a mismatch pastes it all"
    );
}

#[test]
fn indent_and_outdent_lines() {
    let mut e = spaces(editor("[a\n\nb|\nc"));
    e.indent_lines();
    assert_eq!(show(&e), "[    a\n\n    b|\nc", "empty lines stay empty");
    e.outdent_lines();
    e.outdent_lines();
    assert_eq!(show(&e), "[a\n\nb|\nc");
    let mut e = editor("\t\tx|");
    e.outdent_lines();
    assert_eq!(show(&e), "\tx|");
    let mut e = spaces(editor("  x|"));
    e.outdent_lines();
    assert_eq!(show(&e), "x|", "fewer spaces than a level all go");
}

#[test]
fn enter_indents_after_brackets() {
    let now = Instant::now();
    let mut e = spaces(editor("    fn main() {|"));
    e.newline(now);
    assert_eq!(show(&e), "    fn main() {\n        |");
    let mut e = spaces(editor("    call(|)"));
    e.newline(now);
    assert_eq!(
        show(&e),
        "    call(\n        |\n    )",
        "the closer gets its own line"
    );
    let mut e = spaces(editor("if x:|"));
    e.newline(now);
    assert_eq!(show(&e), "if x:\n|", "colons only in Python");
    e.undo();
    e.settings.indent_after_colon = true;
    e.newline(now);
    assert_eq!(show(&e), "if x:\n    |");
}

#[test]
fn typing_a_closer_outdents() {
    let now = Instant::now();
    let mut e = spaces(editor("    if x {\n        y\n        |"));
    e.insert("}", now);
    assert_eq!(show(&e), "    if x {\n        y\n    }|");
    let mut e = spaces(editor("    a {\n        b |"));
    e.insert("}", now);
    assert_eq!(show(&e), "    a {\n        b }|", "only in indentation");
}

#[test]
fn duplicate_lines_and_selections() {
    let mut e = editor("one\nt|wo\nthree");
    e.duplicate();
    assert_eq!(show(&e), "one\ntwo\nt|wo\nthree");
    let mut e = editor("one\nlast|");
    e.duplicate();
    assert_eq!(show(&e), "one\nlast\nlast|", "a last line without a break");
    let mut e = editor("a [bc| d");
    e.duplicate();
    assert_eq!(show(&e), "a bc[bc| d");
}

#[test]
fn move_lines_up_and_down() {
    let mut e = editor("one\nt|wo\nthree");
    e.move_lines(true);
    assert_eq!(show(&e), "one\nthree\nt|wo");
    e.move_lines(true);
    assert_eq!(show(&e), "one\nthree\nt|wo", "nowhere further down");
    e.move_lines(false);
    e.move_lines(false);
    assert_eq!(show(&e), "t|wo\none\nthree");
    // A block of lines, CRLF kept where it was.
    let mut e = editor("a\r\n[b\r\nc|\r\nd");
    e.move_lines(true);
    assert_eq!(show(&e), "a\r\nd\r\n[b\r\nc|");
}

#[test]
fn delete_and_join_lines() {
    let mut e = editor("one\nt|wo\nthree");
    e.delete_lines();
    assert_eq!(show(&e), "one\n|three");
    let mut e = editor("one\nlast|");
    e.delete_lines();
    assert_eq!(show(&e), "one|");
    let mut e = editor("one|\n    two\n\nthree");
    e.join_lines();
    assert_eq!(show(&e), "one| two\n\nthree");
    let mut e = editor("[a\nb\nc|");
    e.join_lines();
    assert_eq!(show(&e), "[a b c|");
}

#[test]
fn insert_line_above_and_below() {
    let now = Instant::now();
    let mut e = editor("  ab|c\nd");
    e.insert_line(false, now);
    assert_eq!(show(&e), "  abc\n  |\nd");
    let mut e = editor("  ab|c");
    e.insert_line(true, now);
    assert_eq!(show(&e), "  |\n  abc");
}

#[test]
fn toggle_line_comments() {
    let mut e = editor("[    a\n\n      b\n|c");
    e.toggle_line_comment("//");
    assert_eq!(show(&e), "[    // a\n\n    //   b\n|c");
    e.toggle_line_comment("//");
    assert_eq!(show(&e), "[    a\n\n      b\n|c");
    // Mixed: comments everything.
    let mut e = editor("[# a\nb|");
    e.toggle_line_comment("#");
    assert_eq!(show(&e), "[# # a\n# b|");
    // A cursor moves with its text.
    let mut e = editor("x = |1");
    e.toggle_line_comment("--");
    assert_eq!(show(&e), "-- x = |1");
}

#[test]
fn toggle_block_comments() {
    let mut e = editor("a [bc| d");
    e.toggle_block_comment("/*", "*/");
    assert_eq!(show(&e), "a [/* bc */| d");
    e.toggle_block_comment("/*", "*/");
    assert_eq!(show(&e), "a [bc| d");
    let mut e = editor("x|y");
    e.toggle_block_comment("<!--", "-->");
    assert_eq!(show(&e), "x<!-- | -->y");
}

#[test]
fn transform_selections_or_everything() {
    let up = |b: &[u8]| Ok(b.to_ascii_uppercase());
    let mut e = editor("a[bc|d e[f|");
    e.transform(up).unwrap();
    assert_eq!(show(&e), "a[BC|d e[F|");
    let mut e = editor("ab|c");
    e.transform(up).unwrap();
    assert_eq!(show(&e), "AB|C");
    let mut e = editor("ab|c");
    let err = e.transform(|_| Err("no".into()));
    assert!(err.is_err());
    assert_eq!(show(&e), "ab|c");
}

#[test]
fn brackets_match_by_kind_and_nesting() {
    let none = |_: usize| false;
    let t = Rope::from("f(a[1], (b)) { x }");
    assert_eq!(
        matching_bracket(&t, 1, 100, &none),
        Some((1, 11)),
        "before the cursor's char"
    );
    assert_eq!(
        matching_bracket(&t, 2, 100, &none),
        Some((1, 11)),
        "after the cursor"
    );
    assert_eq!(matching_bracket(&t, 12, 100, &none), Some((11, 1)));
    assert_eq!(matching_bracket(&t, 3, 100, &none), Some((3, 5)));
    assert_eq!(matching_bracket(&t, 14, 100, &none), Some((13, 17)));
    assert_eq!(
        matching_bracket(&t, 7, 100, &none),
        None,
        "not next to a bracket"
    );
    assert_eq!(matching_bracket(&t, 1, 5, &none), None, "too far");
    // Ignored brackets (in a string, say) don't count.
    let t = Rope::from("(\")\")");
    assert_eq!(matching_bracket(&t, 0, 100, &|i| i == 2), Some((0, 4)));
}
