//! Building a rope bottom-up from a stream of bytes.

use super::node::Child;
use super::params::{BUILD_LEAF, MAX_CHILDREN, MIN_CHILDREN};
use super::tree::{from_children, is_boundary};
use super::{Rope, tree};

/// Builds a rope from bytes pushed in order, in O(n). Leaves are filled to a little under
/// the maximum, leaving room for typing before a leaf has to split.
#[derive(Default)]
pub struct RopeBuilder {
    leaves: Vec<Child>,
    current: Vec<u8>,
    len: usize,
}

impl RopeBuilder {
    pub fn new() -> RopeBuilder {
        RopeBuilder::default()
    }

    /// Bytes pushed so far.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn push(&mut self, mut bytes: &[u8]) {
        self.len += bytes.len();
        while !bytes.is_empty() {
            if self.current.capacity() == 0 {
                self.current.reserve_exact(BUILD_LEAF);
            }
            let room = BUILD_LEAF - self.current.len();
            if bytes.len() <= room {
                self.current.extend_from_slice(bytes);
                return;
            }
            // Fill the leaf, backing off up to 3 bytes to avoid splitting a CRLF pair or a
            // UTF-8 sequence. The leaf stays far above the minimum either way.
            let cut = (room.saturating_sub(3).max(1)..=room)
                .rev()
                .find(|&c| is_boundary(bytes[c - 1], bytes[c]))
                .unwrap_or(room);
            self.current.extend_from_slice(&bytes[..cut]);
            self.flush();
            bytes = &bytes[cut..];
        }
    }

    fn flush(&mut self) {
        let bytes = std::mem::take(&mut self.current);
        self.leaves.push(Child::leaf(bytes));
    }

    pub fn finish(mut self) -> Rope {
        if !self.current.is_empty() {
            self.flush();
        }
        let mut level = self.leaves;
        // Only the last leaf can be short; rebalance it with its neighbour.
        if level.len() >= 2 && level.last().is_some_and(Child::is_underfull) {
            let last = level.pop().expect("two leaves");
            let prev = level.pop().expect("two leaves");
            level.extend(tree::concat_siblings(prev, last));
        }
        while level.len() > 1 {
            level = group(level);
        }
        Rope {
            root: from_children(level),
        }
    }
}

/// Groups one level of nodes into parents of `MAX_CHILDREN`, splitting the tail evenly
/// between the last two parents if it would otherwise be underfull.
fn group(children: Vec<Child>) -> Vec<Child> {
    let n = children.len();
    let height = children[0].height() + 1;
    let mut sizes = vec![MAX_CHILDREN; n / MAX_CHILDREN];
    let rem = n % MAX_CHILDREN;
    if rem > 0 {
        if rem >= MIN_CHILDREN || sizes.is_empty() {
            sizes.push(rem);
        } else {
            let both = sizes.pop().expect("a full group") + rem;
            sizes.extend([both - both / 2, both / 2]);
        }
    }
    let mut it = children.into_iter();
    sizes
        .into_iter()
        .map(|size| Child::internal(height, it.by_ref().take(size).collect()))
        .collect()
}
