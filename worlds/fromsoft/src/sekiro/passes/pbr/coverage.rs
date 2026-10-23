//! The texture that decides a material's alpha test, from the material's own programs: the slot whose alpha reaches the
//! pixel program's discard, where it is sampled (which UV set of the mesh) and whether a vertex colour's alpha multiplies
//! it ([`crate::passes::program_reads`]). Elden Ring's fur and feather cards test one texture on one UV set and colour
//! from another on the first, so the slot the preview colours with (the lowest or eighth) is not the one with the cut-out.
use crate::materials::{Definition, Material};
use crate::passes::program_reads::{self, Reads};
use crate::sekiro::source::{MaterialShader, ShaderBundle, ShaderStage};
use primase_core::{Entity, World};

/// The slot the alpha test reads and how it is sampled.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Coverage {
    /// The key of the material's binding of the slot.
    pub binding: String,
    /// The lowered UV set (`TexCoord(n)`) the slot is sampled with.
    pub texcoord: u8,
    /// Whether the first vertex colour's alpha multiplies the coverage.
    pub vertex_alpha: bool,
    /// What else decides the discard, which the preview does not follow.
    pub left_out: Vec<&'static str>,
}

/// What a material's alpha-tested programs say, or why they could not say it (`Err`, counted by the caller). `Ok(None)`:
/// a material this does not apply to (not a material binary's, or a shader with no alpha-tested G-buffer program).
pub(super) fn coverage(
    world: &World,
    entity: Entity,
    material: &Material,
) -> Result<Option<Coverage>, String> {
    if !matches!(material.definition, Some(Definition::Matbin(_))) {
        return Ok(None);
    }
    let Some(bundle) = world.get::<MaterialShader>(entity).map(|m| m.0) else {
        return Ok(None);
    };
    let Some(bundle) = world.get::<ShaderBundle>(bundle) else {
        return Ok(None);
    };
    let Some(meta) = &bundle.parameters else {
        return Err(format!("{}: no metaparam read", bundle.name));
    };
    let pixel = |switch: u8| bundle.program(&format!("{}_{switch}_Gbuf_[A].ppo", bundle.name));
    let programs: Vec<_> = [0, 1].into_iter().filter_map(pixel).collect();
    if programs.is_empty() {
        return Ok(None);
    }
    let vertex = bundle
        .program(&format!("{}_Gbuf.vpo", bundle.name))
        .or_else(|| bundle.program(&format!("{}_Gbuf_Skin.vpo", bundle.name)))
        .filter(|p| p.stage == ShaderStage::Vertex)
        .ok_or_else(|| format!("{}: no G-buffer vertex program", bundle.name))?;
    let mut reads: Option<Reads> = None;
    for program in programs {
        let found = program_reads::read(&program.code, &vertex.code, meta)
            .map_err(|e| format!("{}: {e}", program.name))?;
        match &reads {
            Some(first)
                if (first.coverage.as_slice(), first.coverage_vertex_alpha)
                    != (found.coverage.as_slice(), found.coverage_vertex_alpha) =>
            {
                return Err(format!(
                    "{}: its static permutations disagree on the slot the alpha test reads",
                    bundle.name
                ));
            }
            Some(_) => {}
            None => reads = Some(found),
        }
    }
    let reads = reads.ok_or("no program read")?;
    let [slot] = &reads.coverage[..] else {
        return Err(format!(
            "{}: {} slots reach the alpha test, not one",
            bundle.name,
            reads.coverage.len()
        ));
    };
    let uv_inputs = program_reads::vertex_uv_inputs(&vertex.code)?;
    let (Some(u), Some(v)) = (&slot.u, &slot.v) else {
        return Err(format!(
            "{}: the coverage slot's coordinates are not made of one vertex input",
            bundle.name
        ));
    };
    if (u.name.as_str(), u.index, u.component / 2) != (v.name.as_str(), v.index, v.component / 2) {
        return Err(format!(
            "{}: the coverage slot's u and v come from different UV pairs",
            bundle.name
        ));
    }
    let texcoord = program_reads::uv_set(&uv_inputs, u)
        .ok_or_else(|| format!("{}: the coverage slot is not sampled with a UV set", bundle.name))?;
    Ok(Some(Coverage {
        binding: slot.texture.clone(),
        texcoord,
        vertex_alpha: reads.coverage_vertex_alpha,
        left_out: reads.left_out.into_iter().collect(),
    }))
}
