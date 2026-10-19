//! `budget-reskate-textures{max=N[,aux_weight=W]}`: at most N textures drawn, the rest replaced by mean colours.
//!
//! A map's textures are weighed by the placed surface drawing them: each `skate.material` drawn by a placed primitive
//! (its own material or the placement's variant) counts the world-space area of those triangles, all of it for its
//! base texture and `aux_weight` of it (0.25 when not given) for its normal map and Studio mask. When more than N
//! textures are drawn, the N - 1 heaviest are kept (ties by the textures' `skate.texture` keys, never by arrival
//! order) and the last place is a white texture: every material whose base texture is not kept draws white with its
//! base texture's mean colour as the base colour factor (`skate.base_factor`; the mean weighs each texel by its
//! alpha, as the writer measures a texture), and a normal map or mask not kept is left off. Each material changed is
//! an approximation, counted. N is the program's choice; no pass, no budget.
//!
//! The weights are the whole world's: run it on the whole map (one part, after its meshes are loaded); a map written in
//! parts budgets each part on its own. It runs after the passes that set textures (`raise-skate`, `cap-textures`,
//! `prepare-reskate-textures`) and before the writer.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Entity, Instance, MaterialVariants, Mesh, Texture, Topology, Transform, World};

use crate::components::{SkateBaseFactor, SkateMaterial, SkateTexture};
use crate::texture::{TexSource, mean_color};

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct BudgetReskateTextures {
    max: usize,
    aux_weight: f64,
}

/// The aux weight when none is given: the reference writer's.
pub const AUX_WEIGHT: f64 = 0.25;

impl BudgetReskateTextures {
    /// At most `max` textures (1 or more), normal maps and masks weighed at [`AUX_WEIGHT`].
    pub fn new(max: usize) -> Result<BudgetReskateTextures, String> {
        if max == 0 {
            return Err("a texture budget is 1 texture or more".into());
        }
        Ok(BudgetReskateTextures {
            max,
            aux_weight: AUX_WEIGHT,
        })
    }

    /// Normal maps and masks weighed at `w` times their materials' area (finite, 0 or more).
    pub fn with_aux_weight(mut self, w: f64) -> Result<BudgetReskateTextures, String> {
        if !(w.is_finite() && w >= 0.0) {
            return Err(format!("aux_weight {w} is not a finite weight of 0 or more"));
        }
        self.aux_weight = w;
        Ok(self)
    }

    pub fn max(&self) -> usize {
        self.max
    }

    pub fn aux_weight(&self) -> f64 {
        self.aux_weight
    }

