//! B-tree algorithms: concatenation, splitting and in-place leaf edits.
//!
//! Invariants (checked by `Rope::check_invariants`):
//! - all leaves are at the same depth;
//! - a non-root leaf holds `MIN_LEAF..=MAX_LEAF` bytes, a root leaf at most `MAX_LEAF`;
//! - a non-root internal node has `MIN_CHILDREN..=MAX_CHILDREN` children, a root at least 2;
//! - cached summaries are exact.
//!
//! Only roots may be underfull, so concatenation only has to repair the seam where an
//! underfull root joins another tree, and splitting builds its halves out of whole valid
//! subtrees joined with `concat`.

use std::sync::Arc;

use super::node::{Child, Internal, Node, fold};
use super::params::{MAX_CHILDREN, MAX_LEAF, MIN_LEAF};
use super::summary::Summary;

/// Extra capacity reserved when an edit grows a leaf, so typing doesn't reallocate per key.
const LEAF_SLACK: usize = 64;

/// Joins two trees.
pub(super) fn concat(a: Child, b: Child) -> Child {
    if a.len() == 0 {
        return b;
    }
    if b.len() == 0 {
        return a;
    }
    let nodes = match a.height().cmp(&b.height()) {
        std::cmp::Ordering::Equal => merge_siblings(a, b),
        std::cmp::Ordering::Greater => append(a, b),
        std::cmp::Ordering::Less => prepend(a, b),
    };
    root_of(nodes)
}

/// Splits a tree at byte `at` into two trees.
pub(super) fn split(node: &Child, at: usize) -> (Child, Child) {
    debug_assert!(at <= node.len());
    if at == 0 {
        return (Child::default(), node.clone());
    }
    if at == node.len() {
        return (node.clone(), Child::default());
    }
    match &node.node {
        Node::Leaf(leaf) => (
            Child::leaf(leaf.bytes[..at].to_vec()),
            Child::leaf(leaf.bytes[at..].to_vec()),
        ),
        Node::Internal(int) => {
            let children = &int.children;
            let mut offset = 0;
            for (i, c) in children.iter().enumerate() {
                if at == offset {
                    return (
                        from_children(children[..i].to_vec()),
                        from_children(children[i..].to_vec()),
                    );
                }
                let end = offset + c.len();
                if at < end {
                    let (l, r) = split(c, at - offset);
                    return (
                        concat(from_children(children[..i].to_vec()), l),
                        concat(r, from_children(children[i + 1..].to_vec())),
                    );
                }
                offset = end;
            }
            unreachable!("split point inside the node")
        }
    }
}

/// A tree made of sibling subtrees (all valid non-root nodes of one height).
pub(super) fn from_children(mut children: Vec<Child>) -> Child {
    match children.len() {
        0 => Child::default(),
        1 => children.pop().expect("one child"),
        _ => Child::internal(children[0].height() + 1, children),
    }
}

/// Replaces `start..end` with `text` inside a single leaf, mutating shared nodes copy-on-write.
/// Returns false, changing nothing, if the edit spans leaves or would take the leaf outside
/// its size limits; the caller then splits and concatenates instead.
pub(super) fn edit_in_place(root: &mut Child, start: usize, end: usize, text: &[u8]) -> bool {
    let Some(leaf_len) = leaf_len_for(root, start, end) else {
        return false;
    };
    let new_len = leaf_len - (end - start) + text.len();
    let is_root = matches!(root.node, Node::Leaf(_));
    if new_len > MAX_LEAF || (!is_root && new_len < MIN_LEAF) {
        return false;
    }
    edit_leaf(root, start, end, text);
    true
}

/// The child of `int` that contains `start..end`, and its offset. An insertion at a boundary
/// goes to the left child.
fn child_for(int: &Internal, start: usize, end: usize) -> Option<(usize, usize)> {
    let mut offset = 0;
    for (i, c) in int.children.iter().enumerate() {
        let next = offset + c.len();
        if start < next || (start == next && end == next) {
            return (end <= next).then_some((i, offset));
        }
        offset = next;
    }
    None
}

fn leaf_len_for(node: &Child, start: usize, end: usize) -> Option<usize> {
    match &node.node {
        Node::Leaf(leaf) => Some(leaf.bytes.len()),
        Node::Internal(int) => {
            let (i, offset) = child_for(int, start, end)?;
            leaf_len_for(&int.children[i], start - offset, end - offset)
        }
    }
}

