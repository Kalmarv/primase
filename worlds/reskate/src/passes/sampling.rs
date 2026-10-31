//! skate samples a material's textures one way: through one coordinate set (the primitive's first), untransformed,
//! with one wrap per material. A reader's materials may sample each slot otherwise (Unity: a tiling and wrap per
//! texture), which `raise-skate` refuses (a transform left on the base colour or normal, a mask input off the first set
//! or wrapping two ways) or keeps only in part (the base colour's u wrap). Two preparations, before `raise-skate`, make
//! a material's slots sample as one; the slot they follow (the anchor) and what becomes of a slot that differs are the
//! program's choices:
//!
//! - `prepare-reskate-uv-transforms{anchor=<slot>,conflicts=adopt|refuse}`: every slot takes the anchor's texture
//!   transform (`adopt`, each slot that differed reported by material and slot as an approximation; `refuse`: an error
//!   naming it), then the transforms are baked into the coordinates by the toolkit's baker
//!   ([`primase_meshops::bake_texture_transforms`]). A blend's layer whose material tiles (a layer's transform comes
//!   before its textures' own) is first given a coordinate set of its own holding both, which `lower-layered-reskate`
//!   draws its overlay with. A slot sampling another set than the first is refused in a material drawn (by a primitive,
//!   as its own, a variant or its file's, or as a drawn blend's layer); one nothing draws (a form
//!   `specialize-material-variants` discarded) is left as it is, counted. A material
//!   drawn whose tiling the baker could not bake (materials sharing a primitive that tile differently: a prototype's
//!   variants, `specialize-material-variants` first) is refused, never erased.
//! - `prepare-reskate-wrap{anchor=<slot>,axis=u|v}`: every slot wraps along both axes as the anchor wraps along `axis`,
//!   each material changed reported by name as an approximation.
//!
//! The anchor is a slot (`base-colour`, `metallic-roughness`, `normal`, `occlusion`, `emissive`, `specular`,
//! `specular-colour`); a material without a texture there follows its first textured slot in glTF's order.

use std::collections::BTreeSet;

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, TextureSlot, UvTransform, Wrap};
use primase_core::{
    Entity, LayeredMaterial, MaterialVariants, Mesh, MeshFile, Name, Semantic, Stream, World,
};

const SLOTS: [(TextureSlot, &str); 7] = [
    (TextureSlot::BaseColor, "base-colour"),
    (TextureSlot::MetallicRoughness, "metallic-roughness"),
    (TextureSlot::Normal, "normal"),
    (TextureSlot::Occlusion, "occlusion"),
    (TextureSlot::Emissive, "emissive"),
    (TextureSlot::Specular, "specular"),
    (TextureSlot::SpecularColor, "specular-colour"),
];

/// A slot as `anchor` names it: [`TextureSlot`] is core's, so the choice is this crate's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Anchor(TextureSlot);

impl Choice for Anchor {
    const VALUES: &'static [(&'static str, Self)] = &{
        let mut values = [("", Anchor(TextureSlot::BaseColor)); SLOTS.len()];
        let mut i = 0;
        while i < SLOTS.len() {
            values[i] = (SLOTS[i].1, Anchor(SLOTS[i].0));
            i += 1;
        }
        values
    };
}

/// `anchor`, as both passes take it: the program's, no default.
const ANCHOR: OptionSpec = OptionSpec::choice::<Anchor>(
    "anchor",
    "the texture slot a material's other slots sample as (a material without a texture there follows its first \
     textured slot)",
);

fn name_of(w: &World, e: Entity) -> String {
    w.get::<Name>(e).map_or_else(|| format!("{e:?}"), |n| n.0.clone())
}

/// The slot material `m`'s slots follow: `anchor` when it has a texture, else its first textured slot; `None`: no
/// texture.
fn anchor_of(m: &PbrMaterial, anchor: TextureSlot) -> Option<TextureSlot> {
    m.slots()
        .find(|(s, _)| *s == anchor)
        .or_else(|| m.slots().next())
        .map(|(s, _)| s)
}

/// The materials a primitive draws with (its own, its variants'), each with its primitive: `Some((prototype, index))`
/// for a loaded mesh, `None` for a prototype whose geometry is still in its file (its file's materials).
fn direct_uses(w: &World) -> Vec<(Entity, Option<(Entity, usize)>)> {
    let mut uses = Vec::new();
    for (e, mesh) in w.iter::<Mesh>() {
        let variants = w.get::<MaterialVariants>(e);
        for (i, p) in mesh.primitives().iter().enumerate() {
            let own = p.material.into_iter();
            let other = variants
                .and_then(|v| v.0.get(i))
                .into_iter()
                .flat_map(|m| m.values().copied());
            uses.extend(own.chain(other).map(|m| (m, Some((e, i)))));
        }
    }
    for (e, f) in w.iter::<MeshFile>() {
        if !w.has::<Mesh>(e) {
            let variants = w.get::<MaterialVariants>(e);
            let other = variants
                .into_iter()
                .flat_map(|v| v.0.iter().flat_map(|m| m.values().copied()));
            uses.extend(
                f.materials
                    .iter()
                    .flatten()
                    .copied()
                    .chain(other)
                    .map(|m| (m, None)),
            );
        }
    }
    uses
}

