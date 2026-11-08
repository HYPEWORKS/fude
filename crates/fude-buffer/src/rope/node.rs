//! Tree nodes.

use std::sync::Arc;

use super::params::{MIN_CHILDREN, MIN_LEAF};
use super::summary::Summary;

/// A shared, copy-on-write node.
#[derive(Clone)]
pub(super) enum Node {
    Leaf(Arc<Leaf>),
    Internal(Arc<Internal>),
}

#[derive(Clone, Default)]
pub(super) struct Leaf {
    pub bytes: Vec<u8>,
}

#[derive(Clone)]
pub(super) struct Internal {
    /// Distance to the leaves: 1 for a node whose children are leaves.
    pub height: usize,
    pub children: Vec<Child>,
}

/// A node and its summary. Parents store these inline, so a descent reads the summaries of
/// all children from one array instead of chasing a pointer per child.
#[derive(Clone)]
pub(super) struct Child {
    pub summary: Summary,
    pub node: Node,
}

impl Default for Child {
    fn default() -> Child {
        Child::leaf(Vec::new())
    }
}

impl Child {
    pub fn leaf(bytes: Vec<u8>) -> Child {
        Child {
            summary: Summary::of(&bytes),
            node: Node::Leaf(Arc::new(Leaf { bytes })),
        }
    }

    pub fn internal(height: usize, children: Vec<Child>) -> Child {
        Child {
            summary: fold(&children),
            node: Node::Internal(Arc::new(Internal { height, children })),
        }
    }

    pub fn len(&self) -> usize {
        self.summary.bytes
    }

    pub fn height(&self) -> usize {
        match &self.node {
            Node::Leaf(_) => 0,
            Node::Internal(n) => n.height,
        }
    }

    /// Below the minimum size, which only a root may be.
    pub fn is_underfull(&self) -> bool {
        match &self.node {
            Node::Leaf(l) => l.bytes.len() < MIN_LEAF,
            Node::Internal(n) => n.children.len() < MIN_CHILDREN,
        }
    }
}

pub(super) fn fold(children: &[Child]) -> Summary {
    children
        .iter()
        .fold(Summary::default(), |acc, c| acc.join(c.summary))
}
