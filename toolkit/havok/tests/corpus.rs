//! Container coverage gate. The independent decoder supplies loose tagfiles
//! and their compendium; no game data is included in the repository.
mod inputs;

use inputs::{Game, OracleSet, Query, expect_found, resolve};
use primase_havok::{Compendium, Tagfile};
#[test]
fn tagfile_containers() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok");
    let Some(dir) = resolve(corpus) else {
        return Ok(());
    };
    let mut files = std::fs::read_dir(dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    let tagfiles: Vec<&std::path::PathBuf> = files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "hkx"))
        .collect();
    expect_found(corpus, ".hkx files", tagfiles.len());
    let comps = files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "compendium"))
        .map(|p| Compendium::read(&std::fs::read(p)?).map_err(Box::<dyn std::error::Error>::from))
        .collect::<Result<Vec<_>, _>>()?;
    expect_found(corpus, ".compendium files", comps.len());
    let mut triangles = 0;
    let mut shapes = std::collections::BTreeMap::<String, usize>::new();
    let mut count = 0;
    let mut bodies = 0;
    let mut through_compendium = 0;
    let mut items = 0;
    let mut patches = 0;
    for p in tagfiles {
        let bytes = std::fs::read(p)?;
        let mut result = Tagfile::read(&bytes, None);
        let mut needed_compendium = false;
        for c in &comps {
            if result.is_ok() {
                break;
            }
            result = Tagfile::read(&bytes, Some(c));
            needed_compendium = true;
        }
        let tag = result.map_err(|e| format!("{}: {e}", p.display()))?;
        through_compendium += usize::from(needed_compendium);
        let root = tag.root()?;
        let variants = root.field("namedVariants")?.array()?;
        for i in 0..variants.len() {
            let physics = variants.get(i)?.field("variant")?.dereference()?;
            let systems = physics.field("systemDatas")?.array()?;
            for j in 0..systems.len() {
                let bodies_array = systems.get(j)?.dereference()?.field("bodyCinfos")?.array()?;
                for k in 0..bodies_array.len() {
                    let body = bodies_array.get(k)?;
                    body.field("position")?.vector::<4>()?;
                    body.field("orientation")?.vector::<4>()?;
                    let shape = body.field("shape")?.dereference()?;
                    *shapes.entry(shape.type_name().into()).or_default() += 1;
                    triangles += primase_havok::compressed_mesh(shape.field("data")?.dereference()?)?
                        .triangles
                        .len();
                    bodies += 1;
                }
            }
        }
        count += 1;
        items += tag.items.len() - 1;
        patches += tag.patches.iter().map(|(_, p)| p.len()).sum::<usize>();
    }
    eprintln!("triangles {triangles}, bodies {bodies}, shapes {shapes:?}");
    assert!(bodies > 0, "no collision bodies in the corpus");
    assert!(triangles > 0, "no decoded triangles in the corpus");
    assert!(
        through_compendium > 0,
        "no tagfile needed the compendium: external type data was not exercised"
    );
    assert!(
        shapes
            .keys()
            .any(|k| k.as_str() == "fsnpCustomParamCompressedMeshShape"
                || k.as_str() == "hknpCompressedMeshShape"),
        "no compressed mesh shape in {shapes:?}"
    );
    eprintln!(
        "tagfiles {count}, bodies {bodies}, items {items}, patches {patches}, compendia {}, through a compendium {through_compendium}",
        comps.len()
    );
    Ok(())
}
