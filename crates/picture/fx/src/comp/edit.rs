// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Editing a graph by hand: connect, disconnect, duplicate, splice a node
//! onto a wire. These sit beside [`Graph::add`] and [`Graph::remove`] (the
//! agent's calls, unchanged) and give the node editor one place for the
//! rules, so a bad edit is refused here and the graph stays as it was.

use super::{Graph, Node, NodeId};

impl Graph {
    /// Feeds input `slot` of `dst` from `src`. Refused (graph unchanged) for
    /// an unknown node, a slot the kind does not have, `src == dst`, or a
    /// connection that would make a loop.
    pub fn connect(&mut self, dst: NodeId, slot: usize, src: NodeId) -> Result<(), String> {
        if src == dst {
            return Err("a node cannot feed itself".into());
        }
        if self.node(src).is_none() {
            return Err(format!("no node {src}"));
        }
        let count = self.node(dst).ok_or_else(|| format!("no node {dst}"))?.def().map_or(0, |d| d.inputs.len());
        if slot >= count {
            return Err(format!("node {dst} has no input {slot}"));
        }
        let before = self.clone();
        if let Some(n) = self.node_mut(dst) {
            while n.inputs.len() <= slot {
                n.inputs.push(None);
            }
            n.inputs[slot] = Some(src);
        }
        if let Err(e) = self.check() {
            *self = before;
            return Err(e);
        }
        Ok(())
    }

    /// Feeds `dst`'s mask from `src`; same refusals as [`Graph::connect`].
    pub fn connect_mask(&mut self, dst: NodeId, src: NodeId) -> Result<(), String> {
        if src == dst {
            return Err("a node cannot mask itself".into());
        }
        if self.node(src).is_none() || self.node(dst).is_none() {
            return Err("no such node".into());
        }
        let before = self.clone();
        if let Some(n) = self.node_mut(dst) {
            n.mask = Some(src);
        }
        if let Err(e) = self.check() {
            *self = before;
            return Err(e);
        }
        Ok(())
    }

    /// Cuts input `slot` of `dst`. True if something was connected.
    pub fn disconnect(&mut self, dst: NodeId, slot: usize) -> bool {
        match self.node_mut(dst) {
            Some(n) if n.input(slot).is_some() => {
                n.inputs[slot] = None;
                true
            }
            _ => false,
        }
    }

    /// Cuts `dst`'s mask. True if there was one.
    pub fn disconnect_mask(&mut self, dst: NodeId) -> bool {
        self.node_mut(dst).is_some_and(|n| n.mask.take().is_some())
    }

    /// Copies `ids` (never the `input` node), `by` away from the originals.
    /// Wires between copied nodes are kept between the copies; wires from
    /// outside still feed them. The output does not change. Returns the new
    /// ids, in the order of `ids`.
    pub fn duplicate(&mut self, ids: &[NodeId], by: [f32; 2]) -> Vec<NodeId> {
        let picked: Vec<NodeId> = ids.iter().copied().filter(|i| self.node(*i).is_some_and(|n| n.kind != "input")).collect();
        let mut next = self.next_id();
        let map: Vec<(NodeId, NodeId)> = picked
            .iter()
            .map(|&old| {
                let new = next;
                next += 1;
                (old, new)
            })
            .collect();
        let remap = |i: NodeId| map.iter().find(|(o, _)| *o == i).map_or(i, |(_, n)| *n);
        let mut copies: Vec<Node> = Vec::new();
        for &(old, new) in &map {
            if let Some(src) = self.node(old) {
                let mut c = src.clone();
                c.id = new;
                c.inputs = c.inputs.iter().map(|i| i.map(remap)).collect();
                c.mask = c.mask.map(remap);
                c.pos = [c.pos[0] + by[0], c.pos[1] + by[1]];
                copies.push(c);
            }
        }
        self.nodes.extend(copies);
        map.into_iter().map(|(_, n)| n).collect()
    }

