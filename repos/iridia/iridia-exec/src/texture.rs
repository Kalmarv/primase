//! A 2D texture as the executor samples it ([`SampledTexture`], an [`Image`]): its stored levels, the colour transfer
//! applied before filtering, the addressing and level-of-detail bias its binding's sampler states ([`SamplerUse`]),
//! and the filtering a caller states ([`Filtering`]).
//!
//! The filtering is the caller's policy: what a source states of its sampler's filter may be absent or not modelled, so
//! a bake says how it filters (the base level bilinear, point or bilinear at the nearest level, or trilinear) and
//! reports it. The addressing and the bias are the binding's data: an axis whose addressing the sampler state leaves
//! unstated is the caller's stated assumption ([`Unstated`]), never a silent wrap, and a stated bias is applied where
//! the level of detail is computed, or named as not applied ([`SampledTexture::unhonoured`]).
//!
//! The level of detail is the quad footprint's, or the level the program states. A sample at an implicit level outside
//! a quad (an evaluation one pixel at a time) has no footprint: it is refused, unless the caller states the level such
//! a sample takes ([`SampledTexture::with_footprintless_level`]).

use iridia_interface::{AddressMode, SamplerState};
use primase_image::sampling::{Address, Decoded, Levels, Subtexel, Transfer, footprint_lod};

use crate::{Image, Lod};

/// How a texture is filtered: the caller's stated policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filtering {
    /// The base level only, whatever level of detail the program asks for: bilinear in f32 over texels decoded once,
    /// the texel coordinate resolved to `subtexel` precision ([`Decoded::bilinear_at`]).
    BaseBilinear { subtexel: Subtexel },
    /// The texel the coordinate falls in, of the stored level nearest the level of detail ([`Levels::point`],
    /// [`Levels::nearest_level`]), in f64: a point filter with a point mip filter.
    Point,
    /// Bilinear in f64 ([`Levels::bilinear`]) in the stored level nearest the level of detail: a linear filter with a
    /// point mip filter.
    Bilinear,
    /// The two stored levels about the level of detail mixed, each bilinear in f64 ([`Levels::trilinear`]).
    Trilinear,
}

impl Filtering {
    /// The level of detail [`Filtering::Point`], [`Filtering::Bilinear`] and [`Filtering::Trilinear`] sample at: the
    /// quad footprint's ([`footprint_lod`]: the program's bias, clamped to Direct3D's -16..15.99, then the sampler's
    /// bias, added to the footprint's base-2 logarithm), or the level the program states plus the sampler's bias
    /// (Direct3D's `sample_l` honours the sampler's MipLODBias). Without a footprint, an implicit level is
    /// `footprintless` (plus the program's clamped bias, then the sampler's), refused when the caller states none.
    fn level_of_detail(
        lod: Lod,
        size: [u32; 2],
        sampler_bias: f64,
        footprintless: Option<f32>,
    ) -> Result<f64, String> {
        let [w, h] = size;
        let grad = |g: [[f32; 2]; 2]| g.map(|d| d.map(f64::from));
        let program = |b: f32| f64::from(b.clamp(-16.0, 15.99));
        let without = || {
            footprintless
                .map(f64::from)
                .ok_or_else(|| "an implicit level without a quad's footprint".to_owned())
        };
        Ok(match lod {
            Lod::Gradients(g) => footprint_lod(w, h, grad(g), sampler_bias),
            Lod::GradientsBias(g, b) => footprint_lod(w, h, grad(g), program(b) + sampler_bias),
            Lod::Level(l) => f64::from(l) + sampler_bias,
            Lod::Implicit => without()? + sampler_bias,
            Lod::Bias(b) => without()? + program(b) + sampler_bias,
        })
    }
}

/// What an addressing axis the sampler state leaves unstated is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unstated {
    /// Refused: the binding must state it.
    Refuse,
    /// The caller's stated assumption, which it reports.
    Assume(Address),
}

/// What a binding's sampler gives a texture's sampling: the addressing along u and v and the level-of-detail bias it
/// states.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SamplerUse {
    pub address: [Address; 2],
    /// Added to a computed level of detail ([`Filtering::Trilinear`]); finite.
    pub mip_lod_bias: Option<f32>,
}

impl SamplerUse {
    /// The addressing `address` and no bias.
    pub fn addressed(address: [Address; 2]) -> SamplerUse {
        SamplerUse {
            address,
            mip_lod_bias: None,
        }
    }

