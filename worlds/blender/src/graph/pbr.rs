//! A `pbr.material` as a Blender material: its surface, its alpha as its mode uses it, its sidedness and the passes
//! every material writes ([`super::aov`]).
//!
//! - **Base colour**: `base_color` times the base colour texture (sRGB) times `COLOR_0` where the primitive has it
//!   and the material takes it (`core.vertex_color`), alpha alike: the importer's colour attributes are 8-bit, so the
//!   save stage gives the scene `COLOR_<n>` as float attributes of its own.
//! - **Metal and roughness**: the factors times the metallic-roughness texture's B and G.
//! - **Normal**: the normal map's `xy` scaled by `normal_scale` (`c' = c s + (1 - s) / 2` per channel, z as stored),
//!   through a tangent-space Normal Map node on the slot's UV set, which computes its tangent frame (MikkTSpace;
//!   counted).
//! - **Occlusion**: Cycles traces it; the map is not drawn, counted.
//! - **Emission**: Emission Color is `emissive` times its texture, Emission Strength `emissive_strength`.
//! - **Specular** (`KHR_materials_specular`): F0 `0.04 * colour * factor` is IOR 1.5 with Specular IOR Level
//!   `0.5 * factor * A` and Specular Tint `colour * colour texture`; what Blender's level does not scale (f90) counted.
//!   Without a stated specular, IOR 1.5 at level 0.5, untinted: glTF's 0.04.
//! - **Alpha**: opaque ignores it (1); mask is `alpha >= cutoff` (1 or 0); blend draws it as it is.
//! - **Sidedness**: a single-sided material is transparent where Cycles sees its back (a Mix Shader by the Geometry
//!   node's Backfacing), and culls back faces in EEVEE and the viewport.
//! - **Unlit**: the colour drawn toward the camera is the base colour, whatever lights the scene (an Emission shader of
//!   strength 1 for camera rays); to every other ray it is a black, non-emitting surface (it is not a light and is not
//!   lit), so it neither lights nor tints the scene.

use primase_core::pbr::{AlphaMode, PbrMaterial, Shading};
use serde_json::json;

use super::texture::{Coordinates, Reading, Sampler};
use super::{Float, Graph, PASS_ALPHA_THRESHOLD, Settings, Socket, Vec3, aov};
use crate::Result;

/// What a material's surface is before its alpha, sidedness and shading model are applied: what a blend of layers
/// mixes, layer by layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    pub base: Vec3,
    pub alpha: Float,
    pub metallic: Float,
    pub roughness: Float,
    /// The tangent-space normal as the Normal Map node reads it (`0.5` per axis is the surface's own); `None`: the
    /// shading normal.
    pub normal: Option<Socket>,
    pub emission: Vec3,
    pub emission_strength: Float,
    pub specular_level: Float,
    pub specular_tint: Vec3,
}

/// The surface `m` states, sampled with `at` (a layer's coordinates), its base colour times `COLOR_0` where `color0`.
pub fn surface(
    s: &mut Sampler,
    g: &mut Graph,
    m: &PbrMaterial,
    at: Coordinates,
    color0: bool,
) -> Result<Surface> {
    let [r, gr, b, a] = m.base_color;
    let mut base = Vec3::Const([r, gr, b]);
    let mut alpha = Float::Const(a);
    if let Some(t) = &m.base_color_texture
        && let Some(x) = s.sample(g, t, Reading::Srgb, at)?
    {
        base = g.vmul(base, x.color);
        alpha = g.mul(alpha, x.alpha);
    }
    if color0 {
        let (c, ca) = g.attribute("COLOR_0");
        base = g.vmul(base, c);
        alpha = g.mul(alpha, ca);
    }
    let (mut metallic, mut roughness) = (Float::Const(m.metallic), Float::Const(m.roughness));
    if let Some(t) = &m.metallic_roughness_texture
        && let Some(x) = s.sample(g, t, Reading::Data, at)?
    {
        let [_, green, blue] = g.separate(x.color);
        roughness = g.mul(roughness, green);
        metallic = g.mul(metallic, blue);
    }
    let mut normal = None;
    if let Some(t) = &m.normal_texture
        && let Some(x) = s.sample(g, t, Reading::Data, at)?
    {
        let k = m.normal_scale;
        let c = if k == 1.0 {
            x.color
        } else {
            let bias = 0.5 - 0.5 * k;
            g.vector_math(
                "MULTIPLY_ADD",
                &[x.color, Vec3::Const([k, k, 1.0]), Vec3::Const([bias, bias, 0.0])],
            )
        };
        let set = at.texcoord.unwrap_or(t.texcoord);
        let n = g.node(
            "ShaderNodeNormalMap",
            &[
                ("space", json!("TANGENT")),
                ("uv_map", json!(Sampler::uv_map(set))),
            ],
        );
        g.float_in(n, "Strength", Float::Const(1.0));
        g.color_in(n, "Color", c);
        normal = Some(Socket {
            node: n,
            output: "Normal",
        });
        s.counts.normal_maps_mikktspace += 1;
    }
    if m.occlusion_texture.is_some() {
        s.counts.occlusion_unwritten += 1;
    }
    let mut emission = Vec3::Const(m.emissive);
    if let Some(t) = &m.emissive_texture
        && let Some(x) = s.sample(g, t, Reading::Srgb, at)?
    {
        emission = g.vmul(emission, x.color);
    }
    let (mut specular_level, mut specular_tint) = (Float::Const(0.5), Vec3::Const([1.0; 3]));
    if let Some(sp) = &m.specular {
        specular_level = Float::Const(0.5 * sp.factor);
        if let Some(t) = &sp.texture
            && let Some(x) = s.sample(g, t, Reading::Data, at)?
        {
            specular_level = g.mul(specular_level, x.alpha);
        }
        specular_tint = Vec3::Const(sp.color);
        if let Some(t) = &sp.color_texture
            && let Some(x) = s.sample(g, t, Reading::Srgb, at)?
        {
            specular_tint = g.vmul(specular_tint, x.color);
        }
        if sp.factor != 1.0 || sp.texture.is_some() {
            s.counts.specular_f90_unexpressed += 1;
        }
    }
    Ok(Surface {
        base,
        alpha,
        metallic,
        roughness,
        normal,
        emission,
        emission_strength: Float::Const(m.emissive_strength),
        specular_level,
        specular_tint,
    })
}

