//! The processes of a snapshot as a tree, by parent.

use crate::Snapshot;

/// A snapshot's processes by parent: indexes into its processes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    /// The processes without a parent (it exited, or they have none), in
    /// the order of their start (then ID).
    pub roots: Vec<usize>,
    /// Each process's children, in the order of their start.
    pub children: Vec<Vec<usize>>,
    /// Each process's parent.
    pub parent: Vec<Option<usize>>,
}

impl Tree {
    /// `s`'s processes as a tree.
    pub fn of(s: &Snapshot) -> Tree {
        let n = s.processes.len();
        let mut parent: Vec<Option<usize>> = (0..n).map(|i| s.parent(i)).collect();
        // (A loop of parents, as IDs reused make possible where starts are
        // unknown: its members are roots.)
        for i in 0..n {
            let mut at = i;
            let mut steps = 0;
            while let Some(p) = parent[at] {
                steps += 1;
                if p == i || steps > n {
                    parent[i] = None;
                    break;
                }
                at = p;
            }
        }
        let order = |a: &usize, b: &usize| {
            let (pa, pb) = (&s.processes[*a].key, &s.processes[*b].key);
            (pa.start, pa.pid).cmp(&(pb.start, pb.pid))
        };
        let mut children = vec![Vec::new(); n];
        let mut roots = Vec::new();
        for (i, p) in parent.iter().enumerate() {
            match p {
                Some(p) => children[*p].push(i),
                None => roots.push(i),
            }
        }
        roots.sort_by(order);
        for c in &mut children {
            c.sort_by(order);
        }
        Tree {
            roots,
            children,
            parent,
        }
    }

    /// Every process, depth first (each before its children), with its
    /// depth.
    pub fn depth_first(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::with_capacity(self.parent.len());
        let mut stack: Vec<(usize, usize)> = self.roots.iter().rev().map(|&r| (r, 0)).collect();
        while let Some((i, depth)) = stack.pop() {
            out.push((i, depth));
            stack.extend(self.children[i].iter().rev().map(|&c| (c, depth + 1)));
        }
        out
    }

    /// The process at `i` and every process descended from it, each before
    /// its children.
    pub fn descendants(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![i];
        while let Some(i) = stack.pop() {
            out.push(i);
            stack.extend(self.children[i].iter().rev());
        }
        out
    }
}