    /// From a binding's sampler state: its u and v addressing (an unstated axis as `unstated` says) and its bias.
    /// Border addressing is refused (no source stores the border colour), as is a bias that isn't finite. Whether an
    /// axis was assumed is [`SamplerUse::assumed`].
    pub fn of(state: &SamplerState, unstated: Unstated) -> Result<SamplerUse, String> {
        let axis = |k: usize| -> Result<Address, String> {
            Ok(match state.wrap[k] {
                Some(AddressMode::Repeat) => Address::Wrap,
                Some(AddressMode::Mirror) => Address::Mirror,
                Some(AddressMode::Clamp) => Address::Clamp,
                Some(AddressMode::MirrorOnce) => Address::MirrorOnce,
                Some(AddressMode::Border) => {
                    return Err(format!(
                        "border addressing along {}: the source stores no border colour",
                        ["u", "v"][k]
                    ));
                }
                None => match unstated {
                    Unstated::Assume(a) => a,
                    Unstated::Refuse => {
                        return Err(format!(
                            "the sampler states no addressing along {}",
                            ["u", "v"][k]
                        ));
                    }
                },
            })
        };
        if let Some(b) = state.mip_lod_bias
            && !b.is_finite()
        {
            return Err(format!("sampler bias {b} is not finite"));
        }
        Ok(SamplerUse {
            address: [axis(0)?, axis(1)?],
            mip_lod_bias: state.mip_lod_bias,
        })
    }

    /// Whether `state` leaves the u or v addressing unstated: what [`SamplerUse::of`] took from [`Unstated`].
    pub fn assumed(state: &SamplerState) -> bool {
        state.wrap[0].is_none() || state.wrap[1].is_none()
    }
}

/// A texture the executor samples (see the module documentation).
#[derive(Clone, Debug)]
pub struct SampledTexture {
    levels: Levels,
    transfer: Transfer,
    sampler: SamplerUse,
    filtering: Filtering,
    /// The level an implicit-level sample without a footprint takes, when the caller states one.
    footprintless: Option<f32>,
    /// The base level decoded once, for [`Filtering::BaseBilinear`].
    base: Option<Decoded>,
}

impl SampledTexture {
    /// `levels` (at least one), read through `transfer`, sampled as `sampler` and `filtering` say; a sampler bias
    /// that isn't finite is refused.
    pub fn new(
        levels: Levels,
        transfer: Transfer,
        sampler: SamplerUse,
        filtering: Filtering,
    ) -> Result<SampledTexture, String> {
        if levels.levels.is_empty() {
            return Err("a texture with no stored level".into());
        }
        if let Some(b) = sampler.mip_lod_bias
            && !b.is_finite()
        {
            return Err(format!("sampler bias {b} is not finite"));
        }
        let base = match filtering {
            Filtering::BaseBilinear { .. } => Some(levels.decoded(0, transfer)),
            Filtering::Point | Filtering::Bilinear | Filtering::Trilinear => None,
        };
        Ok(SampledTexture {
            levels,
            transfer,
            sampler,
            filtering,
            footprintless: None,
            base,
        })
    }

    /// The same texture with `level` the level of detail of an implicit-level sample that has no quad footprint (an
    /// evaluation one pixel at a time): the caller's stated assumption, which it reports. A level that isn't finite
    /// is refused.
    pub fn with_footprintless_level(mut self, level: f32) -> Result<SampledTexture, String> {
        if !level.is_finite() {
            return Err(format!("footprintless level {level} is not finite"));
        }
        self.footprintless = Some(level);
        Ok(self)
    }

    /// The level an implicit-level sample without a footprint takes, if the caller stated one.
    pub fn footprintless_level(&self) -> Option<f32> {
        self.footprintless
    }

    pub fn levels(&self) -> &Levels {
        &self.levels
    }

    pub fn transfer(&self) -> Transfer {
        self.transfer
    }

    pub fn sampler(&self) -> SamplerUse {
        self.sampler
    }

    pub fn address(&self) -> [Address; 2] {
        self.sampler.address
    }

    pub fn filtering(&self) -> Filtering {
        self.filtering
    }

    /// The base level's width and height.
    pub fn dimensions(&self) -> [usize; 2] {
        [self.levels.width as usize, self.levels.height as usize]
    }

    /// The bytes it holds: the stored levels and the texels decoded for filtering.
    pub fn decoded_bytes(&self) -> usize {
        self.levels.levels.iter().map(Vec::len).sum::<usize>()
            + self.base.as_ref().map_or(0, Decoded::decoded_bytes)
    }

