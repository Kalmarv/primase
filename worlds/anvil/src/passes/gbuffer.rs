//! Decode the resolver's output roles through core's one surface reading. A role's ordinal is not a target register:
//! several roles can name different lanes of the same target, and the signature can list targets out of order.
use iridia_interface::Interface;
use primase_core::shader::OutputRole;
use primase_core::shader::{surface::Place, surface::Surface};

use super::resolve_shaders::{output_bindings, target_of};

pub(super) const OCCLUSION_APPROXIMATION: &str = "G-buffer occlusion attenuates all external light; core.pbr carries ambient occlusion only, so direct-light occlusion is not represented";

pub(super) struct Reading {
    surface: Surface,
    targets: Vec<usize>,
    required: Vec<(usize, u8)>,
    specular_occlusion: Option<Place>,
    translucency: Option<Place>,
}

pub(super) struct Texel {
    pub base: [f32; 3],
    pub normal: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
    pub occlusion: f32,
    pub emission: [f32; 3],
    pub specular_occlusion: f32,
    pub translucency: f32,
}

impl Reading {
    pub fn new(interface: &Interface, class: u8) -> Result<Self, String> {
        if !matches!(class, 1 | 3..=5) {
            return Err(format!("shading class {class}: no map surface output contract"));
        }
        let bindings = output_bindings(interface, Some(class), true);
        let surface = Surface::read(bindings.iter().map(|b| (b.lanes, &b.role)));
        if !surface.refused.is_empty() {
            return Err(surface.refused.join("; "));
        }
        if surface.base.is_none() || surface.normal.is_none() || surface.roughness.is_none() {
            return Err("G-buffer lacks an albedo, normal or roughness output role".into());
        }
        if surface.occlusion.is_none()
            || (class == 1 && (surface.metallic.is_none() || surface.emission.is_none()))
        {
            return Err(format!(
                "G-buffer signature omits a surface term required by shading class {class}"
            ));
        }
        let targets = bindings
            .iter()
            .map(|b| {
                interface
                    .outputs
                    .iter()
                    .find(|v| v.label == b.label)
                    .and_then(target_of)
                    .and_then(|t| usize::try_from(t).ok())
                    .ok_or_else(|| format!("output {:?} has no target register", b.label))
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Unknown lanes, velocity and unused target 3 of layout B do not supply a material term.
        let required = bindings
            .iter()
            .zip(&targets)
            .filter_map(|(b, &target)| {
                (!matches!(b.role, OutputRole::Unknown { .. } | OutputRole::MotionVector))
                    .then_some((target, b.lanes))
            })
            .collect();
        let place = |role| {
            bindings.iter().enumerate().find_map(|(output, b)| {
                (b.role == role).then_some(Place {
                    output,
                    lanes: b.lanes,
                })
            })
        };
        Ok(Self {
            surface,
            targets,
            required,
            specular_occlusion: place(OutputRole::SpecularOcclusion),
            translucency: place(OutputRole::Translucency),
        })
    }

    pub fn read(&self, output: impl Fn(usize) -> Option<([f32; 4], u8)>) -> Result<Texel, String> {
        for &(target, lanes) in &self.required {
            let (values, written) =
                output(target).ok_or_else(|| format!("G-buffer target {target} is absent"))?;
            if written & lanes != lanes {
                return Err(format!(
                    "G-buffer target {target} leaves surface lanes {lanes:#06b} unwritten"
                ));
            }
            if values
                .iter()
                .enumerate()
                .any(|(k, v)| lanes & (1 << k) != 0 && !v.is_finite())
            {
                return Err(format!("G-buffer target {target} has a non-finite surface value"));
            }
        }
        let at = |p: Place| output(self.targets[p.output]).map(|(v, _)| v).unwrap_or([0.0; 4]);
        let base_term = self.surface.base.ok_or("G-buffer has no albedo")?;
        let base = base_term.colour(at(base_term.place()));
        let normal = self.surface.normal.ok_or("G-buffer has no normal")?;
        let roughness = self.surface.roughness.ok_or("G-buffer has no roughness")?;
        // The two terms not representable in core.pbr retain their exact values for named approximation/refusal.
        Ok(Texel {
            base,
            normal: normal
                .decode(at(normal.place))
                .ok_or("G-buffer normal has no direction")?,
            roughness: roughness.decode(at(roughness.term().place)),
            metallic: self.surface.metallic.map_or(0.0, |t| t.decode(at(t.place), 0)),
            occlusion: self.surface.occlusion.map_or(1.0, |t| t.decode(at(t.place), 0)),
            emission: self
                .surface
                .emission
                .map_or([0.0; 3], |t| t.decode(at(t.term().place), base)),
            specular_occlusion: self.specular_occlusion.map_or(1.0, |p| p.lane(at(p), 0)),
            translucency: self.translucency.map_or(0.0, |p| p.lane(at(p), 0)),
        })
    }
}

#[cfg(test)]
pub(super) mod fixture {
    use iridia_exec::{Execution, Load, PixelShader, ProgramShader, Sample, Sampler, Store};

    /// Existing installed template fixtures, as used by tests/shader_oracle.rs. No shader binary is generated.
    pub fn program(name: &str) -> Option<Vec<u8>> {
        use anvil_formats::{
            archive::{Archive, At},
            template,
        };
        let dir = crate::inputs::install(crate::inputs::Game::Riders)?;
        let order = [
            "DataPC_MAD_USNAT_patch_01.forge",
            "DataPC_MAD_USNAT.forge",
            "DataPC_patch_01.forge",
            "DataPC.forge",
        ];
        let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("fixture archive");
        for forge in 0..order.len() {
            let entry = archive
                .forge(forge)
                .expect("forge")
                .names
                .iter()
                .position(|n| n.type_id == template::MATERIAL_TEMPLATE && n.name == name);
            if let Some(entry) = entry {
                let body = archive.resource(At { forge, entry }).expect("template").body;
                return Some(
                    template::permutations(&body)
                        .expect("permutations")
                        .into_iter()
                        .find(|p| p.stage == template::Stage::Pixel && p.key == 0x40_0000)
                        .expect("opaque fixture permutation")
                        .container
                        .to_vec(),
                );
            }
        }
        panic!("existing fixture template {name} is absent");
    }

    struct Uniform<'a>(&'a [(u32, [f32; 4])]);
    impl Sampler for Uniform<'_> {
        fn sample(&mut self, r: Sample) -> Result<[f32; 4], String> {
            self.0
                .iter()
                .find(|(t, _)| *t == r.texture)
                .map(|(_, v)| *v)
                .ok_or_else(|| format!("fixture did not establish t{}", r.texture))
        }
        fn load(&mut self, r: Load) -> Result<[u32; 4], String> {
            self.0
                .iter()
                .find(|(t, _)| *t == r.texture)
                .map(|(_, v)| v.map(f32::to_bits))
                .ok_or_else(|| format!("fixture did not establish load t{}", r.texture))
        }
        fn structured(&mut self, slot: u32, _element: u32, _offset: u32) -> Result<u32, String> {
            Err(format!("unexpected structured fixture input t{slot}"))
        }
        fn store(&mut self, _r: Store) -> Result<(), String> {
            Err("unexpected fixture store".into())
        }
    }

