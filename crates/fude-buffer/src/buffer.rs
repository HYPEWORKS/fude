//! A document's text with undo history and saved-state tracking.

use std::ops::Range;

use crate::{Format, Rope};

/// Identifies one state of a buffer's text and format. Undo returns to an earlier
/// version's number, so a result computed from a snapshot is valid exactly when the
/// buffer's version equals the snapshot's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Version(u64);

impl Version {
    /// The number behind the version, for tagging work sent to other threads.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// An immutable copy of a buffer's state, cheap to make (O(1)) and to send to a worker.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub text: Rope,
    pub format: Format,
    pub version: Version,
}

/// One replacement in a transaction. Ranges are in the coordinates of the text before the
/// transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit<'a> {
    pub range: Range<usize>,
    pub text: &'a [u8],
}

impl<'a> Edit<'a> {
    pub fn insert(at: usize, text: &'a [u8]) -> Edit<'a> {
        Edit {
            range: at..at,
            text,
        }
    }

    pub fn delete(range: Range<usize>) -> Edit<'static> {
        Edit { range, text: b"" }
    }

    pub fn replace(range: Range<usize>, text: &'a [u8]) -> Edit<'a> {
        Edit { range, text }
    }
}

/// Which side of inserted text a position sticks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bias {
    Before,
    After,
}

