//! A persistent B-tree rope of bytes.
//!
//! Leaves hold raw bytes, which need not be valid UTF-8, so any file round-trips exactly.
//! Parents cache a [`Summary`] per child (bytes and line breaks), so byte ↔ line conversion
//! is O(log n). Nodes are shared through `Arc` and copied on write: cloning a rope is O(1),
//! which is what snapshots and undo are made of, and an edit copies O(log n) nodes.
//!
//! Line breaks are `\n`, `\r\n` and `\r`. A `\r\n` pair is one break, even when its two
//! bytes sit in different leaves.

mod builder;
mod iter;
mod node;
mod summary;
mod tree;

#[cfg(test)]
mod tests;

use std::fmt;
use std::io::{self, Write};
use std::ops::Range;

pub use builder::RopeBuilder;
pub use iter::{Chunks, Lines};
pub(crate) use summary::Summary;

use node::{Child, Node};

use crate::LineEndingCounts;

/// Node size limits. Tests (and fuzzing, via the `small-nodes` feature) use tiny nodes so
/// small inputs build deep trees and exercise every rebalancing path.
#[cfg(not(any(test, feature = "small-nodes")))]
mod params {
    pub const MAX_LEAF: usize = 4096;
    pub const MIN_LEAF: usize = 1024;
    /// How full the builder makes leaves: room for 512 bytes of typing before a split.
    pub const BUILD_LEAF: usize = 3584;
    pub const MAX_CHILDREN: usize = 16;
    pub const MIN_CHILDREN: usize = 8;
}

#[cfg(any(test, feature = "small-nodes"))]
mod params {
    pub const MAX_LEAF: usize = 16;
    pub const MIN_LEAF: usize = 4;
    pub const BUILD_LEAF: usize = 12;
    pub const MAX_CHILDREN: usize = 4;
    pub const MIN_CHILDREN: usize = 2;
}

/// Text as bytes, in a persistent B-tree. Cheap to clone; see the module docs.
#[derive(Clone, Default)]
pub struct Rope {
    root: Child,
}

/// Where a line is: its content is `start..end`, its line break `end..next`. On the last
/// line, `end == next == len`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineSpan {
    pub start: usize,
    pub end: usize,
    pub next: usize,
}

impl Rope {
    pub fn new() -> Rope {
        Rope::default()
    }

    pub fn len(&self) -> usize {
        self.root.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Lines, counting the (possibly empty) line after the last break: `"a\n"` has two.
    pub fn line_count(&self) -> usize {
        self.root.summary.breaks() + 1
    }

    pub fn line_ending_counts(&self) -> LineEndingCounts {
        let s = &self.root.summary;
        LineEndingCounts {
            lf: s.lf,
            crlf: s.crlf,
            cr: s.cr,
        }
    }

    /// Whether both are the same tree (so certainly equal). O(1).
    pub fn ptr_eq(&self, other: &Rope) -> bool {
        match (&self.root.node, &other.root.node) {
            (Node::Leaf(a), Node::Leaf(b)) => std::sync::Arc::ptr_eq(a, b),
            (Node::Internal(a), Node::Internal(b)) => std::sync::Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    // --- Reading -----------------------------------------------------------------------

    pub fn get(&self, at: usize) -> Option<u8> {
        if at >= self.len() {
            return None;
        }
        let (chunk, start) = self.chunk_at(at);
        Some(chunk[at - start])
    }

    /// # Panics
    /// If `at >= len`.
    pub fn byte(&self, at: usize) -> u8 {
        self.get(at)
            .unwrap_or_else(|| panic!("byte {at} out of bounds (len {})", self.len()))
    }

    /// The stored chunk containing byte `at` (the last chunk when `at == len`), and the
    /// offset where it starts.
    pub fn chunk_at(&self, at: usize) -> (&[u8], usize) {
        assert!(
            at <= self.len(),
            "offset {at} out of bounds (len {})",
            self.len()
        );
        let mut node = &self.root;
        let mut start = 0;
        loop {
            match &node.node {
                Node::Leaf(leaf) => return (&leaf.bytes, start),
                Node::Internal(int) => {
                    let last = int.children.len() - 1;
                    for (i, c) in int.children.iter().enumerate() {
                        if at < start + c.len() || i == last {
                            node = c;
                            break;
                        }
                        start += c.len();
                    }
                }
            }
        }
    }

    pub fn chunks(&self) -> Chunks<'_> {
        Chunks::new(&self.root, 0..self.len())
    }

    /// # Panics
    /// If the range is out of bounds or reversed.
    pub fn chunks_in(&self, range: Range<usize>) -> Chunks<'_> {
        self.check_range(&range);
        Chunks::new(&self.root, range)
    }

    pub fn to_vec(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        self.copy_range(0..self.len(), &mut out);
        out
    }

    /// Appends the bytes of `range` to `out`.
    pub fn copy_range(&self, range: Range<usize>, out: &mut Vec<u8>) {
        out.reserve(range.len());
        for chunk in self.chunks_in(range) {
            out.extend_from_slice(chunk);
        }
    }

    pub fn write_to(&self, out: &mut impl Write) -> io::Result<()> {
        self.chunks().try_for_each(|chunk| out.write_all(chunk))
    }

    // --- Lines -------------------------------------------------------------------------

    /// The line containing byte `at`. An offset between the `\r` and `\n` of a CRLF pair
    /// belongs to the line the pair ends.
    ///
    /// # Panics
    /// If `at > len`.
    pub fn byte_to_line(&self, at: usize) -> usize {
        assert!(
            at <= self.len(),
            "offset {at} out of bounds (len {})",
            self.len()
        );
        let before = self.summary_to(at);
        let inside_crlf = before.ends_with_cr && self.get(at) == Some(b'\n');
        before.breaks() - usize::from(inside_crlf)
    }

    /// The offset where `line` starts.
    ///
    /// # Panics
    /// If `line >= line_count`.
    pub fn line_to_byte(&self, line: usize) -> usize {
        assert!(
            line < self.line_count(),
            "line {line} out of bounds ({} lines)",
            self.line_count()
        );
        if line == 0 {
            return 0;
        }
        let (at, crlf) = self.nth_break(line);
        at + 1 + usize::from(crlf)
    }

    /// Successive lines from `line` on: one O(log n) lookup, then a forward scan, so
    /// walking the visible lines costs O(visible bytes).
    ///
    /// # Panics
    /// If `line >= line_count`.
    pub fn lines_from(&self, line: usize) -> Lines<'_> {
        let start = self.line_to_byte(line);
        Lines::new(self, start)
    }

