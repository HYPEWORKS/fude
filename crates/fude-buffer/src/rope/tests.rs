//! Rope tests. These run with tiny nodes (see `params`), so small inputs build deep trees.

use proptest::prelude::*;

use super::*;
use crate::LineEndingCounts;

/// Line starts of `bytes`, computed the obvious way.
fn naive_line_starts(bytes: &[u8]) -> Vec<usize> {
    let mut starts = vec![0];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                i += 1;
                starts.push(i + 1);
            }
            b'\r' | b'\n' => starts.push(i + 1),
            _ => {}
        }
        i += 1;
    }
    starts
}

fn naive_counts(bytes: &[u8]) -> LineEndingCounts {
    let mut c = LineEndingCounts::default();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                c.crlf += 1;
                i += 1;
            }
            b'\r' => c.cr += 1,
            b'\n' => c.lf += 1,
            _ => {}
        }
        i += 1;
    }
    c
}

/// Checks structure, content and line breaks against the model.
fn check_content(rope: &Rope, model: &[u8]) {
    rope.check_invariants();
    assert_eq!(rope.len(), model.len());
    assert_eq!(rope.to_vec(), model);
    assert_eq!(rope.line_ending_counts(), naive_counts(model));
    assert_eq!(rope.line_count(), naive_line_starts(model).len());
}

/// Also checks every line lookup.
fn check_lines(rope: &Rope, model: &[u8]) {
    check_content(rope, model);
    let starts = naive_line_starts(model);
    for (line, &start) in starts.iter().enumerate() {
        assert_eq!(rope.line_to_byte(line), start, "start of line {line}");
        let next = starts.get(line + 1).copied().unwrap_or(model.len());
        let end = if line + 1 == starts.len() {
            next
        } else if model[..next].ends_with(b"\r\n") {
            next - 2
        } else {
            next - 1
        };
        assert_eq!(rope.line_span(line), LineSpan { start, end, next });
    }
    for at in 0..=model.len() {
        let line = starts.partition_point(|&s| s <= at) - 1;
        assert_eq!(rope.byte_to_line(at), line, "line of byte {at}");
    }
    for first in [0, starts.len() / 2, starts.len() - 1] {
        let walked: Vec<LineSpan> = rope.lines_from(first).collect();
        let looked_up: Vec<LineSpan> = (first..starts.len()).map(|l| rope.line_span(l)).collect();
        assert_eq!(walked, looked_up, "lines from {first}");
    }
}

/// Bytes biased towards the interesting ones: line breaks, UTF-8 lead and continuation
/// bytes, and anything else.
fn bytes(max: usize) -> impl Strategy<Value = Vec<u8>> {
    let byte = prop_oneof![
        4 => Just(b'a'),
        2 => Just(b'\n'),
        2 => Just(b'\r'),
        1 => Just(0xE7),
        1 => Just(0x88),
        1 => any::<u8>(),
    ];
    prop::collection::vec(byte, 0..max)
}

#[derive(Debug, Clone)]
enum Op {
    Replace {
        at: usize,
        len: usize,
        text: Vec<u8>,
    },
    SplitAndAppend(usize),
    Slice(usize, usize),
    Snapshot,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        6 => (any::<usize>(), 0usize..24, bytes(24))
            .prop_map(|(at, len, text)| Op::Replace { at, len, text }),
        1 => (any::<usize>(), 0usize..300).prop_map(|(at, len)| Op::Replace { at, len, text: vec![] }),
        1 => (any::<usize>(), bytes(200)).prop_map(|(at, text)| Op::Replace { at, len: 0, text }),
        1 => any::<usize>().prop_map(Op::SplitAndAppend),
        1 => (any::<usize>(), any::<usize>()).prop_map(|(a, b)| Op::Slice(a, b)),
        1 => Just(Op::Snapshot),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    #[test]
    fn edits_match_a_vec(initial in bytes(400), ops in prop::collection::vec(op(), 0..40)) {
        let mut rope = Rope::from(&initial[..]);
        let mut model = initial;
        let mut snapshots = vec![];
        check_content(&rope, &model);
        for op in ops {
            match op {
                Op::Replace { at, len, text } => {
                    let start = at % (model.len() + 1);
                    let end = (start + len).min(model.len());
                    rope.replace(start..end, &text);
                    model.splice(start..end, text);
                }
                Op::SplitAndAppend(at) => {
                    let at = at % (model.len() + 1);
                    let right = rope.split_off(at);
                    check_content(&rope, &model[..at]);
                    check_content(&right, &model[at..]);
                    rope.append(right);
                }
                Op::Slice(a, b) => {
                    let (a, b) = (a % (model.len() + 1), b % (model.len() + 1));
                    let range = a.min(b)..a.max(b);
                    check_content(&rope.slice(range.clone()), &model[range]);
                }
                Op::Snapshot => snapshots.push((rope.clone(), model.clone())),
            }
            check_content(&rope, &model);
        }
        check_lines(&rope, &model);
        // Edits never show through to earlier snapshots.
        for (snapshot, model) in snapshots {
            check_content(&snapshot, &model);
        }
    }

