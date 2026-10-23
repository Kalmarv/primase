//! Where the target skeleton's joints stand inside a source mesh: the automatic fit, and the offsets a person (or a tool)
//! states from it (`core.joint_placement`).
//!
//! Fitting a mesh to another skeleton poses the source skeleton onto the target's proportions ([`fit`]: `fitted_j`, the
//! model transform of source joint `j` after the fit) and skins the mesh into that pose. Read the other way, the fit
//! places each target joint `t` (paired with source joint `j`) in the source's model space at
//!
//! `P_t = S_j * fitted_j^-1 * T_t`
//!
//! (`S_j` the source joint's model rest, `T_t` the target joint's): the frame the mesh weighted to `j` is carried by, so
//! that `fitted_j = T_t * P_t^-1 * S_j`. The fit's own placement is [`Frames::automatic`]; for a mapped joint it sits
//! at the source joint's position. A stated [`JointOffset`] moves `P_t` in the source's model space: `P_t' = O * P_t`,
//! `O` a translation by the offset's translation after a turn by its rotation about `P_t`'s origin. The mesh weighted to
//! `j` then follows the moved frame, and [`place`] returns the `fitted` that does so:
//! `fitted_j' = fitted_j * S_j^-1 * O^-1 * S_j`.
//!
//! Offsets are independent: each is from the automatic fit, never from another joint's offset, and a mapped joint's
//! offset leaves every other mapped joint where the fit put it. A source joint the map does not list follows its
//! nearest mapped ancestor, as in the fit, and so follows that ancestor's offset. A joint whose offset is the identity
//! keeps the fit's own `fitted_j`, bit for bit.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::{JointId, JointOffset, Skeleton, Transform};

use crate::pose::{Affine, Trs, affine, compose, inverse, locals};
use crate::quat;
use crate::retarget::{between, parents_first, rotate, rotation, with_rotation};

/// The source skeleton posed onto the target's proportions (see [`RetargetSkin`](crate::RetargetSkin), step 1): each
/// source joint's model transform, `pairs` being source joint to target joint.
pub fn fit(
    source: &Skeleton,
    target_rest: &BTreeMap<JointId, Affine>,
    pairs: &BTreeMap<JointId, JointId>,
) -> Result<BTreeMap<JointId, Affine>, String> {
    let local: BTreeMap<JointId, Affine> = locals(source, None)
        .into_iter()
        .map(|(j, r)| (j, affine(&r.matrix())))
        .collect();
    let parent: BTreeMap<JointId, Option<JointId>> = source.joints.iter().map(|j| (j.id, j.parent)).collect();
    // each mapped joint's mapped joints below it with no mapped joint between
    let mut below: BTreeMap<JointId, Vec<JointId>> = BTreeMap::new();
    for j in source.joints.iter().filter(|j| pairs.contains_key(&j.id)) {
        let mut at = parent[&j.id];
        while let Some(p) = at {
            if pairs.contains_key(&p) {
                below.entry(p).or_default().push(j.id);
                break;
            }
            at = parent[&p];
        }
    }
    let mut out: BTreeMap<JointId, Affine> = BTreeMap::new();
    for i in parents_first(source)? {
        let j = &source.joints[i];
        let mut m = match j.parent {
            Some(p) => compose(&out[&p], &local[&j.id]),
            None => local[&j.id],
        };
        if let Some(&t) = pairs.get(&j.id) {
            m[3] = target_rest[&t][3];
            if let Some([child]) = below.get(&j.id).map(Vec::as_slice) {
                // the child's offset in this joint's space, through the rests between them
                let mut chain = affine(&Transform::IDENTITY);
                let mut at = *child;
                while at != j.id {
                    chain = compose(&local[&at], &chain);
                    at = parent[&at].ok_or("a mapped joint below another is not under it")?;
                }
                let now = rotate(rotation(&m), chain[3]);
                let want: [f64; 3] =
                    std::array::from_fn(|k| target_rest[&pairs[child]][3][k] - target_rest[&t][3][k]);
                m = with_rotation(m[3], quat::mul(between(now, want), rotation(&m)), &m);
            }
        }
        out.insert(j.id, m);
    }
    Ok(out)
}

