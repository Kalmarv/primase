//! The state of a sampler as GTA states it: an effect's sampler record (D3D12 values) and the name of a static sampler
//! constant (`gDynamicSamplerIndex_sdiTrilinearWrap`), as core's [`SamplerState`].
//!
//! The programs read their samplers from one heap, indexed at run time by a word of a constant buffer
//! ([`super::sampler_use`]). Two kinds of word are named: the material's (`gDynamicSamplerIndex_DiffuseSampler`,
//! whose heap index the engine takes from the material's sampler value) and the engine's static samplers
//! (`gDynamicSamplerIndex_sdi<Filter><Address>`, whose name states the filter and the addressing). Only the second
//! kind has a state anywhere in the data this reads; the effect's sampler records ([`SamplerDesc`]) are held against
//! it as evidence ([`held_by`]), not used in place of it.
use iridia_interface::{AddressMode, SamplerState};
use iridia_interface::{Filter, Filters, MipFilter};

use super::resolve_shaders::compare;
use rage_engine_formats::sgd2_effect::SamplerDesc;

/// The prefix of the constants naming a static sampler.
const STATIC_PREFIX: &str = "gDynamicSamplerIndex_sdi";

fn address(a: u8) -> Option<AddressMode> {
    Some(match a {
        1 => AddressMode::Repeat,
        2 => AddressMode::Mirror,
        3 => AddressMode::Clamp,
        4 => AddressMode::Border,
        5 => AddressMode::MirrorOnce,
        _ => return None,
    })
}

/// An effect's sampler record as core's state. The D3D12 filter byte: bit 0 the mip filter, bit 2 magnification,
/// bit 4 minification (set: linear), bit 6 anisotropic (the maximum anisotropy only above 1: at 1 the filter is the
/// linear one), bit 7 a comparison (against the record's comparison function). A non-zero LOD bias is carried; the
/// maximum LOD is not (every record of the install has 12).
pub fn state(d: &SamplerDesc) -> SamplerState {
    let linear = |bit: u8| d.filter >> bit & 1 == 1;
    let level = |bit: u8| {
        if linear(bit) {
            Filter::Linear
        } else {
            Filter::Nearest
        }
    };
    let [u, v, w] = d.address;
    SamplerState {
        wrap: [address(u), address(v), address(w)],
        filters: Filters {
            mag: Some(level(2)),
            min: Some(level(4)),
            mip: Some(if linear(0) {
                MipFilter::Linear
            } else {
                MipFilter::Nearest
            }),
        },
        max_anisotropy: (d.filter & 0x40 != 0 && d.max_anisotropy > 1).then_some(d.max_anisotropy),
        compare: if d.filter & 0x80 != 0 {
            compare(d.comparison_func)
        } else {
            None
        },
        mip_lod_bias: (d.mip_lod_bias != 0.0).then_some(d.mip_lod_bias),
    }
}

/// The state a static sampler's constant name states, or `None` for a name that is not one (the material's) or whose
/// form this does not read. The forms are `Point`, `MipLinear` (linear minification and magnification, the nearest mip
/// level), `Trilinear` and `Anisotropic<N>X`, then the addressing (`Wrap`, `Clamp`, `Mirror`, `BorderBlack`,
/// `BorderWhite`); a border's colour is not carried, and a `LQ` after the addressing is not read (the state is the
/// name's without it).
pub fn named(constant: &str) -> Option<SamplerState> {
    let rest = constant.strip_prefix(STATIC_PREFIX)?;
    let rest = rest.strip_suffix("LQ").unwrap_or(rest);
    let (family, mode) = [
        ("BorderBlack", AddressMode::Border),
        ("BorderWhite", AddressMode::Border),
        ("Wrap", AddressMode::Repeat),
        ("Clamp", AddressMode::Clamp),
        ("Mirror", AddressMode::Mirror),
    ]
    .into_iter()
    .find_map(|(suffix, mode)| Some((rest.strip_suffix(suffix)?, mode)))?;
    let all = |f: Filter, mip: MipFilter| Filters {
        mag: Some(f),
        min: Some(f),
        mip: Some(mip),
    };
    let (filters, max_anisotropy) = match family {
        "Point" => (all(Filter::Nearest, MipFilter::Nearest), None),
        "MipLinear" => (all(Filter::Linear, MipFilter::Nearest), None),
        "Trilinear" => (all(Filter::Linear, MipFilter::Linear), None),
        other => {
            let n: u8 = other
                .strip_prefix("Anisotropic")?
                .strip_suffix('X')?
                .parse()
                .ok()?;
            (all(Filter::Linear, MipFilter::Linear), (n > 1).then_some(n))
        }
    };
    Some(SamplerState {
        wrap: [Some(mode); 3],
        filters,
        max_anisotropy,
        compare: None,
        mip_lod_bias: None,
    })
}