    pub fn execute(shader: &ProgramShader, cb3: &[[f32; 4]], textures: &[(u32, [f32; 4])]) -> Execution {
        execute_with(shader, cb3, &mut Uniform(textures))
    }

    pub fn execute_with(shader: &ProgramShader, cb3: &[[f32; 4]], sampler: &mut dyn Sampler) -> Execution {
        let mut inputs = vec![[0u32; 4]; shader.input_count()];
        for v in &shader.interface().inputs {
            let register = v.provenance.d3d.expect("D3D fixture").register as usize;
            let value = match (v.label.name.as_str(), v.label.index) {
                ("SV_Position", _) => [0.5, 0.5, 0.5, 1.0],
                ("SV_IsFrontFace", _) => {
                    inputs[register] = [u32::MAX; 4];
                    continue;
                }
                ("COLOR", _) => [1.0; 4],
                ("TEXCOORD", 0) => [0.25, 0.25, 0.0, 0.0],
                ("TEXCOORD", 1) => [1.0, 0.0, 0.0, 0.0],
                ("TEXCOORD", 2) => [0.0, 0.0, 1.0, 1.0],
                ("TEXCOORD", 3) => [0.0, 0.0, 0.0, 1.0],
                _ if !v.used => continue,
                _ => panic!("unestablished fixture varying {:?}", v.label),
            };
            inputs[register] = value.map(f32::to_bits);
        }
        let buffers: Vec<_> = shader
            .constant_buffers()
            .iter()
            .map(|&(slot, size)| {
                (
                    slot,
                    (0..size as usize)
                        .map(|r| {
                            if slot == 3 {
                                cb3.get(r).copied().unwrap_or([0.0; 4]).map(f32::to_bits)
                            } else {
                                [0; 4]
                            }
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let views: Vec<_> = buffers.iter().map(|(s, b)| (*s, b.as_slice())).collect();
        let out = shader
            .execute(&inputs, &views, sampler)
            .expect("existing program executes");
        assert!(!out.discarded);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::shader::NormalEncoding;

    #[test]
    fn surface_places_follow_target_labels_not_signature_or_role_ordinals() {
        let mut program = iridia_dxbc::lift(include_bytes!("../../tests/fixtures/modes.dxbc")).unwrap();
        program.interface.outputs.reverse();
        let n = NormalEncoding::encode_octahedral_diamond([0.0, 0.0, 1.0], 1).map(|v| f32::from(v) / 255.0);
        let outputs = [
            [0.25, 0.5, 0.75, 0.6],
            [n[0], n[1], n[2], 0.3],
            [0.0, 0.0, 8.0, 0.0],
            [0.8, 0.0, 1.0, 0.0],
        ];
        let reading = Reading::new(&program.interface, 1).unwrap();
        let t = reading
            .read(|target| outputs.get(target).copied().map(|v| (v, 15)))
            .unwrap();
        assert_eq!(t.base, [0.25, 0.5, 0.75]);
        assert_eq!(t.emission, [2.0, 4.0, 6.0]);
        assert_eq!((t.roughness, t.metallic, t.occlusion), (0.6, 0.3, 0.8));
        assert!(t.normal[2] > 0.999);
        assert!(
            reading
                .read(|target| outputs
                    .get(target)
                    .copied()
                    .map(|v| (v, if target == 2 { 3 } else { 15 })))
                .is_err()
        );
        assert!(
            reading
                .read(|target| outputs.get(target).copied().map(|mut v| {
                    if target == 2 {
                        v[2] = f32::NAN;
                    }
                    (v, 15)
                }))
                .is_err()
        );
        let layout_b = Reading::new(&program.interface, 5).unwrap();
        let t = layout_b
            .read(|target| {
                outputs
                    .get(target)
                    .copied()
                    .map(|v| (v, if target == 3 { 0 } else { 15 }))
            })
            .unwrap();
        assert_eq!((t.metallic, t.occlusion, t.emission), (0.0, 0.3, [0.0; 3]));
    }
}