/// A source joint's name, for a message.
fn name(s: &Skeleton, j: JointId) -> String {
    s.joints
        .iter()
        .find(|x| x.id == j)
        .map_or_else(|| format!("{j:?}"), |x| x.name.clone())
}

/// A target joint's frame in the source's model space under the automatic fit: `S_j * fitted_j^-1 * T_t`.
fn automatic(
    source: &Skeleton,
    j: JointId,
    source_rest: &Affine,
    fitted: &Affine,
    target_rest: &Affine,
) -> Result<Affine, String> {
    let back = inverse(fitted).ok_or_else(|| {
        format!(
            "source joint {} is fitted to a transform with no inverse (a scale of nothing): it has no placement",
            name(source, j)
        )
    })?;
    Ok(compose(source_rest, &compose(&back, target_rest)))
}

/// The map that turns by the offset's rotation about `pivot` and then moves by its translation (both in the source's
/// model space).
fn offset_map(pivot: [f64; 3], offset: &JointOffset) -> Result<Affine, String> {
    offset.check()?;
    let q = quat::normalize(offset.rotation.map(f64::from)).ok_or("a zero rotation")?;
    let r = Trs {
        translation: [0.0; 3],
        rotation: q,
        scale: [1.0; 3],
    }
    .matrix();
    let turned = compose(&r, &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], pivot]);
    let moved: [f64; 3] = std::array::from_fn(|k| pivot[k] + f64::from(offset.translation[k]) - turned[3][k]);
    Ok([r[0], r[1], r[2], moved])
}

/// The frame a target joint is carried by in the source's model space, before and after its offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frames {
    /// Where the automatic fit puts it (`P_t`).
    pub automatic: Affine,
    /// Where the stated offset puts it (`O * P_t`; the automatic one when the target has no offset).
    pub placed: Affine,
}

/// Each paired target joint's [`Frames`] given the fit (`fitted`, from [`fit`]) and the offsets stated by target joint:
/// what an editor draws the target skeleton at. Refused for an offset of a joint no pair names, or of a joint whose
/// fitted transform has no inverse.
pub fn frames(
    source: &Skeleton,
    source_rest: &BTreeMap<JointId, Affine>,
    fitted: &BTreeMap<JointId, Affine>,
    target_rest: &BTreeMap<JointId, Affine>,
    pairs: &BTreeMap<JointId, JointId>,
    offsets: &BTreeMap<JointId, JointOffset>,
) -> Result<BTreeMap<JointId, Frames>, String> {
    if let Some(t) = offsets.keys().find(|t| !pairs.values().any(|p| p == *t)) {
        return Err(format!("an offset of target joint {t:?}, which no pair names"));
    }
    pairs
        .iter()
        .map(|(&j, &t)| {
            let auto = automatic(source, j, &source_rest[&j], &fitted[&j], &target_rest[&t])?;
            let placed = match offsets.get(&t) {
                Some(o) if !o.is_identity() => compose(&offset_map(auto[3], o)?, &auto),
                _ => auto,
            };
            Ok((
                t,
                Frames {
                    automatic: auto,
                    placed,
                },
            ))
        })
        .collect()
}

