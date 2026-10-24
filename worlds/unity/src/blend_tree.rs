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
//!   `normalized_blend_values` they are divided by their sum only where the sum is above 1 (a sum of 1 or less stays as
//!   it is, so 0.2 and 0.1 play at 0.2 and 0.1, and all zeros play nothing). The manual says they are divided by their
//!   sum; Unity 6000.6.4f1's editor does it only above 1 (`primase-oracles/unity/blendtree`, 8 points).
//! - freeform cartesian 2D (`blend_type` 3): the gradient bands of Johansen's thesis (eq. 6.6): child `i`'s weight is
//!   `h_i = min over j != i of (1 - (p - p_i) . (p_j - p_i) / |p_j - p_i|^2)`, clamped at 0, then divided by their sum
//!   (all 0 where the sum is: nothing plays). The editor matches it to 1e-7 on 49 points of the bird's Idling positions
//!   and on an uneven 6-child set, outside the hull too (`primase-oracles/unity/blendtree`).
//! - a child that is itself an inner node splits its weight among its own children (weights multiply).
//!
//! The other 2D types (simple directional 1, freeform directional 2) are refused by name: their weights are Unity's
//! polar gradient bands (Johansen's thesis, chapter 6.3.1, for the freeform one) and are not written here until a model
//! matches an engine's numbers.

use std::collections::BTreeMap;

use crate::animator::{BlendNode, UnityAnimatorController, name_hash};
use crate::{Error, Result, invalid};

/// The values of a controller's parameters, by the CRC-32 of the parameter's name, as the controller's blend trees and
/// transitions name them. A value is finite: a NaN or an infinity is refused where it is set or read from the controller,
/// so a blend tree never compares against one.
#[derive(Clone, Debug, PartialEq)]
pub struct Parameters(BTreeMap<u32, f32>);

