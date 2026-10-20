//! `prepare-reskate-blended-surfaces{cutoff=C,materials=all|<name>;<name>...}`: blended surfaces drawn as alpha-tested
//! cutouts, before `raise-skate`. Studio compiles an alpha-blended material to the stock decal/blend shader pair, which
//! skate does not draw as standalone world geometry (a blended tree canopy vanishes); an alpha-tested one compiles to
//! the cutout programs and draws. So a blended PBR material that is neither a decal (drawn over the surface beneath it,
//! which skate's decal path draws: `core.draw_order`'s decal layer) nor water (`core.material_usage` water) is drawn as
//! a mask at the stated cutoff: soft edges become hard, and a mostly transparent surface (glass) mostly vanishes. Each
//! material changed is reported by name as an approximation.
//!
//! An explicit workaround for the compiled rendering path observed, chosen by the program: `materials=all` takes every
//! eligible blended material, or a list of names takes those only (a named material present but not eligible, or not
//! blended, is refused). The cutoff has no default. Not the same predicate as `prepare-reskate-uv-repeats{blended=}`,
//! which adapts only triangles past Studio's repeat limit, after raising.

use std::collections::BTreeSet;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{DrawLayer, DrawOrder, Entity, MaterialUsage, Name, World};

/// Which blended materials become cutouts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlendedSelection {
    /// Every eligible blended material (not a decal, not water).
    All,
    /// The materials of these names only; each must be an eligible blended material where present.
    Named(BTreeSet<String>),
}

/// See the module documentation. Made by [`PrepareReskateBlendedSurfaces::new`]; from options by the same call.
#[derive(Clone, Debug, PartialEq)]
pub struct PrepareReskateBlendedSurfaces {
    cutoff: f32,
    selection: BlendedSelection,
}

impl PrepareReskateBlendedSurfaces {
    /// Blended surfaces of `selection` drawn as cutouts at `cutoff` (finite, above 0 and at most 1); a named selection
    /// names at least one material.
    pub fn new(cutoff: f32, selection: BlendedSelection) -> Result<PrepareReskateBlendedSurfaces, String> {
        if !(cutoff.is_finite() && cutoff > 0.0 && cutoff <= 1.0) {
            return Err(format!("cutoff {cutoff} is not in (0, 1]"));
        }
        if let BlendedSelection::Named(names) = &selection
            && (names.is_empty() || names.iter().any(String::is_empty))
        {
            return Err("materials names no material (all, or names separated by ;)".into());
        }
        Ok(PrepareReskateBlendedSurfaces { cutoff, selection })
    }

    pub fn cutoff(&self) -> f32 {
        self.cutoff
    }

    pub fn selection(&self) -> &BlendedSelection {
        &self.selection
    }

    /// From its options: `cutoff` and `materials` (`all`, or names separated by `;`), both required.
    pub fn from_options(o: &Options) -> Result<PrepareReskateBlendedSurfaces, String> {
        if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "cutoff" | "materials")) {
            return Err(format!("unknown option {k}"));
        }
        let cutoff = match o.get("cutoff") {
            Some(v) => v
                .parse::<f32>()
                .map_err(|_| format!("cutoff={v} is not a number"))?,
            None => return Err("cutoff is required: the alpha a cutout surface draws above".into()),
        };
        let selection = match o.get("materials").map(String::as_str) {
            Some("all") => BlendedSelection::All,
            Some(names) => BlendedSelection::Named(names.split(';').map(str::to_owned).collect()),
            None => return Err("materials is required: all, or names separated by ;".into()),
        };
        PrepareReskateBlendedSurfaces::new(cutoff, selection)
    }
}

/// [`PrepareReskateBlendedSurfaces::from_options`] as a pass factory.
pub fn prepare_reskate_blended_surfaces(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PrepareReskateBlendedSurfaces::from_options(o)?))
}

/// Why a blended material is not drawn as a cutout: drawn over a surface, or water; `None`: eligible.
fn kept_blended(w: &World, e: Entity) -> Option<&'static str> {
    if DrawOrder::of(w, e).layer == DrawLayer::Decal {
        Some("a decal")
    } else if w.get::<MaterialUsage>(e).copied() == Some(MaterialUsage::Water) {
        Some("water")
    } else {
        None
    }
}