/// Whether one of an effect's sampler records is this state.
pub fn held_by(state: &SamplerState, samplers: &[SamplerDesc]) -> bool {
    samplers.iter().any(|d| self::state(d) == *state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(filter: u8, address: u8, max_anisotropy: u8, comparison_func: u8) -> SamplerDesc {
        SamplerDesc {
            filter,
            address: [address; 3],
            mip_lod_bias: 0.0,
            max_anisotropy,
            comparison_func,
            max_lod: 12.0,
            raw: Vec::new(),
        }
    }

    #[test]
    fn a_record_is_its_d3d12_filter_and_addressing() {
        let point = state(&record(0x00, 3, 1, 1));
        assert_eq!(
            point.filters,
            Filters {
                mag: Some(Filter::Nearest),
                min: Some(Filter::Nearest),
                mip: Some(MipFilter::Nearest),
            }
        );
        assert_eq!(point.wrap, [Some(AddressMode::Clamp); 3]);
        // 0x14: linear minification and magnification, the nearest mip level
        assert_eq!(
            state(&record(0x14, 1, 1, 1)).filters.mip,
            Some(MipFilter::Nearest)
        );
        assert_eq!(state(&record(0x14, 1, 1, 1)).filters.mag, Some(Filter::Linear));
        // an anisotropic filter at 1 is the linear one; above 1 it states the maximum
        assert_eq!(state(&record(0x55, 1, 1, 1)).max_anisotropy, None);
        assert_eq!(state(&record(0x55, 1, 4, 1)).max_anisotropy, Some(4));
        // a comparison filter states its function; a plain one does not
        assert_eq!(
            state(&record(0x95, 3, 1, 4)).compare,
            Some(iridia_interface::Compare::LessEqual)
        );
        assert_eq!(state(&record(0x55, 3, 1, 4)).compare, None);
        // a bias is carried where it is not zero
        let mut biased = record(0x55, 1, 1, 1);
        assert_eq!(state(&biased).mip_lod_bias, None);
        biased.mip_lod_bias = -1.0;
        assert_eq!(state(&biased).mip_lod_bias, Some(-1.0));
        assert_ne!(state(&biased), state(&record(0x55, 1, 1, 1)));
    }

    #[test]
    fn a_static_samplers_name_states_its_filter_and_addressing() {
        let n = |s: &str| named(&format!("gDynamicSamplerIndex_sdi{s}")).unwrap();
        assert_eq!(n("TrilinearWrap"), state(&record(0x55, 1, 1, 1)));
        assert_eq!(n("TrilinearClamp"), state(&record(0x55, 3, 1, 1)));
        assert_eq!(n("MipLinearClamp"), state(&record(0x14, 3, 1, 1)));
        assert_eq!(n("MipLinearWrap"), state(&record(0x14, 1, 1, 1)));
        assert_eq!(n("PointClamp"), state(&record(0x00, 3, 1, 1)));
        assert_eq!(n("Anisotropic4XWrap"), state(&record(0x55, 1, 4, 1)));
        assert_eq!(n("Anisotropic4XBorderBlack").wrap, [Some(AddressMode::Border); 3]);
        // the low-quality variant is read as the name without it
        assert_eq!(n("TrilinearWrapLQ"), n("TrilinearWrap"));
        // the material's samplers and the forms this does not read have no state
        assert_eq!(named("gDynamicSamplerIndex_DiffuseSampler"), None);
        assert_eq!(named("gDynamicSamplerIndex_sdiBicubicWrap"), None);
        assert_eq!(named("gDynamicSamplerIndex_sdiAnisotropicXWrap"), None);
        assert_eq!(named("gDynamicSamplerIndex_sdiTrilinear"), None);
    }

    #[test]
    fn a_state_is_held_by_the_record_that_decodes_to_it() {
        let table = [record(0x00, 3, 1, 1), record(0x14, 1, 1, 1)];
        assert!(held_by(
            &named("gDynamicSamplerIndex_sdiMipLinearWrap").unwrap(),
            &table
        ));
        assert!(!held_by(
            &named("gDynamicSamplerIndex_sdiTrilinearWrap").unwrap(),
            &table
        ));
    }
}
