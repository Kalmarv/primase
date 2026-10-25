//! A causal probe, not an assumption that TAE masks hide the flower. No textures or shaders are read.
//! With PRIMASE_DATA configured, prints source FLVER groups, all 233 records, and the authored
//! scale samples of the flower's influencing bones (plus their ancestors). Run with --nocapture.
use std::collections::{BTreeMap, BTreeSet};
use primase_core::{Clip, ClipTarget, Property, Skeleton, World};
use primase_core::convert::Reader;
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_fromsoft::{Binder, Game, Install, Keys, dcx::Decoder, flver::Model, sekiro, tae::{Header, Tae}};
use primase_testdata::{Game as Data, OracleSet, Tool, install, keys, oodle, oracle};
use serde_json::json;
mod common;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// Same source field as character::display_marks: complete #<digits># tokens in the material name.
fn marks(name: &str) -> Vec<u32> {
    let mut rest=name;let mut out=Vec::new();
    while let Some(at)=rest.find('#') {
        rest=&rest[at+1..];
        let n=rest.bytes().take_while(u8::is_ascii_digit).count();
        if n>0 && rest.as_bytes().get(n)==Some(&b'#') {
            if let Ok(group)=rest[..n].parse() {out.push(group);}
            rest=&rest[n+1..];
        }
    }
    out
}

fn leaf(name:&str)->&str {name.rsplit(['/', '\\']).next().unwrap_or(name)}

fn word(bytes:&[u8],at:usize)->Result<u32> {
    Ok(u32::from_le_bytes(bytes.get(at..at+4).ok_or("FLVER probe word out of bounds")?.try_into()?))
}

fn visibility_parameter(name:&str)->bool {
    let name=name.to_ascii_lowercase();
    ["pixelkill","opacity","clipplane","clip_plane","visibility","alpharef"].iter().any(|key|name.contains(key))
}

