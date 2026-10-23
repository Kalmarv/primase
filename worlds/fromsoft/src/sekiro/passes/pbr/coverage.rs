//! What a material binary's own G-buffer programs say its preview is drawn with ([`crate::passes::program_reads`]): the
//! slot whose alpha reaches the pixel program's discard (the coverage), the slot its albedo is made of with the others
//! blended over it (the base), where each is sampled (which UV set of the mesh) and whether a vertex colour's alpha
//! multiplies the coverage. Elden Ring's fur and feather cards test one texture on one UV set and colour from another;
//! its `C[DetailBlend]` materials colour from their own albedo (`Texture2D_7`) with shared detail maps blended over it
//! from lower slots, so neither the lowest nor the eighth slot is the one to draw with.
use crate::materials::{Definition, Material};
use crate::passes::program_reads::{self, Reads, SlotRead};
use crate::sekiro::source::{MaterialShader, ShaderBundle, ShaderStage};
use primase_core::{Entity, World};

/// A slot the programs read and the lowered UV set (`TexCoord(n)`) they sample it with.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Sampled {
    /// The key of the material's binding of the slot.
    pub binding: String,
    pub texcoord: u8,
}

/// The slot the alpha test reads and how it is sampled.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Coverage {
    pub slot: Sampled,
    /// Whether the first vertex colour's alpha multiplies the coverage.
    pub vertex_alpha: bool,
    /// What else decides the discard, which the preview does not follow.
    pub left_out: Vec<&'static str>,
}

/// The slot the albedo is made of and whether other slots are blended over it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Base {
    pub slot: Sampled,
    /// Other material slots reach the albedo too (detail maps, masks): the preview leaves them out.
    pub blended: bool,
    /// The pixel program it was read from.
    pub program: String,
}

/// Why a material binary's programs could not say what they read: the pixel program the material draws with (the
/// bundle's name where the material names none) and the reason.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Unread {
    pub program: String,
    pub why: String,
}

/// What a material's G-buffer programs read.
struct Programs {
    /// The pixel program's file name.
    name: String,
    reads: Reads,
    /// The vertex program's UV inputs ([`program_reads::vertex_uv_inputs`]).
    uv_inputs: Vec<(u32, u8)>,
}

/// What the material's own G-buffer pixel program ([`crate::materials::MaterialView::gbuffer_program`]: its shader,
/// static switch and alpha reference select it) and the shader's G-buffer vertex program read, with the vertex
/// program's UV inputs and the pixel program's name. `Ok(None)`: not a material binary's, or the material has no bundle;
/// `Err`: the programs could not be read (counted by the caller).
fn read(world: &World, entity: Entity, material: &Material) -> Result<Option<Programs>, Unread> {
    if !matches!(material.definition, Some(Definition::Matbin(_))) {
        return Ok(None);
    }
    let Some(view) = material.view() else {
        return Ok(None);
    };
    let Some(bundle) = world.get::<MaterialShader>(entity).map(|m| m.0) else {
        return Ok(None);
    };
    let Some(bundle) = world.get::<ShaderBundle>(bundle) else {
        return Ok(None);
    };
    let name = view.gbuffer_program();
    let unread = |why: String| Unread {
        program: name.clone(),
        why,
    };
    let Some(meta) = &bundle.parameters else {
        return Err(unread(format!("{}: no metaparam read", bundle.name)));
    };
    let pixel = bundle
        .program(&name)
        .ok_or_else(|| unread("not in the bundle".into()))?;
    let vertex = bundle
        .program(&format!("{}_Gbuf.vpo", bundle.name))
        .or_else(|| bundle.program(&format!("{}_Gbuf_Skin.vpo", bundle.name)))
        .filter(|p| p.stage == ShaderStage::Vertex)
        .ok_or_else(|| unread(format!("{}: no G-buffer vertex program", bundle.name)))?;
    let reads = program_reads::read(&pixel.code, &vertex.code, meta).map_err(unread)?;
    let uv_inputs = program_reads::vertex_uv_inputs(&vertex.code).map_err(unread)?;
    Ok(Some(Programs {
        name,
        reads,
        uv_inputs,
    }))
}

/// The UV set a slot is sampled with: its `u` and `v` made of one vertex input's pair.
fn sampled(name: &str, slot: &SlotRead, uv_inputs: &[(u32, u8)], what: &str) -> Result<Sampled, Unread> {
    let unread = |why: String| Unread {
        program: name.to_owned(),
        why,
    };
    let (Some(u), Some(v)) = (&slot.u, &slot.v) else {
        return Err(unread(format!(
            "the {what} slot's coordinates are not made of one vertex input"
        )));
    };
    if (u.name.as_str(), u.index, u.component / 2) != (v.name.as_str(), v.index, v.component / 2) {
        return Err(unread(format!(
            "the {what} slot's u and v come from different UV pairs"
        )));
    }
    let texcoord = program_reads::uv_set(uv_inputs, u)
        .ok_or_else(|| unread(format!("the {what} slot is not sampled with a UV set")))?;
    Ok(Sampled {
        binding: slot.texture.clone(),
        texcoord,
    })
}

/// What a material's alpha-tested program says about its alpha test, or why it could not say it (`Err`, counted by the
/// caller). `Ok(None)`: a material this does not apply to (not a material binary's, or one drawn without an alpha test).
pub(super) fn coverage(
    world: &World,
    entity: Entity,
    material: &Material,
) -> Result<Option<Coverage>, Unread> {
    let Some(Programs {
        name,
        reads,
        uv_inputs,
    }) = read(world, entity, material)?
    else {
        return Ok(None);
    };
    if !name.ends_with("_[A].ppo") {
        return Ok(None);
    }
    let [slot] = &reads.coverage[..] else {
        return Err(Unread {
            why: format!("{} slots reach the alpha test, not one", reads.coverage.len()),
            program: name,
        });
    };
    Ok(Some(Coverage {
        slot: sampled(&name, slot, &uv_inputs, "coverage")?,
        vertex_alpha: reads.coverage_vertex_alpha,
        left_out: reads.left_out.into_iter().collect(),
    }))
}

/// What a material's G-buffer program says its albedo is made of: the base slot and how it is sampled, or why it could
/// not say (`Err`, counted by the caller). `Ok(None)`: not a material binary's.
pub(super) fn base(world: &World, entity: Entity, material: &Material) -> Result<Option<Base>, Unread> {
    let Some(Programs {
        name,
        reads,
        uv_inputs,
    }) = read(world, entity, material)?
    else {
        return Ok(None);
    };
    let Some(slot) = reads.base.as_ref() else {
        return Err(Unread {
            program: name,
            why: "no one slot is the albedo's base".into(),
        });
    };
    Ok(Some(Base {
        slot: sampled(&name, slot, &uv_inputs, "base")?,
        blended: reads.colour.len() > 1,
        program: name,
    }))
}
