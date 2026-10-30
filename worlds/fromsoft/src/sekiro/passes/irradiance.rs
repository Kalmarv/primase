//! Lower authored environment boxes; preserve source records and package states.
use crate::sekiro::IrradianceRegion;
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Component, IrradianceVolume, TextureLayers, Transform, World};

#[derive(Debug)]
pub struct LowerFromSoftIrradiance;
pub const LOWER_FROMSOFT_IRRADIANCE: PassSpec = PassSpec {
    name: "lower-fromsoft-irradiance",
    options: &[],
    make: |_| Ok(Box::new(LowerFromSoftIrradiance)),
};
impl Pass for LowerFromSoftIrradiance {
    fn name(&self) -> &'static str {
        "lower-fromsoft-irradiance"
    }
    fn requires(&self) -> &[&'static str] {
        &[IrradianceRegion::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[IrradianceVolume::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let boxes: Vec<_> = world
            .iter::<IrradianceRegion>()
            .map(|(e, r)| (e, r.clone()))
            .collect();
        for (e, r) in boxes {
            let grids: Vec<_> = r
                .coefficients
                .iter()
                .map(|texture| {
                    world
                        .get::<TextureLayers>(*texture)
                        .map(|t| (t.width, t.height, t.shape, t.format))
                        .ok_or("irradiance coefficient texture is absent")
                })
                .collect::<Result<_, _>>()?;
            if grids[1..].iter().any(|grid| *grid != grids[0]) {
                // Source packages contain mixed coefficient grids. The captured
                // normal offset uses one common sampling grid; its producer for
                // these exceptions is unresolved. Retain source bytes and refuse
                // this lowering rather than resize them or choose a channel.
                world.remove::<IrradianceVolume>(e);
                log.omit(
                    "irradiance coefficient grids differ; common normal-offset grid is unresolved",
                    1,
                );
                continue;
            }
            let fromsoft_formats::msb::RegionShape::Box { width, height, depth } = r.region.shape else {
                return Err("irradiance source region is not a box".into());
            };
            world.insert(
                e,
                IrradianceVolume {
                    coefficients: r.coefficients,
                    basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., -1.]],
                    sampling_from_region: Transform::IDENTITY,
                    normal_offset: true,
                    half_extents: [width / 2., height / 2., depth / 2.],
                    fade_distance: r.fade_distance,
                    diffuse_scale: r.diffuse_intensity,
                    priority: r.priority,
                },
            );
            log.count("irradiance boxes lowered", 1);
        }
        Ok(())
    }
}
