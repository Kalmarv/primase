//! GIDynResource's established one-volume/one-box profile (RR lighting.md §8, 418 offline bodies).
//! Preserve all four encoded streams. Probe positions are established; irradiance, visibility bit semantics,
//! sample origin and the GPU GIDynData texture construction are not. Nothing here invents probe colours.
use super::archive::{Archive, At};
use primase_core::{Entity, Name, World, frame::SourceFrame};

pub const GI_DYN_RESOURCE: u32 = 0xBAE9_8B30;
const VOLUME: u32 = 0x684A_17AD;
const DESC: u32 = 0xCE0C_489A;
const SUB: u32 = 0xACC1_B118;
const BOX: u32 = 0xE96B_B4A2;

#[derive(Clone, Debug, PartialEq)]
pub struct StoredGiDyn {
    pub world_id: u64,
    pub cell_id: u32,
    pub position: [f32; 3],
    pub samples: u32,
    pub probes: u32,
    pub indoor: bool,
    /// Integer voxel bounds, not metres. Voxel size must be supplied by the program's grid, never guessed here.
    pub voxel_bounds: [[i32; 3]; 2],
    pub subvolume_word: u32,
    pub streams: [Vec<u8>; 4],
    pub decoded_bytes: [u32; 3],
}

struct Cursor<'a> { data: &'a [u8], at: usize }
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).ok_or("GIDynResource: offset overflow")?;
        let value = self.data.get(self.at..end).ok_or_else(|| format!("GIDynResource truncated at {} reading {n}", self.at))?;
        self.at = end;
        Ok(value)
    }
    fn word(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| "GIDynResource word")?))
    }
    fn header(&mut self, class: u32) -> Result<(), String> {
        self.take(8)?;
        let found = self.word()?;
        if found != class { return Err(format!("GIDynResource class {found:#010x}, expected {class:#010x}")); }
        Ok(())
    }
}

/// Validate an encoded stream without allocating its decoded size. The u16 grid's flag bit is retained.
fn rle(data: &[u8], width: usize, expected: u64, probes: Option<u32>) -> Result<(), String> {
    let mut c = Cursor { data, at: 0 };
    let mut total = 0u64;
    let value = |c: &mut Cursor<'_>| -> Result<u32, String> {
        let b = c.take(width)?;
        match b { [a] => Ok(u32::from(*a)), [a, b] => Ok(u32::from(u16::from_le_bytes([*a, *b]))),
            _ => Err("GIDynResource: invalid RLE word width".into()) }
    };
    while c.at < data.len() {
        let first = value(&mut c)?;
        let (count, word) = if first == if width == 1 { 0xFE } else { 0xFFFE } {
            (value(&mut c)?, value(&mut c)?)
        } else { (1, first) };
        if count == 0 { return Err("GIDynResource: zero-length RLE escape".into()); }
        if let Some(probes) = probes && word != 0xFFFF && (word & 0x7FFF) >= probes {
            return Err(format!("GIDynResource grid probe {} outside {probes}", word & 0x7FFF));
        }
        total = total.checked_add(u64::from(count)).ok_or("GIDynResource RLE length overflow")?;
        if total > expected { return Err(format!("GIDynResource RLE exceeds {expected} values")); }
    }
    if total != expected { return Err(format!("GIDynResource RLE has {total} values, expected {expected}")); }
    Ok(())
}

fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = (bits >> 10) & 31;
    let fraction = f32::from(bits & 1023);
    sign * match exponent {
        0 => fraction * 2f32.powi(-24),
        31 => if fraction == 0.0 { f32::INFINITY } else { f32::NAN },
        _ => (1.0 + fraction / 1024.0) * 2f32.powi(i32::from(exponent) - 15),
    }
}

