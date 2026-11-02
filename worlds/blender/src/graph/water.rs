//! A `core.water_body` under a `pbr.material`'s surface as a Blender material.
//!
//! - **Surface**: a Principled BSDF that passes what it does not reflect (Transmission Weight 1), not metallic, at the
//!   material's roughness, its IOR the body's (Cycles weighs reflection against transmission by its Fresnel). Its base
//!   colour is white: Cycles tints what passes by it, and the water's colour is its volume's.
//! - **Normal**: the waves' tangent-space x and y, each times its strength, summed over a z of 1, through a Normal Map
//!   node on the first wave's UV set; each wave sampled at its coordinates moved by its velocity times the time (the
//!   frame over [`super::FPS`]). Without waves, the material's own normal map.
//! - **Volume**: a Volume Absorption inside the surface, density the largest extinction and colour
//!   `1 - extinction / density` per channel (Cycles absorbs `density * (1 - colour)` per metre): what passes into the
//!   water is absorbed along its path until it leaves the volume, so behind a surface that is the top of the water a ray
//!   loses `exp(-extinction * d)` over the `d` metres to the bed.
//! - **Not drawn**, counted by name: the scatter (Cycles' volume scattering is a medium's, not the body's diffuse
//!   reflectance core states), the shore fade (the opacity by the depth of what is behind has no node), the base colour
//!   and alpha `pbr.material` states for a viewer without transmission.
//! - **Sidedness and passes** as `pbr.material`'s ([`super::pbr`]): the base colour pass is white, the emission none.

use primase_core::WaterBody;
use primase_core::pbr::{AlphaMode, PbrMaterial};
use serde_json::json;

use super::texture::{Coordinates, Reading, Sampler};
use super::{Float, Graph, PASS_ALPHA_THRESHOLD, Settings, Socket, Vec3, aov};
use crate::Result;

/// What a water body states that Cycles is not given ([`crate::BlenderReport::water_unwritten`]).
pub const SCATTER: &str =
    "water scatter (the body's diffuse reflectance; Cycles' volume scattering is a medium's)";
pub const SHORE: &str = "shore fade (opacity by the depth of what is behind)";
pub const BASE_COLOUR: &str = "base colour of a water material (Cycles tints what passes by it)";
pub const ALPHA: &str = "alpha of a water material (the water is drawn by its transmission)";

/// The graph of material `m` with its water body `w`, and what it does not draw.
pub fn graph(
    s: &mut Sampler,
    m: &PbrMaterial,
    w: &WaterBody,
) -> Result<(Graph, Settings, Vec<&'static str>)> {
    let mut g = Graph::new();
    let mut unwritten = Vec::new();
    if w.scatter != [0.0; 3] {
        unwritten.push(SCATTER);
    }
    if w.shore_fade > 0.0 {
        unwritten.push(SHORE);
    }
    if m.base_color[..3] != [1.0; 3] || m.base_color_texture.is_some() {
        unwritten.push(BASE_COLOUR);
    }
    if m.alpha_mode != AlphaMode::Opaque || m.base_color[3] != 1.0 {
        unwritten.push(ALPHA);
    }
    let normal = match waves(s, &mut g, w)? {
        Some(n) => Some(n),
        None => super::pbr::surface(s, &mut g, m, Coordinates::default(), false)?.normal,
    };
    let n = g.node("ShaderNodeBsdfPrincipled", &[]);
    g.color_in(n, "Base Color", Vec3::Const([1.0; 3]));
    g.float_in(n, "Metallic", Float::Const(0.0));
    g.float_in(n, "Roughness", Float::Const(m.roughness));
    g.float_in(n, "IOR", Float::Const(w.ior));
    g.float_in(n, "Transmission Weight", Float::Const(1.0));
    if let Some(normal) = normal {
        g.link(normal, n, "Normal");
    }
    let shaded = Socket {
        node: n,
        output: "BSDF",
    };
    let mut coverage = Float::Const(1.0);
    let drawn = if m.double_sided {
        shaded
    } else {
        let back = g.input_node("ShaderNodeNewGeometry", "Backfacing");
        coverage = g.sub(Float::Const(1.0), Float::Out(back));
        let clear = g.shader(
            "ShaderNodeBsdfTransparent",
            "BSDF",
            &[],
            &[("Color", Vec3::Const([1.0; 3]))],
        );
        g.mix_shader(Float::Out(back), shaded, clear)
    };
    let volume = absorption(&mut g, w.extinction);
    g.output_with_volume(drawn, volume);
    let coverage = g.at_least(coverage, Float::Const(PASS_ALPHA_THRESHOLD));
    let base = g.vscale(Vec3::Const([1.0; 3]), coverage);
    g.aov_color(aov::BASE_COLOR, base);
    g.aov_color(aov::EMISSION, Vec3::Const([0.0; 3]));
    g.aov_value(aov::COVERAGE, coverage);
    Ok((
        g,
        Settings {
            surface_render_method: "DITHERED",
            use_backface_culling: !m.double_sided,
        },
        unwritten,
    ))
}

/// The Volume Absorption of `extinction` (per metre); `None` for clear water.
fn absorption(g: &mut Graph, extinction: [f32; 3]) -> Option<Socket> {
    let density = extinction.into_iter().fold(0.0f32, f32::max);
    if density <= 0.0 {
        return None;
    }
    Some(g.shader(
        "ShaderNodeVolumeAbsorption",
        "Volume",
        &[("Density", Float::Const(density))],
        &[("Color", Vec3::Const(extinction.map(|e| 1.0 - e / density)))],
    ))
}

/// The Normal Map node of the waves' summed tangent-space normal; `None` without a wave sampled.
fn waves(s: &mut Sampler, g: &mut Graph, w: &WaterBody) -> Result<Option<Socket>> {
    let mut sum: Option<Vec3> = None;
    let mut set = None;
    for wave in &w.waves {
        let at = Coordinates {
            scroll: Some(wave.velocity),
            ..Coordinates::default()
        };
        let Some(x) = s.sample(g, &wave.normal, Reading::Data, at)? else {
            continue;
        };
        set.get_or_insert(wave.normal.texcoord);
        // x and y as the map stores them (`2c - 1`) times the strength, z 0
        let k = wave.strength;
        let xy = g.vector_math(
            "MULTIPLY_ADD",
            &[
                x.color,
                Vec3::Const([2.0 * k, 2.0 * k, 0.0]),
                Vec3::Const([-k, -k, 0.0]),
            ],
        );
        sum = Some(match sum {
            None => xy,
            Some(a) => g.vadd(a, xy),
        });
    }
    let (Some(sum), Some(set)) = (sum, set) else {
        return Ok(None);
    };
    // over a z of 1, encoded as the Normal Map node reads a colour (`c = v / 2 + 1 / 2`)
    let colour = g.vector_math(
        "MULTIPLY_ADD",
        &[sum, Vec3::Const([0.5, 0.5, 0.0]), Vec3::Const([0.5, 0.5, 1.0])],
    );
    let n = g.node(
        "ShaderNodeNormalMap",
        &[
            ("space", json!("TANGENT")),
            ("uv_map", json!(Sampler::uv_map(set))),
        ],
    );
    g.float_in(n, "Strength", Float::Const(1.0));
    g.color_in(n, "Color", colour);
    s.counts.normal_maps_mikktspace += 1;
    Ok(Some(Socket {
        node: n,
        output: "Normal",
    }))
}
