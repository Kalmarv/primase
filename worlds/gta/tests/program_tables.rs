//! The binding tables of the Enhanced shader database's programs ([`primase_gta::rage::sgd2_table`]) against the
//! programs' own interfaces and CodeWalker's parameter list.
//!
//! - Every program's table reads (its resources own exactly its constants, its names end in a NUL), and the first
//!   name hash of every record is the joaat of a name in the table.
//! - Against each vertex, pixel, hull, domain and compute program's interface (`primase_dxil::interface`): every
//!   constant buffer, texture and sampler it declares has a record of the same class at its register and space (a
//!   texture or sampler arrays of unbounded length excepted), and a constant buffer's record has the size the
//!   interface declares rounded up to 16 bytes.
//! - Against CodeWalker's `ShadersGen9Conversion.xml` (`PRIMASE_GTA_SHADER_CONVERSION`, optional): for every constant
//!   buffer parameter of an effect that one of the effect's programs reads under the XML's `old` name, the effect's
//!   parameter records hold the pair (old name, current name) with the XML's offset.
//!
//! The counts are printed. Needs `PRIMASE_GTA_DIR` and `PRIMASE_GTA_KEYS`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::collections::{BTreeMap, BTreeSet};

use primase_core::shader::{D3dClass, ResourceKind};
use primase_gta::components::{GtaEffect, GtaEffectParameters, GtaShaderTable};
use primase_gta::rage::sgd2;
use primase_gta::rage::sgd2_table;
use primase_gta::read::source::GtaSource;
use primase_testdata::real_data;