fn edit_leaf(child: &mut Child, start: usize, end: usize, text: &[u8]) {
    let Child { summary, node } = child;
    match node {
        Node::Leaf(leaf) => {
            let bytes = &mut Arc::make_mut(leaf).bytes;
            let new_len = bytes.len() - (end - start) + text.len();
            if new_len > bytes.capacity() {
                let target = (new_len + LEAF_SLACK).min(MAX_LEAF).max(new_len);
                bytes.reserve_exact(target - bytes.len());
            }
            bytes.splice(start..end, text.iter().copied());
            *summary = Summary::of(bytes);
        }
        Node::Internal(int) => {
            let int = Arc::make_mut(int);
            let (i, offset) = child_for(int, start, end).expect("checked by leaf_len_for");
            edit_leaf(&mut int.children[i], start - offset, end - offset, text);
            *summary = fold(&int.children);
        }
    }
}

/// One or two nodes as a tree.
fn root_of(mut nodes: Vec<Child>) -> Child {
    if nodes.len() == 1 {
        nodes.pop().expect("one node")
    } else {
        Child::internal(nodes[0].height() + 1, nodes)
    }
}

/// Same-height siblings, rebalanced if either is underfull (for the rope builder).
pub(super) fn concat_siblings(x: Child, y: Child) -> Vec<Child> {
    merge_siblings(x, y)
}

/// Puts two same-height nodes side by side. A valid pair is left alone; if either is
/// underfull (a root), they are merged into one node or rebalanced into two valid ones.
fn merge_siblings(x: Child, y: Child) -> Vec<Child> {
    if !x.is_underfull() && !y.is_underfull() {
        return vec![x, y];
    }
    match (x.node, y.node) {
        (Node::Leaf(lx), Node::Leaf(ly)) => {
            let mut bytes = Arc::unwrap_or_clone(lx).bytes;
            bytes.extend_from_slice(&ly.bytes);
            let total = bytes.len();
            if total <= MAX_LEAF {
                return vec![Child::leaf(bytes)];
            }
            let lo = MIN_LEAF.max(total - MAX_LEAF);
            let hi = MAX_LEAF.min(total - MIN_LEAF);
            let at = boundary_near(&bytes, total / 2, lo, hi);
            vec![
                Child::leaf(bytes[..at].to_vec()),
                Child::leaf(bytes[at..].to_vec()),
            ]
        }
        (Node::Internal(ix), Node::Internal(iy)) => {
            let height = ix.height;
            let mut children = Arc::unwrap_or_clone(ix).children;
            children.extend(iy.children.iter().cloned());
            internal_nodes(height, children)
        }
        _ => unreachable!("siblings have the same height"),
    }
}

/// Appends `b` to the taller tree `a`. Returns one or two nodes of `a`'s height.
fn append(a: Child, b: Child) -> Vec<Child> {
    let mut int = take_internal(a);
    let last = int.children.pop().expect("internal nodes have children");
    let replacement = if last.height() == b.height() {
        merge_siblings(last, b)
    } else {
        append(last, b)
    };
    int.children.extend(replacement);
    internal_nodes(int.height, int.children)
}

/// Prepends `a` to the taller tree `b`. Returns one or two nodes of `b`'s height.
fn prepend(a: Child, b: Child) -> Vec<Child> {
    let mut int = take_internal(b);
    let first = int.children.remove(0);
    let replacement = if first.height() == a.height() {
        merge_siblings(a, first)
    } else {
        prepend(a, first)
    };
    int.children.splice(0..0, replacement);
    internal_nodes(int.height, int.children)
}

fn take_internal(child: Child) -> Internal {
    match child.node {
        Node::Internal(int) => Arc::unwrap_or_clone(int),
        Node::Leaf(_) => unreachable!("a taller tree is internal"),
    }
}

/// One internal node, or two if there are too many children for one. Two halves of more
/// than `MAX_CHILDREN` each have at least `MIN_CHILDREN`.
fn internal_nodes(height: usize, mut children: Vec<Child>) -> Vec<Child> {
    if children.len() <= MAX_CHILDREN {
        return vec![Child::internal(height, children)];
    }
    let right = children.split_off(children.len() / 2);
    vec![
        Child::internal(height, children),
        Child::internal(height, right),
    ]
}

/// A split point in `lo..=hi`, as close to `ideal` as possible, that doesn't separate `\r`
/// from `\n` or cut through a UTF-8 sequence. Leaf boundaries don't affect correctness;
/// this just keeps most leaves valid UTF-8 on their own.
pub(super) fn boundary_near(bytes: &[u8], ideal: usize, lo: usize, hi: usize) -> usize {
    debug_assert!(lo <= ideal && ideal <= hi && hi < bytes.len());
    let good = |at: usize| at > 0 && is_boundary(bytes[at - 1], bytes[at]);
    (0..=3)
        .flat_map(|d| [ideal.checked_sub(d), Some(ideal + d)])
        .flatten()
        .find(|&at| (lo..=hi).contains(&at) && good(at))
        .unwrap_or(ideal)
}

pub(super) fn is_boundary(before: u8, after: u8) -> bool {
    let crlf = before == b'\r' && after == b'\n';
    let continuation = after & 0xC0 == 0x80;
    !crlf && !continuation
}
