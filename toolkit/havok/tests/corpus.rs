//! Container coverage gate. The independent decoder supplies loose tagfiles
//! and their compendium; no game data is included in the repository.
use primase_havok::{Compendium, Tagfile};
#[test]
fn tagfile_containers() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("PRIMASE_HAVOK_CORPUS") else {
        eprintln!("SKIPPED: PRIMASE_HAVOK_CORPUS not set");
        return Ok(());
    };
    let mut files = std::fs::read_dir(dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    let comps = files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "compendium"))
        .map(|p| Compendium::read(&std::fs::read(p)?).map_err(Box::<dyn std::error::Error>::from))
        .collect::<Result<Vec<_>, _>>()?;
    let mut triangles = 0;
    let mut shapes = std::collections::BTreeMap::<String, usize>::new();
    let mut count = 0;
    let mut items = 0;
    let mut patches = 0;
    for p in files.iter().filter(|p| p.extension().is_some_and(|e| e == "hkx")) {
        let bytes = std::fs::read(p)?;
        let mut result = Tagfile::read(&bytes, None);
        for c in &comps {
            if result.is_ok() {
                break;
            }
            result = Tagfile::read(&bytes, Some(c));
        }
        let tag = result.map_err(|e| format!("{}: {e}", p.display()))?;
        let root = tag.root()?;
        let variants = root.field("namedVariants")?.array()?;
        for i in 0..variants.len() {
            let physics = variants.get(i)?.field("variant")?.dereference()?;
            let systems = physics.field("systemDatas")?.array()?;
            for j in 0..systems.len() {
                let bodies = systems.get(j)?.dereference()?.field("bodyCinfos")?.array()?;
                for k in 0..bodies.len() {
                    let body = bodies.get(k)?;
                    body.field("position")?.vector::<4>()?;
                    body.field("orientation")?.vector::<4>()?;
                    let shape = body.field("shape")?.dereference()?;
                    *shapes.entry(shape.type_name().into()).or_default() += 1;
                    triangles += primase_havok::compressed_mesh(shape.field("data")?.dereference()?)?
                        .triangles
                        .len();
                }
            }
        }
        count += 1;
        items += tag.items.len() - 1;
        patches += tag.patches.iter().map(|(_, p)| p.len()).sum::<usize>();
    }
    eprintln!("triangles {triangles}, shapes {shapes:?}");
    assert!(count > 0);
    eprintln!(
        "tagfiles {count}, items {items}, patches {patches}, compendia {}",
        comps.len()
    );
    Ok(())
}