/// Verify the name-to-metaparam link and execute both installed pixel permutations. Material
/// constants come from the lab's real binding report; textures are explicit controls, not game texels.
#[test]
fn malenia_pixelkill_hash_and_alpha_permutation_discard()->Result<()> {
    use primase_shader_exec::PixelShader;
    for name in ["C[PixelKill]_Int","C[PixelKill][ID1]_Int","g_AlphaRef","C_c2120__Arts_Petal_snp_0_float_2"] {
        let utf16:Vec<_>=name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        println!("MALENIA_PARAMETER_HASH={}",json!({"name":name,"crc32_utf8":primase_hash::crc32(name.as_bytes()),
            "crc32_utf16le":primase_hash::crc32(&utf16),"petal_override_key":4002817437u32}));
    }
    // the lab's binding report of the c2120 read (our own output: a regression baseline), declared in the registry
    let (Some(root),Some(key_dir),Some(dll),Some(report_path))=(install(Data::Elden),keys(Data::Elden),oodle(Tool::Oodle6),
        oracle(OracleSet::Baseline,Data::Elden,"malenia-binding-report.json"))
        else {println!("MALENIA_SKIP=PRIMASE_DATA Elden install/keys/oodle6/binding report not configured");return Ok(());};
    let codec=common::oodle(dll)?;let decoder=Decoder {oodle:Some(&codec),max_output:1<<30};
    let install=Install::new(root,Game::EldenRing,Keys::from_folder(key_dir,Game::EldenRing)?);
    let archive=install.open()?;
    let report:serde_json::Value=serde_json::from_str(&std::fs::read_to_string(&report_path)?)?;
    let binding=report["views"].as_array().into_iter().flatten()
        .flat_map(|view|view["game_shader"]["bound"].as_array().into_iter().flatten())
        .find(|binding|binding["program"]=="C[c2120]_Arts_Petal_Gbuf.ppo")
        .ok_or("the stated binding report has no petal Gbuf program")?;
    let constants=binding["constants"].as_array().ok_or("binding constants missing")?;
    let mut programs=0;
    for path in Game::EldenRing.shader_binders() {
        if !archive.contains(path) {continue;}
        for file in Binder::read(&archive.read(path,&decoder)?,&decoder)?.files {
            if !leaf(file.name.as_deref().unwrap_or("")).eq_ignore_ascii_case("C[c2120]_Arts_Petal.shaderbdle") {continue;}
            for member in Binder::read(&file.bytes,&decoder)?.files {
                let name=leaf(member.name.as_deref().unwrap_or(""));
                if !["C[c2120]_Arts_Petal_Gbuf.ppo","C[c2120]_Arts_Petal_Gbuf_[A].ppo"].iter().any(|wanted|name.eq_ignore_ascii_case(wanted)) {continue;}
                // The binder member is already the program container. ER's container carries DXIL,
                // not the SHEX/SHDR code chunks the DXBC lifter requires (Game::shader_format).
                let lifted=match Game::EldenRing.shader_format() {
                    primase_core::shader::ShaderFormat::Dxbc=>primase_dxbc::lift(&member.bytes)?,
                    _=>primase_dxil::lift(&member.bytes)?,
                };
                let shader=primase_shader_exec::ProgramShader::new(lifted)?;
                let mut buffers:Vec<_>=shader.constant_buffers().iter().map(|&(slot,rows)|(slot,vec![[0u32;4];rows as usize])).collect();
                for constant in constants {
                    let slot=constant["buffer"].as_u64().ok_or("binding has no buffer")? as u32;
                    let first=constant["register"].as_u64().ok_or("binding has no register")? as usize*4
                        +constant["lane"].as_u64().unwrap_or(0) as usize;
                    let values=if let Some(values)=constant["values"].as_array() {
                        values.iter().map(|value|value.as_f64().map(|v|v as f32).ok_or("non-numeric binding value"))
                            .collect::<std::result::Result<Vec<_>,_>>()?
                    } else {match constant["role"].as_str().unwrap_or("") {
                        "Object(InstanceColour)"=>vec![1.;4],
                        "Object(World)"=>vec![1.,0.,0.,0.,0.,1.,0.,0.,0.,0.,1.,0.,0.,0.,0.,1.],
                        "Frame(Exposure)"=>vec![1.],
                        _=>Vec::new(), // capture's unset/unknown lanes and disabled frame effects remain zero
                    }};
                    if let Some((_,rows))=buffers.iter_mut().find(|(buffer,_)|*buffer==slot) {
                        let count=(rows.len()*4).saturating_sub(first);
                        for (i,value) in values.iter().take(count).enumerate() {
                            rows[(first+i)/4][(first+i)%4]=value.to_bits();
                        }
                    }
                }
                let inputs=vec![[0f32.to_bits(),0f32.to_bits(),1f32.to_bits(),0f32.to_bits()];shader.input_count()];
                let dither=shader.interface().resources.iter().find(|resource|resource.provenance.name=="g_DitherTexture")
                    .and_then(|resource|resource.provenance.d3d).map(|point|point.register);
                let material=buffers.iter().position(|(slot,_)|*slot==3).ok_or("petal PS does not read cb3")?;
                // The exact source buffer address from the bundle's metaparam: float_2 starts at word45.
                if buffers[material].1.len()*4<=45 {return Err("petal cb3 is shorter than word45".into());}
                for value in [0f32,0.5,1.,1.5,3.] {
                    buffers[material].1[45/4][45%4]=value.to_bits();
                    let bound:Vec<_>=buffers.iter().map(|(slot,rows)|(*slot,rows.as_slice())).collect();
                    for noise in [0.5f32] {
                        let mut sampler=|request:primase_shader_exec::Sample|Ok(if Some(request.texture)==dither {[noise;4]}
                            else if matches!(request.texture,40|41) {[0.;4]} else {[1.;4]});
                        let result=shader.execute(&inputs,&bound,&mut sampler)?;
                        println!("MALENIA_PIXELKILL_EXECUTION={}",json!({"program":name,"word":45,"value":value,
                            "dither_control":noise,"discarded":result.discarded,
                            "constants_from":report_path,"instance_colour":[1.,1.,1.,1.],"alpha_ref":[-100.,0.],
                            "discard_site":"executor exposes discard only; no branch label inferred",
                            "scope":"only word45 varies: capture material constants, instance colour white, exposure1, model fade off, opaque albedo/emissive, decals0, dither0.5; NORMAL=(0,0,1)"}));
                        if value==3. {assert!(!result.discarded,"the positive control still discards: another input masks PixelKill");}
                    }
                }
                programs+=1;
            }
        }
        if programs>0 {break;}
    }
    assert_eq!(programs,2,"both real alpha and plain permutations must execute for the control to be valid");
    Ok(())
}