    /// What of its sampler this sampling does not apply, for the caller to report: a stated bias under
    /// [`Filtering::BaseBilinear`], which reads the base level whatever the level of detail.
    pub fn unhonoured(&self) -> Option<&'static str> {
        match (self.filtering, self.sampler.mip_lod_bias) {
            (Filtering::BaseBilinear { .. }, Some(b)) if b != 0.0 => {
                Some("stated sampler bias not applied (base level)")
            }
            _ => None,
        }
    }
}

impl Image for SampledTexture {
    fn sample(&self, uv: [f32; 2], lod: Lod) -> Result<[f32; 4], String> {
        let address = self.sampler.address;
        let level = || {
            Filtering::level_of_detail(
                lod,
                [self.levels.width, self.levels.height],
                self.sampler.mip_lod_bias.map_or(0.0, f64::from),
                self.footprintless,
            )
        };
        let (at, levels, transfer) = ([f64::from(uv[0]), f64::from(uv[1])], &self.levels, self.transfer);
        let v = match (self.filtering, &self.base) {
            (Filtering::BaseBilinear { subtexel }, Some(base)) => {
                return base.bilinear_at(uv, address, subtexel).map_err(|e| e.to_string());
            }
            (Filtering::BaseBilinear { .. }, None) => {
                unreachable!("a base-level texture decodes its base when made")
            }
            (Filtering::Point, _) => levels
                .nearest_level(level()?)
                .and_then(|l| levels.point(l, at, address, transfer)),
            (Filtering::Bilinear, _) => levels
                .nearest_level(level()?)
                .and_then(|l| levels.bilinear(l, at, address, transfer)),
            (Filtering::Trilinear, _) => levels.trilinear(at, level()?, address, transfer),
        }
        .map_err(|e| e.to_string())?;
        Ok(v.map(|c| c as f32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(u: Option<AddressMode>, v: Option<AddressMode>, bias: Option<f32>) -> SamplerState {
        SamplerState {
            wrap: [u, v, None],
            mip_lod_bias: bias,
            ..SamplerState::default()
        }
    }

    /// A 2 x 1 texture, black then white, one level.
    fn two_texels() -> Levels {
        Levels::one(2, 1, vec![0, 0, 0, 255, 255, 255, 255, 255]).unwrap()
    }

    /// The stated addressing is what samples past the edge read; an unstated axis is the caller's assumption or a
    /// refusal; border addressing and a non-finite bias are refused.
    #[test]
    fn the_sampler_states_the_addressing() {
        let clamp = SamplerUse::of(
            &state(Some(AddressMode::Clamp), Some(AddressMode::Clamp), None),
            Unstated::Refuse,
        )
        .unwrap();
        assert_eq!(clamp.address, [Address::Clamp; 2]);
        let base = Filtering::BaseBilinear {
            subtexel: Subtexel::D3d11,
        };
        let clamped = SampledTexture::new(two_texels(), Transfer::Stored, clamp, base).unwrap();
        let wrapped = SampledTexture::new(
            two_texels(),
            Transfer::Stored,
            SamplerUse::addressed([Address::Wrap; 2]),
            base,
        )
        .unwrap();
        // u = 0: half a texel left of the first centre; clamped reads black, wrapped mixes in the white texel
        assert_eq!(clamped.sample([0.0, 0.5], Lod::Implicit).unwrap()[0], 0.0);
        assert_eq!(wrapped.sample([0.0, 0.5], Lod::Implicit).unwrap()[0], 0.5);
        assert!(SamplerUse::of(&state(None, Some(AddressMode::Clamp), None), Unstated::Refuse).is_err());
        let assumed = SamplerUse::of(
            &state(None, Some(AddressMode::Clamp), None),
            Unstated::Assume(Address::Wrap),
        )
        .unwrap();
        assert_eq!(assumed.address, [Address::Wrap, Address::Clamp]);
        assert!(SamplerUse::assumed(&state(None, Some(AddressMode::Clamp), None)));
        assert!(
            SamplerUse::of(
                &state(Some(AddressMode::Border), Some(AddressMode::Clamp), None),
                Unstated::Refuse
            )
            .is_err()
        );
        assert!(
            SamplerUse::of(
                &state(Some(AddressMode::Clamp), Some(AddressMode::Clamp), Some(f32::NAN)),
                Unstated::Refuse
            )
            .is_err()
        );
    }

    /// A stated bias moves the footprint's level and a stated level under trilinear filtering; the base level names
    /// it as not applied; an implicit level is refused.
    #[test]
    fn a_stated_bias_is_applied_or_named() {
        // two levels: 2 x 2 black, 1 x 1 white
        let levels = Levels {
            width: 2,
            height: 2,
            levels: vec![[0, 0, 0, 255].repeat(4), vec![255; 4]],
        };
        let make = |bias: Option<f32>, filtering| {
            SampledTexture::new(
                levels.clone(),
                Transfer::Stored,
                SamplerUse {
                    address: [Address::Wrap; 2],
                    mip_lod_bias: bias,
                },
                filtering,
            )
            .unwrap()
        };
        // a footprint of half a texel: level -1, clamped to 0 (black); a bias of 1 more reaches level 0 still, 2 more
        // reaches level 1 (white)
        let footprint = Lod::Gradients([[0.25, 0.0], [0.0, 0.25]]);
        let plain = make(None, Filtering::Trilinear);
        assert_eq!(plain.sample([0.5, 0.5], footprint).unwrap()[0], 0.0);
        let biased = make(Some(2.0), Filtering::Trilinear);
        assert_eq!(biased.sample([0.5, 0.5], footprint).unwrap()[0], 1.0);
        assert_eq!(biased.unhonoured(), None);
        // a stated level takes the sampler's bias too: level -1 biased by 2 is level 1 (white); unbiased, level 0
        assert_eq!(biased.sample([0.5, 0.5], Lod::Level(-1.0)).unwrap()[0], 1.0);
        assert_eq!(plain.sample([0.5, 0.5], Lod::Level(-1.0)).unwrap()[0], 0.0);
        assert_eq!(plain.sample([0.5, 0.5], Lod::Level(1.0)).unwrap()[0], 1.0);
        assert!(plain.sample([0.5, 0.5], Lod::Implicit).is_err());
        let base = make(
            Some(2.0),
            Filtering::BaseBilinear {
                subtexel: Subtexel::D3d11,
            },
        );
        assert_eq!(base.sample([0.5, 0.5], footprint).unwrap()[0], 0.0);
        assert_eq!(
            base.unhonoured(),
            Some("stated sampler bias not applied (base level)")
        );
    }

    /// Point and bilinear filtering read the stored level nearest the level of detail (a half rounds up); an implicit
    /// level without a footprint is refused unless the caller states the level it takes, and a program's bias then
    /// adds to that level.
    #[test]
    fn nearest_level_filters_and_a_stated_footprintless_level() {
        // two levels: 2 x 2 with columns black and white, 1 x 1 grey
        let levels = Levels {
            width: 2,
            height: 2,
            levels: vec![[[0, 0, 0, 255], [255; 4]].concat().repeat(2), vec![128; 4]],
        };
        let make = |filtering| {
            SampledTexture::new(
                levels.clone(),
                Transfer::Stored,
                SamplerUse::addressed([Address::Wrap; 2]),
                filtering,
            )
            .unwrap()
        };
        let grey = 128.0 / 255.0;
        let point = make(Filtering::Point);
        assert_eq!(point.sample([0.25, 0.25], Lod::Level(0.0)).unwrap()[0], 0.0);
        assert_eq!(point.sample([0.75, 0.25], Lod::Level(0.49)).unwrap()[0], 1.0);
        assert_eq!(point.sample([0.25, 0.25], Lod::Level(0.5)).unwrap()[0], grey);
        let bilinear = make(Filtering::Bilinear);
        assert_eq!(bilinear.sample([0.5, 0.5], Lod::Level(0.4)).unwrap()[0], 0.5);
        assert_eq!(bilinear.sample([0.5, 0.5], Lod::Level(0.6)).unwrap()[0], grey);
        assert!(point.sample([0.25, 0.25], Lod::Implicit).is_err());
        assert!(point.sample([0.25, 0.25], Lod::Bias(0.0)).is_err());
        let stated = make(Filtering::Point).with_footprintless_level(1.0).unwrap();
        assert_eq!(stated.footprintless_level(), Some(1.0));
        assert_eq!(stated.sample([0.25, 0.25], Lod::Implicit).unwrap()[0], grey);
        assert_eq!(stated.sample([0.25, 0.25], Lod::Bias(-1.0)).unwrap()[0], 0.0);
        assert!(make(Filtering::Point).with_footprintless_level(f32::NAN).is_err());
    }
}
