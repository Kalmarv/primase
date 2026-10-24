//! Blend trees: which clips a state plays at a set of parameter values, and with what weight. Only the weights are
//! here (not the time each clip is at, which is the state's); the trees are those of [`crate::animator`], in Unity's
//! compiled form: a node's children are indices of the same tree, a leaf holds the index of the controller's clip it plays.
//!
//! What is implemented, and where its meaning comes from:
//! - a leaf (a state of one clip is a tree of one leaf): weight 1.
//! - 1D (`blend_type` 0): the node's parameter (`event_id`) against the children's thresholds, which must ascend. At or
//!   below the first threshold the first child plays alone, at or above the last the last child; between two the two
//!   neighbouring children play with weights that are linear in the value (Unity's manual, "1D Blending").
//! - direct (`blend_type` 4): child `i`'s weight is the value of its own parameter (`direct_events[i]`); with
//!   `normalized_blend_values` they are divided by their sum (Unity's manual, "Direct Blending"; the manual's wording
//!   is the only source of this rule, no engine number checks it yet).
//! - a child that is itself an inner node splits its weight among its own children (weights multiply).
//!
//! The 2D types (simple directional 1, freeform directional 2, freeform cartesian 3) are refused by name: their weights
//! are Unity's gradient-band algorithms (Johansen's thesis, chapters 6.3 and 6.3.1, for the freeform ones) and are not
//! written here until an engine's own numbers can check them.

use std::collections::BTreeMap;

use crate::animator::{BlendNode, UnityAnimatorController, name_hash};
use crate::{Error, Result, invalid};

/// The values of a controller's parameters, by the CRC-32 of the parameter's name, as the controller's blend trees and
/// transitions name them.
#[derive(Clone, Debug, PartialEq)]
pub struct Parameters(BTreeMap<u32, f32>);

impl Parameters {
    /// Every parameter at its default (a bool or a trigger as 0 or 1, an int as its value).
    pub fn defaults(controller: &UnityAnimatorController) -> Result<Parameters> {
        let mut out = BTreeMap::new();
        for p in &controller.parameters {
            let at =
                usize::try_from(p.index).map_err(|_| invalid(format!("a parameter at index {}", p.index)))?;
            let v = match p.kind {
                1 => controller.default_floats.get(at).copied(),
                3 => controller.default_ints.get(at).map(|&i| i as f32),
                4 | 9 => controller.default_bools.get(at).map(|&b| f32::from(u8::from(b))),
                k => return Err(Error::Unsupported(format!("a parameter of kind {k}"))),
            }
            .ok_or_else(|| invalid(format!("a parameter's default {at} is missing")))?;
            out.insert(p.id, v);
        }
        Ok(Parameters(out))
    }

    /// Set the parameter called `name` (a name the controller has: its CRC-32 is one of its parameters').
    pub fn set(&mut self, name: &str, value: f32) -> Result<()> {
        let id = name_hash(name);
        match self.0.get_mut(&id) {
            Some(v) => {
                *v = value;
                Ok(())
            }
            None => Err(invalid(format!("the controller has no parameter {name:?}"))),
        }
    }

    /// Set the parameter whose name hashes to `id`.
    pub fn set_by_id(&mut self, id: u32, value: f32) -> Result<()> {
        match self.0.get_mut(&id) {
            Some(v) => {
                *v = value;
                Ok(())
            }
            None => Err(invalid(format!("the controller has no parameter {id}"))),
        }
    }

    /// The value of the parameter with this hash.
    pub fn get(&self, id: u32) -> Result<f32> {
        self.0.get(&id).copied().ok_or_else(|| {
            invalid(format!(
                "a blend tree names parameter {id}, which the controller does not have"
            ))
        })
    }
}

/// The clips a blend tree plays at these parameter values: (index into the controller's clips, weight), those of zero
/// weight left out, in the order of the tree's leaves. The weights sum to 1, except a direct tree's not stated
/// normalised, which are its parameters'. The root is the tree's first node.
pub fn child_weights(tree: &[BlendNode], parameters: &Parameters) -> Result<Vec<(u32, f64)>> {
    let mut out = Vec::new();
    node_weights(tree, 0, 1.0, parameters, &mut out, 0)?;
    Ok(out)
}

