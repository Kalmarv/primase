//! D3D9 sampler-state pairs, mapped without supplying unstated defaults.
use crate::{Result, error};
use iridia_interface::{AddressMode, SamplerState};
use iridia_interface::{Filter, MipFilter};

/// Map native D3DSAMP pairs. Unknown states and values are refused at the pair's word offset.
pub fn sampler_states(states: impl IntoIterator<Item = (u32, u32)>) -> Result<SamplerState> {
    let mut out = SamplerState::default();
    for (index, (kind, value)) in states.into_iter().enumerate() {
        match kind {
            1..=3 => {
                out.wrap[(kind - 1) as usize] = Some(match value {
                    1 => AddressMode::Repeat,
                    2 => AddressMode::Mirror,
                    3 => AddressMode::Clamp,
                    4 => AddressMode::Border,
                    5 => AddressMode::MirrorOnce,
                    x => return Err(error(index * 2, format!("sampler address {x}"))),
                })
            }
            5 | 6 => {
                let filter = match value {
                    1 => Filter::Nearest,
                    2 => Filter::Linear,
                    x => return Err(error(index * 2, format!("sampler filter {x}"))),
                };
                if kind == 5 {
                    out.filters.mag = Some(filter);
                } else {
                    out.filters.min = Some(filter);
                }
            }
            7 => {
                out.filters.mip = Some(match value {
                    0 => MipFilter::None,
                    1 => MipFilter::Nearest,
                    2 => MipFilter::Linear,
                    x => return Err(error(index * 2, format!("mip filter {x}"))),
                })
            }
            8 => {
                let bias = f32::from_bits(value);
                if !bias.is_finite() {
                    return Err(error(index * 2, "nonfinite sampler LOD bias"));
                }
                out.mip_lod_bias = Some(bias);
            }
            // An anisotropic filter's enable is independent of its limit in D3D9. Until both
            // per-filter enables can be represented, only the non-anisotropic limit is accepted.
            10 if value == 1 => out.max_anisotropy = Some(1),
            x => return Err(error(index * 2, format!("sampler state {x} not resolved"))),
        }
    }
    Ok(out)
}