/// The materials drawn: by a primitive (its own, its variants', its file's), and the layers of a drawn blend.
fn drawn(w: &World) -> BTreeSet<Entity> {
    let mut drawn: BTreeSet<Entity> = direct_uses(w).into_iter().map(|(m, _)| m).collect();
    let layers: Vec<Entity> = drawn
        .iter()
        .filter_map(|&e| w.get::<LayeredMaterial>(e))
        .flat_map(|l| l.layers.iter().flatten().map(|l| l.material))
        .collect();
    drawn.extend(layers);
    drawn
}

/// The tiling a layer material's slots share once they follow their anchor: the first slot's (`None`: none, or the
/// identity).
fn shared_tiling(m: &PbrMaterial) -> Option<UvTransform> {
    m.textures()
        .find_map(|r| r.transform)
        .filter(|t| *t != UvTransform::IDENTITY)
}

/// Each drawn blend's layer whose material tiles: its coordinates as core samples them, the layer's transform of its
/// selected set and then the material's own tiling (composed exactly, though the two in a row are not one
/// [`UvTransform`] in general: a rotation, then an unequal scale), written into a new set on every primitive the blend
/// draws, which the layer then selects untransformed. A material drawn only as layers, each of them folded, loses its
/// slots' tiling; one also drawn by a primitive keeps it for the baker. A blend whose geometry is still in a file is
/// left alone (its layers' tiling is then refused as not baked).
fn fold_layer_tiling(w: &mut World, log: &mut PassLog) -> Result<(), String> {
    let uses = direct_uses(w);
    let direct: BTreeSet<Entity> = uses.iter().map(|&(m, _)| m).collect();
    let mut folded: BTreeSet<Entity> = BTreeSet::new();
    let mut kept: BTreeSet<Entity> = BTreeSet::new();
    for &blend in &direct {
        let Some(layered) = w.get::<LayeredMaterial>(blend).cloned() else {
            continue;
        };
        let users: Vec<Option<(Entity, usize)>> = uses
            .iter()
            .filter(|(m, _)| *m == blend)
            .map(|&(_, p)| p)
            .collect();
        let loaded: Option<BTreeSet<(Entity, usize)>> = users.iter().copied().collect();
        for (slot, layer) in layered.layers.iter().enumerate() {
            let Some(layer) = layer else { continue };
            let Some(tiling) = w.get::<PbrMaterial>(layer.material).and_then(shared_tiling) else {
                continue;
            };
            let Some(prims) = &loaded else {
                log.count("not baked: a blend's geometry still in a file", 1);
                kept.insert(layer.material);
                continue;
            };
            let from = layer.texcoord.unwrap_or(0);
            let mut moved = Vec::new();
            let mut next = 1u8;
            for &(e, i) in prims {
                let p = &w.get::<Mesh>(e).expect("a loaded use").primitives()[i];
                let Some(Stream::F32x2(uv)) = p.streams.get(&Semantic::TexCoord(from)) else {
                    return Err(format!(
                        "prepare-reskate-uv-transforms: material {}'s layer {slot} samples TexCoord({from}), which                          a primitive drawing it lacks (or holds otherwise than as float pairs)",
                        name_of(w, blend)
                    ));
                };
                let uv: Vec<[f32; 2]> = uv
                    .iter()
                    .map(|&c| tiling.apply(layer.transform.map_or(c, |t| t.apply(c))))
                    .collect();
                for s in p.streams.keys() {
                    if let Semantic::TexCoord(n) = s {
                        next = next.max(n.checked_add(1).ok_or("a primitive holds TexCoord(255)")?);
                    }
                }
                moved.push(((e, i), uv));
            }
            for ((e, i), uv) in moved {
                w.get_mut::<Mesh>(e)
                    .expect("a loaded use")
                    .primitive_mut(i)
                    .expect("a listed primitive")
                    .streams_mut()
                    .insert(Semantic::TexCoord(next), Stream::F32x2(uv));
                w.remove::<MeshFile>(e);
            }
            let l = w.get_mut::<LayeredMaterial>(blend).expect("listed").layers[slot]
                .as_mut()
                .expect("listed");
            l.texcoord = Some(next);
            l.transform = None;
            folded.insert(layer.material);
            log.count("layer tilings folded into a coordinate set of their own", 1);
        }
    }
    for m in folded {
        if direct.contains(&m) || kept.contains(&m) {
            continue;
        }
        for r in w
            .get_mut::<PbrMaterial>(m)
            .expect("a tiling material")
            .textures_mut()
        {
            r.transform = None;
        }
        log.count("layer materials whose tiling the layers took", 1);
    }
    Ok(())
}

