//! The restored-state RGB equation of the shipped generic shader, represented by four ordered core layers.
//! Source HLSL and its SM3 pixel bytecode establish the red tint, then green and blue lerps. This pass runs before
//! the ordinary material lowering. It never substitutes a vertex colour for a missing mask or alpha factor.
use std::collections::{BTreeMap, BTreeSet};

use primase_core::layered::{Channel, Composition, Source};
use primase_core::pass::{OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{
    Component, Entity, Layer, LayerWeights, LayeredMaterial, MaterialVariants, Mesh, Name, Semantic,
    VertexColor, WeightMapping, World,
};

use super::material::texture_ref;
use crate::components::{LayerState, MaterialRef, MaterialState};

#[derive(Clone, Copy, Debug)]
pub struct LowerShaunwhiteLayers;
pub const LOWER_SHAUNWHITE_LAYERS: PassSpec = PassSpec {
    name: "lower-shaunwhite-layers",
    options: &[OptionSpec::required(
        "influence",
        "restored",
        "restored selects full world influence; the un-restored colour path is not lowered",
    )],
    make: |g| {
        if g.required("influence") != "restored" {
            return Err("influence must be restored".into());
        }
        Ok(Box::new(LowerShaunwhiteLayers))
    },
};

fn sampled(texture: primase_core::pbr::TextureRef, channel: usize, factor: f32) -> Source {
    let mut channels = [0.; 4];
    channels[channel] = factor;
    Source::Mask {
        texture,
        channels,
        bias: 0.,
    }
}
fn constant(texture: primase_core::pbr::TextureRef, value: f32) -> Source {
    Source::Mask {
        texture,
        channels: [0.; 4],
        bias: value,
    }
}
fn weight(
    channel: u8,
    vertex: bool,
    mask: Option<primase_core::pbr::TextureRef>,
    alpha: Option<Source>,
    base: primase_core::pbr::TextureRef,
) -> Channel {
    let mut sources = Vec::new();
    if vertex {
        sources.push(Source::Vertex {
            semantic: Semantic::Color(0),
            channel,
        });
    }
    if let Some(mask) = mask {
        sources.push(sampled(mask, usize::from(channel), 1.));
    }
    if let Some(alpha) = alpha {
        sources.push(alpha);
    }
    match sources.len() {
        0 => Channel::Source(constant(base, 1.)),
        1 => Channel::Source(sources.remove(0)),
        _ => Channel::Product { sources },
    }
}

/// Every drawing of the material: VTXCOL is the vertex-shader path with COLOR0, otherwise the shader supplies one.
fn usage(w: &World) -> BTreeMap<Entity, BTreeSet<bool>> {
    let mut out = BTreeMap::<Entity, BTreeSet<bool>>::new();
    for (e, mesh) in w.iter::<Mesh>() {
        for (i, p) in mesh.primitives().iter().enumerate() {
            let vertex = p.streams.contains_key(&Semantic::Color(0));
            for material in p.material.into_iter().chain(
                w.get::<MaterialVariants>(e)
                    .and_then(|v| v.0.get(i))
                    .into_iter()
                    .flat_map(|v| v.values().copied()),
            ) {
                out.entry(material).or_default().insert(vertex);
            }
        }
    }
    out
}

fn child(
    w: &mut World,
    m: &MaterialRef,
    mut state: MaterialState,
    color: [f32; 4],
    slot: crate::components::SlotRef,
    label: &str,
) -> Layer {
    let e = w.spawn();
    state.color = [color[0], color[1], color[2], 1.];
    state.layering = LayerState::default();
    let name = format!("{} / {label}", m.name);
    w.insert(e, Name(name.clone()));
    w.insert(
        e,
        MaterialRef {
            id: m.id,
            name,
            state: Some(state),
            slots: BTreeMap::from([("diffuse".into(), slot)]),
            unread: Vec::new(),
        },
    );
    w.insert(e, VertexColor::Ignored);
    Layer {
        material: e,
        texcoord: Some(slot.texcoord),
        transform: None,
    }
}

fn build(
    w: &mut World,
    host: Entity,
    m: &MaterialRef,
    state: MaterialState,
    vertex: bool,
    log: &mut PassLog,
) {
    let base_slot = m.slots["diffuse"];
    let base = texture_ref(w, &base_slot, log);
    let mask = m.slots.get("mask").map(|s| texture_ref(w, s, log));
    let mut layers = vec![
        Some(child(w, m, state, [1.; 4], base_slot, "untinted base")),
        Some(child(w, m, state, state.color, base_slot, "red tint")),
    ];
    let mut channels = vec![
        Channel::Source(constant(base, 1.)),
        weight(0, vertex, mask, None, base),
    ];
    for (i, name) in ["diffuse_b", "diffuse_c"].into_iter().enumerate() {
        if let Some(slot) = m.slots.get(name) {
            let tint = state.layering.colors[i];
            let sample = texture_ref(w, slot, log);
            let alpha = state.layering.use_alpha[i + 1].then(|| sampled(sample, 3, tint[3]));
            let uses_alpha = alpha.is_some();
            channels.push(weight((i + 1) as u8, vertex, mask, alpha, base));
            layers.push(Some(child(w, m, state, tint, *slot, name)));
            if uses_alpha {
                log.count("layer weights retaining sampled texture alpha", 1);
            }
        }
    }
    w.insert(
        host,
        LayeredMaterial {
            layers,
            weights: LayerWeights {
                channels: Some(channels),
                composition: Composition::Over,
                ..LayerWeights::new(Semantic::Color(0), WeightMapping::Identity)
            },
        },
    );
    w.insert(host, VertexColor::Ignored);
    log.count("restored RGB layer materials", 1);
}

impl Pass for LowerShaunwhiteLayers {
    fn name(&self) -> &'static str {
        "lower-shaunwhite-layers"
    }
    fn options(&self) -> Options {
        Options::from([("influence".into(), "restored".into())])
    }
    fn requires(&self) -> &[&'static str] {
        &[MaterialRef::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            LayeredMaterial::NAME,
            MaterialRef::NAME,
            VertexColor::NAME,
            Name::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, MaterialVariants::NAME]
    }
    fn approximates(&self) -> &'static str {
        "full world influence selects the restored state; scalar/specular and lighting remain the ordinary material lowering's approximation"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let uses = usage(w);
        let all: Vec<_> = w.iter::<MaterialRef>().map(|(e, m)| (e, m.clone())).collect();
        let mut colourless = BTreeMap::new();
        for (e, m) in all {
            if !["mask", "diffuse_b", "diffuse_c"]
                .iter()
                .any(|s| m.slots.contains_key(*s))
            {
                continue;
            }
            let reason = match m.state {
                None => Some("material fields not read"),
                Some(s) if s.water => Some("water shader"),
                Some(s) if s.blended || s.alpha_test => {
                    Some("host alpha blend/test equation not represented by RGB layers")
                }
                Some(s)
                    if s.color
                        .iter()
                        .chain(s.layering.colors.iter().flatten())
                        .any(|v| !(0. ..=1.).contains(v)) =>
                {
                    Some("modulation colours outside 0..1")
                }
                _ if !m.slots.contains_key("diffuse") => Some("base diffuse texture missing"),
                _ if m.slots.iter().any(|(n, s)| {
                    ["diffuse", "mask", "diffuse_b", "diffuse_c"].contains(&n.as_str()) && s.rotation != 0.
                }) =>
                {
                    Some("rotated UV matrix convention not established")
                }
                _ if m.slots.iter().any(|(n, s)| {
                    ["diffuse", "mask", "diffuse_b", "diffuse_c"].contains(&n.as_str())
                        && s.address.is_some_and(|a| a.contains(&2))
                }) =>
                {
                    Some("border addressing not represented by core texture references")
                }
                _ => None,
            };
            if let Some(reason) = reason {
                log.omit(&format!("{}: RGB layers not lowered: {reason}", m.name), 1);
                continue;
            }
            if w.has::<LayeredMaterial>(e) {
                return Err(format!("{} already has core.layered", m.name));
            }
            let state = m.state.ok_or("material fields not read")?;
            let both = uses.get(&e).is_some_and(|modes| modes.len() == 2);
            let vertex = uses.get(&e).is_none_or(|modes| modes.contains(&true));
            build(w, e, &m, state, vertex, log);
            if both {
                let other = w.spawn();
                let mut source = m.clone();
                source.name = format!("{} / without vertex weights", m.name);
                w.insert(other, source.clone());
                w.insert(other, Name(source.name.clone()));
                build(w, other, &source, state, false, log);
                colourless.insert(e, other);
                log.count("materials split for absent vertex colour", 1);
            }
        }
        let meshes: Vec<_> = w.iter::<Mesh>().map(|(e, _)| e).collect();
        for e in meshes {
            let missing: Vec<_> = w
                .get::<Mesh>(e)
                .into_iter()
                .flat_map(|m| m.primitives().iter())
                .map(|p| !p.streams.contains_key(&Semantic::Color(0)))
                .collect();
            if let Some(mesh) = w.get_mut::<Mesh>(e) {
                for (i, absent) in missing.iter().enumerate() {
                    if *absent
                        && let Some(mut p) = mesh.primitive_mut(i)
                        && let Some(old) = *p.material_mut()
                        && let Some(new) = colourless.get(&old)
                    {
                        *p.material_mut() = Some(*new);
                    }
                }
            }
            if let Some(variants) = w.get_mut::<MaterialVariants>(e) {
                for (i, row) in variants.0.iter_mut().enumerate() {
                    if missing.get(i) == Some(&true) {
                        for material in row.values_mut() {
                            if let Some(new) = colourless.get(material) {
                                *material = *new;
                            }
                        }
                    }
                }
            }
        }
        log.approximate(
            "world influence fixed at one: restored state, un-restored greyscale path omitted",
            1,
        );
        log.omit(
            "layer-dependent specular response (host scalar/specular fields retained)",
            1,
        );
        Ok(())
    }
}
