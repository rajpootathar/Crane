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
                    ratio: Rc::new(Cell::new(0.5)),
                    dragging: Rc::new(Cell::new(false)),
                    first: Box::new(Node::Leaf(first)),
                    second: Box::new(Node::Leaf(second)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { first, second, .. } => {
                first.split_leaf_ordered(target, new_pane, dir, before)
                    || second.split_leaf_ordered(target, new_pane, dir, before)
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
}