/// Where `offset` ends up after applying `edits` (sorted, non-overlapping, in original
/// coordinates). A position inside a replaced range moves to the start of the new text
/// (`Before`) or its end (`After`); `bias` also decides which side of an insertion at
/// exactly `offset` it ends up on.
pub fn map_offset(edits: &[Edit<'_>], offset: usize, bias: Bias) -> usize {
    let mut delta = 0isize;
    for e in edits {
        let (start, end) = (e.range.start, e.range.end);
        if offset < start || (offset == start && (start < end || bias == Bias::Before)) {
            break;
        }
        if offset < end {
            let to = match bias {
                Bias::Before => start,
                Bias::After => start + e.text.len(),
            };
            return to.saturating_add_signed(delta);
        }
        delta += e.text.len() as isize - (end - start) as isize;
    }
    offset.saturating_add_signed(delta)
}

#[derive(Clone)]
struct State {
    text: Rope,
    format: Format,
    version: Version,
}

/// One undo step: the states on either side, and the caller's state (selections) to
/// restore with each.
struct Step<S> {
    before: State,
    after: State,
    before_extra: S,
    after_extra: S,
}

/// A document's text and format, with linear undo/redo and "modified since saved".
///
/// History is a list of earlier trees. They share all unchanged nodes with the current
/// one, so a step costs a few KB at most whatever the document size. `S` is whatever the
/// editor wants restored along with the text, typically the selections.
pub struct Buffer<S = ()> {
    current: State,
    undo: Vec<Step<S>>,
    redo: Vec<Step<S>>,
    /// The last undo step may still absorb edits (consecutive typing).
    open: bool,
    /// The version that matches the file on disk, or the initial state of a new document.
    saved: Option<Version>,
    next_version: u64,
}

impl<S> Default for Buffer<S> {
    fn default() -> Buffer<S> {
        Buffer::new(Rope::new(), Format::default())
    }
}

impl<S> Buffer<S> {
    /// A buffer holding `text`, considered saved (as just loaded, or a new empty document).
    pub fn new(text: Rope, format: Format) -> Buffer<S> {
        Buffer {
            current: State {
                text,
                format,
                version: Version(0),
            },
            undo: Vec::new(),
            redo: Vec::new(),
            open: false,
            saved: Some(Version(0)),
            next_version: 1,
        }
    }

    pub fn text(&self) -> &Rope {
        &self.current.text
    }

    pub fn format(&self) -> &Format {
        &self.current.format
    }

    pub fn version(&self) -> Version {
        self.current.version
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.current.text.clone(),
            format: self.current.format,
            version: self.current.version,
        }
    }

    /// Applies a transaction: `edits` sorted by position and non-overlapping, all in the
    /// coordinates of the current text. `before` and `after` are restored by undo and redo.
    /// With `merge`, the edit joins the previous undo step if that is still open (see
    /// [`Buffer::seal`]), which is how typing becomes one step per word.
    ///
    /// # Panics
    /// If an edit is out of bounds, or the edits overlap or are out of order.
    pub fn edit(&mut self, edits: &[Edit<'_>], before: S, after: S, merge: bool) -> Version {
        let len = self.current.text.len();
        let mut prev_end = 0;
        for e in edits {
            assert!(
                e.range.start >= prev_end && e.range.start <= e.range.end && e.range.end <= len,
                "edits must be sorted, non-overlapping and in bounds (len {len}): {edits:?}"
            );
            prev_end = e.range.end;
        }
        if edits.is_empty() {
            return self.current.version;
        }
        let mut text = self.current.text.clone();
        for e in edits.iter().rev() {
            text.replace(e.range.clone(), e.text);
        }
        self.commit(text, self.current.format, before, after, merge)
    }

    /// Replaces the whole text and format as one undo step: for whole-document changes
    /// computed elsewhere, such as converting line endings or reloading from disk.
    pub fn set_content(&mut self, text: Rope, format: Format, before: S, after: S) -> Version {
        self.commit(text, format, before, after, false)
    }

    /// Changes the format (as one undo step), e.g. to save in another encoding.
    pub fn set_format(&mut self, format: Format, before: S, after: S) -> Version {
        let text = self.current.text.clone();
        self.commit(text, format, before, after, false)
    }

    fn commit(&mut self, text: Rope, format: Format, before: S, after: S, merge: bool) -> Version {
        let state = State {
            text,
            format,
            version: Version(self.next_version),
        };
        self.next_version += 1;
        self.redo.clear();
        match self.undo.last_mut() {
            Some(step) if merge && self.open => {
                step.after = state.clone();
                step.after_extra = after;
            }
            _ => self.undo.push(Step {
                before: self.current.clone(),
                after: state.clone(),
                before_extra: before,
                after_extra: after,
            }),
        }
        self.current = state;
        self.open = true;
        self.current.version
    }

    /// Ends the current undo step, so the next edit starts a new one even with `merge`.
    pub fn seal(&mut self) {
        self.open = false;
    }

    /// Steps back. Returns the caller's state from before the undone edit. Either way, the
    /// next edit starts a new undo step.
    pub fn undo(&mut self) -> Option<&S> {
        self.open = false;
        let step = self.undo.pop()?;
        self.current = step.before.clone();
        self.redo.push(step);
        self.redo.last().map(|s| &s.before_extra)
    }

    /// Steps forward again. Returns the caller's state from after the redone edit. Either
    /// way, the next edit starts a new undo step.
    pub fn redo(&mut self) -> Option<&S> {
        self.open = false;
        let step = self.redo.pop()?;
        self.current = step.after.clone();
        self.undo.push(step);
        self.undo.last().map(|s| &s.after_extra)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Differs from the file on disk (or, for a new document, from empty).
    pub fn is_modified(&self) -> bool {
        self.saved != Some(self.current.version)
    }

    /// Records that `version` is what's on disk now, typically after saving a snapshot of
    /// it (the buffer may have moved on while the save ran).
    pub fn mark_saved(&mut self, version: Version) {
        self.saved = Some(version);
        if version == self.current.version {
            // So undo can come back to exactly the saved state.
            self.seal();
        }
    }

    /// Records that no version matches the disk any more (it changed or was deleted).
    pub fn mark_unsaved(&mut self) {
        self.saved = None;
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn text(b: &Buffer<u32>) -> Vec<u8> {
        b.text().to_vec()
    }

    #[test]
    fn transaction_applies_all_edits_in_original_coordinates() {
        let mut b: Buffer<u32> = Buffer::new(Rope::from("one two three"), Format::default());
        b.edit(
            &[
                Edit::replace(0..3, b"1"),
                Edit::insert(4, b">"),
                Edit::delete(7..8),
            ],
            0,
            1,
            false,
        );
        assert_eq!(text(&b), b"1 >twothree");
        assert_eq!(b.undo(), Some(&0));
        assert_eq!(text(&b), b"one two three");
        assert_eq!(b.redo(), Some(&1));
        assert_eq!(text(&b), b"1 >twothree");
    }

    #[test]
    fn merging_and_sealing() {
        let mut b: Buffer<u32> = Buffer::default();
        b.edit(&[Edit::insert(0, b"a")], 0, 1, true);
        b.edit(&[Edit::insert(1, b"b")], 1, 2, true);
        b.seal();
        b.edit(&[Edit::insert(2, b" c")], 2, 4, true);
        assert_eq!(text(&b), b"ab c");
        assert_eq!(b.undo(), Some(&2));
        assert_eq!(text(&b), b"ab");
        assert_eq!(b.undo(), Some(&0), "typing merged into one step");
        assert_eq!(text(&b), b"");
        assert!(!b.can_undo());
    }

    #[test]
    fn modified_tracks_the_saved_version_through_undo() {
        let mut b: Buffer<()> = Buffer::new(Rope::from("x"), Format::default());
        assert!(!b.is_modified());
        b.edit(&[Edit::insert(1, b"y")], (), (), true);
        assert!(b.is_modified());
        b.undo();
        assert!(!b.is_modified(), "back to the saved text");
        b.redo();
        let saved = b.snapshot().version;
        b.edit(&[Edit::insert(2, b"z")], (), (), true);
        b.mark_saved(saved);
        assert!(b.is_modified(), "edited while the save ran");
        b.undo();
        assert!(!b.is_modified());

        b.mark_saved(b.version());
        b.edit(&[Edit::insert(0, b"!")], (), (), true);
        b.undo();
        assert!(
            !b.is_modified(),
            "saving sealed the step, so undo returns to it"
        );
    }

    #[test]
    fn format_changes_are_undoable() {
        let mut b: Buffer<()> = Buffer::default();
        let original = *b.format();
        let utf16 = Format {
            encoding: encoding_rs::UTF_16LE,
            bom: true,
            ..original
        };
        b.set_format(utf16, (), ());
        assert!(b.is_modified());
        b.undo();
        assert_eq!(*b.format(), original);
        assert!(!b.is_modified());
    }

    #[test]
    #[should_panic(expected = "sorted, non-overlapping")]
    fn overlapping_edits_panic() {
        let mut b: Buffer<()> = Buffer::new(Rope::from("abcdef"), Format::default());
        b.edit(&[Edit::delete(0..3), Edit::delete(2..4)], (), (), false);
    }

    #[test]
    fn mapping_offsets() {
        let edits = [Edit::replace(2..4, b"xyz"), Edit::insert(6, b"!")];
        let map = |at, bias| map_offset(&edits, at, bias);
        assert_eq!(map(1, Bias::Before), 1);
        assert_eq!(map(2, Bias::After), 2, "start of a replaced range stays");
        assert_eq!(map(3, Bias::Before), 2);
        assert_eq!(map(3, Bias::After), 5);
        assert_eq!(map(4, Bias::Before), 5, "end of a replaced range");
        assert_eq!(map(6, Bias::Before), 7);
        assert_eq!(map(6, Bias::After), 8);
        assert_eq!(map(9, Bias::After), 11);
    }

    #[derive(Debug, Clone)]
    enum Op {
        Type(usize, Vec<u8>),
        Delete(usize, usize),
        Seal,
        Undo,
        Redo,
        Save,
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => (any::<usize>(), prop::collection::vec(any::<u8>(), 1..8)).prop_map(|(a, t)| Op::Type(a, t)),
            2 => (any::<usize>(), 1usize..6).prop_map(|(a, n)| Op::Delete(a, n)),
            1 => Just(Op::Seal),
            2 => Just(Op::Undo),
            1 => Just(Op::Redo),
            1 => Just(Op::Save),
        ]
    }

    proptest! {
        /// Checks undo, redo and the modified flag against a model that keeps every step's
        /// text in full.
        #[test]
        fn history_matches_model(ops in prop::collection::vec(op(), 0..80)) {
            let mut b: Buffer<usize> = Buffer::default();
            // Model: states[i] is the text after i steps; `at` is the current step.
            let mut states: Vec<Vec<u8>> = vec![vec![]];
            let mut at = 0;
            let mut open = false;
            let mut saved: Option<Vec<u8>> = Some(vec![]);
            let mut saved_step: Option<usize> = Some(0);
            for op in ops {
                match op {
                    Op::Type(pos, t) => {
                        let mut s = states[at].clone();
                        let pos = pos % (s.len() + 1);
                        s.splice(pos..pos, t.iter().copied());
                        b.edit(&[Edit::insert(pos, &t)], at, at + 1, true);
                        if open {
                            states.truncate(at);
                            if saved_step == Some(at) { saved_step = None; }
                            states.push(s);
                        } else {
                            states.truncate(at + 1);
                            if saved_step.is_some_and(|i| i > at) { saved_step = None; }
                            states.push(s);
                            at += 1;
                        }
                        open = true;
                    }
                    Op::Delete(pos, n) => {
                        let mut s = states[at].clone();
                        if s.is_empty() { continue; }
                        let pos = pos % s.len();
                        let end = (pos + n).min(s.len());
                        s.drain(pos..end);
                        b.edit(&[Edit::delete(pos..end)], at, at + 1, false);
                        states.truncate(at + 1);
                        if saved_step.is_some_and(|i| i > at) { saved_step = None; }
                        states.push(s);
                        at += 1;
                        open = true;
                    }
                    Op::Seal => { b.seal(); open = false; }
                    Op::Undo => {
                        let undone = b.undo().is_some();
                        prop_assert_eq!(undone, at > 0);
                        at = at.saturating_sub(1);
                        open = false;
                    }
                    Op::Redo => {
                        let redone = b.redo().is_some();
                        prop_assert_eq!(redone, at + 1 < states.len());
                        if redone { at += 1; }
                        open = false;
                    }
                    Op::Save => {
                        b.mark_saved(b.version());
                        saved = Some(states[at].clone());
                        saved_step = Some(at);
                        open = false;
                    }
                }
                prop_assert_eq!(b.text().to_vec(), states[at].clone());
                prop_assert_eq!(b.is_modified(), saved_step != Some(at));
                if !b.is_modified() {
                    prop_assert_eq!(Some(b.text().to_vec()), saved.clone());
                }
            }
        }
    }
}
