//! Every HoudiniDecalInstancerComponent of the base USNAT forge (Riders Republic build 25394987) read whole
//! (read::ddecal), against the notes' census (docs/entity-grammar.md §9.2, §9.3): 5,490 components, 20,209 model
//! references, 1,355,338 records, every item naming a model it lists (34,477 items over the base forge and its patch),
//! every record's window inside the texture (offset + width <= 1); every DDecalModel read.
mod inputs;

use anvil_formats::archive::{Archive, At};
use anvil_formats::ddecal::{DDECAL_MODEL, HOUDINI_DECAL_INSTANCER, StoredDecalModel, StoredDeferredDecals};
use anvil_formats::entity::{ComponentBody, ENTITY, StoredEntity};
use inputs::{Game, install};

#[test]
fn every_deferred_decal_of_the_base_forge_reads() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut archive = Archive::open(&[dir.join("DataPC_MAD_USNAT.forge")]).expect("archive");
    let (base, patch) = (tally(&mut archive), {
        let mut patch = Archive::open(&[dir.join("DataPC_MAD_USNAT_patch_01.forge")]).expect("archive");
        tally(&mut patch)
    });
    println!("base {base:?}, patch {patch:?}");
    let (components, references, items, records, windows_inside, models) = base;
    assert_eq!((components, references, records), (5_490, 20_209, 1_355_338));
    assert_eq!(items + patch.2, 34_477);
    assert_eq!(windows_inside, records);
    assert_eq!(models, 69);
}

/// The forge's (components, references, items, records, records whose window is inside, DDecalModels).
fn tally(archive: &mut Archive) -> (usize, usize, usize, usize, usize, usize) {
    let names = archive.forge(0).expect("forge").names.clone();
    let (mut components, mut references, mut items, mut records, mut windows_inside) = (0, 0, 0, 0, 0);
    let mut models = 0;
    for (entry, n) in names.iter().enumerate() {
        match n.type_id {
            ENTITY => {
                let body = archive.resource(At { forge: 0, entry }).expect("entity").body;
                let Ok(e) = StoredEntity::parse(
                    &body,
                    primase_anvil::components::component_types::is_component_type,
                ) else {
                    continue;
                };
                for c in &e.components {
                    let (ComponentBody::Raw(body), HOUDINI_DECAL_INSTANCER) = (&c.body, c.kind) else {
                        continue;
                    };
                    let d = StoredDeferredDecals::parse(body).expect("a decal instancer");
                    components += 1;
                    references += d.models.len();
                    for item in &d.items {
                        assert!(
                            item.model() < d.models.len(),
                            "{} of {}",
                            item.model(),
                            d.models.len()
                        );
                        items += 1;
                        for r in &item.decals {
                            records += 1;
                            let [offset, width] = r.window();
                            windows_inside += usize::from(offset + width <= 1.0);
                        }
                    }
                }
            }
            DDECAL_MODEL => {
                let body = archive.resource(At { forge: 0, entry }).expect("model").body;
                StoredDecalModel::parse(&body).expect("a DDecalModel");
                models += 1;
            }
            _ => {}
        }
    }
    (components, references, items, records, windows_inside, models)
}