fn finite(what: impl std::fmt::Display, value: f32) -> Result<f32> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(invalid(format!(
            "parameter {what} set to {value}, which is not a finite number"
        )))
    }
}

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
            out.insert(p.id, finite(p.id, v)?);
        }
        Ok(Parameters(out))
    }

    /// Set the parameter called `name` (a name the controller has: its CRC-32 is one of its parameters').
    pub fn set(&mut self, name: &str, value: f32) -> Result<()> {
        let id = name_hash(name);
        match self.0.get_mut(&id) {
            Some(v) => {
                *v = finite(format!("{name:?}"), value)?;
                Ok(())
            }
            None => Err(invalid(format!("the controller has no parameter {name:?}"))),
        }
    }

    /// Set the parameter whose name hashes to `id`.
    pub fn set_by_id(&mut self, id: u32, value: f32) -> Result<()> {
        match self.0.get_mut(&id) {
            Some(v) => {
                *v = finite(id, value)?;
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
        3 => freeform_cartesian(node, parameters)?,
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
    if n == 0 {
        return Err(invalid("a 1D blend tree of no children"));
    }
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
        let i = t.iter().rposition(|&x| x <= v).ok_or_else(|| {
            invalid(format!(
                "a 1D blend tree value {v} that is neither at, below nor above its thresholds"
            ))
        })?;
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
        if sum > 1.0 {
            for w in &mut out {
                *w /= sum;
            }
        }
    }
    Ok(out)
}

/// The children's shares of a freeform cartesian node: see the module documentation.
fn freeform_cartesian(node: &BlendNode, parameters: &Parameters) -> Result<Vec<f64>> {
    let n = node.children.len();
    if node.positions.len() != n {
        return Err(invalid(format!(
            "a freeform cartesian blend tree of {n} children with {} positions",
            node.positions.len()
        )));
    }
    if n == 1 {
        return Ok(vec![1.0]);
    }
    let p = [
        f64::from(parameters.get(node.event_id)?),
        f64::from(parameters.get(node.event_y_id)?),
    ];
    let at = |i: usize| node.positions[i].map(f64::from);
    let mut h = Vec::with_capacity(n);
    for i in 0..n {
        let (pi, mut least) = (at(i), f64::INFINITY);
        for j in (0..n).filter(|&j| j != i) {
            let d = [at(j)[0] - pi[0], at(j)[1] - pi[1]];
            let length_squared = d[0] * d[0] + d[1] * d[1];
            if length_squared == 0.0 {
                return Err(invalid(format!(
                    "a freeform cartesian blend tree whose children {i} and {j} are at one position"
                )));
            }
            let r = [p[0] - pi[0], p[1] - pi[1]];
            least = least.min(1.0 - (r[0] * d[0] + r[1] * d[1]) / length_squared);
        }
        h.push(least.max(0.0));
    }
    let sum: f64 = h.iter().sum();
    if sum > 0.0 {
        for w in &mut h {
            *w /= sum;
        }
    }
    Ok(h)
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
        twod[0].blend_type = 2;
        let e = child_weights(&twod, &p).unwrap_err().to_string();
        assert!(e.contains("freeform directional"), "{e}");
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
        // normalised only where the sum is above 1 (Unity's editor): a sum of 0.3 plays as it is, all zeros play nothing
        let weights = |a: f32, b: f32| child_weights(&tree, &parameters(&[("a", a), ("b", b)])).unwrap();
        assert_eq!(weights(1.0, 1.0), [(1, 0.5), (2, 0.5)]);
        let small = weights(0.2, 0.1);
        assert!(
            (small[0].1 - 0.2).abs() < 1e-7 && (small[1].1 - 0.1).abs() < 1e-7,
            "{small:?}"
        );
        assert!(weights(0.0, 0.0).is_empty());
        assert!(child_weights(&tree, &parameters(&[("a", -1.0), ("b", 2.0)])).is_err());
    }

    /// A freeform cartesian node: two children at (0, 0) and (1, 0) share a point a quarter of the way as 0.75 and 0.25
    /// (worked by hand from eq. 6.6), and six children at the positions of the editor's "uneven" case give the clip weights
    /// Unity 6000.6.4f1 gave at four of its points (`primase-oracles/unity/blendtree`, the oracle's printed to 9 digits).
    #[test]
    fn a_freeform_cartesian_tree_weights_by_the_gradient_bands() {
        let mut root = node(3, name_hash("x"), &[1, 2], &[], NONE);
        root.event_y_id = name_hash("y");
        root.positions = vec![[0.0, 0.0], [1.0, 0.0]];
        let mut tree = vec![root, leaf(0), leaf(1)];
        let at = |tree: &[BlendNode], x: f32, y: f32| {
            child_weights(tree, &parameters(&[("x", x), ("y", y)])).unwrap()
        };
        assert_eq!(at(&tree, 0.25, 0.0), [(0, 0.75), (1, 0.25)]);
        // a point past a child plays that child alone
        assert_eq!(at(&tree, 3.0, 0.0), [(1, 1.0)]);
        assert_eq!(at(&tree, -2.0, 5.0), [(0, 1.0)]);
        // the editor's uneven set
        let positions = [
            [0.0, 0.0],
            [0.0, 1.0],
            [2.0, 0.0],
            [0.0, -0.5],
            [-1.0, 1.0],
            [1.0, -1.0],
        ];
        let mut uneven = node(3, name_hash("x"), &[1, 2, 3, 4, 5, 6], &[], NONE);
        uneven.event_y_id = name_hash("y");
        uneven.positions = positions.to_vec();
        let mut six = vec![uneven];
        six.extend((0..6).map(leaf));
        for (x, y, want) in [
            (-1.5, -0.5, vec![(3, 0.538_461_6), (4, 0.461_538_5)]),
            (-0.75, 0.0, vec![(0, 0.625), (4, 0.375)]),
            (-0.75, -0.5, vec![(3, 0.860_215), (4, 0.139_784_95)]),
            (-0.75, 0.5, vec![(0, 0.3), (1, 0.2), (4, 0.5)]),
        ] {
            let got = at(&six, x, y);
            assert_eq!(got.len(), want.len(), "({x}, {y}): {got:?}");
            for ((clip, weight), (want_clip, want_weight)) in got.iter().zip(&want) {
                assert_eq!(clip, want_clip, "({x}, {y}): {got:?}");
                assert!((weight - want_weight).abs() < 1e-6, "({x}, {y}): {got:?}");
            }
        }
        // children at one position, positions that do not match the children, and a missing parameter are errors
        tree[0].positions = vec![[0.0, 0.0], [0.0, 0.0]];
        assert!(child_weights(&tree, &parameters(&[("x", 0.0), ("y", 0.0)])).is_err());
        tree[0].positions = vec![[0.0, 0.0]];
        assert!(child_weights(&tree, &parameters(&[("x", 0.0), ("y", 0.0)])).is_err());
        tree[0].positions = vec![[0.0, 0.0], [1.0, 0.0]];
        assert!(child_weights(&tree, &parameters(&[("x", 0.0)])).is_err());
        // a single child plays alone wherever the point is
        let mut one = vec![node(3, name_hash("x"), &[1], &[], NONE), leaf(9)];
        one[0].event_y_id = name_hash("y");
        one[0].positions = vec![[4.0, 4.0]];
        assert_eq!(at(&one, -7.0, 2.0), [(9, 1.0)]);
    }

    /// A NaN or an infinity is not a parameter value: refused where it is set and where the controller's defaults hold it, and
    /// a tree handed one anyway (a map made by hand) is an error, not a panic. A tree of no nodes and a 1D node of no
    /// children are errors too.
    #[test]
    fn a_parameter_that_is_not_finite_and_a_tree_with_no_children_are_refused() {
        let tree = vec![
            node(0, name_hash("x"), &[1, 2], &[0.0, 1.0], NONE),
            leaf(5),
            leaf(6),
        ];
        let mut p = parameters(&[("x", 0.5)]);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(p.set("x", bad).is_err() && p.set_by_id(name_hash("x"), bad).is_err());
            assert_eq!(p.get(name_hash("x")).unwrap(), 0.5);
        }
        assert!(child_weights(&tree, &parameters(&[("x", f32::NAN)])).is_err());
        // the ends hold an infinite value, so the tree itself reads one; it is the parameter that refuses it
        assert_eq!(
            child_weights(&tree, &parameters(&[("x", f32::INFINITY)])).unwrap(),
            [(6, 1.0)]
        );

        let controller = |default: f32| UnityAnimatorController {
            controller: crate::components::UnityObject {
                file: "f".into(),
                path_id: 1,
            },
            name: "c".into(),
            layers: vec![],
            state_machines: vec![],
            parameters: vec![crate::animator::Parameter {
                id: name_hash("x"),
                kind: 1,
                index: 0,
            }],
            default_floats: vec![default],
            default_ints: vec![],
            default_bools: vec![],
            clips: vec![],
            behaviours: vec![],
            tos: vec![],
        };
        assert_eq!(
            Parameters::defaults(&controller(0.25)).unwrap(),
            parameters(&[("x", 0.25)])
        );
        assert!(Parameters::defaults(&controller(f32::NAN)).is_err());
        assert!(Parameters::defaults(&controller(f32::INFINITY)).is_err());

        assert!(child_weights(&[], &p).is_err());
        assert!(one_dimensional(&node(0, name_hash("x"), &[], &[], NONE), &p).is_err());
    }

    /// A state of one clip is a tree of one leaf; a leaf with no clip is refused, not played as nothing.
    #[test]
    fn a_one_leaf_tree_plays_its_clip() {
        let p = parameters(&[]);
        assert_eq!(child_weights(&[leaf(3)], &p).unwrap(), [(3, 1.0)]);
        assert!(child_weights(&[leaf(NONE)], &p).is_err());
    }
}