    /// # Panics
    /// If `line >= line_count`.
    pub fn line_span(&self, line: usize) -> LineSpan {
        let start = self.line_to_byte(line);
        if line + 1 == self.line_count() {
            let len = self.len();
            return LineSpan {
                start,
                end: len,
                next: len,
            };
        }
        let next = self.line_to_byte(line + 1);
        let crlf =
            next - start >= 2 && self.byte(next - 2) == b'\r' && self.byte(next - 1) == b'\n';
        LineSpan {
            start,
            end: next - 1 - usize::from(crlf),
            next,
        }
    }

    /// The summary of bytes `0..at`.
    fn summary_to(&self, mut at: usize) -> Summary {
        let mut acc = Summary::default();
        let mut node = &self.root;
        'descend: loop {
            match &node.node {
                Node::Leaf(leaf) => return acc.join(Summary::of(&leaf.bytes[..at])),
                Node::Internal(int) => {
                    for c in &int.children {
                        if at < c.len() {
                            node = c;
                            continue 'descend;
                        }
                        acc = acc.join(c.summary);
                        at -= c.len();
                    }
                    return acc;
                }
            }
        }
    }

    /// The offset of the first byte of the `n`th line break (1-based), and whether the
    /// break is a CRLF pair.
    fn nth_break(&self, n: usize) -> (usize, bool) {
        let mut acc = Summary::default();
        let mut offset = 0;
        let mut node = &self.root;
        'descend: loop {
            match &node.node {
                Node::Internal(int) => {
                    for c in &int.children {
                        let next = acc.join(c.summary);
                        if next.breaks() >= n {
                            node = c;
                            continue 'descend;
                        }
                        acc = next;
                        offset += c.len();
                    }
                    unreachable!("break {n} is inside the node");
                }
                Node::Leaf(leaf) => {
                    let bytes = &leaf.bytes;
                    let mut count = acc.breaks();
                    for i in memchr::memchr2_iter(b'\r', b'\n', bytes) {
                        let after_cr = match i {
                            0 => acc.ends_with_cr,
                            _ => bytes[i - 1] == b'\r',
                        };
                        // The `\n` of a CRLF pair doesn't start a break.
                        if bytes[i] == b'\r' || !after_cr {
                            count += 1;
                            if count == n {
                                let crlf = bytes[i] == b'\r'
                                    && match bytes.get(i + 1) {
                                        Some(&b) => b == b'\n',
                                        None => self.get(offset + i + 1) == Some(b'\n'),
                                    };
                                return (offset + i, crlf);
                            }
                        }
                    }
                    unreachable!("break {n} is inside the leaf");
                }
            }
        }
    }

    // --- Editing -----------------------------------------------------------------------

    /// Replaces `range` with `text`.
    ///
    /// # Panics
    /// If the range is out of bounds or reversed.
    pub fn replace(&mut self, range: Range<usize>, text: &[u8]) {
        self.check_range(&range);
        if range.is_empty() && text.is_empty() {
            return;
        }
        if tree::edit_in_place(&mut self.root, range.start, range.end, text) {
            return;
        }
        let root = std::mem::take(&mut self.root);
        let (left, rest) = tree::split(&root, range.start);
        let (_, right) = tree::split(&rest, range.end - range.start);
        drop((root, rest));
        let middle = Rope::from(text).root;
        self.root = tree::concat(tree::concat(left, middle), right);
    }

    pub fn insert(&mut self, at: usize, text: &[u8]) {
        self.replace(at..at, text);
    }

    pub fn remove(&mut self, range: Range<usize>) {
        self.replace(range, &[]);
    }

    /// A rope of the bytes in `range`, sharing nodes with this one. O(log n).
    pub fn slice(&self, range: Range<usize>) -> Rope {
        self.check_range(&range);
        let (_, rest) = tree::split(&self.root, range.start);
        let (middle, _) = tree::split(&rest, range.len());
        Rope { root: middle }
    }

    /// Splits off and returns the bytes from `at` on. O(log n).
    pub fn split_off(&mut self, at: usize) -> Rope {
        assert!(
            at <= self.len(),
            "offset {at} out of bounds (len {})",
            self.len()
        );
        let (left, right) = tree::split(&std::mem::take(&mut self.root), at);
        self.root = left;
        Rope { root: right }
    }

    /// Appends `other`. O(log n).
    pub fn append(&mut self, other: Rope) {
        let left = std::mem::take(&mut self.root);
        self.root = tree::concat(left, other.root);
    }

    fn check_range(&self, range: &Range<usize>) {
        assert!(
            range.start <= range.end && range.end <= self.len(),
            "range {range:?} out of bounds (len {})",
            self.len()
        );
    }

    /// Panics if the tree breaks an invariant. For tests and fuzzing.
    #[doc(hidden)]
    pub fn check_invariants(&self) {
        if let Node::Internal(int) = &self.root.node {
            assert!(int.children.len() >= 2, "internal root with one child");
        }
        check_node(&self.root, true);
    }
}

