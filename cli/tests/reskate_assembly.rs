//! The writer's assembly of a prepared GTA Legion tile assigns the same material ids, names and texture ids whether
//! its textures are encoded (`write_map`) or only identified (`prepare-reskate-overlays`, which stacks over the writer's
//! meshes without encoding a texture): the conversion `primase convert --from gta --region legion --to reskate` runs,
//! up to the writer, with the ReSkate passes its documented recipe gives.

use std::sync::{Arc, Mutex};

use primase_core::World;
use primase_core::convert::{Finished, Report, Writer, convert};
use primase_core::pass::{PassManager, PassRegistry, Target};
use primase_gta::convert::{GtaOptions, GtaReader};
use primase_reskate::{AssembledMaterial, EncodingOptions, assembled_materials};
use primase_testdata::real_data;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The cli's registry (`convert::registry`).
fn registry() -> PassRegistry {
    let mut r = PassRegistry::new();
    r.register("select-lod", primase_core::select_lod);
    r.register("drop-optional", primase_core::drop_optional);
    r.register("place-spawn", primase_core::place_spawn);
    r.register("resolve-state", primase_core::state::resolve_state);
    primase_image::register(&mut r);
    primase_meshops::register(&mut r);
    primase_gta::register(&mut r);
    primase_reskate::register(&mut r);
    r
}

/// A part's materials assembled with its textures encoded, then only identified.
type Pair = (Vec<AssembledMaterial>, Vec<AssembledMaterial>);

/// Takes the prepared part and assembles it both ways.
struct Both {
    got: Arc<Mutex<Vec<Pair>>>,
}

impl Writer for Both {
    fn target(&self) -> Target {
        primase_reskate::target()
    }
    fn write(&mut self, part: &World) -> primase_core::convert::Result<()> {
        let e = EncodingOptions::default();
        let encoded = assembled_materials(part, &e, true)?;
        let identified = assembled_materials(part, &e, false)?;
        self.got.lock().expect("one writer").push((encoded, identified));
        Ok(())
    }
    fn finish(self: Box<Self>) -> primase_core::convert::Result<Finished> {
        Ok(Report::new().into())
    }
}

#[test]
fn legion_materials_are_the_same_encoded_or_identified() -> Result<()> {
    let (Some(install), Some(keys)) = (real_data("PRIMASE_GTA_DIR"), real_data("PRIMASE_GTA_KEYS")) else {
        return Ok(());
    };
    let mut reader = GtaReader::open(&GtaOptions {
        install,
        keys,
        region: [95.0, -1034.0, 295.0, -834.0],
        reach: 600.0,
    })?;
    let registry = registry();
    let rect = "x0=95,y0=-1034,x1=295,y1=-834";
    let select = registry.parse(&format!(
        "resolve-state{{assign=gta.map:online;gta.hour:12}}, gta-select-region{{{rect},min_size=0.5,max_priority=3}}, \
         lower-gta-lights{{state=false,{rect}}}, gta-select-collision{{detail=plain}}, \
         lower-gta-collision{{{rect},foliage=leave-out,river=leave-out}}, lower-gta-surfaces, \
         gta-select-water{{{rect},clip=kept}}, place-spawn{{at=centre}}"
    ))?;
    let passes = registry.parse(
        "select-lod{form=1}, lower-gta-embedded-collision{foliage=leave-out,river=leave-out}, lower-gta-surfaces, \
         gta-pose-fragments, lower-gta-pbr{night=false}, gta-winding, gta-tint, gta-terrain-layers, \
         gta-vertex-colours, lower-gta-water, normalize-normals, bake-normal-scale, \
         raise-skate{name=legion,light_scale=0.0003007,unclassified=default}, lower-layered-reskate{overlays=all}, \
         lower-tint-reskate, lower-vertex-alpha-reskate, cap-textures{max=256,normals=box}, prepare-reskate-textures",
    )?;
    let got = Arc::new(Mutex::new(Vec::new()));
    convert(
        &mut reader,
        Box::new(Both { got: got.clone() }),
        &PassManager::new(registry),
        &select,
        &passes,
    )?;
    let got = got.lock().expect("done");
    assert_eq!(got.len(), 1, "one part");
    let (encoded, identified) = &got[0];
    let textured = encoded
        .iter()
        .filter(|m| m.textures.iter().any(|&t| t >= 0))
        .count();
    println!("legion: {} materials, {textured} with textures", encoded.len());
    assert!(
        encoded.len() > 100 && textured > 100,
        "{} materials, {textured} textured",
        encoded.len()
    );
    assert_eq!(encoded, identified);
    Ok(())
}