fn node_weights(
    tree: &[BlendNode],
    at: usize,
    weight: f64,
    parameters: &Parameters,
    out: &mut Vec<(u32, f64)>,
    depth: usize,
) -> Result<()> {
    if depth > tree.len() {
        return Err(invalid("a blend tree that contains itself"));
    }
    let node = tree
        .get(at)
        .ok_or_else(|| invalid(format!("a blend tree node {at} of {}", tree.len())))?;
    if node.children.is_empty() {
        if node.clip == u32::MAX {
            return Err(Error::Unsupported("a blend tree leaf with no clip".into()));
        }
        if weight > 0.0 {
            out.push((node.clip, weight));
        }
        return Ok(());
    }
    let shares = match node.blend_type {
        0 => one_dimensional(node, parameters)?,
        4 => direct(node, parameters)?,
        1 => return Err(Error::Unsupported("a simple directional 2D blend tree".into())),
        2 => return Err(Error::Unsupported("a freeform directional 2D blend tree".into())),
        3 => return Err(Error::Unsupported("a freeform cartesian 2D blend tree".into())),
        k => return Err(Error::Unsupported(format!("a blend tree of type {k}"))),
    };
    for (child, share) in node.children.iter().zip(shares) {
        if share > 0.0 {
            node_weights(tree, *child as usize, weight * share, parameters, out, depth + 1)?;
        }
    }
    Ok(())
}

/// The children's shares of a 1D node: see the module documentation.
fn one_dimensional(node: &BlendNode, parameters: &Parameters) -> Result<Vec<f64>> {
    let n = node.children.len();
    let t = &node.thresholds;
    if t.len() != n {
        return Err(invalid(format!(
            "a 1D blend tree of {n} children with {} thresholds",
            t.len()
        )));
    }
    if t.windows(2)
        .any(|w| w[0] > w[1] || w[0].is_nan() || w[1].is_nan())
    {
        return Err(invalid("a 1D blend tree whose thresholds do not ascend"));
    }
    let v = parameters.get(node.event_id)?;
    let mut out = vec![0.0; n];
    if v <= t[0] {
        out[0] = 1.0;
    } else if v >= t[n - 1] {
        out[n - 1] = 1.0;
    } else {
        // the last child whose threshold is at or below the value, then the next one (a larger threshold, else the value
        // was at or past the last)
        let i = t
            .iter()
            .rposition(|&x| x <= v)
            .expect("above the first threshold");
        let u = f64::from(v - t[i]) / f64::from(t[i + 1] - t[i]);
        out[i] = 1.0 - u;
        out[i + 1] = u;
    }
    Ok(out)
}

