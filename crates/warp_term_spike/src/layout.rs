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

    /// Remove the leaf `target`, collapsing the parent split. Returns the node
    /// to replace `self` with (None if the whole subtree is gone).
    pub fn close_leaf(self, target: PaneId) -> Option<Node> {
        match self {
            Node::Leaf(id) if id == target => None,
            leaf @ Node::Leaf(_) => Some(leaf),
            Node::Split {
                dir,
                ratio,
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
