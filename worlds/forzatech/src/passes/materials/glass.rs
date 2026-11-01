//! # `glass`
//!
//! Evidence, over the 17 permutations of the family in the Tokyo track's `bin.zip`, 17 `GBufferLightScenario` and 17
//! forward `TrackLightScenario` pixel shaders (each render target channel followed back through the `CBMP` and `TXMP`
//! maps):
//! - the G-buffer shader writes the albedo (target 0 XYZ) `TintColor` (`_Standard`) or, with `BaseColorTexOnOff`
//!   (`_BCT*`), `BaseColour(uv0)`; the metalness is the literal 0; the roughness (target 1 W) is `Roughness`, or with
//!   `RoughnessTexOnOff` (`_RT`) `Roughness(uv0).r`; the normal (target 2) is the vertex normal, or with
//!   `NormalTexOnOff` (`_NT`) the standard decode of `Normal(uv0)` on the vertex tangent; both are then tilted per
//!   instance by `GlassNormalOffsetStrength` and `MaxPerturbAngle` (0 in every material of the box). The `_M`
//!   permutations (`Modulate`) change the albedo and the roughness by `ModulateTexture(uv2 * ModulateUVTiling)`;
//! - the surface is drawn by the forward `glassTrack` pass, which writes two targets: target 0 the reflected radiance
//!   (the fresnel term between `F0` and 1 times the lit sky) and its gloss, and target 1 the transmittance the
//!   background is multiplied by: `1 - k * c` per channel, where `c` is the colour above (`TintColor` or
//!   `BaseColour.rgb`) and `k = (0.4 * Alpha + 0.5) * (0.98 - a view-dependent term) * (1 - Cloudyness) * (1 - fog)`;
//!   the `_ATT` permutations (`AlphaTestTexOnOff`) drop the `Alpha` factor and discard where `BaseColour.a` is below
//!   `1e-4`: a cut-out, not a coverage.
//!
//! The blend state is in no shipped file (the shader containers carry none), but the two-target form is that of dual-source
//! blending, `dst = target0 + dst * target1`, as in the car glass group: the reflected radiance is added to the
//! background whatever the transmittance. `Cloudyness` scales `k` only, so at 1 the transmittance is 1 (a clear pane)
//! and the pane is still seen by its reflection; likewise a pane of dark `TintColor` is clear. That form is inferred
//! from the outputs, not observed in a state. glTF's blend cannot add a term, so the reflection is carried as coverage.
//!
//! The rule: `pbr.material` blended (`AlphaMode::Blend`), dielectric, of base colour `c` and the opacity
//! `a = max(t, r)`, `t = (ATT ? 1 : 0.4 * Alpha + 0.5) * 0.98 * (1 - Cloudyness) * luminance(c)` (the transmittance
//! `1 - k * c` per channel replaced by its luminance) and `r = F0 + (1 - F0) / 21` the cosine-weighted mean of Schlick's
//! fresnel over the hemisphere (`F0` is 0.04 when the material has none), baked into the alpha of the base colour
//! texture where `c` is one (zero where the cut-out discards, which drops the reflection too); roughness `Roughness`,
//! or the roughness texture's red; the normal map where `NormalTexOnOff`; and `F0` as the specular reflectance. A
//! material whose opacity `r` raised is counted as `reflection carried as coverage`. Left out, named: the per-channel
//! tint of the transmittance, the view-dependent term and the fog (the fresnel and distance of the forward pass), the
//! reflected radiance's colour (the base colour stays `c`), modulate (`ModulateTexture`), and the per-instance normal
//! tilt.
use super::{Binding, Derived, Lowering, Params, approximate, left_out, seen};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial, Specular, TextureRef};
use primase_core::{Entity, Name, TexRole, Texture, World};
use primase_image::{TextureMetadata, map_rgba8_levels};

/// The Rec. 601 luma weights the family's own shaders use for luminance.
fn luminance([r, g, b]: [f64; 3]) -> f64 {
    0.299 * r + 0.587 * g + 0.114 * b
}

impl Lowering {
    /// `texture` with its alpha made the glass's opacity: `max(k * luminance(rgb), reflect)`, 0 where `cut` and the
    /// texel's own alpha is below `1e-4`. Its colour is kept as stored. Made once per source, `k`, `reflect` and `cut`;
    /// `true` when `reflect` raised a texel.
    fn glass_opacity(
        &mut self,
        w: &mut World,
        source: Entity,
        k: f64,
        reflect: f64,
        cut: bool,
        log: &mut PassLog,
    ) -> Result<Option<(Entity, bool)>, String> {
        let key = vec![(k as f32).to_bits(), (reflect as f32).to_bits(), u32::from(cut)];
        if let Some(&e) = self.baked.get(&(source, key.clone(), None)) {
            return Ok(Some((e, self.reflected.contains(&e))));
        }
        let Some(t) = w.get::<Texture>(source) else {
            log.count("glass colour maps left out: no pixels", 1);
            return Ok(None);
        };
        let space = t.color_space;
        let mut raised = false;
        let texture = map_rgba8_levels(
            t,
            0..t.levels.len(),
            TextureMetadata {
                color_space: space,
                role: TexRole::Color,
                normal: None,
            },
            |texel| {
                let [r, g, b, a] = texel.rgba;
                let linear = [r, g, b].map(|x| seen(space, x));
                let opacity = if cut && f64::from(a) / 255.0 < 1e-4 {
                    0.0
                } else {
                    let transmission = k * luminance(linear);
                    raised |= transmission < reflect;
                    transmission.max(reflect).clamp(0.0, 1.0)
                };
                Ok([r, g, b, (opacity * 255.0).round() as u8])
            },
        )
        .map_err(|e| format!("making a glass colour map: {e}"))?;
        let name = w.get::<Name>(source).map(|n| n.0.clone()).unwrap_or_default();
        let e = w.spawn();
        w.insert(e, Name(format!("{name} (glass opacity)")));
        w.insert(e, texture);
        log.count("glass colour textures baked with their opacity", 1);
        self.baked.insert((source, key, None), e);
        if raised {
            self.reflected.insert(e);
        }
        Ok(Some((e, raised)))
    }