/// The fit with the stated offsets applied (see the module documentation): `fitted` for every source joint, the
/// automatic one for every joint an offset does not reach (bit for bit), and for the others as the module documentation
/// says. Returns the new fit and the source joints it moved (those with a non-identity offset, and the unmapped joints
/// under them that follow).
pub fn place(
    source: &Skeleton,
    source_rest: &BTreeMap<JointId, Affine>,
    fitted: &BTreeMap<JointId, Affine>,
    target_rest: &BTreeMap<JointId, Affine>,
    pairs: &BTreeMap<JointId, JointId>,
    offsets: &BTreeMap<JointId, JointOffset>,
) -> Result<(BTreeMap<JointId, Affine>, BTreeSet<JointId>), String> {
    if let Some(t) = offsets.keys().find(|t| !pairs.values().any(|p| p == *t)) {
        return Err(format!("an offset of target joint {t:?}, which no pair names"));
    }
    let mut out = fitted.clone();
    let mut moved = BTreeSet::new();
    for (&j, &t) in pairs {
        let Some(offset) = offsets.get(&t).filter(|o| !o.is_identity()) else {
            continue;
        };
        let auto = automatic(source, j, &source_rest[&j], &fitted[&j], &target_rest[&t])?;
        let undo = inverse(&offset_map(auto[3], offset)?)
            .ok_or_else(|| format!("the offset of target joint {t:?} has no inverse"))?;
        let rest_back = inverse(&source_rest[&j]).ok_or_else(|| {
            format!(
                "source joint {} has a rest with no inverse (a scale of nothing): it has no placement",
                name(source, j)
            )
        })?;
        out.insert(
            j,
            compose(
                &fitted[&j],
                &compose(&rest_back, &compose(&undo, &source_rest[&j])),
            ),
        );
        moved.insert(j);
    }
    // a joint the map does not list follows its parent, as in the fit
    let local: BTreeMap<JointId, Affine> = locals(source, None)
        .into_iter()
        .map(|(j, r)| (j, affine(&r.matrix())))
        .collect();
    for i in parents_first(source)? {
        let j = &source.joints[i];
        if pairs.contains_key(&j.id) {
            continue;
        }
        if let Some(p) = j.parent.filter(|p| moved.contains(p)) {
            out.insert(j.id, compose(&out[&p], &local[&j.id]));
            moved.insert(j.id);
        }
    }
    Ok((out, moved))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::model_transforms;
    use primase_core::{Joint, Rest};

    fn trs(t: [f32; 3], q: [f32; 4]) -> Rest {
        Rest::Trs {
            translation: t,
            rotation: q,
            scale: [1.0; 3],
        }
    }

    fn joint(id: u32, parent: Option<u32>, rest: Rest) -> Joint {
        Joint {
            id: JointId(id),
            name: format!("j{id}"),
            parent: parent.map(JointId),
            rest,
        }
    }

    type Rests = BTreeMap<JointId, Affine>;

    /// The skeletons of `retarget-skin`'s tests: a source root 1, arm 2, hand 3 and a cloth joint 4 under the arm the
    /// map does not list; a taller target 10, 20 (turned 45 degrees down), 30. Returns (source, source rests, target
    /// rests, pairs, fit).
    fn setup() -> (Skeleton, Rests, Rests, BTreeMap<JointId, JointId>, Rests) {
        let id = [0.0, 0.0, 0.0, 1.0];
        let source = Skeleton {
            joints: vec![
                joint(1, None, trs([0.0, 1.0, 0.0], id)),
                joint(2, Some(1), trs([0.2, 0.5, 0.0], id)),
                joint(3, Some(2), trs([0.5, 0.0, 0.0], id)),
                joint(4, Some(2), trs([0.4, -0.1, 0.0], id)),
            ],
        };
        let (s8, c8) = (
            (std::f32::consts::PI / 8.0).sin(),
            (std::f32::consts::PI / 8.0).cos(),
        );
        let target = Skeleton {
            joints: vec![
                joint(10, None, trs([0.0, 2.0, 0.0], id)),
                joint(20, Some(10), trs([0.3, 0.6, 0.0], [0.0, 0.0, -s8, c8])),
                joint(30, Some(20), trs([0.6, 0.0, 0.0], id)),
            ],
        };
        let rests = |s: &Skeleton| model_transforms(s, &locals(s, None)).unwrap();
        let pairs = BTreeMap::from([
            (JointId(1), JointId(10)),
            (JointId(2), JointId(20)),
            (JointId(3), JointId(30)),
        ]);
        let (source_rest, target_rest) = (rests(&source), rests(&target));
        let fitted = fit(&source, &target_rest, &pairs).unwrap();
        (source, source_rest, target_rest, pairs, fitted)
    }

    fn near(a: &Affine, b: &Affine) -> bool {
        a.iter()
            .flatten()
            .zip(b.iter().flatten())
            .all(|(x, y)| (x - y).abs() < 1e-9)
    }

    fn move_by(y: f32) -> JointOffset {
        JointOffset {
            translation: [0.0, y, 0.0],
            ..JointOffset::IDENTITY
        }
    }

    /// The fit's own placement of a mapped joint sits at the source joint's position, and the fit is recovered from it
    /// (`fitted_j = T_t * P_t^-1 * S_j`).
    #[test]
    fn the_automatic_frame_is_the_source_joints_position_and_gives_the_fit_back() {
        let (source, source_rest, target_rest, pairs, fitted) = setup();
        let all = frames(
            &source,
            &source_rest,
            &fitted,
            &target_rest,
            &pairs,
            &BTreeMap::new(),
        )
        .unwrap();
        for (j, t) in &pairs {
            let f = all[t];
            assert_eq!(f.automatic, f.placed);
            for (a, b) in f.automatic[3].iter().zip(&source_rest[j][3]) {
                assert!((a - b).abs() < 1e-12);
            }
            let again = compose(
                &target_rest[t],
                &compose(&inverse(&f.automatic).unwrap(), &source_rest[j]),
            );
            assert!(near(&again, &fitted[j]), "{j:?}: {again:?} vs {:?}", fitted[j]);
        }
    }

    /// An identity offset (stated or not) leaves the fit untouched, bit for bit; a stated one moves the joint's frame
    /// by the offset and gives the `fitted` that follows the moved frame; no other mapped joint moves, and the
    /// unmapped joint under the arm follows the arm.
    #[test]
    fn a_stated_offset_moves_its_frame_and_what_follows_it() {
        let (source, source_rest, target_rest, pairs, fitted) = setup();
        let none = BTreeMap::from([
            (JointId(20), JointOffset::IDENTITY),
            (JointId(30), JointOffset::IDENTITY),
        ]);
        let (same, moved) = place(&source, &source_rest, &fitted, &target_rest, &pairs, &none).unwrap();
        assert_eq!(same, fitted);
        assert!(moved.is_empty());

        let quarter = std::f32::consts::FRAC_1_SQRT_2;
        let offset = JointOffset {
            translation: [0.0, 0.1, 0.0],
            rotation: [0.0, 0.0, quarter, quarter],
        };
        let offsets = BTreeMap::from([(JointId(20), offset)]);
        let (placed, moved) = place(&source, &source_rest, &fitted, &target_rest, &pairs, &offsets).unwrap();
        assert_eq!(moved, BTreeSet::from([JointId(2), JointId(4)]));
        for j in [JointId(1), JointId(3)] {
            assert_eq!(placed[&j], fitted[&j]);
        }
        // the frame moved by the offset, and the fit that follows it
        let all = frames(&source, &source_rest, &fitted, &target_rest, &pairs, &offsets).unwrap();
        let f = all[&JointId(20)];
        let pivot = f.automatic[3];
        for ((placed, auto), moved) in f.placed[3].iter().zip(&pivot).zip([0.0, 0.1, 0.0]) {
            // a turn about the origin of the frame leaves the origin where it was, then the translation moves it
            assert!(
                (placed - (auto + moved)).abs() < 1e-6,
                "{:?} vs {pivot:?}",
                f.placed[3]
            );
        }
        let again = compose(
            &target_rest[&JointId(20)],
            &compose(&inverse(&f.placed).unwrap(), &source_rest[&JointId(2)]),
        );
        assert!(
            near(&again, &placed[&JointId(2)]),
            "{again:?} vs {:?}",
            placed[&JointId(2)]
        );
        // the cloth joint stays on the arm's deformation
        let follows = compose(
            &placed[&JointId(2)],
            &compose(
                &inverse(&source_rest[&JointId(2)]).unwrap(),
                &source_rest[&JointId(4)],
            ),
        );
        assert!(near(&follows, &placed[&JointId(4)]));
    }

    #[test]
    fn an_offset_of_a_joint_no_pair_names_is_refused() {
        let (source, source_rest, target_rest, pairs, fitted) = setup();
        let offsets = BTreeMap::from([(JointId(99), move_by(0.1))]);
        let err = place(&source, &source_rest, &fitted, &target_rest, &pairs, &offsets).unwrap_err();
        assert!(err.contains("which no pair names"), "{err}");
        let err = frames(&source, &source_rest, &fitted, &target_rest, &pairs, &offsets).unwrap_err();
        assert!(err.contains("which no pair names"), "{err}");
    }
}
