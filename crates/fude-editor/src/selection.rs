//! Selections: one or more ranges, each with an anchor and a moving head.

use std::ops::Range;

/// A selected range, or a cursor when `anchor == head`. Offsets are bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Selection {
    /// Where the selection started; stays put while extending.
    pub anchor: usize,
    /// Where the cursor is.
    pub head: usize,
    /// The x position vertical movement aims for, kept across lines of different
    /// lengths. Cleared by anything but vertical movement.
    pub goal_x: Option<f32>,
}

impl Selection {
    pub fn cursor(at: usize) -> Selection {
        Selection {
            anchor: at,
            head: at,
            goal_x: None,
        }
    }

    pub fn new(anchor: usize, head: usize) -> Selection {
        Selection {
            anchor,
            head,
            goal_x: None,
        }
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// Sorted, non-overlapping selections; never empty. One is the primary: the one the view
/// follows and the status bar describes.
#[derive(Clone, Debug, PartialEq)]
pub struct Selections {
    ranges: Vec<Selection>,
    primary: usize,
}

impl Selections {
    pub fn single(selection: Selection) -> Selections {
        Selections {
            ranges: vec![selection],
            primary: 0,
        }
    }

    /// Normalizes `ranges` (sorts, merges overlaps). `primary` indexes the input.
    ///
    /// # Panics
    /// If `ranges` is empty.
    pub fn new(ranges: Vec<Selection>, primary: usize) -> Selections {
        assert!(!ranges.is_empty(), "at least one selection");
        let mut s = Selections { ranges, primary };
        s.normalize();
        s
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Selection> {
        self.ranges.iter()
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn primary(&self) -> &Selection {
        &self.ranges[self.primary]
    }

    pub fn primary_index(&self) -> usize {
        self.primary
    }

    /// Applies `f` to every selection, then normalizes.
    pub fn map(&self, mut f: impl FnMut(&Selection) -> Selection) -> Selections {
        Selections::new(self.ranges.iter().map(&mut f).collect(), self.primary)
    }

    /// Just the primary selection.
    pub fn only_primary(&self) -> Selections {
        Selections::single(*self.primary())
    }

    /// Sorts and merges selections that overlap, or touch where one is a cursor.
    fn normalize(&mut self) {
        let mut indexed: Vec<(usize, Selection)> = self.ranges.drain(..).enumerate().collect();
        indexed.sort_by_key(|(_, s)| (s.start(), s.end()));
        let mut merged: Vec<Selection> = Vec::with_capacity(indexed.len());
        let mut primary = 0;
        for (i, s) in indexed {
            if let Some(last) = merged.last_mut() {
                let touches = s.start() == last.end() && (s.is_empty() || last.is_empty());
                if s.start() < last.end() || touches {
                    // Keep the direction of the selection that starts first.
                    let (start, end) = (last.start(), last.end().max(s.end()));
                    *last = if last.head >= last.anchor {
                        Selection::new(start, end)
                    } else {
                        Selection::new(end, start)
                    };
                    if i == self.primary {
                        primary = merged.len() - 1;
                    }
                    continue;
                }
            }
            if i == self.primary {
                primary = merged.len();
            }
            merged.push(s);
        }
        self.ranges = merged;
        self.primary = primary;
    }
}
