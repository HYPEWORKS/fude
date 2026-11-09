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

    /// The selections that touch `range` (sorted, like all of them): found by binary
    /// search, so drawing a screen costs the same with ten thousand cursors elsewhere.
    pub fn touching(&self, range: std::ops::Range<usize>) -> &[Selection] {
        let first = self.ranges.partition_point(|s| s.end() < range.start);
        let last = self.ranges.partition_point(|s| s.start() <= range.end);
        &self.ranges[first..last.max(first)]
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

    /// Without selection `index` (unless it's the only one). The one before it, or else
    /// the first, becomes primary if it was.
    pub fn without(&self, index: usize) -> Selections {
        if self.ranges.len() == 1 || index >= self.ranges.len() {
            return self.clone();
        }
        let mut ranges = self.ranges.clone();
        ranges.remove(index);
        let primary = match self.primary {
            p if p == index => index.saturating_sub(1),
            p if p > index => p - 1,
            p => p,
        };
        Selections { ranges, primary }
    }

    /// With `selection` added, as the primary.
    pub fn with(&self, selection: Selection) -> Selections {
        let mut ranges = self.ranges.clone();
        ranges.push(selection);
        let primary = ranges.len() - 1;
        Selections::new(ranges, primary)
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