/// CA parameters are separate from MATBIN inputs. Also compare rest transforms through parents,
/// since full-size local scales alone do not establish that the skeletons put a mesh in the same place.
#[test]
fn malenia_dynamic_parameter_defaults_and_rest_model_transforms()->Result<()> {
    let (Some(root),Some(key_dir),Some(dll))=(install(Data::Elden),keys(Data::Elden),oodle(Tool::Oodle6))
        else {println!("MALENIA_SKIP=PRIMASE_DATA Elden install/keys/oodle6 not configured");return Ok(());};
    let codec=common::oodle(dll)?;let decoder=Decoder {oodle:Some(&codec),max_output:1<<30};
    let install=Install::new(root,Game::EldenRing,Keys::from_folder(key_dir,Game::EldenRing)?);
    let archive=install.open()?;
    let mut wanted:BTreeSet<String>=["c[c2120]_arts_petal","c[c2120]_clone_bodyglow","c[detailblend]"].into_iter().map(str::to_owned).collect();
    for path in Game::EldenRing.shader_binders() {
        if wanted.is_empty() {break;}
        if !archive.contains(path) {continue;}
        for file in Binder::read(&archive.read(path,&decoder)?,&decoder)?.files {
            let name=leaf(file.name.as_deref().unwrap_or(""));
            let folded=name.to_ascii_lowercase();
            let Some(stem)=folded.strip_suffix(".shaderbdle") else {continue;};
            if !wanted.remove(stem) {continue;}
            let bundle=Binder::read(&file.bytes,&decoder)?;
            println!("MALENIA_SHADER_MEMBERS={}",json!({"shader":stem,"members":bundle.files.iter()
                .map(|file|leaf(file.name.as_deref().unwrap_or(""))).collect::<Vec<_>>()}));
            for member in bundle.files {
                let name=member.name.as_deref().unwrap_or("");
                if !name.to_ascii_lowercase().ends_with(".metaparam") {continue;}
                match primase_fromsoft::metaparam::Metaparam::read(&member.bytes) {
                    Ok(meta)=>println!("MALENIA_DYNAMIC_DEFAULTS={}",json!({"shader":stem,"flags":meta.flags,
                        "ca_block_size":meta.ca_param_block_size,"ca_components":meta.ca_param_components,
                        "ca_params":meta.ca_params,"mtd_params":meta.mtd_params,
                        "gxmd_key":meta.gxmd_key,"render_passes":meta.passes})),
                    Err(error)=>println!("MALENIA_DYNAMIC_REFUSAL={}",json!({"shader":stem,"error":error.to_string()})),
                }
            }
        }
    }
    for missing in wanted {println!("MALENIA_SHADER_MISSING={missing}");}
    let mut reference=World::new();
    sekiro::read_clip_skeleton(&archive,&decoder,"c2120",&mut reference)?.ok_or("no c2120 reference skeleton")?;
    for (file,model) in sekiro::character_skeletons(&archive,&decoder,"c2120")? {
        let model_fk=primase_animation::pose::model_transforms(&model,&primase_animation::pose::locals(&model,None))?;
        for (entity,havok) in reference.iter::<Skeleton>() {
            let havok_fk=primase_animation::pose::model_transforms(havok,&primase_animation::pose::locals(havok,None))?;
            let mut matched=0;let mut different=0;let mut maximum=0f64;
            for joint in model.joints.iter().filter(|joint|joint.name.starts_with("Xtra_Petal")) {
                let Some(source)=havok.joints.iter().find(|source|source.name==joint.name) else {continue;};
                let a=&model_fk[&joint.id];let b=&havok_fk[&source.id];
                let difference=a.iter().flatten().zip(b.iter().flatten()).map(|(a,b)|(a-b).abs()).fold(0f64,f64::max);
                matched+=1;maximum=maximum.max(difference);if difference>0.001 {different+=1;}
                if joint.name.ends_with("_01") || difference>0.1 {
                    println!("MALENIA_PETAL_REST_FK={}",json!({"file":file,"skeleton":entity,"name":joint.name,
                        "model_parent":joint.parent,"havok_parent":source.parent,"model_world":a,"havok_world":b,
                        "max_element_difference":difference,"frame":"both in canonical world frame, including every parent"}));
                }
            }
            println!("MALENIA_PETAL_REST_FK_TOTAL={}",json!({"file":file,"skeleton":entity,"matched":matched,
                "different_over_0_001":different,"maximum_element_difference":maximum}));
        }
    }
    Ok(())
}

