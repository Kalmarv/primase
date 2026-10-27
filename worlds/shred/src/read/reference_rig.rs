//! Explicitly supplied animation bundles. No rig variant is chosen and no render-skeleton join is inferred.
use super::pak::Pak;
use crate::{Result, components::ReferenceRig, invalid};
use primase_core::{World, frame::SourceFrame};

/// The caller selects the archive and the exact extensionless animation-container entry.
#[derive(Debug)]
pub struct Archive<'a> {
    pub label: &'a str,
    pub pak: &'a Pak,
    pub entry: &'a [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UninterpretedAsset {
    pub pak: String,
    pub entry: Vec<u8>,
    pub offset: usize,
    pub asset_type: u32,
    pub asset_id: u32,
    pub guid: [u8; 16],
    pub bytes: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub bundles: usize,
    pub bundles_without_rigs: usize,
    pub records: usize,
    pub rigs: usize,
    pub joints: usize,
    pub padding_bytes: usize,
    pub uninterpreted_assets: Vec<UninterpretedAsset>,
    /// Source rig space and its header transform are retained; core Skeleton/binding conversion is deferred.
    pub source_space_rigs: usize,
    /// Each rig retains its complete payload, including auxiliary data outside the supported reference fields.
    pub rigs_with_uninterpreted_fields: usize,
}

pub fn read_reference_rigs(archives: &[Archive<'_>], frame: SourceFrame) -> Result<(World, Report)> {
    let mut world = World::new();
    let mut report = Report::default();
    for archive in archives {
        let matches: Vec<_> = archive
            .pak
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| e.attributes != 1 && e.name == archive.entry)
            .collect();
        let [(index, _)] = matches.as_slice() else {
            return Err(invalid(
                0,
                format!(
                    "{}: requested animation entry must occur exactly once",
                    archive.label
                ),
            ));
        };
        let bytes = archive.pak.payload(*index)?;
        append_bundle(
            &mut world,
            &mut report,
            archive.label,
            archive.entry,
            &bytes,
            frame,
        )?;
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
    // Decode and validate the entire bundle before mutating the caller's World or report.
    let records = morpheme_formats::bundle_be32(bytes).map_err(|e| invalid(e.offset, e.detail))?;
    let mut rigs = Vec::new();
    let mut unsupported = Vec::new();
    let mut padding_bytes = 0;
    for record in &records {
        padding_bytes += record.padding.len();
        if record.asset_type == 1 {
            let origin = record
                .offset
                .checked_add(32)
                .ok_or_else(|| invalid(record.offset, "rig origin overflow"))?;
            let rig = morpheme_formats::rig_be32(record.payload, origin)
                .map_err(|e| invalid(e.offset, e.detail))?;
            let component = ReferenceRig {
                pak: pak.into(),
                entry: entry.to_vec(),
                record_offset: record.offset,
                asset_id: record.asset_id,
                guid: record.guid,
                header_word: record.header_word,
                frame,
                rig,
                source_payload: record.payload.to_vec(),
            };
            component.matrices().map_err(|e| invalid(origin, e))?;
            rigs.push(component);
        } else {
            unsupported.push(UninterpretedAsset {
                pak: pak.into(),
                entry: entry.to_vec(),
                offset: record.offset,
                asset_type: record.asset_type,
                asset_id: record.asset_id,
                guid: record.guid,
                bytes: record.payload.len(),
            });
        }
    }
    report.bundles += 1;
    report.bundles_without_rigs += usize::from(rigs.is_empty());
    report.records += records.len();
    report.padding_bytes += padding_bytes;
    report.uninterpreted_assets.extend(unsupported);
    for rig in rigs {
        report.rigs += 1;
        report.joints += rig.rig.joints.len();
        report.source_space_rigs += 1;
        report.rigs_with_uninterpreted_fields += 1;
        let entity = world.spawn();
        world.insert(entity, rig);
    }
    Ok(())
}
