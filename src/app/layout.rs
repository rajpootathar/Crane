//! The split-tree layout inside a Tab — the warpui port of Crane's
//! `Node::Leaf` / `Node::Split`. Each leaf is a `PaneId` (a persistent
//! terminal view). Splits carry a draggable `ratio`.

use std::cell::Cell;
use std::rc::Rc;

pub type PaneId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Horizontal, // side-by-side (a row)
    Vertical,   // stacked (a column)
}

pub enum Node {
    Leaf(PaneId),
    Split {
        dir: Dir,
        ratio: Rc<Cell<f32>>,
        /// Drag state — MUST persist here (not in the transient SplitBox, which
        /// is rebuilt every frame) so a drag survives re-renders between
        /// mouse-down and mouse-drag.
        dragging: Rc<Cell<bool>>,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    /// Split the leaf `target` into `Split(target, new_pane)` in `dir`.
    /// Returns true if `target` was found.
    pub fn split_leaf(&mut self, target: PaneId, new_pane: PaneId, dir: Dir) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                let first = Box::new(Node::Leaf(*id));
                let second = Box::new(Node::Leaf(new_pane));
                *self = Node::Split {
                    dir,
                    ratio: Rc::new(Cell::new(0.5)),
                    dragging: Rc::new(Cell::new(false)),
                    first,
                    second,
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { first, second, .. } => {
                first.split_leaf(target, new_pane, dir)
                    || second.split_leaf(target, new_pane, dir)
            }
        }
    }

    /// Like `split_leaf`, but `before` controls order: true = `new` becomes the
    /// first (left/top) child, false = second (right/bottom).
    pub fn split_leaf_ordered(
        &mut self,
        target: PaneId,
        new_pane: PaneId,
        dir: Dir,
        before: bool,
    ) -> bool {
        self.split_leaf_at(target, new_pane, dir, before, 0.5)
    }

    /// Like `split_leaf_ordered` but with an explicit first-child `ratio`.
    pub fn split_leaf_at(
        &mut self,
        target: PaneId,
        new_pane: PaneId,
        dir: Dir,
        before: bool,
        ratio: f32,
    ) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                let existing = *id;
                let (first, second) = if before {
                    (new_pane, existing)
                } else {
                    (existing, new_pane)
                };
                *self = Node::Split {
                    dir,
                    ratio: Rc::new(Cell::new(ratio)),
                    dragging: Rc::new(Cell::new(false)),
                    first: Box::new(Node::Leaf(first)),
                    second: Box::new(Node::Leaf(second)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { first, second, .. } => {
                first.split_leaf_at(target, new_pane, dir, before, ratio)
                    || second.split_leaf_at(target, new_pane, dir, before, ratio)
            }
        }
    }

    /// Remove the leaf `target`, collapsing the parent split. Returns the node
    /// to replace `self` with (None if the whole subtree is gone).
    pub fn close_leaf(self, target: PaneId) -> Option<Node> {
        match self {
            Node::Leaf(id) if id == target => None,
            leaf @ Node::Leaf(_) => Some(leaf),
            Node::Split {
                dir,
                ratio,
                dragging,
                first,
                second,
            } => {
                let f = first.close_leaf(target);
                let s = second.close_leaf(target);
                match (f, s) {
                    (None, Some(n)) | (Some(n), None) => Some(n),
                    (Some(f), Some(s)) => Some(Node::Split {
                        dir,
                        ratio,
                        dragging,
                        first: Box::new(f),
                        second: Box::new(s),
                    }),
                    (None, None) => None,
                }
            }
        }
    }

    /// Swap two panes' positions in the tree (drop on center = swap).
    pub fn swap_leaves(&mut self, a: PaneId, b: PaneId) {
        match self {
            Node::Leaf(id) => {
                if *id == a {
                    *id = b;
                } else if *id == b {
                    *id = a;
                }
            }
            Node::Split { first, second, .. } => {
                first.swap_leaves(a, b);
                second.swap_leaves(a, b);
            }
        }
    }

    pub fn first_leaf(&self) -> PaneId {
        match self {
            Node::Leaf(id) => *id,
            Node::Split { first, .. } => first.first_leaf(),
        }
    }

    pub fn leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { first, second, .. } => {
                first.leaves(out);
                second.leaves(out);
            }
        }
    }

    /// True if `id` is a leaf anywhere in this subtree. Non-allocating
    /// alternative to `leaves(&mut Vec::new()).contains(&id)` — callers that
    /// only need membership (e.g. "which Tab owns this pane?") used to pay a
    /// heap allocation per Layout per call for a Vec they immediately
    /// discarded.
    pub fn contains_leaf(&self, id: PaneId) -> bool {
        match self {
            Node::Leaf(pid) => *pid == id,
            Node::Split { first, second, .. } => {
                first.contains_leaf(id) || second.contains_leaf(id)
            }
        }
    }
}
