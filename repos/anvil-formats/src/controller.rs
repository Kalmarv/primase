//! TimeOfDay/WeatherPropertyControllerData's registry-ordered entries (lighting.md §6–7).
//! The packed track header includes the first key's time in its upper half. No scanning of float payloads for
//! coincidental ClassIDs: each nested header, path node, key vector and trailer is consumed in order.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::archive::{Archive, At};

#[derive(Clone, Debug, PartialEq)]
pub struct Controller {
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub path: Vec<u32>,
    pub kind: u32,
    pub times: Vec<u16>,
    pub values: Vec<[f32; 4]>,
    pub path_flags: [u8; 2],
    pub binding: u64,
    pub trailer: [u8; 5],
}

pub const TIME_OF_DAY: u32 = 0xF0C8_5018;
pub const WEATHER: u32 = 0x8870_04DC;
const ENTRY: u32 = 0x050A_3620;
const PATH: u32 = 0x3C00_B2E0;
const NODE: u32 = 0x1A87_50BA;
const FLOAT: u32 = 0xCFD6_6AAD;
const VECTOR: u32 = 0x8BE9_C203;
const BYTE: u32 = 0x5841_BD2B;

impl Track {
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "finite floating point curve interpolation; no byte addressing"
    )]
    pub fn at(&self, time: f32) -> Result<[f32; 4], String> {
        if !time.is_finite() || self.times.is_empty() || self.times.len() != self.values.len() {
            return Err("controller: invalid sample/key vector".into());
        }
        if self.times.windows(2).any(|pair| matches!(pair, [a,b] if a>=b)) {
            return Err("controller: unordered key times".into());
        }
        let first = self.times.first().copied().ok_or("controller: no first time")?;
        if time <= f32::from(first) {
            return self
                .values
                .first()
                .copied()
                .ok_or("controller: no first value".into());
        }
        for (times, values) in self.times.windows(2).zip(self.values.windows(2)) {
            let ([start, end], [a, b]) = (times, values) else {
                return Err("controller: invalid key pair".into());
            };
            if time <= f32::from(*end) {
                if self.kind == BYTE {
                    return Ok(*a);
                }
                let f = (time - f32::from(*start)) / (f32::from(*end) - f32::from(*start));
                let mut result = [0.0; 4];
                for ((out, a), b) in result.iter_mut().zip(a).zip(b) {
                    *out = a * (1.0 - f) + b * f;
                }
                if result.iter().any(|v| !v.is_finite()) {
                    return Err("controller: nonfinite interpolated value".into());
                }
                return Ok(result);
            }
        }
        self.values
            .last()
            .copied()
            .ok_or("controller: no last key".into())
    }
}
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Cursor<'a> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let end = self.at.checked_add(N).ok_or("controller offset overflow")?;
        let value = self
            .bytes
            .get(self.at..end)
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| format!("controller truncated at {} reading {N} bytes", self.at))?;
        self.at = end;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn header(&mut self, class: u32) -> Result<(), String> {
        let _id = self.take::<8>()?;
        let found = self.u32()?;
        if found != class {
            return Err(format!(
                "controller: expected class {class:08x}, found {found:08x} at {}",
                self.at.saturating_sub(4)
            ));
        }
        Ok(())
    }
}
impl Controller {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut c = Cursor { bytes, at: 0 };
        if c.take::<1>()? != [1] {
            return Err("controller: unknown ManagedObject header".into());
        }
        let count = usize::try_from(c.u32()?).map_err(|_| "controller: entry count overflow")?;
        if count > 8192 {
            return Err("controller: entry count exceeds supported resource bound".into());
        }
        let mut tracks = Vec::with_capacity(count);
        for _ in 0..count {
            c.header(ENTRY)?;
            c.header(PATH)?;
            let nodes = usize::try_from(c.u32()?).map_err(|_| "controller: path node count overflow")?;
            if !(1..=32).contains(&nodes) {
                return Err("controller: invalid property path length".into());
            }
            let mut path = Vec::with_capacity(nodes);
            for _ in 0..nodes {
                c.header(NODE)?;
                // Fixed scalar-path node profile; conditioned/indexed/solver nodes need their own grammar.
                if c.take::<2>()? != [255; 2] || c.take::<8>()? != [0; 8] || c.take::<3>()? != [1, 0, 0] {
                    return Err(
                        "controller: conditioned/indexed PropertyPathNode is not the scalar-path profile"
                            .into(),
                    );
                }
                path.push(c.u32()?);
                if c.take::<25>()? != [0; 25] || c.take::<1>()? != [3] {
                    return Err("controller: PropertyPathNode solver/index fields differ from the established profile".into());
                }
            }
            let path_flags = c.take()?;
            let binding = u64::from_le_bytes(c.take()?);
            let _track_id = c.take::<8>()?;
            let kind = c.u32()?;
            let width = match kind {
                FLOAT => 1,
                VECTOR => 4,
                BYTE => 1,
                _ => return Err(format!("controller: unsupported track class {kind:08x}")),
            };
            let packed = c.u32()?;
            let keys = usize::try_from(packed & 65535).map_err(|_| "controller: key count overflow")?;
            if !(1..=4096).contains(&keys) {
                return Err("controller: invalid key count".into());
            }
            let mut times = Vec::with_capacity(keys);
            times.push(u16::try_from(packed >> 16).map_err(|_| "controller: first time overflow")?);
            for _ in 1..keys {
                times.push(u16::from_le_bytes(c.take()?));
            }
            if times.windows(2).any(|p| matches!(p,[a,b] if a>=b)) {
                return Err("controller: key times are not strictly increasing".into());
            }
            let mut values = Vec::with_capacity(keys);
            for _ in 0..keys {
                let mut value = [0.0; 4];
                for lane in value.iter_mut().take(width) {
                    *lane = if kind == BYTE {
                        let [byte] = c.take::<1>()?;
                        f32::from(byte)
                    } else {
                        f32::from_bits(c.u32()?)
                    };
                    if !lane.is_finite() {
                        return Err("controller: non-finite key".into());
                    }
                }
                values.push(value);
            }
            tracks.push(Track {
                path,
                kind,
                times,
                values,
                path_flags,
                binding,
                trailer: c.take()?,
            });
        }
        if c.at != bytes.len() {
            return Err(format!(
                "controller: {} trailing bytes",
                bytes.len().saturating_sub(c.at)
            ));
        }
        Ok(Self { tracks })
    }
    /// Match the complete suffix, rejecting duplicate candidates instead of choosing a fog/sky property by its
    /// final name alone (several cloud and fog objects each have Albedo and Density).
    pub fn value(&self, path: &[&str], time: f32) -> Result<Option<[f32; 4]>, String> {
        let ids: Vec<_> = path
            .iter()
            .map(|s| primase_foundations::hash::crc32(s.as_bytes()))
            .collect();
        let mut matches = self.tracks.iter().filter(|t| t.path.ends_with(&ids));
        let value = matches.next().map(|t| t.at(time)).transpose()?;
        if matches.next().is_some() {
            return Err(format!("controller: ambiguous property {}", path.join("/")));
        }
        Ok(value)
    }
}