    /// The `glass` family's material.
    pub(super) fn glass(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<PbrMaterial, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        let mut pbr = PbrMaterial {
            metallic: 0.0,
            roughness: p.float("Roughness").unwrap_or(1.0).clamp(0.0, 1.0),
            alpha_mode: AlphaMode::Blend,
            ..PbrMaterial::default()
        };

        // the opacity factor shared by every texel and the constant colour
        let cloudy = f64::from(p.float("Cloudyness").unwrap_or(0.0)).clamp(0.0, 1.0);
        let attest = p.on("AlphaTestTexOnOff");
        let alpha_factor = if attest {
            1.0
        } else {
            0.4 * f64::from(p.float("Alpha").unwrap_or(0.0)) + 0.5
        };
        let k = alpha_factor * 0.98 * (1.0 - cloudy);

        // F0 is the reflectance at normal incidence; glTF's dielectric default is 0.04
        let f0 = p
            .float("F0")
            .or_else(|| p.vec4("F0").map(|c| c[0]))
            .map(|x| x.max(0.0));
        // the forward pass adds the reflected radiance whatever the transmission (dual-source blending, `dst = src0 + dst *
        // src1`), so a pane is never more clear than its reflectance: the cosine-weighted mean of Schlick's fresnel,
        // `F0 + (1 - F0) / 21`, is the coverage the reflection is carried as
        let f0_mean = f0
            .filter(|x| x.is_finite())
            .map_or(0.04, |x| f64::from(x.min(1.0)));
        let reflect = f0_mean + (1.0 - f0_mean) / 21.0;
        let mut reflected = false;

        match (p.on("BaseColorTexOnOff"), p.texture("BaseColour")) {
            (true, Binding::Texture(t)) => {
                if let Some((opacity, raised)) = self.glass_opacity(w, t, k, reflect, attest, log)? {
                    pbr.base_color_texture = Some(TextureRef::new(opacity));
                    reflected = raised;
                }
            }
            (true, Binding::Absent) => {
                approximate(
                    log,
                    e,
                    "base colour texture in no archive: the tint colour",
                    triangles,
                );
            }
            _ => {}
        }
        if pbr.base_color_texture.is_none() {
            let tint = p.vec4("TintColor").map_or([1.0; 3], |c| [c[0], c[1], c[2]]);
            let tint = tint.map(|x| x.max(0.0));
            let transmission = k * luminance(tint.map(f64::from));
            reflected = transmission < reflect;
            pbr.base_color = [
                tint[0],
                tint[1],
                tint[2],
                transmission.max(reflect).clamp(0.0, 1.0) as f32,
            ];
        }

        if p.on("RoughnessTexOnOff") {
            match p.texture("Roughness") {
                Binding::Texture(t) => {
                    if let Some(rough) = self.derive(w, t, Derived::RedRoughness(1.0_f32.to_bits()), log)? {
                        pbr.metallic_roughness_texture = Some(TextureRef::new(rough));
                        pbr.roughness = 1.0;
                    }
                }
                Binding::Absent | Binding::None => {
                    approximate(
                        log,
                        e,
                        "roughness texture in no archive: the constant roughness",
                        triangles,
                    );
                }
            }
        }
        if p.on("NormalTexOnOff") {
            match p.texture("Normal") {
                Binding::Texture(t) => {
                    if Self::normal_map(w, t, log) {
                        pbr.normal_texture = Some(TextureRef::new(t));
                    } else {
                        approximate(log, e, "normal map not lowered", triangles);
                    }
                }
                Binding::Absent | Binding::None => approximate(log, e, "no normal map", triangles),
            }
        }

        if let Some(f0) = f0.filter(|x| x.is_finite()) {
            pbr.specular = Some(if f0 <= 0.04 {
                Specular {
                    factor: f0 / 0.04,
                    ..Specular::default()
                }
            } else {
                Specular {
                    color: [f0 / 0.04; 3],
                    ..Specular::default()
                }
            });
        }

        if reflected {
            approximate(log, e, "reflection carried as coverage", triangles);
        }
        left_out(log, &p, e, "Modulate", "modulate texture left out", triangles);
        approximate(
            log,
            e,
            "per-channel tint of the transmittance (its luminance is the opacity), the forward pass's fog and the reflected radiance's colour left out",
            triangles,
        );
        Ok(pbr)
    }
}
