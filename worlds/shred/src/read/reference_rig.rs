//! Reference rigs as entities: a world of the rigs of explicitly supplied animation bundles, each in its authored rig
//! space with the source frame it is asked in.
use crate::{Result, components::ReferenceRig};
use primase_core::{World, frame::SourceFrame};
use rme_formats::reference_rig::{self, Archive, Report};

pub fn read_reference_rigs(archives: &[Archive<'_>], frame: SourceFrame) -> Result<(World, Report)> {
    let mut world = World::new();
    let mut report = Report::default();
    for archive in archives {
        let bundle = reference_rig::read_archive(archive)?;
        append(&mut world, &mut report, bundle, frame);
    }
    Ok((world, report))
}

/// Reads one caller-selected decoded animation container; also useful when the caller owns archive I/O.
pub fn append_bundle(
    world: &mut World,
    report: &mut Report,
    pak: &str,
    entry: &[u8],
    bytes: &[u8],
    frame: SourceFrame,
) -> Result<()> {
    // The entire bundle is decoded and validated before the caller's World or report change.
    let bundle = reference_rig::read_bundle(pak, entry, bytes)?;
    append(world, report, bundle, frame);
    Ok(())
}

fn append(world: &mut World, report: &mut Report, bundle: reference_rig::Bundle, frame: SourceFrame) {
    report.add(&bundle);
    for record in bundle.rigs {
        let entity = world.spawn();
        world.insert(entity, ReferenceRig { frame, record });
    }
}