/// What a slot that samples otherwise than its anchor becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplingConflicts {
    /// It takes the anchor's, reported as an approximation by material and slot.
    Adopt,
    /// An error naming it.
    Refuse,
}

/// `prepare-reskate-uv-transforms`: see the module documentation. Made by [`PrepareReskateUvTransforms::new`]; from
/// options by the same call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrepareReskateUvTransforms {
    anchor: TextureSlot,
    conflicts: SamplingConflicts,
}

impl PrepareReskateUvTransforms {
    pub fn new(anchor: TextureSlot, conflicts: SamplingConflicts) -> PrepareReskateUvTransforms {
        PrepareReskateUvTransforms { anchor, conflicts }
    }

    pub fn anchor(&self) -> TextureSlot {
        self.anchor
    }

    pub fn conflicts(&self) -> SamplingConflicts {
        self.conflicts
    }

    /// From its options: `anchor=<slot>` and `conflicts=adopt|refuse`.
    fn from_given(g: &Given) -> PrepareReskateUvTransforms {
        PrepareReskateUvTransforms::new(g.choice::<Anchor>("anchor").0, g.choice("conflicts"))
    }
}

impl Choice for SamplingConflicts {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("adopt", SamplingConflicts::Adopt),
        ("refuse", SamplingConflicts::Refuse),
    ];
}

/// `prepare-reskate-uv-transforms`' declaration ([`primase_core::pass::PassRegistry::add`]): which slot a material's
/// textures sample as, and what becomes of a slot that differs, are the program's, no default.
pub const PREPARE_RESKATE_UV_TRANSFORMS: PassSpec = PassSpec {
    name: "prepare-reskate-uv-transforms",
    options: &[
        ANCHOR,
        OptionSpec::choice::<SamplingConflicts>(
            "conflicts",
            "a slot tiling otherwise than its anchor takes the anchor's, reported (adopt), or is an error (refuse)",
        ),
    ],
    make: |g| Ok(Box::new(PrepareReskateUvTransforms::from_given(g))),
};

impl Pass for PrepareReskateUvTransforms {
    fn name(&self) -> &'static str {
        "prepare-reskate-uv-transforms"
    }
    fn options(&self) -> Options {
        Options::from([
            ("anchor".to_owned(), Anchor(self.anchor).text().to_owned()),
            ("conflicts".to_owned(), self.conflicts.text().to_owned()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "pbr.material"]
    }
    fn removes(&self) -> &[&'static str] {
        &["core.mesh_file"]
    }
    /// It lowers texture transforms into the coordinates.
    fn legalizes(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn approximates(&self) -> &'static str {
        "slots tiling otherwise than their anchor take its tiling; coordinates transformed once in f32"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let sampled = drawn(w);
        let materials: Vec<Entity> = w.iter::<PbrMaterial>().map(|(e, _)| e).collect();
        for e in materials {
            let name = name_of(w, e);
            let m = w.get_mut::<PbrMaterial>(e).expect("listed");
            let Some(anchor) = anchor_of(m, self.anchor) else {
                continue;
            };
            if let Some((s, r)) = m.slots().find(|(_, r)| r.texcoord != 0) {
                if !sampled.contains(&e) {
                    log.count(
                        "materials nothing draws sampling another set, left as they are",
                        1,
                    );
                    continue;
                }
                return Err(format!(
                    "prepare-reskate-uv-transforms: material {name}'s {} texture samples coordinate set {}; skate \
                     samples the first only",
                    s.name(),
                    r.texcoord
                ));
            }
            let norm = |t: Option<UvTransform>| t.filter(|t| *t != UvTransform::IDENTITY);
            let want = m
                .slots()
                .find(|(s, _)| *s == anchor)
                .and_then(|(_, r)| norm(r.transform));
            for (s, r) in m.slots_mut() {
                if norm(r.transform) != want {
                    match self.conflicts {
                        SamplingConflicts::Refuse => {
                            return Err(format!(
                                "prepare-reskate-uv-transforms: material {name}'s {} texture tiles otherwise than its \
                                 {} texture (conflicts=refuse)",
                                s.name(),
                                anchor.name()
                            ));
                        }
                        SamplingConflicts::Adopt => {
                            log.approximate(
                                &format!(
                                    "texture tiling replaced by the {}'s (one coordinate set): {name} {}",
                                    anchor.name(),
                                    s.name()
                                ),
                                1,
                            );
                            log.count("texture slots given their anchor's tiling", 1);
                        }
                    }
                }
                r.transform = want;
            }
        }
        fold_layer_tiling(w, log)?;
        let baked = primase_meshops::bake_texture_transforms(w);
        log.absorb(baked);
        log.approximate(
            "coordinate sets transformed in f32",
            log.counts.get("coordinate sets baked").copied().unwrap_or(0),
        );
        // a tiling left on a drawn material would be lost: refused, with the baker's reasons
        for e in drawn(w) {
            if w.get::<PbrMaterial>(e)
                .is_some_and(|m| m.textures().any(|r| r.transform.is_some()))
            {
                let reasons: Vec<&str> = log
                    .counts
                    .keys()
                    .filter_map(|k| k.strip_prefix("not baked: "))
                    .collect();
                return Err(format!(
                    "prepare-reskate-uv-transforms: material {}'s tiling could not be baked into the coordinates ({}); \
                     skate samples untransformed (materials sharing a primitive that tile differently: \
                     specialize-material-variants first)",
                    name_of(w, e),
                    reasons.join("; ")
                ));
            }
        }
        Ok(())
    }
}

/// `prepare-reskate-wrap`: see the module documentation. Made by [`PrepareReskateWrap::new`]; from options by the same
/// call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrepareReskateWrap {
    anchor: TextureSlot,
    /// The anchor's axis every slot wraps by: 0 u, 1 v.
    axis: usize,
}

