use egui::{pos2, vec2, Rect};

use crate::pane::PaneId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    /// Panes left | right (Terminator "split vertically", Ctrl+Shift+E).
    SideBySide,
    /// Panes top / bottom (Terminator "split horizontally", Ctrl+Shift+O).
    Stacked,
}

/// The split tree of one tab.
pub enum Node {
    Leaf(PaneId),
    Split { orient: Orientation, ratio: f32, a: Box<Node>, b: Box<Node> },
}

/// A draggable separator between two children of a split.
pub struct Divider {
    pub path: Vec<bool>,
    pub orient: Orientation,
    pub rect: Rect,
    pub parent: Rect,
}

impl Node {
    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            Node::Leaf(x) => *x == id,
            Node::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    pub fn leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(x) => out.push(*x),
            Node::Split { a, b, .. } => {
                a.leaves(out);
                b.leaves(out);
            }
        }
    }

    pub fn first_leaf(&self) -> PaneId {
        match self {
            Node::Leaf(x) => *x,
            Node::Split { a, .. } => a.first_leaf(),
        }
    }

    /// Replace leaf `target` with a split holding `target` and `new`.
    pub fn split(&mut self, target: PaneId, new: PaneId, orient: Orientation) -> bool {
        match self {
            Node::Leaf(x) if *x == target => {
                *self = Node::Split {
                    orient,
                    ratio: 0.5,
                    a: Box::new(Node::Leaf(target)),
                    b: Box::new(Node::Leaf(new)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split(target, new, orient) || b.split(target, new, orient),
        }
    }

    /// Remove a leaf; its sibling takes the parent's place. `None` if the tree became empty.
    pub fn remove(self, target: PaneId) -> Option<Node> {
        match self {
            Node::Leaf(x) => (x != target).then_some(Node::Leaf(x)),
            Node::Split { orient, ratio, a, b } => match (a.remove(target), b.remove(target)) {
                (Some(a), Some(b)) => Some(Node::Split { orient, ratio, a: Box::new(a), b: Box::new(b) }),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            },
        }
    }

    /// Move the nearest enclosing divider of the given orientation.
    pub fn adjust(&mut self, target: PaneId, orient: Orientation, delta: f32) -> bool {
        let Node::Split { orient: o, ratio, a, b } = self else { return false };
        if !(a.contains(target) || b.contains(target)) {
            return false;
        }
        if a.adjust(target, orient, delta) || b.adjust(target, orient, delta) {
            return true;
        }
        if *o == orient {
            *ratio = (*ratio + delta).clamp(0.05, 0.95);
            return true;
        }
        false
    }

    pub fn set_ratio_at(&mut self, path: &[bool], value: f32) {
        match (self, path.split_first()) {
            (Node::Split { ratio, .. }, None) => *ratio = value.clamp(0.05, 0.95),
            (Node::Split { a, b, .. }, Some((&right, rest))) => {
                if right { b.set_ratio_at(rest, value) } else { a.set_ratio_at(rest, value) }
            }
            _ => {}
        }
    }

    pub fn layout(
        &self,
        rect: Rect,
        gap: f32,
        panes: &mut Vec<(PaneId, Rect)>,
        dividers: &mut Vec<Divider>,
        path: &mut Vec<bool>,
    ) {
        match self {
            Node::Leaf(id) => panes.push((*id, rect)),
            Node::Split { orient, ratio, a, b } => {
                let (ra, rd, rb) = match orient {
                    Orientation::SideBySide => {
                        let wa = ((rect.width() - gap) * ratio).round();
                        (
                            Rect::from_min_size(rect.min, vec2(wa, rect.height())),
                            Rect::from_min_size(pos2(rect.min.x + wa, rect.min.y), vec2(gap, rect.height())),
                            Rect::from_min_max(pos2(rect.min.x + wa + gap, rect.min.y), rect.max),
                        )
                    }
                    Orientation::Stacked => {
                        let ha = ((rect.height() - gap) * ratio).round();
                        (
                            Rect::from_min_size(rect.min, vec2(rect.width(), ha)),
                            Rect::from_min_size(pos2(rect.min.x, rect.min.y + ha), vec2(rect.width(), gap)),
                            Rect::from_min_max(pos2(rect.min.x, rect.min.y + ha + gap), rect.max),
                        )
                    }
                };
                dividers.push(Divider { path: path.clone(), orient: *orient, rect: rd, parent: rect });
                path.push(false);
                a.layout(ra, gap, panes, dividers, path);
                path.pop();
                path.push(true);
                b.layout(rb, gap, panes, dividers, path);
                path.pop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_remove() {
        let mut root = Node::Leaf(1);
        assert!(root.split(1, 2, Orientation::SideBySide));
        assert!(root.split(2, 3, Orientation::Stacked));
        let mut v = vec![];
        root.leaves(&mut v);
        assert_eq!(v, vec![1, 2, 3]);

        let root = root.remove(2).unwrap();
        let mut v = vec![];
        root.leaves(&mut v);
        assert_eq!(v, vec![1, 3]);

        let root = root.remove(1).unwrap();
        assert!(matches!(root, Node::Leaf(3)));
        assert!(root.remove(3).is_none());
    }

    #[test]
    fn layout_fills_rect() {
        let mut root = Node::Leaf(1);
        root.split(1, 2, Orientation::SideBySide);
        let (mut p, mut d) = (vec![], vec![]);
        root.layout(Rect::from_min_size(pos2(0.0, 0.0), vec2(104.0, 50.0)), 4.0, &mut p, &mut d, &mut vec![]);
        assert_eq!(p.len(), 2);
        assert_eq!(d.len(), 1);
        assert_eq!(p[0].1.width(), 50.0);
        assert_eq!(p[1].1.min.x, 54.0);
    }
}