impl StoredGiDyn {
    pub fn parse(body: &[u8]) -> Result<Self, String> {
        let mut c = Cursor { data: body, at: 0 };
        if c.take(1)? != [1] { return Err("GIDynResource: unknown managed header".into()); }
        let world_id = u64::from_le_bytes(c.take(8)?.try_into().map_err(|_| "GIDynResource world id")?);
        let cell_id = c.word()?;
        let volumes = c.word()?;
        if volumes != 1 { return Err(format!("GIDynResource: {volumes} volumes, only the established one-volume profile is read")); }
        c.header(VOLUME)?;
        c.header(DESC)?;
        let position = [f32::from_bits(c.word()?), f32::from_bits(c.word()?), f32::from_bits(c.word()?)];
        if !position.iter().all(|x| x.is_finite()) { return Err("GIDynResource: nonfinite PositionOffset".into()); }
        let samples = c.word()?;
        let probes = c.word()?;
        let indoor = match c.take(1)? { [0] => false, [1] => true, _ => return Err("GIDynResource: invalid IsIndoor".into()) };
        let subvolumes = c.word()?;
        if subvolumes != 1 { return Err(format!("GIDynResource: {subvolumes} subvolumes, only the established one-box profile is read")); }
        c.header(SUB)?;
        c.header(BOX)?;
        let mut voxel_bounds = [[0; 3]; 2];
        for corner in &mut voxel_bounds { for axis in corner { *axis = i32::from_le_bytes(c.take(4)?.try_into().map_err(|_| "GIDynResource box")?); } }
        let mut voxels = 1u64;
        for axis in 0..3 {
            let extent = i64::from(voxel_bounds[1][axis]) - i64::from(voxel_bounds[0][axis]);
            if extent <= 0 { return Err("GIDynResource: empty or inverted voxel box".into()); }
            voxels = voxels.checked_mul(extent as u64).ok_or("GIDynResource voxel count overflow")?;
        }
        let subvolume_word = c.word()?;
        let mut streams: [Vec<u8>; 4] = std::array::from_fn(|_| Vec::new());
        for stream in &mut streams {
            let n = usize::try_from(c.word()?).map_err(|_| "GIDynResource stream size")?;
            *stream = c.take(n)?.to_vec();
        }
        let decoded_bytes = [c.word()?, c.word()?, c.word()?];
        if c.at != body.len() { return Err(format!("GIDynResource: {} trailing bytes", body.len() - c.at)); }
        if u64::from(decoded_bytes[0]) != u64::from(samples) * 10 || Some(u64::from(decoded_bytes[2])) != voxels.checked_mul(2) {
            return Err("GIDynResource: footer sample/grid byte sizes do not match counts".into());
        }
        rle(&streams[0], 2, u64::from(samples) * 5, None)?;
        rle(&streams[1], 1, u64::from(decoded_bytes[1]), None)?;
        rle(&streams[2], 2, voxels, Some(probes))?;
        if streams[3].len() as u64 != u64::from(probes) * 8 { return Err("GIDynResource: probe vector size is not 8 * probes".into()); }
        let result = Self { world_id, cell_id, position, samples, probes, indoor, voxel_bounds,
            subvolume_word, streams, decoded_bytes };
        for bytes in result.streams[3].chunks_exact(8) {
            if bytes[6..8] != [0, 0] { return Err("GIDynResource: nonzero probe fourth lane, profile not established".into()); }
        }
        if result.probe_positions().flatten().any(|x| !x.is_finite()) { return Err("GIDynResource: nonfinite probe position".into()); }
        Ok(result)
    }

    /// Source coordinates in metres; the fourth half lane is validated separately. No voxel-to-metre guess.
    pub fn probe_positions(&self) -> impl Iterator<Item = [f32; 3]> + '_ {
        self.streams[3].chunks_exact(8).map(|b| std::array::from_fn(|i| {
            self.position[i] + half(u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]))
        }))
    }
}

/// Explicit single-resource read. Area readers must select cells before calling this: never load all GI by default.
pub fn read_gi_dyn(archive: &mut Archive, world: &mut World, at: At, frame: SourceFrame) -> Result<Entity, String> {
    let name = archive.forge(at.forge).and_then(|f| f.names.get(at.entry)).map(|n| n.name.clone()).unwrap_or_default();
    let resource = archive.resource(at).map_err(|e| e.to_string())?;
    if resource.type_id != GI_DYN_RESOURCE { return Err(format!("GIDynResource: type {:#x}", resource.type_id)); }
    let stored = StoredGiDyn::parse(&resource.body)?;
    let entity = world.spawn();
    world.insert(entity, Name(name));
    world.insert(entity, crate::components::AnvilGiDyn { index_id: resource.class_id, frame, stored });
    Ok(entity)
}
