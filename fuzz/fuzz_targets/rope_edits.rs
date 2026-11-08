//! Random edits, splits, slices and snapshots, checked against a `Vec<u8>`.

#![no_main]

use fude_buffer::{LineSpan, Rope};
use libfuzzer_sys::fuzz_target;

/// Reads the fuzzer's bytes as a little program.
struct Input<'a>(&'a [u8]);

impl Input<'_> {
    fn byte(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }

    fn num(&mut self) -> Option<usize> {
        Some(usize::from(self.byte()?) | usize::from(self.byte()?) << 8)
    }

    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = usize::from(self.byte()? % 48).min(self.0.len());
        let (text, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(text.to_vec())
    }
}

fn line_starts(bytes: &[u8]) -> Vec<usize> {
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

fn check(rope: &Rope, model: &[u8]) {
    rope.check_invariants();
    assert_eq!(rope.to_vec(), model);
    let starts = line_starts(model);
    assert_eq!(rope.line_count(), starts.len());
    for (line, &start) in starts.iter().enumerate() {
        assert_eq!(rope.line_to_byte(line), start);
    }
    for at in 0..=model.len() {
        assert_eq!(rope.byte_to_line(at), starts.partition_point(|&s| s <= at) - 1);
    }
    let walked: Vec<LineSpan> = rope.lines_from(0).collect();
    let looked_up: Vec<LineSpan> = (0..starts.len()).map(|l| rope.line_span(l)).collect();
    assert_eq!(walked, looked_up);
}

fuzz_target!(|data: &[u8]| {
    let mut input = Input(data);
    let Some(initial) = input.bytes() else { return };
    let mut rope = Rope::from(&initial[..]);
    let mut model = initial;
    let mut snapshots = Vec::new();
    while let Some(op) = input.byte() {
        let Some(a) = input.num() else { break };
        let at = a % (model.len() + 1);
        match op % 5 {
            0 | 1 => {
                let Some(len) = input.byte() else { break };
                let Some(text) = input.bytes() else { break };
                let end = (at + usize::from(len % 64)).min(model.len());
                rope.replace(at..end, &text);
                model.splice(at..end, text);
            }
            2 => {
                let right = rope.split_off(at);
                assert_eq!(right.to_vec(), &model[at..]);
                rope.check_invariants();
                right.check_invariants();
                rope.append(right);
            }
            3 => {
                let Some(b) = input.num() else { break };
                let b = b % (model.len() + 1);
                let range = at.min(b)..at.max(b);
                let slice = rope.slice(range.clone());
                slice.check_invariants();
                assert_eq!(slice.to_vec(), &model[range]);
            }
            _ => snapshots.push((rope.clone(), model.clone())),
        }
        rope.check_invariants();
    }
    check(&rope, &model);
    for (snapshot, model) in &snapshots {
        check(snapshot, model);
    }
});
