//! A Sekiro area through the convert loop ([`primase_core::convert`]): [`AreaReader`] reads an area of an install
//! with [`super::area::read_area`] and gives each part the closure of its roots in that world.
//!
//! [`super::area::read_area`] decodes every model, texture and collision file it keeps while it reads (the archive's files
//! are compressed whole), so the read world already holds the payloads: a part's load is its roots' closure, nothing
//! read again, with the source files (`fromsoft.texture_payloads`, `fromsoft.shader_programs`) its materials bind
//! and no others, shared with the read world rather than copied. The read is eager: the whole area's source stays
//! resident while the conversion runs, whatever the part size. A prototype's box before any pass is its stored model's posed box in the world frame, the extent the
//! read's region rule uses. What the read left out or could not find ([`super::area::Report::notices`]) is the
//! reader's notices; a load takes nothing from the source and notices nothing.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use primase_core::convert::{Notice, Reader, Report, Result};
use primase_core::{Entity, World};

use super::area::{Options, read_game_area};
use crate::Install;
use crate::dcx::Decoder;
use crate::materials::Material;
use crate::sekiro::source::{ShaderPrograms, TexturePayloads};

/// Every component and resource an area's read and load put in a world, by name (the names in the namespaces
/// [`Reader::emits`] states): what a coverage report lists of this reader. The reader's honesty test (real data:
/// `installs.sekiro`, `tools.oodle6`) reads slice A of m11_01 and checks that its world holds no other name.
pub const EMITTED_NAMES: &[&str] = &[
    "core.condition",
    "core.instance",
    "core.name",
    "core.shadows",
    "core.source",
    "core.state_axis",
    "core.transform",
    "fromsoft.area_extras",
    "fromsoft.draw_param",
    "fromsoft.flver",
    "fromsoft.layered_material",
    "fromsoft.light",
    "fromsoft.map_collision",
    "fromsoft.material",
    "fromsoft.material_shader",
    "fromsoft.material_slots",
    "fromsoft.object_collision",
    "fromsoft.prototype",
    "fromsoft.read_scope",
    "fromsoft.shader_bundle",
    "fromsoft.shader_programs",
    "fromsoft.shadow_flags",
    "fromsoft.texture_payloads",
];

/// See the module documentation.
#[derive(Debug)]
pub struct AreaReader<'a> {
    install: Install,
    area: String,
    decoder: Decoder<'a>,
    options: Options,
    /// The read's notices, until the conversion takes them.
    pending: Vec<Notice>,
    /// Each prototype's box, as the read took it (`area::Report::prototype_boxes`).
    boxes: BTreeMap<Entity, Option<[[f64; 3]; 2]>>,
}

impl<'a> AreaReader<'a> {
    /// The area `area` (`m11_01` or `m11_01_00_00`) of the install at `install`, read as `options` says, archives
    /// decoded with `decoder`.
    pub fn new(
        install: impl Into<PathBuf>,
        area: &str,
        decoder: Decoder<'a>,
        options: Options,
    ) -> AreaReader<'a> {
        Self::of(Install::sekiro(install), area, decoder, options)
    }

    /// The map `area` of `install` (any game's: [`read_game_area`]), read as `options` says.
    pub fn of(install: Install, area: &str, decoder: Decoder<'a>, options: Options) -> AreaReader<'a> {
        AreaReader {
            install,
            area: area.to_owned(),
            decoder,
            options,
            pending: Vec::new(),
            boxes: BTreeMap::new(),
        }
    }

    /// The conversion's report of a read, its notices kept for [`Reader::notices`].
    fn record(&mut self, r: &super::area::Report) -> Report {
        self.pending.extend(r.notices());
        let mut report = Report::new();
        report.push((
            "sekiro area".into(),
            format!(
                "{}: {} placements of {} prototypes ({} distant-backdrop prototypes with {} placements), {} materials, \
                 {} textures, {} layered-shader programs, models missing {:?}",
                r.area,
                r.placements,
                r.prototypes,
                r.far_prototypes,
                r.far_placements,
                r.materials,
                r.textures,
                r.shader_programs,
                r.missing_models
            ),
        ));
        report.push(("sekiro collision".into(), format!("{:?}", r.collision)));
        report.push((
            "sekiro object collision".into(),
            format!("{:?}", r.object_collision),
        ));
        report.push(("sekiro extras".into(), format!("{:?}", r.extras)));
        if r.draw_param.asked {
            report.push(("sekiro draw param".into(), format!("{:?}", r.draw_param)));
        }
        if r.object_animation.asked {
            report.push((
                "sekiro object animations".into(),
                format!("{:?}", r.object_animation),
            ));
        }
        if let Some(states) = &r.states {
            report.push(("sekiro states".into(), format!("{states:?}")));
        }
        report
    }
}

impl Reader for AreaReader<'_> {
    fn read(&mut self) -> Result<(World, Report)> {
        let mut world = World::new();
        let r = read_game_area(
            &self.install,
            &self.area,
            &self.decoder,
            &self.options,
            &mut world,
        )?;
        self.boxes = r
            .prototype_boxes
            .iter()
            .map(|(&e, b)| (e, b.map(|b| b.map(|c| c.map(f64::from)))))
            .collect();
        Ok((world, self.record(&r)))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        let mut part = world.subset(&world.closure(roots.iter().copied()));
        let (textures, programs) = scope_sources(&mut part)?;
        Ok((part, vec![scoped(textures, programs)]))
    }
    /// The whole area is its own closure, so the loaded world is the read world itself with its source files scoped:
    /// no second copy of every model, collision body and material beside the one the read made (the copy was the
    /// load's peak, about as large as the read world, and shared only the texture and program payloads).
    fn load_all(&mut self, mut world: World) -> Result<(World, Report)> {
        let (textures, programs) = scope_sources(&mut world)?;
        Ok((world, vec![scoped(textures, programs)]))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "fromsoft"]
    }
    fn notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.pending)
    }
    /// The box the read took for it (a model whose box can't be computed refuses the read).
    fn prototype_box(&self, _world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        self.boxes.get(&prototype).copied().flatten()
    }
}

