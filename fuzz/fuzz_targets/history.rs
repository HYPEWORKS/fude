//! Edits, undo, redo and saves, checked against a list of every state.

#![no_main]

use fude_buffer::{Buffer, Edit};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut buffer: Buffer<usize> = Buffer::default();
    // states[i] is the text after step i; `at` is the current step.
    let mut states: Vec<Vec<u8>> = vec![vec![]];
    let mut at = 0;
    let mut open = false;
    let mut saved = Some(0);
    for pair in data.chunks(2) {
        let (op, arg) = (pair[0], pair.get(1).copied().unwrap_or(0));
        match op % 6 {
            0 | 1 => {
                let merge = op % 6 == 0;
                let mut s = states[at].clone();
                let pos = usize::from(arg) % (s.len() + 1);
                s.insert(pos, arg);
                buffer.edit(&[Edit::insert(pos, &[arg])], at, at + 1, merge);
                if merge && open {
                    if saved == Some(at) {
                        saved = None;
                    }
                    states[at] = s;
                } else {
                    states.truncate(at + 1);
                    if saved.is_some_and(|i| i > at) {
                        saved = None;
                    }
                    states.push(s);
                    at += 1;
                }
                open = true;
            }
            2 => {
                assert_eq!(buffer.undo().is_some(), at > 0);
                at = at.saturating_sub(1);
                open = false;
            }
            3 => {
                let redone = buffer.redo().is_some();
                assert_eq!(redone, at + 1 < states.len());
                if redone {
                    at += 1;
                }
                open = false;
            }
            4 => {
                buffer.seal();
                open = false;
            }
            _ => {
                buffer.mark_saved(buffer.version());
                saved = Some(at);
                open = false;
            }
        }
        assert_eq!(buffer.text().to_vec(), states[at]);
        assert_eq!(buffer.is_modified(), saved != Some(at));
        buffer.text().check_invariants();
    }
});
