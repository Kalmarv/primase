//! `pack-orm{size=metallic-roughness|occlusion,missing-levels=generate|refuse}`: a material's separate occlusion and
//! metallic-roughness maps packed into one image, as glTF's ORM layout: R occlusion, G roughness, B metallic, A 255.
//! Both slots then name the packed texture; materials sharing a pair share it. glTF allows the maps apart; a target
//! with one mask image needs them together.
//!
//! The two slots must sample alike (coordinate set, wrap and transform) and both maps be linear: otherwise the pass
//! refuses, nothing guessed. The packed image takes the size and stored levels of the map `size` names; a level of the
//! other map of another size is resampled nearest to it, counted. A level the other map does not store is generated
//! from its full level (`missing-levels=generate`: box-filtered, as [`crate::mip_chain`], counted) or refused
//! (`missing-levels=refuse`). Every map is decoded and checked through [`crate::texture_level_rgba8`], whatever its
//! stored format; a malformed level is an error, never a white one.

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{ColorSpace, Component, Entity, Name, TexFormat, TexRole, Texture, World};

use crate::{Image, Samples, mip_chain, texture_level_rgba8_with};
use primase_core::parallel::{Threads, rows_mut};

/// Which map the packed image takes its size and stored levels from (`size=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrmSize {
    /// The metallic-roughness map's (`metallic-roughness`).
    MetallicRoughness,
    /// The occlusion map's (`occlusion`).
    Occlusion,
}

impl OrmSize {
    /// Its option word.
    pub fn word(self) -> &'static str {
        match self {
            OrmSize::MetallicRoughness => "metallic-roughness",
            OrmSize::Occlusion => "occlusion",
        }
    }
}

/// What becomes of a level of the packed image the other map does not store (`missing-levels=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingLevels {
    /// Made from the other map's full level, box-filtered down (`generate`), counted.
    Generate,
    /// An error (`refuse`).
    Refuse,
}

impl MissingLevels {
    /// Its option word.
    pub fn word(self) -> &'static str {
        match self {
            MissingLevels::Generate => "generate",
            MissingLevels::Refuse => "refuse",
        }
    }
}

/// `pack-orm` (see the module documentation): `size`, which map gives the packed image its size and levels;
/// `missing_levels`, what becomes of a level the other map does not store. Both are the program's choice. The maps are
/// decoded and packed on the world's stated threads (`core.threads`, a row at a time): the same image on any count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackOrm {
    size: OrmSize,
    missing_levels: MissingLevels,
}

impl PackOrm {
    pub fn new(size: OrmSize, missing_levels: MissingLevels) -> PackOrm {
        PackOrm { size, missing_levels }
    }
    pub fn size(&self) -> OrmSize {
        self.size
    }
    pub fn missing_levels(&self) -> MissingLevels {
        self.missing_levels
    }
    /// From its options (both required), through [`PackOrm::new`].
    pub fn from_options(o: &Options) -> Result<PackOrm, String> {
        let mut size = None;
        let mut missing = None;
        for (k, v) in o {
            match (k.as_str(), v.as_str()) {
                ("size", "metallic-roughness") => size = Some(OrmSize::MetallicRoughness),
                ("size", "occlusion") => size = Some(OrmSize::Occlusion),
                ("size", v) => return Err(format!("size={v} is not metallic-roughness or occlusion")),
                ("missing-levels", "generate") => missing = Some(MissingLevels::Generate),
                ("missing-levels", "refuse") => missing = Some(MissingLevels::Refuse),
                ("missing-levels", v) => return Err(format!("missing-levels={v} is not generate or refuse")),
                (k, _) => return Err(format!("unknown option {k}")),
            }
        }
        Ok(PackOrm::new(
            size.ok_or("pack-orm needs size=metallic-roughness|occlusion")?,
            missing.ok_or("pack-orm needs missing-levels=generate|refuse")?,
        ))
    }
}

/// A map's stored levels decoded, and its generated chain once asked for.
struct Decoded {
    levels: Vec<Image>,
    generated: Option<Vec<Image>>,
}