/// The Blender material drawing `surface` with the host material `host`'s alpha mode, sidedness and shading: its node
/// tree ends in the material output and writes the passes.
pub fn finish(g: &mut Graph, surface: &Surface, host: &PbrMaterial) -> Settings {
    let alpha = match host.alpha_mode {
        AlphaMode::Opaque => Float::Const(1.0),
        AlphaMode::Mask { cutoff } => g.at_least(surface.alpha, Float::Const(cutoff)),
        AlphaMode::Blend => surface.alpha,
    };
    let shaded = match host.shading {
        Shading::Lit => principled(g, surface, alpha),
        Shading::Unlit => unlit(g, surface.base, alpha),
    };
    let mut coverage = g.at_least(alpha, Float::Const(PASS_ALPHA_THRESHOLD));
    let drawn = if host.double_sided {
        shaded
    } else {
        let back = g.input_node("ShaderNodeNewGeometry", "Backfacing");
        let front = g.sub(Float::Const(1.0), Float::Out(back));
        coverage = g.mul(coverage, front);
        let clear = g.shader(
            "ShaderNodeBsdfTransparent",
            "BSDF",
            &[],
            &[("Color", Vec3::Const([1.0; 3]))],
        );
        g.mix_shader(Float::Out(back), shaded, clear)
    };
    g.output(drawn);
    let base = g.vscale(surface.base, coverage);
    g.aov_color(aov::BASE_COLOR, base);
    let emitted = match host.shading {
        Shading::Lit => g.vscale(surface.emission, surface.emission_strength),
        // an unlit surface's colour is what it shows, not light it emits
        Shading::Unlit => Vec3::Const([0.0; 3]),
    };
    let emitted = g.vscale(emitted, coverage);
    g.aov_color(aov::EMISSION, emitted);
    g.aov_value(aov::COVERAGE, coverage);
    Settings {
        surface_render_method: if host.alpha_mode == AlphaMode::Blend {
            "BLENDED"
        } else {
            "DITHERED"
        },
        use_backface_culling: !host.double_sided,
    }
}

/// The Principled BSDF of `s` with `alpha`.
fn principled(g: &mut Graph, s: &Surface, alpha: Float) -> Socket {
    let n = g.node("ShaderNodeBsdfPrincipled", &[]);
    g.color_in(n, "Base Color", s.base);
    g.float_in(n, "Metallic", s.metallic);
    g.float_in(n, "Roughness", s.roughness);
    g.float_in(n, "IOR", Float::Const(1.5));
    g.float_in(n, "Alpha", alpha);
    if let Some(normal) = s.normal {
        g.link(normal, n, "Normal");
    }
    g.float_in(n, "Specular IOR Level", s.specular_level);
    g.color_in(n, "Specular Tint", s.specular_tint);
    g.color_in(n, "Emission Color", s.emission);
    g.float_in(n, "Emission Strength", s.emission_strength);
    Socket {
        node: n,
        output: "BSDF",
    }
}

/// An unlit surface of colour `base` with `alpha` (see the module docs).
fn unlit(g: &mut Graph, base: Vec3, alpha: Float) -> Socket {
    let camera = g.input_node("ShaderNodeLightPath", "Is Camera Ray");
    let emission = g.shader(
        "ShaderNodeEmission",
        "Emission",
        &[("Strength", Float::Const(1.0))],
        &[("Color", base)],
    );
    let black = g.shader(
        "ShaderNodeBsdfDiffuse",
        "BSDF",
        &[("Roughness", Float::Const(0.0))],
        &[("Color", Vec3::Const([0.0; 3]))],
    );
    let surface = g.mix_shader(Float::Out(camera), black, emission);
    if alpha == Float::Const(1.0) {
        return surface;
    }
    let clear = g.shader(
        "ShaderNodeBsdfTransparent",
        "BSDF",
        &[],
        &[("Color", Vec3::Const([1.0; 3]))],
    );
    g.mix_shader(alpha, clear, surface)
}

/// The graph of `pbr.material` `m`, drawn with `COLOR_0` where `color0`.
pub fn graph(s: &mut Sampler, m: &PbrMaterial, color0: bool) -> Result<(Graph, Settings)> {
    let mut g = Graph::new();
    let surface = surface(s, &mut g, m, Coordinates::default(), color0)?;
    let settings = finish(&mut g, &surface, m);
    Ok((g, settings))
}