/// Stored definitions, not values guessed from shader registers. No textures or animation splines.
#[test]
fn malenia_named_material_defaults_and_actual_npc_resident_slots()->Result<()> {
    let (Some(root),Some(key_dir),Some(dll))=(install(Data::Elden),keys(Data::Elden),oodle(Tool::Oodle6))
        else {println!("MALENIA_SKIP=PRIMASE_DATA Elden install/keys/oodle6 not configured");return Ok(());};
    let codec=common::oodle(dll)?;let decoder=Decoder {oodle:Some(&codec),max_output:1<<30};
    let install=Install::new(root,Game::EldenRing,Keys::from_folder(key_dir,Game::EldenRing)?);
    let archive=install.open()?;
    let npcs=sekiro::read_param(&install,&archive,&decoder,"NpcParam.param")?;
    if npcs.param_type!="NPC_PARAM_ST" || npcs.data_version!=9 || npcs.row_size!=736 {
        return Err("unrecognised ER NPC layout for resident material slots".into());
    }
    // Paramdex ER NpcParam v9: the tail starting at residentMaterialExParamId00 is 96 bytes.
    // Five s32 ids, sleep loot ids (8), foot IK (8), sound+variation (4), super armour (12),
    // five resistance ids (20), tutorial flag (4), step/lock floats (12), final padding/DLC (8).
    // Thus the first resident slot is 736-96=640. Byte 4 is behaviorVariationId, not a slot.
    let mut referenced=BTreeSet::new();
    for row in npcs.rows.iter().filter(|row|row.id/10000==2120) {
        let slots=(0..5).map(|i|word(&row.data,640+4*i).map(|value|value as i32)).collect::<Result<Vec<_>>>()?;
        referenced.extend(slots.iter().copied().filter(|id|*id>=0));
        println!("MALENIA_NPC_RESIDENT={}",json!({"row":row.id,"first_byte":640,"slots":slots,
            "behavior_variation_id":word(&row.data,4)? as i32}));
    }
    let extras=sekiro::read_param(&install,&archive,&decoder,"MaterialExParam.param")?;
    for row in extras.rows.iter().filter(|row|referenced.contains(&row.id)) {
        if row.data.len()!=96 {return Err("unrecognised resident MaterialEx layout".into());}
        let units:Vec<_>=row.data[..64].as_chunks::<2>().0.iter().map(|unit|u16::from_le_bytes(*unit)).take_while(|unit|*unit!=0).collect();
        let values=(0..5).map(|i|word(&row.data,68+4*i).map(f32::from_bits)).collect::<Result<Vec<_>>>()?;
        println!("MALENIA_RESIDENT_MATERIAL_EX={}",json!({"row":row.id,"parameter":String::from_utf16(&units)?,
            "material_id":word(&row.data,64)? as i32,"values":values}));
    }
    let binder=Binder::read(&archive.read("/chr/c2120.chrbnd.dcx",&decoder)?,&decoder)?;
    let mut used=BTreeMap::<String,Vec<String>>::new();
    for file in &binder.files {
        if !leaf(file.name.as_deref().unwrap_or("")).to_ascii_lowercase().ends_with(".flver") {continue;}
        let model=Model::read(&decoder.decode_if_dcx(&file.bytes)?)?;
        for material in model.materials {
            let stem=leaf(&material.mtd).rsplit_once('.').map_or(leaf(&material.mtd),|(stem,_)|stem).to_ascii_lowercase();
            used.entry(stem).or_default().push(material.name);
        }
    }
    let mut found=BTreeSet::new();
    for path in Game::EldenRing.material_binders() {
        if !archive.contains(path) {continue;}
        let binder=Binder::read(&archive.read(path,&decoder)?,&decoder)?;
        for file in binder.files {
            let name=file.name.as_deref().unwrap_or("");
            if !name.to_ascii_lowercase().ends_with(".matbin") {continue;}
            let stem=leaf(name).rsplit_once('.').map_or(leaf(name),|(stem,_)|stem).to_ascii_lowercase();
            let Some(materials)=used.get(&stem) else {continue;};
            if !found.insert(stem.clone()) {continue;}
            let definition=primase_fromsoft::matbin::Material::read(&decoder.decode_if_dcx(&file.bytes)?)?;
            let controls:Vec<_>=definition.params.iter().filter(|param|visibility_parameter(&param.name)).collect();
            println!("MALENIA_MATBIN_DEFAULTS={}",json!({"definition":name,"materials":materials,"shader":definition.shader,
                "visibility_controls":controls,"all_parameter_names":definition.params.iter().map(|param|&param.name).collect::<Vec<_>>(),
                "petal_parameters":stem.contains("arts_petal").then_some(&definition.params)}));
        }
    }
    for (stem,materials) in used.iter().filter(|(stem,_)|!found.contains(*stem)) {
        println!("MALENIA_MATBIN_MISSING={}",json!({"definition":stem,"materials":materials}));
    }
    if let Some(root)=primase_testdata::install(Data::Sekiro) {
        let install=Install::sekiro(root);let archive=install.open()?;
        let binder=Binder::read(&archive.read("/mtd/allmaterialbnd.mtdbnd.dcx",&decoder)?,&decoder)?;
        let mut materials=0;let mut controls=0;
        for file in binder.files {
            let name=file.name.as_deref().unwrap_or("");if !name.to_ascii_lowercase().ends_with(".mtd") {continue;}
            let definition=primase_fromsoft::mtd::MaterialDefinition::read(&decoder.decode_if_dcx(&file.bytes)?)?;
            let values:Vec<_>=definition.parameters.iter().filter(|param|visibility_parameter(&param.name)).collect();
            if !values.is_empty() {materials+=1;controls+=values.len();
                println!("SEKIRO_MTD_VISIBILITY_DEFAULTS={}",json!({"definition":name,"shader":definition.shader,"parameters":values}));}
        }
        println!("SEKIRO_MTD_VISIBILITY_TOTAL={}",json!({"materials":materials,"controls":controls}));
    }
    Ok(())
}