fn rgba(image: &Image) -> &[u8] {
    match &image.samples {
        Samples::U8(v) => v,
        Samples::U16(_) => unreachable!("decoded levels are RGBA8"),
    }
}

impl Decoded {
    fn of(threads: Threads, t: &Texture, what: &str) -> Result<Decoded, String> {
        if t.color_space != ColorSpace::Linear {
            return Err(format!(
                "the {what} map is not linear: a mask map's channels are data"
            ));
        }
        if t.levels.is_empty() {
            return Err(format!("the {what} map stores no level"));
        }
        let levels = (0..t.levels.len())
            .map(|i| {
                texture_level_rgba8_with(threads, t, i)
                    .map_err(|e| format!("the {what} map's level {i}: {e}"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Decoded {
            levels,
            generated: None,
        })
    }
    /// Level `i`, generated from the full level where it is not stored (`generate`), with whether it was.
    fn level(&mut self, i: usize, missing: MissingLevels, what: &str) -> Result<(&Image, bool), String> {
        if i < self.levels.len() {
            return Ok((&self.levels[i], false));
        }
        if missing == MissingLevels::Refuse {
            return Err(format!(
                "the {what} map stores {} levels, the packed image needs level {i} (missing-levels=refuse)",
                self.levels.len()
            ));
        }
        if self.generated.is_none() {
            self.generated = Some(mip_chain(&self.levels[0]).map_err(|e| e.to_string())?);
        }
        let chain = self.generated.as_ref().expect("generated");
        Ok((&chain[i.min(chain.len() - 1)], true))
    }
}

/// What packing a pair counted.
#[derive(Default)]
struct Tally {
    resampled: u64,
    generated: u64,
}

impl PackOrm {
    /// The pair packed: the size-giving map's levels, the other's at each level (resampled, generated as chosen).
    fn pack(
        &self,
        threads: Threads,
        mr: &Texture,
        ao: &Texture,
        tally: &mut Tally,
    ) -> Result<Texture, String> {
        let mut mr = Decoded::of(threads, mr, "metallic-roughness")?;
        let mut ao = Decoded::of(threads, ao, "occlusion")?;
        let (width, height, count) = match self.size {
            OrmSize::MetallicRoughness => (mr.levels[0].width, mr.levels[0].height, mr.levels.len()),
            OrmSize::Occlusion => (ao.levels[0].width, ao.levels[0].height, ao.levels.len()),
        };
        let mut levels = Vec::with_capacity(count);
        for i in 0..count {
            let (w, h) = ((width >> i).max(1), (height >> i).max(1));
            let (m, m_generated) = mr.level(i, self.missing_levels, "metallic-roughness")?;
            let (a, a_generated) = ao.level(i, self.missing_levels, "occlusion")?;
            tally.generated += u64::from(m_generated) + u64::from(a_generated);
            for img in [m, a] {
                if (img.width, img.height) != (w, h) {
                    tally.resampled += 1;
                }
            }
            let at = |img: &Image, x: u32, y: u32| -> [u8; 4] {
                let (sx, sy) = (
                    (u64::from(x) * u64::from(img.width) / u64::from(w)) as usize,
                    (u64::from(y) * u64::from(img.height) / u64::from(h)) as usize,
                );
                let p = (sy * img.width as usize + sx) * 4;
                rgba(img)[p..p + 4].try_into().expect("four channels")
            };
            // each row of the packed level from its own texels (at most the stated threads rows at once)
            let mut out = vec![0u8; w as usize * h as usize * 4];
            rows_mut(threads, &mut out, w as usize * 4, |y, row| {
                for (x, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let (o, r) = (at(a, x as u32, y as u32), at(m, x as u32, y as u32));
                    *p = [o[0], r[1], r[2], 255];
                }
            })
            .map_err(|e| e.to_string())?;
            levels.push(out);
        }
        Ok(Texture {
            width,
            height,
            format: TexFormat::Rgba8Unorm,
            levels,
            color_space: ColorSpace::Linear,
            role: TexRole::Mask,
            normal: None,
        })
    }
}

impl Pass for PackOrm {
    fn name(&self) -> &'static str {
        "pack-orm"
    }
    fn options(&self) -> Options {
        Options::from([
            ("size".to_owned(), self.size.word().to_owned()),
            ("missing-levels".to_owned(), self.missing_levels.word().to_owned()),
        ])
    }
    // a world without materials has nothing to pack
    fn modifies(&self) -> &[&'static str] {
        &[PbrMaterial::NAME]
    }
    /// The packed image is a new texture, named after its metallic-roughness map.
    fn provides(&self) -> &[&'static str] {
        &[Texture::NAME, Name::NAME]
    }
    fn approximates(&self) -> &'static str {
        "a map's level of another size resampled nearest to the packed size; with missing-levels=generate, a level a \
         map does not store box-filtered from its full level"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<(Entity, PbrMaterial)> =
            world.iter::<PbrMaterial>().map(|(e, m)| (e, m.clone())).collect();
        // (metallic-roughness texture, occlusion texture) -> packed texture
        let mut packed: BTreeMap<(Entity, Entity), Entity> = BTreeMap::new();
        // recorded as each image is made and each material rebound: a later refusal keeps what was done before it
        const MATERIALS: &str = "materials given a packed occlusion-roughness-metallic image";
        const IMAGES: &str = "packed occlusion-roughness-metallic images";
        log.count(MATERIALS, 0);
        log.count(IMAGES, 0);
        for (e, m) in materials {
            let (Some(mr), Some(ao)) = (m.metallic_roughness_texture, m.occlusion_texture) else {
                continue;
            };
            if mr.texture == ao.texture {
                continue;
            }
            if (mr.texcoord, mr.wrap, mr.transform) != (ao.texcoord, ao.wrap, ao.transform) {
                return Err(format!(
                    "material {e:?}: its occlusion and metallic-roughness maps sample differently (coordinate set, \
                     wrap or transform): one packed image cannot sample both"
                ));
            }
            let texture = match packed.get(&(mr.texture, ao.texture)) {
                Some(&t) => t,
                None => {
                    let source = |t: Entity, what: &str| {
                        world
                            .get::<Texture>(t)
                            .ok_or_else(|| format!("material {e:?}: its {what} map has no pixels loaded"))
                    };
                    let mut tally = Tally::default();
                    let image = self
                        .pack(
                            Threads::of(world),
                            source(mr.texture, "metallic-roughness")?,
                            source(ao.texture, "occlusion")?,
                            &mut tally,
                        )
                        .map_err(|why| format!("material {e:?}: {why}"))?;
                    let name = world.get::<Name>(mr.texture).map(|n| format!("{}_orm", n.0));
                    let t = world.spawn();
                    world.insert(t, image);
                    if let Some(name) = name {
                        world.insert(t, Name(name));
                    }
                    packed.insert((mr.texture, ao.texture), t);
                    log.count(IMAGES, 1);
                    if tally.resampled > 0 {
                        log.approximate("map levels resampled nearest to the packed size", tally.resampled);
                    }
                    if tally.generated > 0 {
                        log.approximate(
                            "map levels not stored, box-filtered from the map's full level",
                            tally.generated,
                        );
                    }
                    t
                }
            };
            let p = world.get_mut::<PbrMaterial>(e).expect("listed");
            for r in [&mut p.metallic_roughness_texture, &mut p.occlusion_texture]
                .into_iter()
                .flatten()
            {
                r.texture = texture;
            }
            log.count(MATERIALS, 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
    use primase_core::pbr::{TextureRef, UvTransform};

    fn map(w: u32, h: u32, levels: Vec<Vec<[u8; 4]>>) -> Texture {
        Texture {
            width: w,
            height: h,
            format: TexFormat::Rgba8Unorm,
            levels: levels
                .into_iter()
                .map(|l| l.into_iter().flatten().collect())
                .collect(),
            color_space: ColorSpace::Linear,
            role: TexRole::Mask,
            normal: None,
        }
    }

    /// A world with a metallic-roughness map `mr` and an occlusion map `ao`, and `n` materials using both.
    fn world(mr: Texture, ao: Texture, n: usize) -> (World, Vec<Entity>) {
        let mut w = World::new();
        let (a, b) = (w.spawn(), w.spawn());
        w.insert(a, mr);
        w.insert(a, Name("rock_mr".into()));
        w.insert(b, ao);
        let materials = (0..n)
            .map(|_| {
                let e = w.spawn();
                w.insert(
                    e,
                    PbrMaterial {
                        metallic_roughness_texture: Some(TextureRef::new(a)),
                        occlusion_texture: Some(TextureRef::new(b)),
                        ..PbrMaterial::default()
                    },
                );
                e
            })
            .collect();
        (w, materials)
    }

    fn run(pass: PackOrm, w: &mut World) -> Result<RunReport, String> {
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .run(
                w,
                &Pipeline {
                    passes: vec![Box::new(pass)],
                },
                &mut report,
            )
            .map_err(|e| e.to_string())?;
        Ok(report)
    }

    const MR: PackOrm = PackOrm {
        size: OrmSize::MetallicRoughness,
        missing_levels: MissingLevels::Generate,
    };

    #[test]
    fn occlusion_moves_to_red_beside_roughness_and_metallic_and_a_pair_is_packed_once() {
        let (mut w, ms) = world(
            map(2, 1, vec![vec![[0, 100, 50, 255], [0, 101, 51, 255]]]),
            map(2, 1, vec![vec![[200, 0, 0, 255], [201, 0, 0, 255]]]),
            3,
        );
        let report = run(MR, &mut w).unwrap();
        let p = w.get::<PbrMaterial>(ms[0]).unwrap();
        let t = p.metallic_roughness_texture.unwrap().texture;
        assert_eq!(p.occlusion_texture.unwrap().texture, t);
        for &m in &ms {
            assert_eq!(
                w.get::<PbrMaterial>(m)
                    .unwrap()
                    .occlusion_texture
                    .unwrap()
                    .texture,
                t
            );
        }
        assert_eq!(
            w.get::<Texture>(t).unwrap().levels[0],
            [200, 100, 50, 255, 201, 101, 51, 255]
        );
        assert_eq!(w.get::<Name>(t).unwrap().0, "rock_mr_orm");
        let log = &report.steps[0].log;
        assert_eq!(log.counts["packed occlusion-roughness-metallic images"], 1);
        assert_eq!(
            log.counts["materials given a packed occlusion-roughness-metallic image"],
            3
        );
        assert!(log.approximations.is_empty());
    }

    #[test]
    fn maps_that_sample_differently_are_refused() {
        let (mut w, ms) = world(map(1, 1, vec![vec![[0; 4]]]), map(1, 1, vec![vec![[0; 4]]]), 1);
        w.get_mut::<PbrMaterial>(ms[0])
            .unwrap()
            .occlusion_texture
            .as_mut()
            .unwrap()
            .transform = Some(UvTransform {
            scale: [2.0, 2.0],
            ..UvTransform::IDENTITY
        });
        assert!(run(MR, &mut w).unwrap_err().contains("sample differently"));
        // and a map whose channels are colour
        let (mut w, _) = world(map(1, 1, vec![vec![[0; 4]]]), map(1, 1, vec![vec![[0; 4]]]), 1);
        let ao = w.iter::<Texture>().nth(1).map(|(e, _)| e).unwrap();
        w.get_mut::<Texture>(ao).unwrap().color_space = ColorSpace::Srgb;
        assert!(run(MR, &mut w).unwrap_err().contains("not linear"));
    }

    /// The metallic-roughness map has 2 x 2 and 1 x 1 levels; the occlusion map only its 2 x 2.
    fn short_occlusion() -> (World, Vec<Entity>) {
        world(
            map(2, 2, vec![vec![[0, 10, 20, 255]; 4], vec![[0, 30, 40, 255]]]),
            map(
                2,
                2,
                vec![vec![
                    [100, 0, 0, 255],
                    [200, 0, 0, 255],
                    [100, 0, 0, 255],
                    [200, 0, 0, 255],
                ]],
            ),
            1,
        )
    }

    #[test]
    fn a_level_the_occlusion_map_lacks_is_generated_from_its_full_level_or_refused() {
        let (mut w, ms) = short_occlusion();
        let report = run(MR, &mut w).unwrap();
        let t = w
            .get::<PbrMaterial>(ms[0])
            .unwrap()
            .occlusion_texture
            .unwrap()
            .texture;
        let packed = w.get::<Texture>(t).unwrap();
        // the 1 x 1 level's occlusion: the box mean of 100, 200, 100, 200, not white
        assert_eq!(packed.levels[1], [150, 30, 40, 255]);
        assert_eq!(
            report.steps[0].log.approximations["map levels not stored, box-filtered from the map's full level"],
            1
        );
        let (mut w, _) = short_occlusion();
        let refused = run(
            PackOrm::new(OrmSize::MetallicRoughness, MissingLevels::Refuse),
            &mut w,
        )
        .unwrap_err();
        assert!(refused.contains("missing-levels=refuse"), "{refused}");
    }

    #[test]
    fn the_other_map_is_resampled_to_the_size_chosen() {
        // a 2 x 1 occlusion map and a 1 x 1 metallic-roughness map
        let pair = || {
            world(
                map(1, 1, vec![vec![[0, 10, 20, 255]]]),
                map(2, 1, vec![vec![[100, 0, 0, 255], [200, 0, 0, 255]]]),
                1,
            )
        };
        let (mut w, ms) = pair();
        let report = run(MR, &mut w).unwrap();
        let t = w
            .get::<PbrMaterial>(ms[0])
            .unwrap()
            .occlusion_texture
            .unwrap()
            .texture;
        assert_eq!(w.get::<Texture>(t).unwrap().levels, [vec![100, 10, 20, 255]]);
        assert_eq!(
            report.steps[0].log.approximations["map levels resampled nearest to the packed size"],
            1
        );
        let (mut w, ms) = pair();
        run(PackOrm::new(OrmSize::Occlusion, MissingLevels::Generate), &mut w).unwrap();
        let t = w
            .get::<PbrMaterial>(ms[0])
            .unwrap()
            .occlusion_texture
            .unwrap()
            .texture;
        assert_eq!(
            w.get::<Texture>(t).unwrap().levels,
            [vec![100, 10, 20, 255, 200, 10, 20, 255]]
        );
    }

    /// A pair packed before a refused one stays packed, and its records with it.
    #[test]
    fn what_was_packed_before_a_refusal_is_recorded() {
        // the first material's maps differ in size (resampled, counted); the second samples differently (refused)
        let (mut w, ms) = world(
            map(1, 1, vec![vec![[0, 10, 20, 255]]]),
            map(2, 1, vec![vec![[100, 0, 0, 255], [200, 0, 0, 255]]]),
            2,
        );
        w.get_mut::<PbrMaterial>(ms[1])
            .unwrap()
            .occlusion_texture
            .as_mut()
            .unwrap()
            .texcoord = 1;
        let mut log = PassLog::default();
        assert!(
            MR.run(&mut w, &mut log)
                .unwrap_err()
                .contains("sample differently")
        );
        let first = w.get::<PbrMaterial>(ms[0]).unwrap();
        assert_eq!(
            first.metallic_roughness_texture.unwrap().texture,
            first.occlusion_texture.unwrap().texture
        );
        assert_eq!(log.counts["packed occlusion-roughness-metallic images"], 1);
        assert_eq!(
            log.counts["materials given a packed occlusion-roughness-metallic image"],
            1
        );
        assert_eq!(
            log.approximations["map levels resampled nearest to the packed size"],
            1
        );
    }

    #[test]
    fn options_say_every_choice_and_build_the_same_pass() {
        for pass in [
            PackOrm::new(OrmSize::MetallicRoughness, MissingLevels::Generate),
            PackOrm::new(OrmSize::Occlusion, MissingLevels::Refuse),
        ] {
            assert_eq!(PackOrm::from_options(&pass.options()), Ok(pass));
        }
        assert!(PackOrm::from_options(&Options::new()).is_err());
        assert!(
            PackOrm::from_options(&Options::from([("size".into(), "metallic-roughness".into())])).is_err()
        );
    }
}