fn check_node(child: &Child, is_root: bool) -> usize {
    use params::*;
    match &child.node {
        Node::Leaf(leaf) => {
            let n = leaf.bytes.len();
            assert!(n <= MAX_LEAF, "leaf of {n} bytes");
            assert!(is_root || n >= MIN_LEAF, "non-root leaf of {n} bytes");
            assert_eq!(child.summary, Summary::of(&leaf.bytes), "leaf summary");
            0
        }
        Node::Internal(int) => {
            let n = int.children.len();
            assert!(n <= MAX_CHILDREN, "node with {n} children");
            assert!(
                is_root || n >= MIN_CHILDREN,
                "non-root node with {n} children"
            );
            for c in &int.children {
                let height = check_node(c, false);
                assert_eq!(height + 1, int.height, "unbalanced tree");
            }
            assert_eq!(child.summary, node::fold(&int.children), "node summary");
            int.height
        }
    }
}

impl From<&[u8]> for Rope {
    fn from(bytes: &[u8]) -> Rope {
        if bytes.len() <= params::MAX_LEAF {
            return Rope {
                root: Child::leaf(bytes.to_vec()),
            };
        }
        let mut builder = RopeBuilder::new();
        builder.push(bytes);
        builder.finish()
    }
}

impl From<&str> for Rope {
    fn from(s: &str) -> Rope {
        Rope::from(s.as_bytes())
    }
}

impl PartialEq for Rope {
    fn eq(&self, other: &Rope) -> bool {
        if self.len() != other.len() {
            return false;
        }
        if self.ptr_eq(other) {
            return true;
        }
        let (mut a, mut b) = (self.chunks(), other.chunks());
        let (mut x, mut y): (&[u8], &[u8]) = (&[], &[]);
        loop {
            if x.is_empty() {
                match a.next() {
                    Some(c) => x = c,
                    None => return true,
                }
            }
            if y.is_empty() {
                match b.next() {
                    Some(c) => y = c,
                    None => return true,
                }
            }
            let n = x.len().min(y.len());
            if x[..n] != y[..n] {
                return false;
            }
            x = &x[n..];
            y = &y[n..];
        }
    }
}

impl Eq for Rope {}

impl fmt::Debug for Rope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const PREVIEW: usize = 64;
        let mut head = Vec::new();
        self.copy_range(0..self.len().min(PREVIEW), &mut head);
        let ellipsis = if self.len() > PREVIEW { "…" } else { "" };
        write!(
            f,
            "Rope({} bytes, {} lines: {:?}{ellipsis})",
            self.len(),
            self.line_count(),
            String::from_utf8_lossy(&head)
        )
    }
}
