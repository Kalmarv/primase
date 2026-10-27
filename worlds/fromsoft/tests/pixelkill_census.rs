//! Metadata census for the permutation decision, not a claim about the engine's draw calls.
//! Reads material definitions and bundle inventories only: no images, spline decode or shader lift.
use fromsoft_formats::{Binder, Game, Install, Keys, dcx::Decoder, metaparam::Metaparam};
use primase_fromsoft::materials::{Definition, MaterialView, ParameterValue};
use primase_testdata::{Game as Data, Tool, install, keys, oodle};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
mod common;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn leaf(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}
fn stem(name: &str) -> String {
    let name = leaf(name);
    name.rsplit_once('.')
        .map_or(name, |(stem, _)| stem)
        .to_ascii_lowercase()
}
fn override_key(name: &str) -> u32 {
    primase_foundations::hash::crc32(&name.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>())
}
fn pixelkill(key: u32) -> bool {
    key == override_key("C[PixelKill]_Int") || key == override_key("C[PixelKill][ID1]_Int")
}

struct Bundle {
    metadata: Metaparam,
    programs: BTreeSet<String>,
}

fn census(game: Game, install: Install, decoder: &Decoder<'_>) -> Result<()> {
    let archive = install.open()?;
    let mut bundles = BTreeMap::new();
    let mut refused = BTreeMap::new();
    let mut duplicate_bundles = 0usize;
    for path in game.shader_binders() {
        if !archive.contains(path) {
            continue;
        }
        for file in Binder::read(&archive.read(path, decoder)?, decoder)?.files {
            let name = file.name.as_deref().unwrap_or("");
            if !name.to_ascii_lowercase().ends_with(".shaderbdle") {
                continue;
            }
            let id = stem(name);
            // Same first-encounter order is retained for the definitions below; duplicate inventory
            // is reported rather than counted as another shader or inferred to be another draw.
            if bundles.contains_key(&id) || refused.contains_key(&id) {
                duplicate_bundles += 1;
                continue;
            }
            let inner = Binder::read(&file.bytes, decoder)?;
            let programs = inner
                .files
                .iter()
                .filter_map(|file| file.name.as_deref())
                .map(leaf)
                .filter(|name| name.to_ascii_lowercase().ends_with(".ppo"))
                .map(str::to_ascii_lowercase)
                .collect();
            let Some(meta) = inner.files.iter().find(|file| {
                file.name
                    .as_deref()
                    .is_some_and(|name| name.to_ascii_lowercase().ends_with(".metaparam"))
            }) else {
                refused.insert(id, "no metaparam".to_owned());
                continue;
            };
            match Metaparam::read(&meta.bytes) {
                Ok(metadata) => {
                    bundles.insert(id, Bundle { metadata, programs });
                }
                Err(error) => {
                    refused.insert(id, error.to_string());
                }
            }
        }
    }
    let mut definitions = BTreeMap::new();
    let paths: &[&str] = if game == Game::Sekiro {
        &["/mtd/allmaterialbnd.mtdbnd.dcx"]
    } else {
        game.material_binders()
    };
    for path in paths {
        if !archive.contains(path) {
            continue;
        }
        for file in Binder::read(&archive.read(path, decoder)?, decoder)?.files {
            let name = file.name.as_deref().unwrap_or("");
            let extension = if game == Game::Sekiro { ".mtd" } else { ".matbin" };
            if !name.to_ascii_lowercase().ends_with(extension) {
                continue;
            }
            let id = stem(name);
            if definitions.contains_key(&id) {
                continue;
            }
            let bytes = decoder.decode_if_dcx(&file.bytes)?;
            let definition = if game == Game::Sekiro {
                Definition::Mtd(fromsoft_formats::mtd::MaterialDefinition::read(&bytes)?)
            } else {
                Definition::Matbin(fromsoft_formats::matbin::Material::read(&bytes)?)
            };
            definitions.insert(id, definition);
        }
    }
    let bindings = BTreeMap::new();
    let mut counts = BTreeMap::<String, usize>::new();
    let mut examples = BTreeMap::<String, Vec<Value>>::new();
    let mut missing = BTreeMap::new();
    let mut kill_materials = 0usize;
    let mut kill_bundles = 0usize;
    for (id, definition) in &definitions {
        let view = MaterialView {
            mtd: id,
            definition: definition.as_ref(),
            bindings: &bindings,
        };
        let shader = view.definition.shader_name().to_ascii_lowercase();
        let Some(bundle) = bundles.get(&shader) else {
            missing.insert(id.clone(), json!({"shader":shader,"reason":refused.get(&shader)}));
            continue;
        };
        let plain = view.gbuffer_program().replace("_[A]", "").to_ascii_lowercase();
        let alpha = plain.strip_suffix(".ppo").ok_or("program extension")?.to_owned() + "_[a].ppo";
        let kill: Vec<_> = bundle
            .metadata
            .mtd_params
            .iter()
            .filter(|p| pixelkill(p.key))
            .collect();
        let any_override = bundle.metadata.mtd_params.iter().any(|p| p.key != 0)
            || bundle.metadata.ca_params.iter().any(|p| p.key != 0);
        let category = format!(
            "pixelkill={} any_override={} alpha_ref={} plain={} alpha={}",
            !kill.is_empty(),
            any_override,
            view.alpha_reference() > 0,
            bundle.programs.contains(&plain),
            bundle.programs.contains(&alpha)
        );
        *counts.entry(category.clone()).or_default() += 1;
        let parameter_names: Vec<_> = match definition {
            Definition::Mtd(mtd) => mtd.parameters.iter().map(|p| p.name.as_str()).collect(),
            Definition::Matbin(matbin) => matbin.params.iter().map(|p| p.name.as_str()).collect(),
        };
        let switches: BTreeMap<_, _> = parameter_names
            .iter()
            .filter(|name| name.starts_with("GXFT_") || name.starts_with("g_GX_"))
            .map(|name| {
                (
                    (*name).to_owned(),
                    format!("{:?}", view.definition.parameter(name)),
                )
            })
            .collect();
        let effective: Vec<_> = kill.iter().map(|p| json!({"parameter":p,"material_value":format!("{:?}",view.definition.parameter(&p.name))})).collect();
        let row = json!({"definition":id,"shader":shader,"alpha_reference":view.alpha_reference(),
            "plain":plain,"has_plain":bundle.programs.contains(&plain),"alpha":alpha,"has_alpha":bundle.programs.contains(&alpha),
            "flags":bundle.metadata.flags,"gxmd_key":bundle.metadata.gxmd_key,"kill":effective,"switches":switches});
        if !kill.is_empty() {
            kill_materials += 1;
            println!("PIXELKILL_MATERIAL={}", json!({"game":game.name(),"row":row}));
        } else if examples.entry(category.clone()).or_default().len() < 8 {
            examples.entry(category).or_default().push(row);
        }
        // Feature cross-tab includes negative controls, so a ubiquitous flag is not mistaken for a kill switch.
        *counts
            .entry(format!(
                "flags={} pixelkill={} alpha_ref={}",
                bundle.metadata.flags,
                !kill.is_empty(),
                view.alpha_reference() > 0
            ))
            .or_default() += 1;
        // Values here describe stored alpha references, not an inferred need for a particular program.
        if let Some(ParameterValue::Int(values)) = view.definition.parameter("g_AlphaRef") {
            *counts
                .entry(format!("alpha_ref_first={:?}", values.first()))
                .or_default() += 1;
        }
    }
    for (shader, bundle) in &bundles {
        let kill: Vec<_> = bundle
            .metadata
            .mtd_params
            .iter()
            .filter(|p| pixelkill(p.key))
            .collect();
        if !kill.is_empty() {
            kill_bundles += 1;
            println!(
                "PIXELKILL_BUNDLE={}",
                json!({"game":game.name(),"shader":shader,"flags":bundle.metadata.flags,
                "kill":kill,"programs":bundle.programs})
            );
        }
    }
    println!(
        "PIXELKILL_CENSUS={}",
        json!({"game":game.name(),"definitions":definitions.len(),
        "bundles":bundles.len(),"pixelkill_materials":kill_materials,"pixelkill_bundles":kill_bundles,
        "duplicate_bundles":duplicate_bundles,"refused":refused,"unmatched_definitions":missing,
        "counts":counts,"negative_examples":examples,
        "scope":"one row per first encountered material definition, exact shader/static-switch Gbuf pair; program existence is not proof of runtime selection or necessity"})
    );
    assert!(
        !definitions.is_empty(),
        "no material definitions: census did not observe its denominator"
    );
    assert!(
        !bundles.is_empty(),
        "no readable bundles: census did not observe its denominator"
    );
    Ok(())
}

#[test]
fn pixelkill_permutation_inventory_across_materials() -> Result<()> {
    let (Some(root), Some(key_dir), Some(dll)) =
        (install(Data::Elden), keys(Data::Elden), oodle(Tool::Oodle6))
    else {
        println!("PIXELKILL_SKIP=PRIMASE_DATA Elden install/keys/oodle6 not configured");
        return Ok(());
    };
    let codec = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    census(
        Game::EldenRing,
        Install::new(
            root,
            Game::EldenRing,
            Keys::from_folder(key_dir, Game::EldenRing)?,
        ),
        &decoder,
    )?;
    if let Some(root) = install(Data::Sekiro)
        && let Some(sekiro) = common::sekiro(root)?
    {
        census(Game::Sekiro, sekiro, &decoder)?;
    } else {
        println!("PIXELKILL_SKIP=Sekiro control not configured");
    }
    Ok(())
}
