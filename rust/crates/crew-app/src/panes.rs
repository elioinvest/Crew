//! How the terminal area is divided: a tree of splits whose leaves each show one session or nothing.
//! Closing a pane doesn't stop its session; it keeps running and can be reopened from the sidebar.

use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// The new pane goes to the right.
    Row,
    /// The new pane goes below.
    Column,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf { id: u64, session: Option<u64> },
    Split { id: u64, dir: Dir, ratio: f32, a: Box<Node>, b: Box<Node> },
}

impl Node {
    fn leaves<'a>(&'a self, out: &mut Vec<(u64, Option<u64>)>) {
        match self {
            Node::Leaf { id, session } => out.push((*id, *session)),
            Node::Split { a, b, .. } => {
                a.leaves(out);
                b.leaves(out);
            }
        }
    }

    fn id(&self) -> u64 {
        match self {
            Node::Leaf { id, .. } | Node::Split { id, .. } => *id,
        }
    }

    fn first_leaf(&self) -> u64 {
        match self {
            Node::Leaf { id, .. } => *id,
            Node::Split { a, .. } => a.first_leaf(),
        }
    }

    fn for_each_leaf(&mut self, f: &mut impl FnMut(u64, &mut Option<u64>)) {
        match self {
            Node::Leaf { id, session } => f(*id, session),
            Node::Split { a, b, .. } => {
                a.for_each_leaf(f);
                b.for_each_leaf(f);
            }
        }
    }

    fn replace(&mut self, target: u64, make: &mut Option<impl FnOnce(Node) -> Node>) -> bool {
        if self.id() == target {
            if let Some(make) = make.take() {
                let old = std::mem::replace(self, Node::Leaf { id: 0, session: None });
                *self = make(old);
            }
            return true;
        }
        match self {
            Node::Leaf { .. } => false,
            Node::Split { a, b, .. } => a.replace(target, make) || b.replace(target, make),
        }
    }

    /// Replaces the split holding leaf `target` with the other side; returns that side's first leaf.
    fn remove_leaf(&mut self, target: u64) -> Option<u64> {
        let Node::Split { a, b, .. } = self else { return None };
        let keep = if a.id() == target {
            Some(std::mem::replace(b.as_mut(), Node::Leaf { id: 0, session: None }))
        } else if b.id() == target {
            Some(std::mem::replace(a.as_mut(), Node::Leaf { id: 0, session: None }))
        } else {
            None
        };
        if let Some(keep) = keep {
            let first = keep.first_leaf();
            *self = keep;
            return Some(first);
        }
        a.remove_leaf(target).or_else(|| b.remove_leaf(target))
    }

    fn set_ratio(&mut self, target: u64, value: f32) {
        if let Node::Split { id, ratio, a, b, .. } = self {
            if *id == target {
                *ratio = value.clamp(0.1, 0.9);
            } else {
                a.set_ratio(target, value);
                b.set_ratio(target, value);
            }
        }
    }
}

pub struct Layout {
    pub root: Node,
    pub focus: u64,
    next: u64,
}

impl Layout {
    pub fn new() -> Self {
        Self { root: Node::Leaf { id: 1, session: None }, focus: 1, next: 2 }
    }

    /// A saved split shape, with every pane empty.
    pub fn restored(shape: Option<&Value>) -> Self {
        let mut layout = Self::new();
        if let Some(root) = shape.and_then(|s| layout.build(s, 0)) {
            layout.focus = root.first_leaf();
            layout.root = root;
        }
        layout
    }

    fn build(&mut self, shape: &Value, depth: usize) -> Option<Node> {
        if depth > 8 {
            return None;
        }
        let Some([first, second]) = shape.get("children").and_then(Value::as_array).map(Vec::as_slice) else {
            return Some(Node::Leaf { id: self.fresh_id(), session: None });
        };
        let dir = if shape.get("dir").and_then(Value::as_str) == Some("column") { Dir::Column } else { Dir::Row };
        let ratio = shape.get("ratio").and_then(Value::as_f64).unwrap_or(0.5).clamp(0.15, 0.85) as f32;
        let (a, b) = (Box::new(self.build(first, depth + 1)?), Box::new(self.build(second, depth + 1)?));
        Some(Node::Split { id: self.fresh_id(), dir, ratio, a, b })
    }

    /// The split shape without what's in the panes, to save.
    pub fn shape(&self) -> Value {
        fn of(node: &Node) -> Value {
            match node {
                Node::Leaf { .. } => json!({}),
                Node::Split { dir, ratio, a, b, .. } => json!({ "dir": if *dir == Dir::Column { "column" } else { "row" }, "ratio": ratio, "children": [of(a), of(b)] }),
            }
        }
        of(&self.root)
    }

    /// Puts a session in a pane. If it's showing in another, the two panes swap what they show.
    pub fn place(&mut self, session: u64, leaf: u64) {
        let from = self.leaf_of(session);
        let there = self.leaves().into_iter().find(|l| l.0 == leaf).and_then(|l| l.1);
        self.root.for_each_leaf(&mut |id, s| {
            if id == leaf {
                *s = Some(session);
            } else if Some(id) == from {
                *s = there;
            }
        });
        self.focus = leaf;
    }

    fn fresh_id(&mut self) -> u64 {
        self.next += 1;
        self.next - 1
    }