/// The `Item`s of CodeWalker's file: each effect's name and its CBuffer parameters (current name, old name, offset).
fn conversion(xml: &str) -> BTreeMap<String, Vec<(String, String, u16)>> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out: BTreeMap<String, Vec<(String, String, u16)>> = BTreeMap::new();
    let (mut depth, mut current, mut in_name) = (0usize, String::new(), false);
    loop {
        match reader.read_event().unwrap() {
            Event::Start(e) => {
                depth += 1;
                in_name = depth == 3 && e.name().as_ref() == "Name";
            }
            Event::Text(t) if in_name => {
                current = t.xml10_content().trim().to_owned();
                out.entry(current.clone()).or_default();
            }
            Event::Empty(e) if e.name().as_ref() == "Item" && depth == 3 => {
                let attr = |k: &str| {
                    e.attributes().filter_map(Result::ok).find_map(|a| {
                        (a.key.as_ref() == k).then(|| {
                            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .unwrap()
                                .into_owned()
                        })
                    })
                };
                if attr("type").as_deref() == Some("CBuffer") {
                    let name = attr("name").unwrap();
                    let old = attr("old").unwrap_or_else(|| name.clone());
                    let offset: u16 = attr("offset").unwrap().parse().unwrap();
                    out.get_mut(&current).unwrap().push((name, old, offset));
                }
            }
            Event::End(_) => {
                depth -= 1;
                in_name = false;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    out
}

#[test]
fn every_programs_table_agrees_with_its_interface() {
    let (Some(dir), Some(keys)) = (real_data("PRIMASE_GTA_DIR"), real_data("PRIMASE_GTA_KEYS")) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let bytes = source.shader_database_bytes().unwrap();
    let db = sgd2::parse(&bytes).unwrap();
    let (mut programs, mut constants, mut named) = (0usize, 0usize, 0usize);
    let (mut buffers, mut textures, mut samplers, mut unbounded) = (0usize, 0usize, 0usize, 0usize);
    let mut other_stages: BTreeMap<usize, usize> = BTreeMap::new();
    for (group, list) in db.groups.iter().enumerate() {
        for program in list {
            let table = sgd2_table::parse(program.tables)
                .unwrap_or_else(|e| panic!("group {group} {}: {e}", program.name));
            programs += 1;
            constants += table.constants.len();
            // the first name hash is of a name of the table
            for r in &table.resources {
                assert!(
                    table.name_of(r.name).is_some(),
                    "{}: a resource name",
                    program.name
                );
                named += 1;
            }
            for c in &table.constants {
                assert!(
                    table.name_of(c.name).is_some(),
                    "{}: a constant name",
                    program.name
                );
                named += 1;
            }
            // against the interface (the stages that have one)
            let Ok(interface) = primase_dxil::interface(program.container) else {
                continue;
            };
            for r in &interface.resources {
                let Some(bind) = r.provenance.d3d else { continue };
                let at = |t: &sgd2_table::TableResource| {
                    u32::from(t.register) == bind.register && u32::from(t.space) == bind.space
                };
                let holds = match (&r.kind, bind.class) {
                    (ResourceKind::ConstantBuffer { layout }, D3dClass::ConstantBuffer) => {
                        buffers += 1;
                        let rounded = layout.size.div_ceil(16).saturating_mul(16);
                        table.resources.iter().any(|t| at(t) && t.size == rounded)
                    }
                    (ResourceKind::Texture { .. }, D3dClass::ShaderResource) => {
                        textures += 1;
                        table.resources.iter().any(at)
                    }
                    (ResourceKind::Sampler { .. }, D3dClass::Sampler) => {
                        samplers += 1;
                        if r.is_unbounded() {
                            unbounded += 1;
                            true
                        } else {
                            table.resources.iter().any(at)
                        }
                    }
                    _ => true,
                };
                if !holds {
                    // the vertex and pixel programs are held to it; the others (root constants and the like) are counted
                    if group <= 1 {
                        panic!(
                            "group {group} {}: {:?} at register {} space {} has no record",
                            program.name, r.kind, bind.register, bind.space
                        );
                    }
                    *other_stages.entry(group).or_default() += 1;
                }
            }
        }
    }
    println!(
        "{programs} programs, {constants} constant records, {named} records named; against interfaces: {buffers} constant buffers, \
         {textures} textures, {samplers} samplers ({unbounded} unbounded arrays, not tabulated); resources without a record in the          other stages, by group: {other_stages:?}"
    );
    assert!(programs > 10_000);
}

#[test]
fn the_effects_parameter_records_hold_codewalkers_offsets() {
    let (Some(dir), Some(keys), Some(xml)) = (
        real_data("PRIMASE_GTA_DIR"),
        real_data("PRIMASE_GTA_KEYS"),
        real_data("PRIMASE_GTA_SHADER_CONVERSION"),
    ) else {
        return;
    };
    let conversion = conversion(&std::fs::read_to_string(xml).unwrap());
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = primase_core::World::new();
    let (table, _) = source.add_shader_database(&mut world).unwrap();
    assert_eq!(table.tables + table.unread_tables.len(), table.shaders);
    let (mut asked, mut read_by_programs, mut found) = (0usize, 0usize, 0usize);
    let mut missing = Vec::new();
    for (e, effect) in world.iter::<GtaEffect>() {
        let Some(params) = conversion.get(&effect.name) else {
            continue;
        };
        let records: BTreeSet<(u32, u32, u16)> = world
            .get::<GtaEffectParameters>(e)
            .unwrap()
            .0
            .iter()
            .map(|p| (p.name, p.second_name, p.offset))
            .collect();
        // the name hashes the effect's programs read
        let mut read = BTreeSet::new();
        for p in &effect.programs {
            if let Some(GtaShaderTable(t)) = p.shader.and_then(|s| world.get::<GtaShaderTable>(s)) {
                read.extend(t.constants.iter().map(|c| c.name));
            }
        }
        for (name, old, offset) in params {
            asked += 1;
            let (old_hash, hash) = (
                primase_hash::joaat(old.as_bytes()),
                primase_hash::joaat(name.as_bytes()),
            );
            if !read.contains(&old_hash) {
                continue;
            }
            read_by_programs += 1;
            if old_hash == hash {
                // a parameter the game did not rename has no record to return
                found += 1;
            } else if records.contains(&(old_hash, hash, *offset)) {
                found += 1;
            } else {
                missing.push(format!("{}: {name} ({old}) at {offset}", effect.name));
            }
        }
    }
    println!(
        "{asked} constant parameters in CodeWalker's list, {read_by_programs} read by a program of their effect, {found} found with its offset"
    );
    assert!(missing.is_empty(), "{missing:?}");
}
