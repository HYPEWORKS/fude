//! Iterating over a rope's leaves.

use std::iter::FusedIterator;
use std::ops::Range;

use super::node::{Child, Node};
use super::{LineSpan, Rope};

/// The bytes of a range of a rope, as the slices it is stored in. Chunks are never empty
/// and are usually a few KB, but may start or end anywhere, including inside a UTF-8
/// sequence or between `\r` and `\n`.
#[derive(Clone)]
pub struct Chunks<'a> {
    /// The path to the current leaf: each internal node's children and the index taken.
    stack: Vec<(&'a [Child], usize)>,
    leaf: &'a [u8],
    /// Offset of `leaf` in the rope.
    leaf_start: usize,
    range: Range<usize>,
}

impl<'a> Chunks<'a> {
    pub(super) fn new(root: &'a Child, range: Range<usize>) -> Chunks<'a> {
        let mut chunks = Chunks {
            stack: Vec::new(),
            leaf: &[],
            leaf_start: 0,
            range,
        };
        if !chunks.range.is_empty() {
            chunks.descend(root, chunks.range.start);
        }
        chunks
    }

    /// Walks down from `node` to the leaf containing relative offset `at`.
    fn descend(&mut self, mut node: &'a Child, mut at: usize) {
        loop {
            match &node.node {
                Node::Leaf(leaf) => {
                    self.leaf = &leaf.bytes;
                    return;
                }
                Node::Internal(int) => {
                    let mut i = 0;
                    while i + 1 < int.children.len() && at >= int.children[i].len() {
                        at -= int.children[i].len();
                        self.leaf_start += int.children[i].len();
                        i += 1;
                    }
                    self.stack.push((&int.children, i));
                    node = &int.children[i];
                }
            }
        }
    }

    /// Moves to the next leaf, or past the end.
    fn advance(&mut self) {
        self.leaf_start += self.leaf.len();
        self.leaf = &[];
        while let Some(top) = self.stack.last_mut() {
            let children = top.0;
            if top.1 + 1 < children.len() {
                top.1 += 1;
                let next = &children[top.1];
                self.descend(next, 0);
                return;
            }
            self.stack.pop();
        }
    }
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.leaf.is_empty() || self.leaf_start >= self.range.end {
            return None;
        }
        let start = self.range.start.saturating_sub(self.leaf_start);
        let end = (self.range.end - self.leaf_start).min(self.leaf.len());
        let chunk = &self.leaf[start..end];
        self.advance();
        Some(chunk)
    }
}

impl FusedIterator for Chunks<'_> {}

/// Successive lines; see [`Rope::lines_from`].
#[derive(Clone)]
pub struct Lines<'a> {
    rope: &'a Rope,
    chunks: Chunks<'a>,
    chunk: &'a [u8],
    /// Offset of `chunk` in the rope.
    chunk_start: usize,
    /// Where the next line starts.
    pos: usize,
    done: bool,
}

impl<'a> Lines<'a> {
    pub(super) fn new(rope: &'a Rope, start: usize) -> Lines<'a> {
        Lines {
            rope,
            chunks: rope.chunks_in(start..rope.len()),
            chunk: &[],
            chunk_start: start,
            pos: start,
            done: false,
        }
    }
}

impl Iterator for Lines<'_> {
    type Item = LineSpan;

    fn next(&mut self) -> Option<LineSpan> {
        if self.done {
            return None;
        }
        let start = self.pos;
        loop {
            // Scan from the line start, or this chunk's start if the line began in an
            // earlier chunk. `pos` can also be past the chunk, when a CRLF pair straddled it.
            let local = self.pos.saturating_sub(self.chunk_start);
            if local < self.chunk.len()
                && let Some(i) = memchr::memchr2(b'\r', b'\n', &self.chunk[local..])
            {
                let i = local + i;
                let at = self.chunk_start + i;
                let crlf = self.chunk[i] == b'\r'
                    && match self.chunk.get(i + 1) {
                        Some(&b) => b == b'\n',
                        None => self.rope.get(at + 1) == Some(b'\n'),
                    };
                self.pos = at + 1 + usize::from(crlf);
                return Some(LineSpan {
                    start,
                    end: at,
                    next: self.pos,
                });
            }
            match self.chunks.next() {
                Some(chunk) => {
                    self.chunk_start += self.chunk.len();
                    self.chunk = chunk;
                }
                None => {
                    self.done = true;
                    let len = self.rope.len();
                    return Some(LineSpan {
                        start,
                        end: len,
                        next: len,
                    });
                }
            }
        }
    }
}

impl FusedIterator for Lines<'_> {}
