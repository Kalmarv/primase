//! Explicit environment selection, followed by reader loading and core probe lowering.
use crate::sekiro::environment_data::{
    EnvironmentCatalog, EnvironmentGroup, EnvironmentLoadRequest, EnvironmentPayloads, EnvironmentRegion,
    EnvironmentSelection,
};
use fromsoft_formats::envmap::Tier;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
mod graph;
use primase_core::{
    Component, ContentNote, EnvironmentProbeBlend, IrradianceVolume, Name, ProbeMipPolicy, ProbeState,
    ReflectionProbeRegion, Resource, TextureLayers, Transform, World,
};

#[derive(Clone, Copy, Debug)]
pub struct SelectEldenRingEnvironment(EnvironmentSelection);
impl SelectEldenRingEnvironment {
    fn from_given(g: &Given) -> Result<Self, String> {
        let selection = EnvironmentSelection {
            state_a: g
                .required("state-a")
                .parse()
                .map_err(|_| "state-a must be a package number0..99")?,
            state_b: g
                .required("state-b")
                .parse()
                .map_err(|_| "state-b must be a package number0..99")?,
            factor: g
                .required("factor")
                .parse()
                .map_err(|_| "factor must be a finite number0..1")?,
            tier: match g.required("tier") {
                "high" => Tier::High,
                "middle" => Tier::Middle,
                "low" => Tier::Low,
                _ => return Err("tier must be high, middle or low".into()),
            },
        };
        selection.verify_values()?;
        Ok(Self(selection))
    }
}
pub const SELECT_ELDEN_RING_ENVIRONMENT: PassSpec = PassSpec {
    name: "select-fromsoft-environment",
    options: &[
        OptionSpec::required("state-a", "5", "explicit authored environment state A"),
        OptionSpec::required("state-b", "6", "explicit authored environment state B"),
        OptionSpec::required(
            "factor",
            "0.35333335",
            "explicit finite A-to-B blend weight0..1; clock selector untraced",
        ),
        OptionSpec::required(
            "tier",
            "high",
            "explicit authored quality tier: high, middle or low",
        ),
    ],
    make: |g| Ok(Box::new(SelectEldenRingEnvironment::from_given(g)?)),
};
impl Pass for SelectEldenRingEnvironment {
    fn name(&self) -> &'static str {
        "select-fromsoft-environment"
    }
    fn options(&self) -> Options {
        Options::from([
            ("state-a".into(), self.0.state_a.to_string()),
            ("state-b".into(), self.0.state_b.to_string()),
            ("factor".into(), self.0.factor.to_string()),
            ("tier".into(), self.0.tier.word().into()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[EnvironmentCatalog::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[EnvironmentSelection::NAME, EnvironmentLoadRequest::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[EnvironmentSelection::NAME, EnvironmentLoadRequest::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let catalog = world
            .resource::<EnvironmentCatalog>()
            .ok_or("no authored environment catalog")?;
        let mut packages = Vec::new();
        for state in [self.0.state_a, self.0.state_b] {
            let matching: Vec<_> = catalog
                .packages
                .iter()
                .filter(|p| p.package.state == state && p.package.tier == self.0.tier)
                .map(|p| p.package.clone())
                .collect();
            if matching.is_empty() {
                return Err(format!(
                    "environment state {state:02} tier {} has no installed package",
                    self.0.tier.word()
                ));
            }
            for (_, region) in world.iter::<EnvironmentRegion>() {
                if !matching
                    .iter()
                    .any(|p| region.available_packages.contains(&p.path))
                {
                    return Err(format!(
                        "environment box {} group{:04} has no installed payload in state{state:02}/{}",
                        region.region.name,
                        region.group,
                        self.0.tier.word()
                    ));
                }
            }
            for package in matching {
                if !packages.contains(&package) {
                    packages.push(package);
                }
            }
        }
        log.count(
            "environment packages explicitly requested for loading",
            packages.len() as u64,
        );
        world.insert_resource(self.0);
        world.insert_resource(EnvironmentLoadRequest(packages));
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LowerEldenRingEnvironmentProbes;
pub const LOWER_ELDEN_RING_ENVIRONMENT_PROBES: PassSpec = PassSpec {
    name: "lower-fromsoft-environment-probes",
    options: &[],
    make: |_| Ok(Box::new(LowerEldenRingEnvironmentProbes)),
};
fn group(payloads: &EnvironmentPayloads, state: u8, tier: Tier, id: u32) -> Result<EnvironmentGroup, String> {
    let mut found = EnvironmentGroup::default();
    for payload in payloads
        .0
        .iter()
        .filter(|p| p.package.state == state && p.package.tier == tier)
    {
        let Some(other) = payload.groups.get(&id) else {
            continue;
        };
        for (a, b) in found
            .coefficients
            .iter_mut()
            .chain(std::iter::once(&mut found.cube))
            .zip(other.coefficients.iter().chain(std::iter::once(&other.cube)))
        {
            if let Some(e) = b
                && a.replace(*e).is_some()
            {
                return Err(format!(
                    "duplicate environment group {id:04} channel across packages"
                ));
            }
        }
    }
    if found.cube.is_none() || found.coefficients.iter().any(Option::is_none) {
        return Err(format!(
            "computed group {id:04} has no installed complete payload in state{state:02}/{}",
            tier.word()
        ));
    }
    Ok(found)
}
fn note(world: &mut World, key: &str, reason: String, unsupported: bool) {
    let existing = world
        .iter::<ContentNote>()
        .find(|(_, n)| n.key == key)
        .map(|(e, _)| e);
    let entity = existing.unwrap_or_else(|| world.spawn());
    world.insert(
        entity,
        ContentNote {
            key: key.into(),
            reason,
            unsupported,
            at: None,
        },
    );
}
impl Pass for LowerEldenRingEnvironmentProbes {
    fn name(&self) -> &'static str {
        "lower-fromsoft-environment-probes"
    }
    fn requires(&self) -> &[&'static str] {
        &[
            EnvironmentRegion::NAME,
            EnvironmentSelection::NAME,
            EnvironmentPayloads::NAME,
        ]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            ReflectionProbeRegion::NAME,
            IrradianceVolume::NAME,
            Transform::NAME,
            Name::NAME,
            ContentNote::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &[ContentNote::NAME, ReflectionProbeRegion::NAME, Transform::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "caller-stated environment states and tier; executable clock/quality selector not traced; one captured Stormveil box remains unassociated"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let selection = *world
            .resource::<EnvironmentSelection>()
            .ok_or("no explicit environment selection")?;
        let payloads = world
            .resource::<EnvironmentPayloads>()
            .ok_or("environment packages have not been loaded")?;
        let regions: Vec<_> = world
            .iter::<EnvironmentRegion>()
            .map(|(e, r)| {
                Ok((
                    e,
                    r.clone(),
                    [
                        group(payloads, selection.state_a, selection.tier, r.group)?,
                        group(payloads, selection.state_b, selection.tier, r.group)?,
                    ],
                ))
            })
            .collect::<Result<_, String>>()?;
        let stormveil = regions.iter().any(|(_, r, _)| r.area == "m10_00_00_00");
        for (entity, r, groups) in regions {
            let fromsoft_formats::msb::RegionShape::Box { width, height, depth } = r.region.shape else {
                return Err("environment effect region is not a box".into());
            };
            let half_extents = [width / 2., height / 2., depth / 2.];
            let mut cols = fromsoft_formats::msb::euler_matrix(
                r.region.position,
                r.region.rotation_degrees,
                half_extents,
                0,
            )
            .map_err(|e| e.to_string())?;
            let centre_offset = cols[1];
            for (position, offset) in cols[3].iter_mut().zip(centre_offset) {
                *position += offset;
            }
            let mut transform = crate::pose::placed(cols);
            transform.cols[2] = transform.cols[2].map(|v| -v);
            if let Some(old) = world.get::<ReflectionProbeRegion>(entity).cloned() {
                for volume in [old.blend.state_a.diffuse_volume, old.blend.state_b.diffuse_volume]
                    .into_iter()
                    .flatten()
                {
                    world.despawn(volume);
                }
            }
            let mut make_state = |group: EnvironmentGroup, state: u8| -> Result<ProbeState, String> {
                let coefficients = group.coefficients.map(|v| v.expect("validated complete group"));
                let cube = group.cube.expect("validated complete group");
                let mips = world
                    .get::<TextureLayers>(cube)
                    .ok_or("environment cube payload is absent")?
                    .levels
                    .len();
                let diffuse = world.spawn();
                world.insert(
                    diffuse,
                    Name(format!("{}:{}:diffuse:{state:02}", r.area, r.region.name)),
                );
                world.insert(diffuse, transform);
                world.insert(
                    diffuse,
                    IrradianceVolume {
                        coefficients,
                        basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., -1.]],
                        sampling_from_region: Transform::IDENTITY,
                        normal_offset: true,
                        half_extents,
                        fade_distance: r.fields.fade_distance,
                        diffuse_scale: r.fields.diffuse_scale,
                        priority: r.fields.priority,
                    },
                );
                Ok(ProbeState {
                    cube,
                    diffuse_volume: Some(diffuse),
                    intensity: r.fields.specular_scale,
                    mip_policy: ProbeMipPolicy::LinearRoughness {
                        minimum: 0.,
                        maximum: mips.saturating_sub(1) as f32,
                    },
                })
            };
            let a = make_state(groups[0], selection.state_a)?;
            let b = make_state(groups[1], selection.state_b)?;
            world.insert(entity, transform);
            world.insert(
                entity,
                ReflectionProbeRegion {
                    blend: EnvironmentProbeBlend {
                        state_a: a,
                        state_b: b,
                        factor: selection.factor,
                    },
                    half_extents,
                    fade_distance: r.fields.fade_distance,
                    priority: r.fields.priority,
                    reflection_origin: r.reflection_origin,
                    parallax_from_region: None,
                },
            );
            log.count("environment boxes lowered", 1);
        }
        note(world, "environment-selection", selection.notice(), false);
        note(world,"environment-group-rule","environment box-to-group association inferred from 55 capture-verified Elden Ring 2.7.2.0 joins: type-data +0x0a signed override, default numeric Env_Box name; the executable producer is not traced".into(),false);
        if stormveil {
            note(world,"environment-unresolved-capture","1 captured environment box (frame 6993 sequence 1897) has no installed association and is not reconstructed".into(),true);
        }
        log.approximate(&selection.notice(), 1);
        Ok(())
    }
}

/// Bind already lowered core probes into the prepared native lighting graph.
#[derive(Clone, Copy, Debug)]
pub struct BindFromSoftEnvironmentProbes;
pub const BIND_FROMSOFT_ENVIRONMENT_PROBES: PassSpec = PassSpec {
    name: "bind-fromsoft-environment-probes",
    options: &[],
    make: |_| Ok(Box::new(BindFromSoftEnvironmentProbes)),
};
impl Pass for BindFromSoftEnvironmentProbes {
    fn name(&self) -> &'static str {
        "bind-fromsoft-environment-probes"
    }
    fn requires(&self) -> &[&'static str] {
        &[
            primase_core::render_graph::RenderGraph::NAME,
            crate::sekiro::PreparedLighting::NAME,
            ReflectionProbeRegion::NAME,
            EnvironmentSelection::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &[primase_core::render_graph::RenderGraph::NAME]
    }
    fn approximates(&self) -> &'static str {
        "direct two-cube blending for all environment regions; native partial preblend schedule and region exclusion masks not reproduced; reflection luminance correction omitted"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        use primase_core::render_graph::{LightingFrame, LightingPort, RenderGraph};
        use primase_core::shader::NormalEncoding;
        let mut render = world.resource::<RenderGraph>().ok_or("no source graph")?.clone();
        let index = graph::select(&mut render, world)?.ok_or("no core environment probe regions")?;
        let pipeline = render
            .lighting
            .as_mut()
            .ok_or("base lighting pipeline has not been prepared")?;
        pipeline.passes.push(index);
        pipeline.passes.sort_unstable();
        pipeline.passes.dedup();
        let normal = pipeline
            .imports
            .iter()
            .find_map(|(p, v)| (*p == LightingPort::GBuffer(0)).then_some(*v))
            .ok_or("environment has no world-normal import")?;
        pipeline.frame = Some(LightingFrame {
            world_from_lighting: [[1., 0., 0.], [0., 1., 0.], [0., 0., -1.]],
            normal_inputs: vec![(
                normal,
                NormalEncoding::XyzBiased {
                    bias_bits: 0x3f00_2008,
                },
            )],
        });
        for notice in &mut pipeline.notices {
            if notice == "native environment, ambient occlusion and shadow producer stages remain unexecuted"
            {
                *notice = "native ambient occlusion and shadow producer stages remain unexecuted".into();
            }
        }
        for notice in [graph::DIRECT, graph::LUMINANCE, graph::MASKS] {
            if !pipeline.notices.iter().any(|n| n == notice) {
                pipeline.notices.push(notice.into());
            }
            log.approximate(notice, 1);
        }
        render.check()?;
        world.insert_resource(render);
        log.count(
            "environment lighting regions bound",
            world.iter::<ReflectionProbeRegion>().count() as u64,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    fn manager() -> PassManager {
        let mut registry = PassRegistry::new();
        super::super::register(&mut registry);
        PassManager::new(registry).with_pass_checks(true)
    }
    fn catalog() -> EnvironmentCatalog {
        EnvironmentCatalog {
            area: "m10_00_00_00".into(),
            packages: [5, 6]
                .into_iter()
                .flat_map(|state| {
                    [Tier::High, Tier::Middle, Tier::Low].map(move |tier| {
                        fromsoft_formats::envmap::PackageInventory {
                            package: fromsoft_formats::envmap::Package {
                                area: "m10_00_00_00".into(),
                                state,
                                tier,
                                part: 0,
                                path: format!("synthetic/state{state}/{}", tier.word()),
                            },
                            members: Vec::new(),
                            member_bytes: 0,
                        }
                    })
                })
                .collect(),
        }
    }

    #[test]
    fn environment_selection_requests_payloads_without_mutating_authored_states_or_lowering() {
        let manager = manager();
        let mut world = World::new();
        let authored = catalog();
        world.insert_resource(authored.clone());
        let pipeline = manager
            .registry
            .parse("select-fromsoft-environment{state-a=5,state-b=6,factor=0.35333335,tier=high}")
            .unwrap();
        manager
            .run(&mut world, &pipeline, &mut RunReport::default())
            .unwrap();
        assert_eq!(world.resource::<EnvironmentCatalog>(), Some(&authored));
        let request = world.resource::<EnvironmentLoadRequest>().unwrap();
        assert_eq!(request.0.len(), 2);
        assert!(request.0.iter().all(|p| p.tier == Tier::High));
        assert_eq!(
            world.resource::<EnvironmentSelection>().unwrap().factor.to_bits(),
            0x3eb4e81c
        );
        assert!(world.resource::<EnvironmentPayloads>().is_none());
        assert_eq!(world.iter::<TextureLayers>().count(), 0);
        assert_eq!(world.iter::<ReflectionProbeRegion>().count(), 0);
        assert_eq!(world.iter::<ContentNote>().count(), 0);
    }
    #[test]
    fn environment_choices_have_no_reader_or_pass_default_and_lowering_requires_loaded_payloads() {
        let manager = manager();
        for text in [
            "select-fromsoft-environment",
            "select-fromsoft-environment{state-a=5,state-b=6,factor=0.5}",
            "select-fromsoft-environment{state-a=5,state-b=6,factor=NaN,tier=high}",
        ] {
            assert!(manager.registry.parse(text).is_err(), "{text}");
        }
        let p = manager
            .registry
            .parse("lower-fromsoft-environment-probes")
            .unwrap();
        let held = [EnvironmentRegion::NAME.into(), EnvironmentSelection::NAME.into()]
            .into_iter()
            .collect();
        assert_eq!(
            p.unfed(&held).passes,
            vec![(
                "lower-fromsoft-environment-probes".into(),
                vec![EnvironmentPayloads::NAME]
            )]
        );
        assert!(
            group(&EnvironmentPayloads::default(), 5, Tier::High, 9999)
                .unwrap_err()
                .contains("computed group 9999 has no installed complete payload")
        );
    }

    #[test]
    fn environment_graph_binding_requires_the_registered_base_pass_marker() {
        use primase_core::render_graph::RenderGraph;
        let manager = manager();
        let mut world = World::new();
        world.insert_resource(RenderGraph::default());
        let pipeline = manager
            .registry
            .parse("bind-fromsoft-environment-probes")
            .unwrap();
        let error = manager
            .run(&mut world, &pipeline, &mut RunReport::default())
            .unwrap_err();
        assert!(
            error.to_string().contains(crate::sekiro::PreparedLighting::NAME),
            "{error}"
        );
        assert!(world.resource::<RenderGraph>().unwrap().lighting.is_none());
    }
}