/// TAE 198 names 10300; inspect the game's corresponding material parameter before assigning semantics.
/// MATERIAL_EX_PARAM_ST v4 is 32 UTF-16 units, material id, five floats, eight padding bytes.
#[test]
fn malenia_material_ex_defaults_and_event198_target()->Result<()> {
    let (Some(root),Some(key_dir),Some(dll))=(install(Data::Elden),keys(Data::Elden),oodle(Tool::Oodle6))
        else {println!("MALENIA_SKIP=PRIMASE_DATA Elden install/keys/oodle6 not configured");return Ok(());};
    let codec=common::oodle(dll)?;let decoder=Decoder {oodle:Some(&codec),max_output:1<<30};
    let install=Install::new(root,Game::EldenRing,Keys::from_folder(key_dir,Game::EldenRing)?);
    let archive=install.open()?;
    let materials=sekiro::read_param(&install,&archive,&decoder,"MaterialExParam.param")?;
    if materials.param_type!="MATERIAL_EX_PARAM_ST" || materials.data_version!=4 || materials.row_size!=96 {
        return Err(format!("unknown MaterialEx layout: {} v{} {} bytes",materials.param_type,materials.data_version,materials.row_size).into());
    }
    let npcs=sekiro::read_param(&install,&archive,&decoder,"NpcParam.param")?;
    let mut targets:BTreeSet<i32>=[10300,10301,21200,21201,21202,21203,21204].into_iter().collect();
    let material_ids:BTreeSet<_>=materials.rows.iter().map(|row|row.id).filter(|id|*id>=10000).collect();
    for row in npcs.rows.iter().filter(|row|row.id/10000==2120) {
        let mut candidates=Vec::new();
        for (index,bytes) in row.data.as_chunks::<4>().0.iter().enumerate() {
            let id=i32::from_le_bytes(*bytes);
            if material_ids.contains(&id) {
                targets.insert(id);candidates.push(json!({"byte_offset":index*4,"value":id}));
            }
        }
        println!("MALENIA_NPC_MATERIAL_CANDIDATES={}",json!({"row":row.id,"row_size":npcs.row_size,
            "candidates":candidates,"scope":"aligned words matching an existing MaterialEx id >=10000; not yet asserted to be references"}));
    }
    for row in materials.rows.iter().filter(|row|targets.contains(&row.id)) {
        let units:Vec<_>=row.data[..64].as_chunks::<2>().0.iter().map(|unit|u16::from_le_bytes(*unit)).take_while(|unit|*unit!=0).collect();
        let name=String::from_utf16(&units)?;
        let values=(0..5).map(|i|word(&row.data,68+4*i).map(f32::from_bits)).collect::<Result<Vec<_>>>()?;
        println!("MALENIA_MATERIAL_EX={}",json!({"row":row.id,"parameter":name,"material_id":word(&row.data,64)? as i32,
            "values":values,"raw":row.data}));
    }
    for id in targets {if !materials.rows.iter().any(|row|row.id==id) {println!("MALENIA_MATERIAL_EX_ABSENT={id}");}}
    Ok(())
}