/// The name of a look's controller resource in the forges: `TOD_<look>` for the time of day, `WEA_<look>` for the
/// weather; `None` for a class that is no controller.
fn controller_name(class: u32, look: &str) -> Option<String> {
    match class {
        TIME_OF_DAY => Some(format!("TOD_{look}")),
        WEATHER => Some(format!("WEA_{look}")),
        _ => None,
    }
}

/// The plan's forge files that hold `name`, in search order, as the first entry of that name: nothing is decoded.
pub fn named_resource(archive: &Archive, name: &str) -> Option<At> {
    archive
        .listing()
        .find(|(_, entry)| entry.name == name)
        .map(|(forge, entry)| At {
            forge,
            entry: entry.entry,
        })
}

/// A look's authored controllers: the time-of-day one (`TOD_<look>`) and the weather one (`WEA_<look>`), which the
/// forges need not hold.
#[derive(Clone, Debug, PartialEq)]
pub struct LookControllers {
    pub time_of_day: Controller,
    pub weather: Option<Controller>,
}

/// The authored controllers of the look `look`: the time-of-day one (`TOD_<look>`, which the atmosphere has) and the
/// weather one (`WEA_<look>`, when a forge holds it). Only the class of each resource is checked, and the
/// controller's own parse states what it can hold. The error is the reason; an absent time-of-day controller is one,
/// an absent weather controller is not.
pub fn look_controllers(archive: &mut Archive, look: &str) -> Result<LookControllers, String> {
    let read = |archive: &mut Archive, class: u32| -> Result<Option<Controller>, String> {
        let Some(name) = controller_name(class, look) else {
            return Err(format!("{class:#x}: not a look controller class"));
        };
        let Some(at) = named_resource(archive, &name) else {
            return Ok(None);
        };
        let resource = archive.resource(at).map_err(|error| error.to_string())?;
        if resource.type_id != class {
            return Err(format!(
                "{name}: expected controller {class:#x}, found {:#x}",
                resource.type_id
            ));
        }
        Controller::parse(&resource.body).map(Some)
    };
    let time_of_day =
        read(archive, TIME_OF_DAY)?.ok_or_else(|| format!("TOD_{look}: time-of-day controller absent"))?;
    let weather = read(archive, WEATHER)?;
    Ok(LookControllers { time_of_day, weather })
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "fixtures"
)]
mod tests {
    use super::*;
    #[test]
    fn packed_first_time_is_preserved_and_noon_interpolates_the_curve() {
        let mut b = vec![1];
        b.extend(1u32.to_le_bytes());
        for class in [ENTRY, PATH] {
            b.extend([0; 8]);
            b.extend(class.to_le_bytes());
        }
        b.extend(1u32.to_le_bytes());
        b.extend([0; 8]);
        b.extend(NODE.to_le_bytes());
        b.extend([255; 2]);
        b.extend([0; 8]);
        b.extend([1, 0, 0]);
        b.extend(primase_foundations::hash::crc32(b"Density").to_le_bytes());
        b.extend([0; 25]);
        b.push(3);
        b.extend([0; 2]);
        b.extend([0; 8]);
        b.extend([0; 8]);
        b.extend(FLOAT.to_le_bytes());
        b.extend(((1200u32 << 16) | 2).to_le_bytes());
        b.extend(1680u16.to_le_bytes());
        b.extend(2f32.to_le_bytes());
        b.extend(4f32.to_le_bytes());
        b.extend([0; 5]);
        let c = Controller::parse(&b).unwrap();
        assert_eq!(c.tracks[0].times, [1200, 1680]);
        assert_eq!(c.value(&["Density"], 1440.0).unwrap().unwrap()[0], 3.0);
        b.push(0);
        assert!(Controller::parse(&b).is_err());
    }
    #[test]
    fn a_look_names_its_time_of_day_and_weather_controllers() {
        assert_eq!(
            controller_name(TIME_OF_DAY, "MAD_EVENTS_Clear_Day").as_deref(),
            Some("TOD_MAD_EVENTS_Clear_Day")
        );
        assert_eq!(
            controller_name(WEATHER, "MAD_EVENTS_Clear_Day").as_deref(),
            Some("WEA_MAD_EVENTS_Clear_Day")
        );
        assert_eq!(controller_name(0x1234_5678, "look"), None);
    }
}