    pub fn leaves(&self) -> Vec<(u64, Option<u64>)> {
        let mut out = Vec::new();
        self.root.leaves(&mut out);
        out
    }

    pub fn is_split(&self) -> bool {
        matches!(self.root, Node::Split { .. })
    }

    pub fn focused_session(&self) -> Option<u64> {
        self.leaves().into_iter().find(|l| l.0 == self.focus).and_then(|l| l.1)
    }

    pub fn leaf_of(&self, session: u64) -> Option<u64> {
        self.leaves().into_iter().find(|l| l.1 == Some(session)).map(|l| l.0)
    }

    /// Shows a session in the focused pane. A session is only ever in one pane: if it's already
    /// showing, that pane takes the focus instead.
    pub fn show(&mut self, session: u64) {
        if let Some(leaf) = self.leaf_of(session) {
            self.focus = leaf;
            return;
        }
        let focus = self.focus;
        self.root.for_each_leaf(&mut |id, s| {
            if id == focus {
                *s = Some(session);
            }
        });
    }

    /// The pane showing a session that went away goes empty.
    pub fn forget(&mut self, session: u64) {
        self.root.for_each_leaf(&mut |_, s| {
            if *s == Some(session) {
                *s = None;
            }
        });
    }

    /// The new pane starts empty and focused.
    pub fn split(&mut self, dir: Dir) {
        let (split_id, leaf_id) = (self.fresh_id(), self.fresh_id());
        let mut make = Some(move |old: Node| Node::Split {
            id: split_id,
            dir,
            ratio: 0.5,
            a: Box::new(old),
            b: Box::new(Node::Leaf { id: leaf_id, session: None }),
        });
        self.root.replace(self.focus, &mut make);
        self.focus = leaf_id;
    }

    pub fn close(&mut self, leaf: u64) {
        if let Some(first) = self.root.remove_leaf(leaf) {
            if self.focus == leaf {
                self.focus = first;
            }
        }
    }

    pub fn set_ratio(&mut self, split: u64, ratio: f32) {
        self.root.set_ratio(split, ratio);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shape_comes_back_with_empty_panes() {
        let mut l = Layout::new();
        l.show(7);
        l.split(Dir::Row);
        l.split(Dir::Column);
        let shape = l.shape();
        let back = Layout::restored(Some(&shape));
        assert_eq!(back.shape(), shape);
        assert_eq!(back.leaves().len(), 3);
        assert!(back.leaves().iter().all(|l| l.1.is_none()));
        assert_eq!(back.focus, back.leaves()[0].0);
        // Something else in the setting is just no splits.
        assert_eq!(Layout::restored(Some(&json!("nonsense"))).leaves().len(), 1);
        assert_eq!(Layout::restored(None).leaves().len(), 1);
        assert_eq!(Layout::restored(Some(&json!({ "dir": "row", "ratio": 5, "children": [{}, {}] }))).shape()["ratio"], 0.85f32 as f64);
    }

    #[test]
    fn placing_a_session_swaps_panes() {
        let mut l = Layout::new();
        l.show(1);
        l.split(Dir::Row);
        l.show(2);
        let (first, second) = (l.leaf_of(1).unwrap(), l.leaf_of(2).unwrap());
        l.place(1, second);
        assert_eq!((l.leaf_of(1), l.leaf_of(2)), (Some(second), Some(first)));
        assert_eq!(l.focus, second);
        l.place(1, second);
        assert_eq!(l.leaf_of(1), Some(second), "already there");
        assert_eq!(l.leaf_of(2), Some(first));
    }

    #[test]
    fn splitting_focuses_a_new_empty_pane() {
        let mut l = Layout::new();
        l.show(10);
        l.split(Dir::Row);
        assert!(l.is_split());
        assert_eq!(l.focused_session(), None);
        l.show(20);
        assert_eq!(l.leaves().iter().map(|x| x.1).collect::<Vec<_>>(), [Some(10), Some(20)]);
    }

    #[test]
    fn a_session_shows_in_one_pane_only() {
        let mut l = Layout::new();
        l.show(10);
        l.split(Dir::Row);
        l.show(10);
        assert_eq!(l.focused_session(), Some(10), "the pane already showing it takes the focus");
        assert_eq!(l.leaves().iter().filter(|x| x.1 == Some(10)).count(), 1);
    }

    #[test]
    fn closing_a_pane_keeps_the_rest_and_moves_focus() {
        let mut l = Layout::new();
        l.show(10);
        l.split(Dir::Row);
        l.show(20);
        l.split(Dir::Column);
        l.show(30);
        let focused = l.focus;
        l.close(focused);
        assert_eq!(l.focused_session(), Some(20));
        assert_eq!(l.leaves().len(), 2);
        let (first, _) = l.leaves()[0];
        l.close(first);
        assert!(!l.is_split());
        assert_eq!(l.focused_session(), Some(20));
        let only = l.focus;
        l.close(only); // the last pane stays
        assert_eq!(l.leaves().len(), 1);
    }

    #[test]
    fn a_closed_sessions_pane_goes_empty_and_ratios_stay_in_range() {
        let mut l = Layout::new();
        l.show(10);
        l.split(Dir::Row);
        l.forget(10);
        assert_eq!(l.leaf_of(10), None);
        let Node::Split { id, .. } = l.root else { panic!() };
        l.set_ratio(id, 5.);
        assert!(matches!(l.root, Node::Split { ratio, .. } if ratio == 0.9));
    }
}