impl Pass for PrepareReskateBlendedSurfaces {
    fn name(&self) -> &'static str {
        "prepare-reskate-blended-surfaces"
    }
    fn options(&self) -> Options {
        Options::from([
            ("cutoff".to_owned(), self.cutoff.to_string()),
            (
                "materials".to_owned(),
                match &self.selection {
                    BlendedSelection::All => "all".to_owned(),
                    BlendedSelection::Named(n) => n.iter().cloned().collect::<Vec<_>>().join(";"),
                },
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
        "blended surfaces drawn as alpha-tested cutouts: soft edges hard, mostly transparent surfaces mostly gone"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let name_of =
            |w: &World, e: Entity| w.get::<Name>(e).map_or_else(|| format!("{e:?}"), |n| n.0.clone());
        let materials: Vec<Entity> = w.iter::<PbrMaterial>().map(|(e, _)| e).collect();
        for e in materials {
            let name = name_of(w, e);
            let named = match &self.selection {
                BlendedSelection::All => None,
                BlendedSelection::Named(n) => Some(n.contains(&name)),
            };
            if named == Some(false) {
                continue;
            }
            let blended = w
                .get::<PbrMaterial>(e)
                .is_some_and(|m| m.alpha_mode == AlphaMode::Blend);
            let kept = kept_blended(w, e);
            match (blended, kept, named) {
                // a material the program named must be one this pass changes
                (false, _, Some(true)) => {
                    return Err(format!(
                        "prepare-reskate-blended-surfaces: material {name} is named but is not blended"
                    ));
                }
                (true, Some(why), Some(true)) => {
                    return Err(format!(
                        "prepare-reskate-blended-surfaces: material {name} is named but is {why}, which stays blended"
                    ));
                }
                (false, _, _) => {}
                (true, Some(why), _) => {
                    log.count(&format!("blended materials kept blended: {why}"), 1);
                }
                (true, None, _) => {
                    if let Some(m) = w.get_mut::<PbrMaterial>(e) {
                        m.alpha_mode = AlphaMode::Mask { cutoff: self.cutoff };
                    }
                    log.approximate(
                        &format!("blended surface drawn as a cutout at {}: {name}", self.cutoff),
                        1,
                    );
                    log.count("blended surfaces drawn as cutouts", 1);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material(w: &mut World, name: &str, alpha: AlphaMode) -> Entity {
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        w.insert(
            e,
            PbrMaterial {
                alpha_mode: alpha,
                ..PbrMaterial::default()
            },
        );
        e
    }

    fn mode(w: &World, e: Entity) -> AlphaMode {
        w.get::<PbrMaterial>(e).unwrap().alpha_mode
    }

    /// Every eligible blended material becomes a cutout at the cutoff, each reported by name; a decal, water and an
    /// opaque material are left as they are.
    #[test]
    fn eligible_blended_surfaces_become_cutouts() {
        let mut w = World::new();
        let leaves = material(&mut w, "leaves", AlphaMode::Blend);
        let glass = material(&mut w, "glass", AlphaMode::Blend);
        let decal = material(&mut w, "decal", AlphaMode::Blend);
        w.insert(decal, DrawOrder::conventional(DrawLayer::Decal, 2500));
        let water = material(&mut w, "water", AlphaMode::Blend);
        w.insert(water, MaterialUsage::Water);
        let stone = material(&mut w, "stone", AlphaMode::Opaque);
        let mut log = PassLog::default();
        PrepareReskateBlendedSurfaces::new(0.5, BlendedSelection::All)
            .unwrap()
            .run(&mut w, &mut log)
            .unwrap();
        for e in [leaves, glass] {
            assert_eq!(mode(&w, e), AlphaMode::Mask { cutoff: 0.5 });
        }
        assert_eq!(mode(&w, decal), AlphaMode::Blend);
        assert_eq!(mode(&w, water), AlphaMode::Blend);
        assert_eq!(mode(&w, stone), AlphaMode::Opaque);
        assert_eq!(
            log.approximations["blended surface drawn as a cutout at 0.5: leaves"],
            1
        );
        assert_eq!(
            log.approximations["blended surface drawn as a cutout at 0.5: glass"],
            1
        );
        assert_eq!(log.counts["blended materials kept blended: a decal"], 1);
        assert_eq!(log.counts["blended materials kept blended: water"], 1);
    }

    /// A named selection changes those materials only (glass is not foliage because both blend), and refuses a named
    /// material that is not blended or stays blended.
    #[test]
    fn a_named_selection_changes_those_only() {
        let mut w = World::new();
        let leaves = material(&mut w, "leaves", AlphaMode::Blend);
        let glass = material(&mut w, "glass", AlphaMode::Blend);
        let mut log = PassLog::default();
        let pass = PrepareReskateBlendedSurfaces::from_options(&Options::from([
            ("cutoff".to_owned(), "0.4".to_owned()),
            ("materials".to_owned(), "leaves".to_owned()),
        ]))
        .unwrap();
        pass.run(&mut w, &mut log).unwrap();
        assert_eq!(mode(&w, leaves), AlphaMode::Mask { cutoff: 0.4 });
        assert_eq!(mode(&w, glass), AlphaMode::Blend);
        // a named opaque material
        let mut w = World::new();
        material(&mut w, "leaves", AlphaMode::Opaque);
        let e = pass.run(&mut w, &mut PassLog::default()).unwrap_err();
        assert!(e.contains("leaves is named but is not blended"), "{e}");
        // a named decal
        let mut w = World::new();
        let d = material(&mut w, "leaves", AlphaMode::Blend);
        w.insert(d, DrawOrder::conventional(DrawLayer::Decal, 2500));
        let e = pass.run(&mut w, &mut PassLog::default()).unwrap_err();
        assert!(e.contains("is a decal, which stays blended"), "{e}");
    }

    /// Both choices are required and printed; a cutoff outside (0, 1] and an empty name list are refused.
    #[test]
    fn the_choices_are_required_and_printed() {
        let o = |pairs: &[(&str, &str)]| -> Options {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        assert!(PrepareReskateBlendedSurfaces::from_options(&o(&[("materials", "all")])).is_err());
        assert!(PrepareReskateBlendedSurfaces::from_options(&o(&[("cutoff", "0.5")])).is_err());
        for bad in ["0", "1.5", "NaN"] {
            assert!(
                PrepareReskateBlendedSurfaces::from_options(&o(&[("cutoff", bad), ("materials", "all")]))
                    .is_err(),
                "{bad}"
            );
        }
        assert!(
            PrepareReskateBlendedSurfaces::from_options(&o(&[("cutoff", "0.5"), ("materials", "")])).is_err()
        );
        let p = PrepareReskateBlendedSurfaces::from_options(&o(&[("cutoff", "0.5"), ("materials", "a;b")]))
            .unwrap();
        assert_eq!(
            PrepareReskateBlendedSurfaces::from_options(&p.options()).unwrap(),
            p
        );
        assert_eq!(p.options()["materials"], "a;b");
    }
}