/// Fast follow-up: source fields the geometry-only view does not interpret, and all event kinds.
/// Separate from the all-clip spline scan so this probe does not repeat that cost.
#[test]
fn malenia_draw_flags_material_records_and_all_tae_controls()->Result<()> {
    let (Some(root),Some(key_dir),Some(dll))=(install(Data::Elden),keys(Data::Elden),oodle(Tool::Oodle6))
        else {println!("MALENIA_SKIP=PRIMASE_DATA Elden install/keys/oodle6 not configured");return Ok(());};
    let codec=common::oodle(dll)?;let decoder=Decoder {oodle:Some(&codec),max_output:1<<30};
    let install=Install::new(root,Game::EldenRing,Keys::from_folder(key_dir,Game::EldenRing)?);
    let archive=install.open()?;
    let npc=sekiro::read_npc_param(&install,&archive,&decoder)?;
    for row in npc.rows.iter().filter(|row|row.id/10000==2120) {
        println!("MALENIA_NPC={}",json!({"row":row.id,"mask":row.display_mask,
            "shown_groups":(0..32).filter(|bit|row.display_mask & (1<<bit)!=0).collect::<Vec<_>>(),
            "scope":"row-number family for inspection; not inferred to be an authored placement"}));
    }
    let binder=Binder::read(&archive.read("/chr/c2120.chrbnd.dcx",&decoder)?,&decoder)?;
    for file in &binder.files {
        let member=file.name.as_deref().unwrap_or("");if !leaf(member).to_ascii_lowercase().ends_with(".flver") {continue;}
        let bytes=decoder.decode_if_dcx(&file.bytes)?;let model=Model::read(&bytes)?;
        // FLVER2's checked LE tables: 128-byte header, dummy64, material32, node128.
        // Node flags are the word between bounding-box min and max; material+24 is its stored index.
        let material_table=128+word(&bytes,20)? as usize*64;
        let node_table=material_table+model.materials.len()*32;
        for (index,mesh) in model.meshes.iter().enumerate() {
            let material=&model.materials[mesh.material];
            let node=usize::try_from(mesh.node).ok().and_then(|index|model.bones.get(index).map(|bone|(index,bone)));
            let node_data=match node {Some((index,bone))=>json!({"index":index,"name":bone.name,"parent":bone.parent,
                "flags":word(&bytes,node_table+index*128+60)?,"translation":bone.translation,"rotation":bone.rotation,"scale":bone.scale}),None=>serde_json::Value::Null};
            println!("MALENIA_RAW_MESH={}",json!({"mesh":index,"material":material.name,"shader":material.mtd,
                "material_stored_index":word(&bytes,material_table+mesh.material*32+24)?,"node":node_data,
                "display_marks":marks(&material.name),"face_flags":mesh.faces.iter().map(|face|face.flags).collect::<Vec<_>>(),
                "gx":material.gx,"textures":material.textures.iter().map(|texture|json!({"kind":texture.kind,"path":texture.path})).collect::<Vec<_>>()}));
        }
    }
    let mut reader=sekiro::CharacterReader::animations(install.clone(),"c2120",decoder);
    let (plans,_)=reader.read().map_err(|error|error.to_string())?;
    let mut kinds=BTreeMap::new();
    for set in common::clip_binders(&plans) {
        let binder=Binder::read(&archive.read(&set,&decoder)?,&decoder)?;
        for file in &binder.files {
            let member=file.name.as_deref().unwrap_or("");if !leaf(member).to_ascii_lowercase().ends_with(".tae") {continue;}
            let tae=Tae::read(&decoder.decode_if_dcx(&file.bytes)?)?;
            for animation in &tae.animations {
                let resolved=tae.resolved_animation(animation.id)?.ok_or("TAE record disappeared")?;
                for event in &resolved.events {
                    *kinds.entry(event.kind).or_insert(0usize)+=1;
                    if matches!(animation.id,2000|3029|3019|20002) || matches!(event.kind,193..=200|233|235|710|711|713) {
                        println!("MALENIA_EVENT_CONTROL={}",json!({"binder":set,"file":member,"animation":animation.id,
                            "resolved":resolved.id,"kind":event.kind,"start":event.start,"end":event.end,"parameters":event.parameters}));
                    }
                }
            }
        }
    }
    println!("MALENIA_EVENT_KIND_COUNTS={}",json!(kinds));
    Ok(())
}

