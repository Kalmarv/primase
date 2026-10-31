//! Steady scalar Reinhard settings for a caller-selected Sekiro map file.
use super::{
    Records, Sample,
    composition::{Activation, ScalarBank, ScalarRow, scalar},
};
use fromsoft_formats::{Archive, Result, dcx::Decoder, gparam::ParamExtra};

/// Resolved native Reinhard scalars. This does not assert that a renderer-ready LUT or gamma has been resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reinhard {
    pub contrast: f32,
    pub white_balance: f32,
}

/// Resolve a constant scalar row at value ID zero using the native loader's saved reference and extra flags.
/// Time-varying fields are refused here; their interpolation is a separate operation.
fn constant_row(track: &[Sample<f32>], extra: &ParamExtra, reference: f32, field: &str) -> Result<ScalarRow> {
    let refuse = |detail: &str| crate::invalid("Sekiro Reinhard", 0, format!("{field}: {detail}"));
    let flags = extra
        .unknown_0c
        .ok_or_else(|| refuse("group load flags unavailable"))?;
    // A request for ID zero is unchanged by the force-zero-ID byte. An absent stored row is created from the
    // registered initializer; the low extra byte determines whether this fallback starts active (140319d40).
    let mut samples = track.iter().filter(|sample| sample.id == 0);
    let Some(first) = samples.next() else {
        return Ok(if flags & 0xff == 0 {
            ScalarRow::Active(reference)
        } else {
            ScalarRow::Inactive
        });
    };
    // Stored rows become active even when the extra ID list selects the saved initializer instead of file bytes
    // (1403175e4..627 and 140317738..748). Presence and authored-value selection are distinct.
    if !extra.ids.contains(&0) {
        return Ok(ScalarRow::Active(reference));
    }
    if !first.value.is_finite() || samples.any(|sample| sample.value.to_bits() != first.value.to_bits()) {
        return Err(refuse(
            "time-varying or non-finite scalar requires explicit sampling",
        ));
    }
    Ok(ScalarRow::Active(first.value))
}

/// Compose the steady default, MapBase and caller-selected map-file Reinhard settings at value ID zero.
///
/// MapBase is attached to the map context by 140ed5a60 -> 140f46340 -> 140189d20. The positive activation
/// parameter activates its descriptor; 1401787d0 initializes scalar weight to one. The generic consumer then
/// applies bank 1, bank 4 and bank 5 in ascending order. Transitions and additional override banks are not modeled.
/// Reference values 1.6/1.7 come from the native initializer 14013bed0, saved by 140341930.
pub fn resolve(default: &Records, map_base: &Records, selected: &Records) -> Result<Reinhard> {
    let compose = |field: &str, reference, select: fn(&Records) -> &[Sample<f32>]| {
        let mut banks = Vec::new();
        for (index, records) in [(1u8, default), (4, map_base), (5, selected)] {
            if records.version != 5 {
                return Err(crate::invalid(
                    "Sekiro Reinhard",
                    usize::from(index),
                    "only GPARAM version 5 load flags are established",
                ));
            }
            let extra = records.tone_extra.as_ref().ok_or_else(|| {
                crate::invalid(
                    "Sekiro Reinhard",
                    usize::from(index),
                    "Tone Map[Yebis] extra record unavailable",
                )
            })?;
            banks.push(ScalarBank {
                index,
                activation: Some(Activation::Active { weight: 1.0 }),
                row: constant_row(select(records), extra, reference, field)?,
            });
        }
        scalar(reference, &banks)
    };
    Ok(Reinhard {
        contrast: compose("ReinhardContrast", 1.6, |r| &r.tone.reinhard_contrast)?,
        white_balance: compose("ReinhardWhiteBalance", 1.7, |r| &r.tone.reinhard_white_balance)?,
    })
}

/// Read the required installed layers and resolve the selected file's steady Reinhard scalars.
pub fn read(archive: &Archive, decoder: &Decoder<'_>, selected_stem: &str) -> Result<Reinhard> {
    if archive.game() != fromsoft_formats::Game::Sekiro
        || fromsoft_formats::area::DrawParamId::from_stem(selected_stem).is_none()
    {
        return Err(crate::invalid(
            "Sekiro Reinhard",
            0,
            "expected a Sekiro map GPARAM stem",
        ));
    }
    resolve(
        &super::read(archive, decoder, "default")?,
        &super::read(archive, decoder, "MapBase")?,
        &super::read(archive, decoder, selected_stem)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_unlisted_and_absent_rows_have_distinct_activation() {
        let mut extra = ParamExtra {
            group_index: 4,
            ids: vec![0],
            unknown_0c: Some(1),
        };
        let stored = vec![Sample {
            id: 0,
            hour: Some(0.0),
            value: 1.45,
        }];
        assert_eq!(
            constant_row(&stored, &extra, 1.6, "contrast").unwrap(),
            ScalarRow::Active(1.45)
        );
        assert_eq!(
            constant_row(&[], &extra, 1.6, "contrast").unwrap(),
            ScalarRow::Inactive
        );
        extra.ids.clear();
        assert_eq!(
            constant_row(&stored, &extra, 1.6, "contrast").unwrap(),
            ScalarRow::Active(1.6)
        );
        extra.unknown_0c = Some(0);
        assert_eq!(
            constant_row(&[], &extra, 1.6, "contrast").unwrap(),
            ScalarRow::Active(1.6)
        );
        extra.ids.push(0);
        let mut changing = stored;
        changing.push(Sample {
            id: 0,
            hour: Some(18.0),
            value: 1.7,
        });
        assert!(constant_row(&changing, &extra, 1.6, "contrast").is_err());
    }
}