    /// Builds the same bytes from differently sized pushes, so leaf boundaries land
    /// everywhere, including between `\r` and `\n`.
    #[test]
    fn line_lookups_match_naive(data in bytes(600), cuts in prop::collection::vec(1usize..40, 1..20)) {
        let mut builder = RopeBuilder::new();
        let mut rest = &data[..];
        for cut in cuts.iter().cycle() {
            if rest.is_empty() {
                break;
            }
            let n = (*cut).min(rest.len());
            builder.push(&rest[..n]);
            rest = &rest[n..];
        }
        let rope = builder.finish();
        check_lines(&rope, &data);
    }

    #[test]
    fn concatenating_ropes_of_any_heights(parts in prop::collection::vec(bytes(300), 1..8)) {
        let mut rope = Rope::new();
        let mut model = vec![];
        for part in parts {
            if model.len() % 2 == 0 {
                rope.append(Rope::from(&part[..]));
                model.extend_from_slice(&part);
            } else {
                let mut front = Rope::from(&part[..]);
                front.append(rope);
                rope = front;
                model.splice(0..0, part);
            }
            check_content(&rope, &model);
        }
        check_lines(&rope, &model);
    }

    #[test]
    fn chunks_in_any_range(data in bytes(500), a in any::<usize>(), b in any::<usize>()) {
        let rope = Rope::from(&data[..]);
        let (a, b) = (a % (data.len() + 1), b % (data.len() + 1));
        let range = a.min(b)..a.max(b);
        let chunks: Vec<&[u8]> = rope.chunks_in(range.clone()).collect();
        prop_assert!(chunks.iter().all(|c| !c.is_empty()));
        prop_assert_eq!(chunks.concat(), &data[range]);
    }

    #[test]
    fn equality_ignores_tree_shape(data in bytes(300), at in any::<usize>()) {
        let at = at % (data.len() + 1);
        let whole = Rope::from(&data[..]);
        let mut joined = Rope::from(&data[..at]);
        joined.append(Rope::from(&data[at..]));
        prop_assert_eq!(&whole, &joined);
        if !data.is_empty() {
            let mut other = data.clone();
            other[at.min(data.len() - 1)] ^= 1;
            prop_assert_ne!(whole, Rope::from(&other[..]));
        }
    }
}

#[test]
fn crlf_split_across_leaves_is_one_line_break() {
    // Both halves are valid leaves, so concatenation keeps them apart.
    let mut rope = Rope::from("aaaa\r");
    rope.append(Rope::from("\nbbbb"));
    rope.check_invariants();
    assert!(matches!(rope.root.node, Node::Internal(_)), "two leaves");
    assert_eq!(rope.line_count(), 2);
    assert_eq!(
        rope.line_ending_counts(),
        LineEndingCounts {
            lf: 0,
            crlf: 1,
            cr: 0
        }
    );
    assert_eq!(rope.line_to_byte(1), 6);
    assert_eq!(rope.byte_to_line(5), 0, "between \\r and \\n");
    assert_eq!(rope.byte_to_line(6), 1);
    assert_eq!(
        rope.line_span(0),
        LineSpan {
            start: 0,
            end: 4,
            next: 6
        }
    );

    // Deleting the `\n` leaves a lone CR; deleting the `\r` leaves a lone LF.
    let mut cr = rope.clone();
    cr.remove(5..6);
    assert_eq!(cr.line_ending_counts().cr, 1);
    let mut lf = rope.clone();
    lf.remove(4..5);
    assert_eq!(lf.line_ending_counts().lf, 1);
    assert_eq!(rope.line_ending_counts().crlf, 1, "snapshot unchanged");
}

#[test]
fn empty_rope() {
    let rope = Rope::new();
    check_lines(&rope, b"");
    assert_eq!(rope.chunks().count(), 0);
    assert_eq!(rope.get(0), None);
    assert_eq!(rope.chunk_at(0), (&b""[..], 0));
}

#[test]
fn typing_mutates_unshared_leaves_in_place() {
    let mut rope = Rope::from(&b"0123456789"[..]);
    let before = match &rope.root.node {
        Node::Leaf(leaf) => std::sync::Arc::as_ptr(leaf),
        Node::Internal(_) => unreachable!(),
    };
    rope.insert(3, b"x");
    let after = match &rope.root.node {
        Node::Leaf(leaf) => std::sync::Arc::as_ptr(leaf),
        Node::Internal(_) => unreachable!(),
    };
    assert_eq!(before, after, "no copy when the leaf isn't shared");

    let snapshot = rope.clone();
    rope.insert(0, b"y");
    assert!(!rope.ptr_eq(&snapshot), "copied when shared");
    assert_eq!(snapshot.to_vec(), b"012x3456789");
    assert_eq!(rope.to_vec(), b"y012x3456789");
}

#[test]
#[should_panic(expected = "out of bounds")]
fn replace_out_of_bounds_panics() {
    Rope::from("abc").replace(2..4, b"");
}