#[test]
fn malenia_source_mesh_groups_tae233_and_petal_bone_scales() -> Result<()> {
    let (Some(root),Some(key_dir),Some(dll))=(install(Data::Elden),keys(Data::Elden),oodle(Tool::Oodle6))
        else {println!("MALENIA_SKIP=PRIMASE_DATA Elden install/keys/oodle6 not configured");return Ok(());};
    let codec=common::oodle(dll)?;
    let decoder=Decoder {oodle:Some(&codec),max_output:1<<30};
    let install=Install::new(root,Game::EldenRing,Keys::from_folder(key_dir,Game::EldenRing)?);
    let archive=install.open()?;
    let binder=Binder::read(&archive.read("/chr/c2120.chrbnd.dcx",&decoder)?,&decoder)?;
    let mut petal_bones=BTreeSet::new();
    let mut source_names=BTreeSet::new();
    let mut flver_bind:BTreeMap<String,Vec<serde_json::Value>>=BTreeMap::new();
    let mut petal_meshes=0;
    for file in &binder.files {
        let member=file.name.as_deref().unwrap_or("");
        if !leaf(member).to_ascii_lowercase().ends_with(".flver") {continue;}
        let model=Model::read(&decoder.decode_if_dcx(&file.bytes)?)?;
        for (mesh_index,mesh) in model.meshes.iter().enumerate() {
            let material=model.materials.get(mesh.material).ok_or("FLVER material index")?;
            println!("MALENIA_MESH={}",json!({"file":member,"mesh":mesh_index,"material_index":mesh.material,
                "material":material.name,"shader":material.mtd,"display_groups":marks(&material.name),
                "dynamic":mesh.dynamic,"node":mesh.node,"palette":mesh.palette,"vertices":mesh.vertices.len(),
                "lod0_triangles":mesh.faces.iter().filter(|f|f.is_lod0()).map(|f|f.triangulate().len()/3).sum::<usize>()}));
            if !material.name.to_ascii_lowercase().contains("arts_petal") {continue;}
            petal_meshes+=1;
            let mut weighted=BTreeSet::new();
            for vertex in &mesh.vertices {
                if mesh.dynamic {
                    for (&index,&weight) in vertex.bone_indices.iter().zip(&vertex.bone_weights) {
                        if weight==0. {continue;}
                        let bone=if mesh.palette.is_empty() {usize::from(index)} else {
                            *mesh.palette.get(usize::from(index)).ok_or("petal palette index")? as usize
                        };
                        weighted.insert(bone);
                    }
                } else {
                    let bone=if model.stores_normal_w(mesh_index) {vertex.normal_w} else {mesh.node};
                    weighted.insert(usize::try_from(bone)?);
                }
            }
            // A parent's zero scale collapses its descendants too. Report both sets explicitly.
            let mut ancestors=weighted.clone();
            for &bone in &weighted {
                let mut at=bone;let mut path=BTreeSet::new();
                loop {
                    if !path.insert(at) {return Err("petal bone parent cycle".into());}
                    let b=model.bones.get(at).ok_or("petal bone index")?;
                    if b.parent<0 {break;}
                    at=usize::try_from(b.parent)?;ancestors.insert(at);
                }
            }
            for &bone in &ancestors {
                let b=model.bones.get(bone).ok_or("petal ancestor index")?;
                source_names.insert(b.name.clone());
                flver_bind.entry(b.name.clone()).or_default().push(json!({"file":member,"bone":bone,"parent":b.parent,
                    "translation":b.translation,"rotation_euler_xyz":b.rotation,"rotation_order":"X then Z then Y","scale":b.scale,
                    "frame":"FLVER source; world reflects Z"}));
                println!("MALENIA_PETAL_BONE={}",json!({"mesh":mesh_index,"bone":bone,"name":b.name,
                    "direct_influence":weighted.contains(&bone),"parent":b.parent,"flver_rest_scale":b.scale}));
            }
            petal_bones.extend(ancestors);
        }
    }
    assert!(petal_meshes>0,"probe did not find the flower material; no visibility conclusion is valid");
    drop(binder);
    let mut reader=sekiro::CharacterReader::animations(install.clone(),"c2120",decoder);
    let (plans,_)=reader.read().map_err(|error|error.to_string())?;
    let listing:Vec<_>=plans.iter::<sekiro::ClipSource>().map(|(entity,source)|(entity,source.clone())).collect();
    let mut reference=World::new();
    sekiro::read_clip_skeleton(&archive,&decoder,"c2120",&mut reference)?.ok_or("c2120 skeleton binder absent")?;
    for name in &source_names {
        let matches:Vec<_>=reference.iter::<Skeleton>().flat_map(|(entity,skeleton)|skeleton.joints.iter()
            .filter(move |joint|&joint.name==name).map(move |joint|json!({"skeleton":entity,"joint":joint.id,
                "parent":joint.parent,"rest":joint.rest,"matrix":joint.rest.matrix(),"frame":"world; Z reflected"}))).collect();
        println!("MALENIA_REFERENCE={}",json!({"bone":name,"flver_bind":flver_bind.get(name),
            "havok_reference":matches,"missing_from_havok":matches.is_empty()}));
    }
    let sets=common::clip_binders(&plans);
    let mut attack_ids=BTreeSet::new();
    for set in sets {
        let binder=Binder::read(&archive.read(&set,&decoder)?,&decoder)?;
        for file in &binder.files {
            let member=file.name.as_deref().unwrap_or("");
            if !leaf(member).to_ascii_lowercase().ends_with(".tae") {continue;}
            let tae=Tae::read(&decoder.decode_if_dcx(&file.bytes)?)?;
            for animation in &tae.animations {
                let mut resolved=animation;let mut seen=BTreeSet::new();
                while let Header::ImportOther {from,..}=&resolved.header {
                    if !seen.insert(resolved.id) {return Err("TAE import cycle".into());}
                    resolved=tae.animation(i64::from(*from)).ok_or("TAE import target missing")?;
                }
                for event in resolved.events.iter().filter(|e|matches!(e.kind,233|711|713)) {
                    if event.kind==233 {attack_ids.insert(animation.id);}
                    println!("MALENIA_TAE={}",json!({"binder":set,"file":member,"animation":animation.id,
                        "resolved_animation":resolved.id,"header":animation.header,"kind":event.kind,
                        "start":event.start,"end":event.end,"bytes":event.parameters,
                        "mask32":if event.kind==233 {event.parameters.get(..32)} else {None}}));
                }
            }
        }
    }
    let mut registry=PassRegistry::new();primase_havok::register(&mut registry);
    let manager=PassManager::new(registry).with_pass_checks(true);
    let pipeline=manager.registry.parse("lower-havok-clips{absent=neutral,rate=1,clips=all,root-motion=leave}")?;
    let mut decoded=0;
    for (entity,entry) in &listing {
        let id=primase_fromsoft::tae::clip_animation_id(&entry.name);
        let control=entry.name=="a000_002000" || entry.name=="a000_003029";
        let mut world=match reader.load(&plans,&[*entity].into()) {
            Ok((world,_))=>world,
            Err(error)=>{println!("MALENIA_CLIP_REFUSED={}",json!({"key":entry.key,"report":error.to_string()}));continue;}
        };
        let mut tracked=BTreeSet::new();let mut duration=None;
        for (_,animation) in world.iter::<primase_havok::world::HavokAnimation>() {
            duration=Some(animation.spline.duration);
            let skeleton=world.get::<Skeleton>(animation.skeleton).ok_or("Havok animation skeleton missing")?;
            for &bone in &animation.track_to_bone {
                let Some(joint)=usize::try_from(bone).ok().and_then(|index|skeleton.joints.get(index)) else {continue;};
                if joint.name.starts_with("Xtra_Petal") {tracked.insert(joint.name.clone());}
            }
            if control {
                for joint in skeleton.joints.iter().filter(|j|source_names.contains(&j.name)) {
                    let transform=animation.reference_pose.get(joint.id.0 as usize).ok_or("missing raw reference transform")?;
                    println!("MALENIA_REFERENCE_TRS={}",json!({"clip":entry.key,"bone":joint.name,
                        "translation":transform.translation,"rotation":transform.rotation,"scale":transform.scale,
                        "frame":"Havok source; compare FLVER source before Z reflection","flver_bind":flver_bind.get(&joint.name)}));
                }
            }
        }
        println!("MALENIA_CLIP_TRACKS={}",json!({"key":entry.key,"duration":duration,"petal_track_bones":tracked,
            "petal_track_bone_count":tracked.len(),"control":control}));
        if !control && tracked.is_empty() && !id.is_some_and(|id|attack_ids.contains(&id)) {continue;}
        manager.run(&mut world,&pipeline,&mut RunReport::default())?;
        for (_,clip) in world.iter::<Clip>() {
            let mut bones=BTreeMap::new();
            for (skeleton_entity,skeleton) in world.iter::<Skeleton>() {
                for joint in skeleton.joints.iter().filter(|j|source_names.contains(&j.name)) {
                    // A collapsed reference may be singular: report its matrix without requiring invertible TRS.
                    let matrix=joint.rest.matrix();
                    let rest=match joint.rest {primase_core::Rest::Trs {scale,..}=>scale.map(f64::from),
                        primase_core::Rest::Affine(_)=>std::array::from_fn(|axis|matrix.cols[axis].iter().map(|v|f64::from(*v).powi(2)).sum::<f64>().sqrt())};
                    let channels:Vec<_>=clip.channels.iter().filter(|c|c.property==Property::Scale
                        && c.target==(ClipTarget::Joint {skeleton:skeleton_entity,joint:joint.id})).collect();
                    let mut min=rest;let mut max=rest;let mut samples=0;
                    for channel in &channels {
                        for scale in channel.values.as_chunks::<3>().0 {
                            for axis in 0..3 {if samples==0 {min[axis]=f64::from(scale[axis]);max[axis]=min[axis];}
                                else {min[axis]=min[axis].min(f64::from(scale[axis]));max[axis]=max[axis].max(f64::from(scale[axis]));}}
                            samples+=1;
                        }
                    }
                    let mut translation_min=matrix.cols[3];let mut translation_max=translation_min;let mut translation_samples=0;
                    for channel in clip.channels.iter().filter(|c|c.property==Property::Translation
                        && c.target==(ClipTarget::Joint {skeleton:skeleton_entity,joint:joint.id})) {
                        for translation in channel.values.as_chunks::<3>().0 {
                            for axis in 0..3 {if translation_samples==0 {translation_min[axis]=translation[axis];translation_max[axis]=translation[axis];}
                                else {translation_min[axis]=translation_min[axis].min(translation[axis]);translation_max[axis]=translation_max[axis].max(translation[axis]);}}
                            translation_samples+=1;
                        }
                    }
                    bones.insert(joint.name.clone(),json!({"joint":joint.id,"rest":rest,"min":min,"max":max,
                        "scale_samples":samples,"scale_track_present":!channels.is_empty(),
                        "translation_min":translation_min,"translation_max":translation_max,"translation_samples":translation_samples}));
                }
            }
            println!("MALENIA_CLIP_SCALE={}",json!({"key":entry.key,"duration":clip.duration,
                "sample_policy":"lower-havok-clips absent=neutral rate=1 (native keys)","bones":bones}));
            decoded+=1;
        }
    }
    println!("MALENIA_TOTAL={}",json!({"petal_meshes":petal_meshes,"petal_bones_and_ancestors":petal_bones.len(),
        "animations_with_233":attack_ids.len(),"clips_sampled":decoded,"listed":listing.len()}));
    assert!(decoded>=2,"both requested control clips must be decoded; inspect refusals above");
    Ok(())
}