    /// From its options (`max` required), through [`BudgetReskateTextures::new`] and the same setter.
    pub fn from_options(o: &Options) -> Result<BudgetReskateTextures, String> {
        let max = o
            .get("max")
            .ok_or("max=N is required: the most textures the map draws")?;
        let max = max
            .parse::<usize>()
            .map_err(|_| format!("max={max} is not a number of textures"))?;
        let mut pass = BudgetReskateTextures::new(max)?;
        for (k, v) in o {
            match k.as_str() {
                "max" => {}
                "aux_weight" => {
                    let w = v
                        .parse::<f64>()
                        .map_err(|_| format!("aux_weight={v} is not a number"))?;
                    pass = pass.with_aux_weight(w)?;
                }
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(pass)
    }
}

/// [`BudgetReskateTextures::from_options`] as a pass factory.
pub fn budget_reskate_textures(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(BudgetReskateTextures::from_options(o)?))
}

/// The world-space area of a primitive's triangles placed by `t`.
fn placed_area(mesh_prim: &primase_core::Primitive, t: &Transform) -> f64 {
    if mesh_prim.topology != Topology::Triangles {
        return 0.0;
    }
    let Some(pos) = mesh_prim.positions() else {
        return 0.0;
    };
    let at = |i: u32| -> Option<[f64; 3]> {
        let p = pos.get(i as usize)?.map(f64::from);
        let c = t.cols.map(|c| c.map(f64::from));
        Some(std::array::from_fn(|r| {
            c[0][r] * p[0] + c[1][r] * p[1] + c[2][r] * p[2] + c[3][r]
        }))
    };
    let sequential: Vec<u32>;
    let idx = match &mesh_prim.indices {
        Some(i) => i.as_slice(),
        None => {
            sequential = (0..pos.len() as u32).collect();
            &sequential
        }
    };
    let mut area = 0.0;
    for tri in idx.as_chunks::<3>().0 {
        let (Some(a), Some(b), Some(c)) = (at(tri[0]), at(tri[1]), at(tri[2])) else {
            continue;
        };
        let (u, v) = (
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
        );
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        area += 0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    }
    area
}

/// The mean colour of a texture's first level as the writer measures it, linear RGBA (alpha 1) when `srgb`.
fn mean_factor(w: &World, t: Entity) -> Result<[f32; 4], String> {
    let (Some(tex), Some(st)) = (w.get::<Texture>(t), w.get::<SkateTexture>(t)) else {
        return Ok([0.5, 0.5, 0.5, 1.0]);
    };
    let src = TexSource::from_texture(&st.key, tex, None, None).map_err(|e| e.to_string())?;
    let mean = src
        .decode(0, tex.width, tex.height)
        .map_err(|e| e.to_string())?
        .map_or(0x808080, |px| mean_color(&px));
    let srgb = st.linear != Some(true);
    let channel = |c: u32| {
        let v = c as f32 / 255.0;
        if srgb {
            primase_image::srgb_to_linear_f32(v)
        } else {
            v
        }
    };
    Ok([
        channel((mean >> 16) & 255),
        channel((mean >> 8) & 255),
        channel(mean & 255),
        1.0,
    ])
}

impl Pass for BudgetReskateTextures {
    fn name(&self) -> &'static str {
        "budget-reskate-textures"
    }
    fn options(&self) -> Options {
        Options::from([
            ("max".to_owned(), self.max.to_string()),
            ("aux_weight".to_owned(), self.aux_weight.to_string()),
        ])
    }
    // the materials are optional: a map without them has no texture to prepare
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.base_factor"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["skate.material"]
    }
    fn approximates(&self) -> &'static str {
        "textures past the budget: base textures drawn as their mean colour, normal maps and masks left off"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let max = self.max;
        // the placed area each material draws
        let mut area: BTreeMap<Entity, f64> = BTreeMap::new();
        for (e, i) in w.iter::<Instance>() {
            let Some(mesh) = w.get::<Mesh>(i.prototype) else {
                continue;
            };
            let t = w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY);
            let variants = w.get::<MaterialVariants>(i.prototype);
            for (k, p) in mesh.primitives().iter().enumerate() {
                let m = variants
                    .and_then(|v| v.0.get(k))
                    .and_then(|row| row.get(&i.variant).copied())
                    .or(p.material);
                if let Some(m) = m.filter(|&m| w.has::<SkateMaterial>(m)) {
                    *area.entry(m).or_default() += placed_area(p, &t);
                }
            }
        }
        // each texture's weight
        let mut weight: BTreeMap<Entity, f64> = BTreeMap::new();
        for (&m, &a) in &area {
            let mat = w.get::<SkateMaterial>(m).expect("listed");
            for (t, k) in [
                (mat.base, 1.0),
                (mat.normal, self.aux_weight),
                (mat.mask, self.aux_weight),
            ] {
                if let Some(t) = t {
                    *weight.entry(t).or_default() += a * k;
                }
            }
        }
        log.count("textures drawn", weight.len() as u64);
        if weight.len() <= max {
            return Ok(());
        }
        let key = |t: Entity| {
            w.get::<SkateTexture>(t)
                .map_or_else(String::new, |s| s.key.clone())
        };
        let mut ranked: Vec<(Entity, f64, String)> = weight.iter().map(|(&t, &v)| (t, v, key(t))).collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
        let keep: BTreeSet<Entity> = ranked.iter().take(max - 1).map(|r| r.0).collect();
        // every change prepared before the first
        let mut changes: Vec<(Entity, SkateMaterial, Option<[f32; 4]>)> = Vec::new();
        for &m in area.keys() {
            let mut mat = w.get::<SkateMaterial>(m).expect("listed").clone();
            let mut factor = None;
            mat.normal = mat.normal.filter(|t| keep.contains(t));
            mat.mask = mat.mask.filter(|t| keep.contains(t));
            if let Some(base) = mat.base.filter(|t| !keep.contains(t)) {
                factor = Some(mean_factor(w, base)?);
                mat.base = None;
                mat.tint = None;
                mat.base_color = Some([255, 255, 255, 255]);
            }
            let original = w.get::<SkateMaterial>(m).expect("listed");
            if &mat != original {
                changes.push((m, mat, factor));
            }
        }
        for (m, mat, factor) in changes {
            if let Some(f) = factor {
                w.insert(m, SkateBaseFactor(f));
                log.approximate(
                    &format!(
                        "materials past a budget of {max} textures drawn as their base texture's mean colour"
                    ),
                    1,
                );
            } else {
                log.approximate(
                    &format!(
                        "materials past a budget of {max} textures drawn without their normal map or mask"
                    ),
                    1,
                );
            }
            w.insert(m, mat);
        }
        log.count(
            "textures dropped by the budget",
            (weight.len() - keep.len()) as u64,
        );
        Ok(())
    }
}
