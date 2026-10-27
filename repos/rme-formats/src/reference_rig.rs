//! Reference rigs of Morpheme animation containers, read from the entry the caller names. No rig variant is chosen and
//! no render-skeleton join is inferred.
use crate::pak::Pak;
use crate::{Affine, Result, invalid};
use morpheme_formats::Rig;

/// The caller selects the archive and the exact extensionless animation-container entry.
#[derive(Debug)]
pub struct Archive<'a> {
    pub label: &'a str,
    pub pak: &'a Pak,
    pub entry: &'a [u8],
}

/// A bundle record that is not a type 1 reference rig; only its identity and size are kept.
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

/// Counts over the bundles read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub bundles: usize,
    pub bundles_without_rigs: usize,
    pub records: usize,
    pub rigs: usize,
    pub joints: usize,
    pub padding_bytes: usize,
    pub uninterpreted_assets: Vec<UninterpretedAsset>,
    /// Rigs are kept in their authored rig space with their header transform; no render-skeleton conversion is made.
    pub source_space_rigs: usize,
    /// Each rig retains its complete payload, including auxiliary data outside the supported reference fields.
    pub rigs_with_uninterpreted_fields: usize,
}

impl Report {
    /// Count one more bundle.
    pub fn add(&mut self, bundle: &Bundle) {
        self.bundles += 1;
        self.bundles_without_rigs += usize::from(bundle.rigs.is_empty());
        self.records += bundle.records;
        self.padding_bytes += bundle.padding_bytes;
        self.uninterpreted_assets
            .extend(bundle.uninterpreted.iter().cloned());
        for rig in &bundle.rigs {
            self.rigs += 1;
            self.joints += rig.rig.joints.len();
            self.source_space_rigs += 1;
            self.rigs_with_uninterpreted_fields += 1;
        }
    }
}

/// One type 1 record of a bundle: a reference rig in its authored rig space.
#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceRig {
    pub pak: String,
    pub entry: Vec<u8>,
    pub record_offset: usize,
    pub asset_id: u32,
    pub guid: [u8; 16],
    pub header_word: u32,
    pub rig: Rig,
    /// Original type 1 payload, retaining auxiliary tables and header fields not yet assigned meanings.
    /// Padding and serialized runtime-pointer words are not required to be zero.
    pub source_payload: Vec<u8>,
}

impl ReferenceRig {
    /// Global references in authored rig space. The separate rig-header orientation/vector is not applied.
    /// Matches the native reference constructor: XYZW to an affine matrix, then parent global * local.
    /// Local entries use f32 arithmetic; parent composition uses f64 intermediates rounded to f32 once.
    /// This preserves the construction's meaning, without promising bit equality to native FMA rounding.
    pub fn source_matrices(&self) -> std::result::Result<Vec<Affine>, String> {
        let mut matrices: Vec<Option<Affine>> = vec![None; self.rig.joints.len()];
        for start in 0..self.rig.joints.len() {
            let mut chain = Vec::new();
            let mut next = Some(start);
            while let Some(index) = next {
                if matrices
                    .get(index)
                    .ok_or("reference parent outside rig")?
                    .is_some()
                {
                    break;
                }
                if chain.contains(&index) {
                    return Err("cyclic reference hierarchy".into());
                }
                chain.push(index);
                next = self
                    .rig
                    .joints
                    .get(index)
                    .ok_or("reference joint missing")?
                    .parent
                    .map(|p| p as usize);
            }
            for index in chain.into_iter().rev() {
                let joint = &self.rig.joints[index];
                let [x, y, z, w] = joint.rotation;
                // Do not normalize the authored quaternion: the native constructor uses these values directly.
                let local = Affine {
                    cols: [
                        [
                            1. - 2. * (y * y + z * z),
                            2. * (x * y + w * z),
                            2. * (x * z - w * y),
                        ],
                        [
                            2. * (x * y - w * z),
                            1. - 2. * (x * x + z * z),
                            2. * (y * z + w * x),
                        ],
                        [
                            2. * (x * z + w * y),
                            2. * (y * z - w * x),
                            1. - 2. * (x * x + y * y),
                        ],
                        joint.translation,
                    ],
                };
                let global = if let Some(parent) = joint.parent {
                    matrices
                        .get(parent as usize)
                        .and_then(|m| *m)
                        .ok_or("reference parent not resolved")?
                        .compose(&local)
                } else {
                    local
                };
                if global.cols.iter().flatten().any(|v| !v.is_finite()) {
                    return Err("nonfinite global reference".into());
                }
                matrices[index] = Some(global);
            }
        }
        matrices
            .into_iter()
            .map(|m| m.ok_or_else(|| "unresolved reference joint".into()))
            .collect()
    }
}

/// What one animation container holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Bundle {
    /// Every record of the container, rigs and others.
    pub records: usize,
    /// Bytes of record padding.
    pub padding_bytes: usize,
    pub rigs: Vec<ReferenceRig>,
    pub uninterpreted: Vec<UninterpretedAsset>,
}

/// Read the animation container `archive` names: its entry must occur exactly once.
pub fn read_archive(archive: &Archive<'_>) -> Result<Bundle> {
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
    read_bundle(archive.label, archive.entry, &bytes)
}

/// Read one decoded animation container; also useful when the caller owns archive I/O. `pak` and `entry` label the
/// records. The whole container is decoded and validated before anything is returned.
pub fn read_bundle(pak: &str, entry: &[u8], bytes: &[u8]) -> Result<Bundle> {
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
            let rig = ReferenceRig {
                pak: pak.into(),
                entry: entry.to_vec(),
                record_offset: record.offset,
                asset_id: record.asset_id,
                guid: record.guid,
                header_word: record.header_word,
                rig,
                source_payload: record.payload.to_vec(),
            };
            rig.source_matrices().map_err(|e| invalid(origin, e))?;
            rigs.push(rig);
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
    Ok(Bundle {
        records: records.len(),
        padding_bytes,
        rigs,
        uninterpreted: unsupported,
    })
}