impl PrepareReskateWrap {
    /// Every slot wraps as `anchor` wraps along `axis` (0: u, 1: v; refused otherwise).
    pub fn new(anchor: TextureSlot, axis: usize) -> Result<PrepareReskateWrap, String> {
        if axis > 1 {
            return Err(format!("axis {axis}: 0 (u) or 1 (v)"));
        }
        Ok(PrepareReskateWrap { anchor, axis })
    }

    pub fn anchor(&self) -> TextureSlot {
        self.anchor
    }

    pub fn axis(&self) -> usize {
        self.axis
    }

    /// From its options: `anchor=<slot>` and `axis=u|v`, through [`PrepareReskateWrap::new`].
    fn from_given(g: &Given) -> Result<PrepareReskateWrap, String> {
        PrepareReskateWrap::new(
            g.choice::<Anchor>("anchor").0,
            g.choice::<WrapAxis>("axis") as usize,
        )
    }
}

/// `axis`'s values, as [`PrepareReskateWrap`] counts them (0: u, 1: v).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WrapAxis {
    U = 0,
    V = 1,
}

impl Choice for WrapAxis {
    const VALUES: &'static [(&'static str, Self)] = &[("u", WrapAxis::U), ("v", WrapAxis::V)];
}

/// `prepare-reskate-wrap`'s declaration ([`primase_core::pass::PassRegistry::add`]): the anchor and its axis are the
/// program's, no default.
pub const PREPARE_RESKATE_WRAP: PassSpec = PassSpec {
    name: "prepare-reskate-wrap",
    options: &[
        ANCHOR,
        OptionSpec::choice::<WrapAxis>("axis", "u or v: the anchor's wrap along it is every slot's"),
    ],
    make: |g| Ok(Box::new(PrepareReskateWrap::from_given(g)?)),
};

impl Pass for PrepareReskateWrap {
    fn name(&self) -> &'static str {
        "prepare-reskate-wrap"
    }
    fn options(&self) -> Options {
        Options::from([
            ("anchor".to_owned(), Anchor(self.anchor).text().to_owned()),
            (
                "axis".to_owned(),
                [WrapAxis::U, WrapAxis::V][self.axis].text().to_owned(),
            ),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn approximates(&self) -> &'static str {
        "slots wrapping otherwise than their anchor along its axis take its wrap"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let axis = ["u", "v"][self.axis];
        let materials: Vec<Entity> = w.iter::<PbrMaterial>().map(|(e, _)| e).collect();
        for e in materials {
            let name = name_of(w, e);
            let m = w.get_mut::<PbrMaterial>(e).expect("listed");
            let Some(anchor) = anchor_of(m, self.anchor) else {
                continue;
            };
            let want: Wrap = m
                .slots()
                .find(|(s, _)| *s == anchor)
                .map(|(_, r)| r.wrap[self.axis])
                .expect("the anchor is a slot set");
            let mut changed = 0;
            for (_, r) in m.slots_mut() {
                if r.wrap != [want; 2] {
                    r.wrap = [want; 2];
                    changed += 1;
                }
            }
            if changed > 0 {
                log.count("texture slots given their anchor's wrap", changed);
                log.approximate(
                    &format!(
                        "texture wrap made one (the {}'s along {axis}): {name}",
                        anchor.name()
                    ),
                    1,
                );
            }
        }
        Ok(())
    }
}