/// The load's report line.
fn scoped(textures: usize, programs: usize) -> (String, String) {
    (
        "sekiro part".into(),
        format!("{textures} texture files and {programs} layered-shader programs its materials bind"),
    )
}

/// Keep in `part` only the source files its materials bind: their bindings' texture files, the default textures the
/// engine binds to the slots they bind nothing to ([`super::shaders::unbound_defaults`]), and their layered
/// definitions' programs. Returns how many of each it keeps.
pub(super) fn scope_sources(part: &mut World) -> Result<(usize, usize)> {
    use crate::sekiro::source::{MaterialShader, ShaderBundle};
    let mut textures = BTreeSet::new();
    let mut programs = BTreeSet::new();
    for (e, m) in part.iter::<Material>() {
        textures.extend(m.bindings.values().map(|b| b.texture.clone()));
        if let Some(bundle) = part
            .get::<MaterialShader>(e)
            .and_then(|s| part.get::<ShaderBundle>(s.0))
            && let Some(meta) = &bundle.parameters
        {
            textures.extend(super::shaders::unbound_defaults(bundle.game, m, meta));
        }
        if let Some(l) = &m.layered {
            textures.extend(l.bindings.values().map(|b| b.texture.clone()));
            programs.insert(l.program_name()?);
        }
    }
    let mut kept = (0, 0);
    if let Some(p) = part.resource_mut::<TexturePayloads>() {
        p.0.retain(|name, _| textures.contains(name));
        kept.0 = p.0.len();
    }
    if let Some(p) = part.resource_mut::<ShaderPrograms>() {
        p.0.retain(|name, _| programs.contains(name));
        kept.1 = p.0.len();
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::convert::NoticeKind;

    #[test]
    fn a_read_missing_a_model_gives_the_conversion_its_notices_once() {
        let mut reader = AreaReader::new(
            "install",
            "m11_01",
            Decoder {
                oodle: None,
                max_output: 0,
            },
            Options::default(),
        );
        let read = super::super::area::Report {
            missing_models: vec!["obj/o000001".into()],
            skipped_missing: 2,
            ..Default::default()
        };
        reader.record(&read);
        let notices = reader.notices();
        let model = notices
            .iter()
            .find(|n| n.reason == "placed models not found in the archive")
            .expect("the missing model's notice");
        assert_eq!((model.kind, model.count), (NoticeKind::Unsupported, 1));
        assert_eq!(model.examples, ["obj/o000001"]);
        assert!(
            notices
                .iter()
                .any(|n| n.reason == "placements of models not found in the archive" && n.count == 2)
        );
        assert_eq!(notices, read.notices());
        // taken once: the conversion asks again after each load, and a load notices nothing
        assert!(reader.notices().is_empty());
    }

    /// A part loads the texture files its own materials bind, the very buffers the read holds, and no other part's.
    #[test]
    fn a_part_loads_only_the_source_files_its_materials_bind() {
        use crate::materials::Binding;
        use std::sync::Arc;
        let mut reader = AreaReader::new(
            "install",
            "m11_01",
            Decoder {
                oodle: None,
                max_output: 0,
            },
            Options::default(),
        );
        let mut world = World::new();
        let mut material = |texture: &str| {
            let mut m = crate::materials::resolve(
                &crate::flver::Material {
                    name: "fixture".into(),
                    mtd: "fixture.mtd".into(),
                    gx: vec![],
                    textures: vec![],
                },
                None,
                |_| true,
            );
            m.bindings.insert(
                "Texture2D_8_AlbedoMap".into(),
                Binding {
                    texture: texture.into(),
                    scale: [1.; 2],
                    uv_number: 1,
                    address: [None; 2],
                    available: true,
                },
            );
            let e = world.spawn();
            world.insert(e, m);
            e
        };
        let a = material("a_a");
        let _b = material("b_a");
        let file = |byte: u8| -> Arc<[u8]> { Arc::from(vec![byte; 4]) };
        world.insert_resource(TexturePayloads(
            [("a_a", 1), ("b_a", 2), ("unbound_a", 3)]
                .into_iter()
                .map(|(n, b)| (n.to_owned(), file(b)))
                .collect(),
        ));
        world.insert_resource(ShaderPrograms(
            [("M[Multiple]_0_Gbuf.ppo".to_owned(), file(4))]
                .into_iter()
                .collect(),
        ));
        let (part, report) = reader.load(&world, &BTreeSet::from([a])).unwrap();
        let payloads = &part.resource::<TexturePayloads>().unwrap().0;
        assert_eq!(payloads.keys().collect::<Vec<_>>(), ["a_a"]);
        // the read's buffer, not a copy
        assert!(Arc::ptr_eq(
            &payloads["a_a"],
            &world.resource::<TexturePayloads>().unwrap().0["a_a"]
        ));
        // no layered material: no program
        assert!(part.resource::<ShaderPrograms>().unwrap().0.is_empty());
        assert_eq!(
            report,
            [(
                "sekiro part".to_string(),
                "1 texture files and 0 layered-shader programs its materials bind".to_string()
            )]
        );
    }
}