    /// Adds a node of `kind` onto the wire into input `slot` of `dst`: the
    /// new node reads what `dst` read, and `dst` reads the new node. The
    /// output does not change. `None` if `dst` or the slot is not there or
    /// the kind is unknown.
    pub fn splice(&mut self, kind: &str, dst: NodeId, slot: usize) -> Option<NodeId> {
        let def = super::nodes::find(kind)?;
        let count = self.node(dst)?.def().map_or(0, |d| d.inputs.len());
        if slot >= count {
            return None;
        }
        let src = self.node(dst)?.input(slot);
        let was = self.output;
        let id = self.add(kind, &if def.inputs.is_empty() { Vec::new() } else { vec![src] });
        self.output = was;
        let n = self.node_mut(dst)?;
        while n.inputs.len() <= slot {
            n.inputs.push(None);
        }
        n.inputs[slot] = Some(id);
        Some(id)
    }

    /// The box the nodes sit in, `[left, top, right, bottom]` by their
    /// positions.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        let mut it = self.nodes.iter().map(|n| n.pos);
        let first = it.next()?;
        Some(it.fold([first[0], first[1], first[0], first[1]], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_refuses_loops_self_and_missing_slots() {
        let mut g = Graph::passthrough();
        let a = g.add("blur", &[Some(1)]);
        let b = g.add("blur", &[Some(a)]);
        let before = g.clone();
        assert!(g.connect(a, 0, b).is_err(), "a loop");
        assert!(g.connect(a, 0, a).is_err(), "itself");
        assert!(g.connect(a, 3, 1).is_err(), "no such slot");
        assert!(g.connect(a, 0, 99).is_err(), "no such node");
        assert_eq!(g, before, "a refused edit changes nothing");
        assert!(g.connect(b, 0, 1).is_ok());
        assert_eq!(g.node(b).unwrap().inputs, vec![Some(1)]);
    }

    #[test]
    fn disconnect_cuts_one_wire() {
        let mut g = Graph::passthrough();
        let a = g.add("blur", &[Some(1)]);
        assert!(g.disconnect(a, 0));
        assert!(!g.disconnect(a, 0), "already cut");
        assert_eq!(g.node(a).unwrap().input(0), None);
        assert!(g.check().is_ok());
    }

    #[test]
    fn duplicate_keeps_inner_wires_and_skips_the_input() {
        let mut g = Graph::passthrough();
        let a = g.add("blur", &[Some(1)]);
        let b = g.add("blur", &[Some(a)]);
        let out = g.output_id();
        let copies = g.duplicate(&[1, a, b], [20.0, 30.0]);
        assert_eq!(copies.len(), 2, "the input stays single");
        let (ca, cb) = (copies[0], copies[1]);
        assert_eq!(g.node(ca).unwrap().inputs, vec![Some(1)], "outside wire kept");
        assert_eq!(g.node(cb).unwrap().inputs, vec![Some(ca)], "inner wire between copies");
        assert_eq!(g.node(ca).unwrap().pos[0], g.node(a).unwrap().pos[0] + 20.0);
        assert_eq!(g.output_id(), out);
        assert!(g.check().is_ok());
    }

    #[test]
    fn splice_puts_a_node_on_a_wire() {
        let mut g = Graph::passthrough();
        let a = g.add("blur", &[Some(1)]);
        let s = g.splice("grade", a, 0).unwrap();
        assert_eq!(g.node(s).unwrap().inputs, vec![Some(1)]);
        assert_eq!(g.node(a).unwrap().inputs, vec![Some(s)]);
        assert_eq!(g.output_id(), Some(a), "the output stays put");
        assert!(g.splice("grade", a, 5).is_none());
        assert!(g.splice("nonsense", a, 0).is_none());
        assert!(g.check().is_ok());
    }

    #[test]
    fn bounds_cover_every_node() {
        let mut g = Graph::passthrough();
        let a = g.add("blur", &[Some(1)]);
        g.node_mut(a).unwrap().pos = [-40.0, 90.0];
        assert_eq!(g.bounds(), Some([-40.0, 0.0, 0.0, 90.0]));
        assert_eq!(Graph::default().bounds(), None);
    }
}