/// The children's shares of a direct node: see the module documentation.
fn direct(node: &BlendNode, parameters: &Parameters) -> Result<Vec<f64>> {
    let n = node.children.len();
    if node.direct_events.len() != n {
        return Err(invalid(format!(
            "a direct blend tree of {n} children with {} parameters",
            node.direct_events.len()
        )));
    }
    let mut out = Vec::with_capacity(n);
    for &id in &node.direct_events {
        let v = parameters.get(id)?;
        if !v.is_finite() || v < 0.0 {
            return Err(invalid(format!("a direct blend tree weight of {v}")));
        }
        out.push(f64::from(v));
    }
    if node.normalized_blend_values {
        let sum: f64 = out.iter().sum();
        if sum <= 0.0 {
            return Err(invalid("a normalised direct blend tree whose weights are all 0"));
        }
        for w in &mut out {
            *w /= sum;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: u32 = u32::MAX;

    fn node(blend_type: i64, event_id: u32, children: &[u32], thresholds: &[f32], clip: u32) -> BlendNode {
        BlendNode {
            blend_type,
            event_id,
            event_y_id: NONE,
            children: children.to_vec(),
            thresholds: thresholds.to_vec(),
            positions: vec![],
            magnitudes: vec![],
            pair_vectors: vec![],
            pair_avg_mag_inv: vec![],
            neighbors: vec![],
            direct_events: vec![],
            normalized_blend_values: false,
            clip,
            duration: 1.0,
            cycle_offset: 0.0,
            mirror: false,
        }
    }

    fn leaf(clip: u32) -> BlendNode {
        node(0, NONE, &[], &[], clip)
    }

    fn parameters(values: &[(&str, f32)]) -> Parameters {
        Parameters(values.iter().map(|(n, v)| (name_hash(n), *v)).collect())
    }

    /// Hand-worked: thresholds 0, 1 and 3; the value 0.5 is halfway between the first two clips, 2 halfway between the last
    /// two (1 + (3 - 1) / 2), exactly a threshold is that clip alone, and beyond the ends the end clip plays alone.
    #[test]
    fn a_1d_tree_blends_the_two_neighbouring_clips_linearly() {
        let tree = vec![
            node(0, name_hash("speed"), &[1, 2, 3], &[0.0, 1.0, 3.0], NONE),
            leaf(10),
            leaf(11),
            leaf(12),
        ];
        let at = |v: f32| child_weights(&tree, &parameters(&[("speed", v)])).unwrap();
        assert_eq!(at(0.5), [(10, 0.5), (11, 0.5)]);
        assert_eq!(at(2.0), [(11, 0.5), (12, 0.5)]);
        assert_eq!(at(1.0), [(11, 1.0)]);
        assert_eq!(at(0.0), [(10, 1.0)]);
        assert_eq!(at(-4.0), [(10, 1.0)]);
        assert_eq!(at(7.0), [(12, 1.0)]);
        let w = at(0.25);
        assert_eq!(w, [(10, 0.75), (11, 0.25)]);
        assert_eq!(w.iter().map(|x| x.1).sum::<f64>(), 1.0);
    }

    /// A 1D tree whose second child is a direct tree of two clips (weights 0.25 and 0.75 by their parameters): at 0.5 the
    /// first clip has half the weight and the direct tree's clips split the other half. An unsorted threshold list, a
    /// missing parameter and a 2D type are errors.
    #[test]
    fn inner_nodes_split_their_weight_among_their_children() {
        let mut inner = node(4, NONE, &[3, 4], &[], NONE);
        inner.direct_events = vec![name_hash("a"), name_hash("b")];
        let tree = vec![
            node(0, name_hash("x"), &[1, 2], &[0.0, 1.0], NONE),
            leaf(5),
            inner,
            leaf(6),
            leaf(7),
        ];
        let p = parameters(&[("x", 0.5), ("a", 0.25), ("b", 0.75)]);
        assert_eq!(
            child_weights(&tree, &p).unwrap(),
            [(5, 0.5), (6, 0.125), (7, 0.375)]
        );
        let mut unsorted = tree.clone();
        unsorted[0].thresholds = vec![1.0, 0.0];
        assert!(child_weights(&unsorted, &p).is_err());
        assert!(child_weights(&tree, &parameters(&[("x", 0.5)])).is_err());
        let mut twod = tree.clone();
        twod[0].blend_type = 3;
        let e = child_weights(&twod, &p).unwrap_err().to_string();
        assert!(e.contains("freeform cartesian"), "{e}");
    }

    /// A direct tree takes the parameters as the weights, or their shares of their sum when it states them normalised.
    #[test]
    fn a_direct_tree_weights_by_its_parameters() {
        let mut root = node(4, NONE, &[1, 2], &[], NONE);
        root.direct_events = vec![name_hash("a"), name_hash("b")];
        let mut tree = vec![root, leaf(1), leaf(2)];
        let p = parameters(&[("a", 1.0), ("b", 3.0)]);
        assert_eq!(child_weights(&tree, &p).unwrap(), [(1, 1.0), (2, 3.0)]);
        tree[0].normalized_blend_values = true;
        assert_eq!(child_weights(&tree, &p).unwrap(), [(1, 0.25), (2, 0.75)]);
        assert!(child_weights(&tree, &parameters(&[("a", 0.0), ("b", 0.0)])).is_err());
        assert!(child_weights(&tree, &parameters(&[("a", -1.0), ("b", 2.0)])).is_err());
    }

    /// A state of one clip is a tree of one leaf; a leaf with no clip is refused, not played as nothing.
    #[test]
    fn a_one_leaf_tree_plays_its_clip() {
        let p = parameters(&[]);
        assert_eq!(child_weights(&[leaf(3)], &p).unwrap(), [(3, 1.0)]);
        assert!(child_weights(&[leaf(NONE)], &p).is_err());
    }
}
