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
//!   ([`primase_meshops::bake_texture_transforms`]). A slot sampling another set than the first is refused. A material
//!   drawn whose tiling the baker could not bake (materials sharing a primitive that tile differently: a prototype's
//!   variants, `specialize-material-variants` first) is refused, never erased.
//! - `prepare-reskate-wrap{anchor=<slot>,axis=u|v}`: every slot wraps along both axes as the anchor wraps along `axis`,
//!   each material changed reported by name as an approximation.
//!
//! The anchor is a slot (`base-colour`, `metallic-roughness`, `normal`, `occlusion`, `emissive`, `specular`,
//! `specular-colour`); a material without a texture there follows its first textured slot in glTF's order.

use std::collections::BTreeSet;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{PbrMaterial, TextureSlot, UvTransform, Wrap};
use primase_core::{Entity, LayeredMaterial, MaterialVariants, Mesh, MeshFile, Name, World};

const SLOTS: [(TextureSlot, &str); 7] = [
    (TextureSlot::BaseColor, "base-colour"),
    (TextureSlot::MetallicRoughness, "metallic-roughness"),
    (TextureSlot::Normal, "normal"),
    (TextureSlot::Occlusion, "occlusion"),
    (TextureSlot::Emissive, "emissive"),
    (TextureSlot::Specular, "specular"),
    (TextureSlot::SpecularColor, "specular-colour"),
];

fn slot_of(v: &str) -> Result<TextureSlot, String> {
    SLOTS
        .iter()
        .find(|(_, k)| *k == v)
        .map(|(s, _)| *s)
        .ok_or_else(|| {
            format!(
                "anchor={v}: one of {}",
                SLOTS.iter().map(|(_, k)| *k).collect::<Vec<_>>().join(", ")
            )
        })
}

fn slot_key(s: TextureSlot) -> &'static str {
    SLOTS
        .iter()
        .find(|(x, _)| *x == s)
        .map(|(_, k)| *k)
        .expect("every slot listed")
}

/// The required `anchor` option, the other keys `others` allowed.
fn anchor_option(o: &Options, others: &[&str]) -> Result<TextureSlot, String> {
    if let Some(k) = o
        .keys()
        .find(|k| k.as_str() != "anchor" && !others.contains(&k.as_str()))
    {
        return Err(format!("unknown option {k}"));
    }
    match o.get("anchor") {
        Some(v) => slot_of(v),
        None => Err("anchor is required: the texture slot a material's other slots sample as".into()),
    }
}

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

/// The materials drawn: by a primitive (its own, its variants', its file's), and the layers of a drawn blend.
fn drawn(w: &World) -> BTreeSet<Entity> {
    let mut drawn = BTreeSet::new();
    for (e, mesh) in w.iter::<Mesh>() {
        drawn.extend(mesh.primitives().iter().filter_map(|p| p.material));
        if let Some(v) = w.get::<MaterialVariants>(e) {
            drawn.extend(v.0.iter().flat_map(|m| m.values().copied()));
        }
    }
    for (e, f) in w.iter::<MeshFile>() {
        if !w.has::<Mesh>(e) {
            drawn.extend(f.materials.iter().flatten());
            if let Some(v) = w.get::<MaterialVariants>(e) {
                drawn.extend(v.0.iter().flat_map(|m| m.values().copied()));
            }
        }
    }
    let layers: Vec<Entity> = drawn
        .iter()
        .filter_map(|&e| w.get::<LayeredMaterial>(e))
        .flat_map(|l| l.layers.iter().flatten().map(|l| l.material))
        .collect();
    drawn.extend(layers);
    drawn
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

    /// From its options: `anchor=<slot>` and `conflicts=adopt|refuse`, both required.
    pub fn from_options(o: &Options) -> Result<PrepareReskateUvTransforms, String> {
        let anchor = anchor_option(o, &["conflicts"])?;
        let conflicts = match o.get("conflicts").map(String::as_str) {
            Some("adopt") => SamplingConflicts::Adopt,
            Some("refuse") => SamplingConflicts::Refuse,
            Some(v) => return Err(format!("conflicts={v}: adopt or refuse")),
            None => {
                return Err(
                    "conflicts is required: adopt or refuse (a slot tiling otherwise than its anchor)".into(),
                );
            }
        };
        Ok(PrepareReskateUvTransforms::new(anchor, conflicts))
    }
}

/// [`PrepareReskateUvTransforms::from_options`] as a pass factory.
pub fn prepare_reskate_uv_transforms(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PrepareReskateUvTransforms::from_options(o)?))
}

impl Pass for PrepareReskateUvTransforms {
    fn name(&self) -> &'static str {
        "prepare-reskate-uv-transforms"
    }
    fn options(&self) -> Options {
        Options::from([
            ("anchor".to_owned(), slot_key(self.anchor).to_owned()),
            (
                "conflicts".to_owned(),
                match self.conflicts {
                    SamplingConflicts::Adopt => "adopt",
                    SamplingConflicts::Refuse => "refuse",
                }
                .to_owned(),
            ),
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
        let materials: Vec<Entity> = w.iter::<PbrMaterial>().map(|(e, _)| e).collect();
        for e in materials {
            let name = name_of(w, e);
            let m = w.get_mut::<PbrMaterial>(e).expect("listed");
            let Some(anchor) = anchor_of(m, self.anchor) else {
                continue;
            };
            if let Some((s, r)) = m.slots().find(|(_, r)| r.texcoord != 0) {
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

    /// From its options: `anchor=<slot>` and `axis=u|v`, both required.
    pub fn from_options(o: &Options) -> Result<PrepareReskateWrap, String> {
        let anchor = anchor_option(o, &["axis"])?;
        let axis = match o.get("axis").map(String::as_str) {
            Some("u") => 0,
            Some("v") => 1,
            Some(v) => return Err(format!("axis={v}: u or v")),
            None => {
                return Err("axis is required: u or v (the anchor's wrap along it is every slot's)".into());
            }
        };
        PrepareReskateWrap::new(anchor, axis)
    }
}

/// [`PrepareReskateWrap::from_options`] as a pass factory.
pub fn prepare_reskate_wrap(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PrepareReskateWrap::from_options(o)?))
}

impl Pass for PrepareReskateWrap {
    fn name(&self) -> &'static str {
        "prepare-reskate-wrap"
    }
    fn options(&self) -> Options {
        Options::from([
            ("anchor".to_owned(), slot_key(self.anchor).to_owned()),
            ("axis".to_owned(), ["u", "v"][self.axis].to_owned()),
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
