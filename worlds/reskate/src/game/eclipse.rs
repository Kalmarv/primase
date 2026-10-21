//! An ECLP resource (the resource an `EclipseAnimationAsset` names, codec `0x45434c50`) as keyed tracks, exactly as
//! stored: the layout skate.'s executable encodes (checked against its encoder), read here without a world, names or
//! frame conversion. Which track drives which joint (`ChannelToDofAsset`), the frame rate and the joint mapping belong
//! to the caller.
//!
//! The resource is 16 little-endian `u32`: for rotations, vectors and floats each a record count, the record table's
//! offset and a word that must be 0; then the quaternion, vector, float and time formats; a word that must be 0 (0 in all
//! 4469 ECLP resources of the base game's data; its meaning is unknown); the size of the encoded stream and its offset (`data`). Key times and values live in
//! the stream, addressed from `data`; the record tables are addressed from the resource's start.
//!
//! Read: the formats listed on [`decode`]. Refused, each with an error naming it: a stream, record table or key
//! outside the bytes, a non-zero reserved word, a format this reader does not decode (including quaternion format 0 and
//! the 32-bit quaternion form, which the executable has but no sample uses), a track of 0 keys, key frames not strictly
//! increasing, a quaternion whose squared norm is not 1 within 1e-4 (formats 1 and 2), and a non-finite value.

/// The four format words of the header, as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Formats {
    pub quaternion: u32,
    pub vector: u32,
    pub scalar: u32,
    pub time: u32,
}

/// One track: `values[i]` is the value at key frame `frames[i]`. Frame numbers are strictly increasing, and the two
/// vectors have the same, non-zero length.
#[derive(Clone, Debug, PartialEq)]
pub struct Track<T> {
    pub frames: Vec<u32>,
    pub values: Vec<T>,
}

/// A decoded ECLP resource: its formats and its tracks in stored order.
#[derive(Clone, Debug, PartialEq)]
pub struct Eclipse {
    pub formats: Formats,
    /// Quaternions `[x, y, z, w]`: unit (formats 1 and 2 are checked), format 3's as stored.
    pub rotations: Vec<Track<[f32; 4]>>,
    pub translations: Vec<Track<[f32; 3]>>,
    pub scalars: Vec<Track<f32>>,
}

/// Scale of the 48-bit smallest-three form's components.
const Q48_SCALE: f64 = 2.157951530534774e-5;
/// Scales of packed64's two 21-bit components and its 20-bit component.
const Q64_SCALE_21: f64 = 6.743499056938163e-7;
const Q64_SCALE_20: f64 = 1.3487003798218211e-6;
/// How far from 1 a reconstructed quaternion's squared norm may be.
const NORM_TOLERANCE: f64 = 1e-4;

const HEADER_WORDS: usize = 16;
const ROTATION_RECORD: usize = 12;
const VECTOR_RECORD: usize = 48;
const FLOAT_RECORD: usize = 20;

/// `len` bytes at `offset`, or an error naming `what`.
fn at<'a>(b: &'a [u8], offset: usize, len: usize, what: &str) -> Result<&'a [u8], String> {
    offset
        .checked_add(len)
        .and_then(|end| b.get(offset..end))
        .ok_or_else(|| format!("{what}: {len} bytes at {offset} run past the {} bytes", b.len()))
}

fn word(b: &[u8], offset: usize, what: &str) -> Result<u32, String> {
    let w = at(b, offset, 4, what)?;
    Ok(u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
}

fn float(b: &[u8], offset: usize, what: &str) -> Result<f32, String> {
    Ok(f32::from_bits(word(b, offset, what)?))
}

fn size(w: u32, what: &str) -> Result<usize, String> {
    usize::try_from(w).map_err(|_| format!("{what}: {w} does not fit"))
}

/// The encoded stream: the whole resource, where the stream starts in it, and the time format its key times use.
struct Stream<'a> {
    bytes: &'a [u8],
    data: usize,
    time: u32,
}

impl Stream<'_> {
    /// `len` bytes of element `index` (of `stride` bytes each) of the array that starts `offset` bytes into the stream.
    fn element(
        &self,
        offset: u32,
        index: usize,
        stride: usize,
        len: usize,
        what: &str,
    ) -> Result<&[u8], String> {
        let start = size(offset, what)?
            .checked_add(self.data)
            .and_then(|s| index.checked_mul(stride).and_then(|o| s.checked_add(o)))
            .ok_or_else(|| format!("{what}: offset overflows"))?;
        at(self.bytes, start, len, what)
    }
}

/// The quaternion whose three kept components are `kept` (in order, filling the slots but `omitted`), the omitted
/// one non-negative and making it unit. Computed in f64, rounded once.
fn quaternion(kept: [f64; 3], omitted: usize, what: &str) -> Result<[f32; 4], String> {
    let rest = (1.0 - kept.iter().map(|v| v * v).sum::<f64>()).max(0.0).sqrt();
    let mut kept = kept.into_iter();
    let q: [f64; 4] = std::array::from_fn(|i| {
        if i == omitted {
            rest
        } else {
            kept.next().unwrap_or(f64::NAN)
        }
    });
    let norm = q.iter().map(|v| v * v).sum::<f64>();
    if (norm - 1.0).abs() > NORM_TOLERANCE {
        return Err(format!("{what}: a quaternion of squared norm {norm}, not 1"));
    }
    Ok(q.map(|v| v as f32))
}

/// packed64 from 8 bytes: little-endian `v`; the omitted index is `v & 3`.
fn packed64(raw: &[u8], what: &str) -> Result<[f32; 4], String> {
    let v = u64::from_le_bytes(
        raw.try_into()
            .map_err(|_| format!("{what}: packed64 is not 8 bytes"))?,
    );
    let c21 = |shift: u32| ((((v >> shift) & 0x1f_ffff) as i64) - 1_048_576) as f64 * Q64_SCALE_21;
    let c20 = ((((v >> 2) & 0xf_ffff) as i64) - 524_288) as f64 * Q64_SCALE_20;
    quaternion([c21(43), c21(22), c20], (v & 3) as usize, what)
}

/// The 48-bit smallest-three form from 6 bytes: three big-endian `u16` `a`, `c`, `d`; the omitted index is
/// `((a & 1) << 1) | (c & 1)`.
fn smallest_three_48(w: &[u8], what: &str) -> Result<[f32; 4], String> {
    let [a0, a1, c0, c1, d0, d1] = *w else {
        return Err(format!("{what}: a 48-bit quaternion is not 6 bytes"));
    };
    let (a, c, d) = (
        u16::from_be_bytes([a0, a1]),
        u16::from_be_bytes([c0, c1]),
        u16::from_be_bytes([d0, d1]),
    );
    let omitted = (usize::from(a & 1) << 1) | usize::from(c & 1);
    let k = |v: u16| (i64::from(v) - 32768) as f64 * Q48_SCALE;
    quaternion([k(a & !1), k(c & !1), k(d)], omitted, what)
}

fn increasing(frames: &[u32], what: &str) -> Result<(), String> {
    match frames.windows(2).position(|w| w[0] >= w[1]) {
        Some(i) => Err(format!(
            "{what}: key frames are not strictly increasing ({} then {} at key {})",
            frames[i],
            frames[i + 1],
            i + 1
        )),
        None => Ok(()),
    }
}

/// The `count` key frames starting `offset` bytes into the stream, in the stream's time format.
fn times(s: &Stream, count: usize, offset: u32, what: &str) -> Result<Vec<u32>, String> {
    if count == 0 {
        return Err(format!("{what}: a track of 0 keys"));
    }
    let width = match s.time {
        0 => 1,
        1 => 2,
        t => return Err(format!("{what}: unknown time format {t}")),
    };
    let len = count
        .checked_mul(width)
        .ok_or_else(|| format!("{what}: key count overflows"))?;
    let raw = s.element(offset, 0, width, len, &format!("{what} key times"))?;
    let frames: Vec<u32> = if width == 1 {
        raw.iter().map(|&t| u32::from(t)).collect()
    } else {
        raw.as_chunks::<2>()
            .0
            .iter()
            .map(|&t| u32::from(u16::from_be_bytes(t)))
            .collect()
    };
    increasing(&frames, what)?;
    Ok(frames)
}

/// A vector or float record's track (they share their rules): count 2 stores the two key frames in the record's
/// offset words and `two` (`lo`, `extent`) as the values; else the keys are read as for rotations and key `j`'s
/// value is `value(j)`.
fn keyed<T>(
    (count, kt, kv): (u32, u32, u32),
    s: &Stream,
    two: [T; 2],
    what: &str,
    value: impl Fn(usize) -> Result<T, String>,
) -> Result<Track<T>, String> {
    if count == 2 {
        let frames = vec![kt, kv];
        increasing(&frames, what)?;
        return Ok(Track {
            frames,
            values: two.into(),
        });
    }
    let n = size(count, what)?;
    let frames = times(s, n, kt, what)?;
    let values = (0..n).map(value).collect::<Result<Vec<T>, String>>()?;
    Ok(Track { frames, values })
}

/// `lo + t * extent` in f64 from the stored f32s, rounded once.
fn dequantise(lo: f32, extent: f32, t: f64) -> f32 {
    (f64::from(lo) + t * f64::from(extent)) as f32
}

fn finite<const N: usize>(v: [f32; N], what: &str) -> Result<[f32; N], String> {
    if v.iter().all(|x| x.is_finite()) {
        Ok(v)
    } else {
        Err(format!("{what}: a non-finite value"))
    }
}

/// The `count` records of `stride` bytes at `offset`, each as a slice; the whole table must lie in the bytes.
fn records<'a>(
    b: &'a [u8],
    count: u32,
    offset: u32,
    stride: usize,
    what: &str,
) -> Result<Vec<&'a [u8]>, String> {
    let total = size(count, what)?
        .checked_mul(stride)
        .ok_or_else(|| format!("{what}: record count overflows"))?;
    let table = at(b, size(offset, what)?, total, &format!("{what} record table"))?;
    Ok(table.chunks_exact(stride).collect())
}

/// Decode the ECLP resource `bytes`. Read:
/// - rotation records (12 bytes: count, key times offset, key values offset). Count 1 is one key at frame 0 whose
///   value is a packed64 quaternion stored inline at record + 4. Otherwise `count` key times (time format 1:
///   big-endian `u16`; 0: `u8`) and `count` values by quaternion format: 1 the 48-bit smallest-three form, 2 packed64,
///   3 four little-endian `f32`;
/// - vector records (48 bytes: count, times offset, values offset, at +16 `lo`, at +32 `extent`, three `f32` each).
///   Count 2 is two keys at the frames the offset words hold, with values `lo` and `extent` as stored. Otherwise by
///   vector format 0 three `u8` (`lo + q / 255 * extent`), 1 three big-endian `u16` (`/ 65535`), 2 three
///   little-endian `f32`;
/// - float records (20 bytes: count, times offset, values offset, `lo`, `extent`): the same, one component.
///
/// Anything else is refused (see the module documentation). A count of 0 is refused: the encoder never writes an
/// empty track. Quantised vector and float values are computed in f64 from the stored f32 `lo` and `extent` and rounded once.
pub fn decode(bytes: &[u8]) -> Result<Eclipse, String> {
    let h = |i: usize| word(bytes, i * 4, "header");
    at(bytes, 0, HEADER_WORDS * 4, "header")?;
    for (i, what) in [
        (2, "rotation table"),
        (5, "vector table"),
        (8, "float table"),
        (13, "unused"),
    ] {
        if h(i)? != 0 {
            return Err(format!("header word {i} ({what}, reserved) is {}, not 0", h(i)?));
        }
    }
    let formats = Formats {
        quaternion: h(9)?,
        vector: h(10)?,
        scalar: h(11)?,
        time: h(12)?,
    };
    let data = size(h(15)?, "stream offset")?;
    at(bytes, data, size(h(14)?, "stream size")?, "encoded stream")?;
    let s = Stream {
        bytes,
        data,
        time: formats.time,
    };

    let mut rotations = Vec::new();
    for (i, r) in records(bytes, h(0)?, h(1)?, ROTATION_RECORD, "rotation")?
        .into_iter()
        .enumerate()
    {
        let what = format!("rotation {i}");
        let (n, kt, kv) = (word(r, 0, &what)?, word(r, 4, &what)?, word(r, 8, &what)?);
        let (frames, values) = if n == 1 {
            (vec![0], vec![packed64(&r[4..12], &what)?])
        } else {
            let count = size(n, &what)?;
            let frames = times(&s, count, kt, &what)?;
            let values = (0..count)
                .map(|j| match formats.quaternion {
                    1 => smallest_three_48(s.element(kv, j, 6, 6, &what)?, &what),
                    2 => packed64(s.element(kv, j, 8, 8, &what)?, &what),
                    3 => {
                        let q = s.element(kv, j, 16, 16, &what)?;
                        finite(
                            std::array::from_fn(|c| {
                                f32::from_le_bytes([q[4 * c], q[4 * c + 1], q[4 * c + 2], q[4 * c + 3]])
                            }),
                            &what,
                        )
                    }
                    f => Err(format!("{what}: unsupported quaternion format {f}")),
                })
                .collect::<Result<Vec<_>, _>>()?;
            (frames, values)
        };
        rotations.push(Track { frames, values });
    }

    let mut translations = Vec::new();
    for (i, r) in records(bytes, h(3)?, h(4)?, VECTOR_RECORD, "vector")?
        .into_iter()
        .enumerate()
    {
        let what = format!("vector {i}");
        let rec = (word(r, 0, &what)?, word(r, 4, &what)?, word(r, 8, &what)?);
        let triple = |o: usize| -> Result<[f32; 3], String> {
            Ok([
                float(r, o, &what)?,
                float(r, o + 4, &what)?,
                float(r, o + 8, &what)?,
            ])
        };
        let (lo, extent) = (triple(16)?, triple(32)?);
        let kv = rec.2;
        let track = keyed(
            rec,
            &s,
            [finite(lo, &what)?, finite(extent, &what)?],
            &what,
            |j| {
                let at = |stride: usize| s.element(kv, j, stride, stride, &what);
                let v: [f32; 3] = match formats.vector {
                    0 => {
                        let q = at(3)?;
                        std::array::from_fn(|c| dequantise(lo[c], extent[c], f64::from(q[c]) / 255.0))
                    }
                    1 => {
                        let q = at(6)?;
                        std::array::from_fn(|c| {
                            dequantise(
                                lo[c],
                                extent[c],
                                f64::from(u16::from_be_bytes([q[2 * c], q[2 * c + 1]])) / 65535.0,
                            )
                        })
                    }
                    2 => {
                        let q = at(12)?;
                        std::array::from_fn(|c| {
                            f32::from_le_bytes([q[4 * c], q[4 * c + 1], q[4 * c + 2], q[4 * c + 3]])
                        })
                    }
                    f => return Err(format!("{what}: unsupported vector format {f}")),
                };
                finite(v, &what)
            },
        )?;
        translations.push(track);
    }

    let mut scalars = Vec::new();
    for (i, r) in records(bytes, h(6)?, h(7)?, FLOAT_RECORD, "float")?
        .into_iter()
        .enumerate()
    {
        let what = format!("float {i}");
        let rec = (word(r, 0, &what)?, word(r, 4, &what)?, word(r, 8, &what)?);
        let (lo, extent) = (float(r, 12, &what)?, float(r, 16, &what)?);
        let kv = rec.2;
        let track = keyed(
            rec,
            &s,
            [finite([lo], &what)?[0], finite([extent], &what)?[0]],
            &what,
            |j| {
                let v = match formats.scalar {
                    0 => dequantise(lo, extent, f64::from(s.element(kv, j, 1, 1, &what)?[0]) / 255.0),
                    1 => {
                        let q = s.element(kv, j, 2, 2, &what)?;
                        dequantise(lo, extent, f64::from(u16::from_be_bytes([q[0], q[1]])) / 65535.0)
                    }
                    2 => {
                        let q = s.element(kv, j, 4, 4, &what)?;
                        f32::from_le_bytes([q[0], q[1], q[2], q[3]])
                    }
                    f => return Err(format!("{what}: unsupported float format {f}")),
                };
                finite([v], &what).map(|[v]| v)
            },
        )?;
        scalars.push(track);
    }
    Ok(Eclipse {
        formats,
        rotations,
        translations,
        scalars,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resource builder: the header, then the three record tables, then the stream.
    #[derive(Default)]
    struct Clip {
        formats: [u32; 4],
        rotations: Vec<[u32; 3]>,
        vectors: Vec<Vec<u8>>,
        floats: Vec<Vec<u8>>,
        stream: Vec<u8>,
    }

    fn le(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    fn f32s(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn be(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }

    /// A vector record: count, key times offset, key values offset, `lo`, `extent`.
    fn vector(count: u32, kt: u32, kv: u32, lo: [f32; 3], extent: [f32; 3]) -> Vec<u8> {
        [
            le(&[count, kt, kv, 0]),
            f32s(&lo),
            f32s(&[0.0]),
            f32s(&extent),
            f32s(&[0.0]),
        ]
        .concat()
    }

    /// A float record: count, key times offset, key values offset, `lo`, `extent`.
    fn scalar(count: u32, kt: u32, kv: u32, lo: f32, extent: f32) -> Vec<u8> {
        [le(&[count, kt, kv]), f32s(&[lo, extent])].concat()
    }

    impl Clip {
        /// Appends `bytes` to the stream and returns their offset in it.
        fn put(&mut self, bytes: &[u8]) -> u32 {
            let at = self.stream.len() as u32;
            self.stream.extend(bytes);
            at
        }

        fn bytes(&self) -> Vec<u8> {
            let rotation_table = 64;
            let vector_table = rotation_table + self.rotations.len() * ROTATION_RECORD;
            let float_table = vector_table + self.vectors.len() * VECTOR_RECORD;
            let data = float_table + self.floats.len() * FLOAT_RECORD;
            let mut b = le(&[
                self.rotations.len() as u32,
                rotation_table as u32,
                0,
                self.vectors.len() as u32,
                vector_table as u32,
                0,
                self.floats.len() as u32,
                float_table as u32,
                0,
                self.formats[0],
                self.formats[1],
                self.formats[2],
                self.formats[3],
                0,
                self.stream.len() as u32,
                data as u32,
            ]);
            b.extend(self.rotations.iter().flat_map(|r| le(r)));
            b.extend(self.vectors.concat());
            b.extend(self.floats.concat());
            b.extend(&self.stream);
            b
        }
    }

    /// packed64 from the biased integer components (`a`, `b` 21-bit, `c` 20-bit) and the omitted index.
    fn pack64(a: u64, b: u64, c: u64, omitted: u64) -> [u8; 8] {
        ((a << 43) | (b << 22) | (c << 2) | omitted).to_le_bytes()
    }

    /// `[.5, .5, .5, .5]` with component `omitted` left out.
    fn half64(omitted: u64) -> [u8; 8] {
        // 0.5 / scale, rounded, plus the bias
        pack64(
            741_455 + 1_048_576,
            741_455 + 1_048_576,
            370_727 + 524_288,
            omitted,
        )
    }

    fn near(a: &[f32], b: &[f32], tolerance: f32) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() <= tolerance, "{a:?} against {b:?}");
        }
    }

    fn refused(b: &[u8]) -> String {
        decode(b).expect_err("must be refused")
    }

    /// A clip of one rotation track of two keys, the keys' times and values being `times` and `values`.
    fn rotation_clip(quaternion_format: u32, time_format: u32, times: &[u8], values: &[u8]) -> Clip {
        let mut c = Clip {
            formats: [quaternion_format, 0, 0, time_format],
            ..Clip::default()
        };
        let kt = c.put(times);
        let kv = c.put(values);
        c.rotations = vec![[2, kt, kv]];
        c
    }

    #[test]
    fn smallest_three_48_keys_with_u16_times() {
        // key 0: identity (w omitted: both flag bits set); key 1: [.5, .5, .5, .5] with x omitted
        let half = 23_170u16 + 32_768;
        let c = rotation_clip(
            1,
            1,
            &be(&[0, 40]),
            &be(&[32_768 | 1, 32_768 | 1, 32_768, half & !1, half & !1, half]),
        );
        let e = decode(&c.bytes()).unwrap();
        assert_eq!(
            e.formats,
            Formats {
                quaternion: 1,
                vector: 0,
                scalar: 0,
                time: 1
            }
        );
        assert_eq!(e.rotations[0].frames, [0, 40]);
        near(&e.rotations[0].values[0], &[0.0, 0.0, 0.0, 1.0], 0.0);
        near(&e.rotations[0].values[1], &[0.5, 0.5, 0.5, 0.5], 1e-3);
    }

    #[test]
    fn smallest_three_48_omitted_index_comes_from_the_low_bits() {
        // each index in turn: the kept values 0, 0, 0 fill the other slots, the omitted one is 1
        for omitted in 0..4usize {
            let (a, c) = (32_768 | (omitted >> 1) as u16, 32_768 | (omitted & 1) as u16);
            let k = rotation_clip(1, 0, &[0, 5], &be(&[a, c, 32_768, a, c, 32_768]));
            let q = decode(&k.bytes()).unwrap().rotations[0].values[1];
            let mut expected = [0.0; 4];
            expected[omitted] = 1.0;
            assert_eq!(q, expected, "omitted {omitted}");
        }
    }

    #[test]
    fn packed64_keys_with_u8_times() {
        let values = [half64(3), pack64(1_048_576, 1_048_576, 524_288, 0)].concat();
        let c = rotation_clip(2, 0, &[3, 9], &values);
        let e = decode(&c.bytes()).unwrap();
        assert_eq!(e.rotations[0].frames, [3, 9]);
        near(&e.rotations[0].values[0], &[0.5, 0.5, 0.5, 0.5], 1e-5);
        near(&e.rotations[0].values[1], &[1.0, 0.0, 0.0, 0.0], 0.0);
    }

    #[test]
    fn packed64_inline_single_key_is_frame_zero() {
        // the record's last two words are the quaternion, whatever the quaternion format is
        let q = half64(2);
        let word = |i: usize| u32::from_le_bytes(q[i..i + 4].try_into().unwrap());
        let c = Clip {
            formats: [3, 0, 0, 1],
            rotations: vec![[1, word(0), word(4)]],
            ..Clip::default()
        };
        let e = decode(&c.bytes()).unwrap();
        assert_eq!(e.rotations[0].frames, [0]);
        near(&e.rotations[0].values[0], &[0.5, 0.5, 0.5, 0.5], 1e-5);
    }

    #[test]
    fn float_quaternions_are_as_stored() {
        let c = rotation_clip(3, 0, &[0, 1], &f32s(&[0.0, 0.0, 0.0, 1.0, 0.25, 0.5, 0.75, 2.0]));
        let e = decode(&c.bytes()).unwrap();
        assert_eq!(
            e.rotations[0].values,
            [[0.0, 0.0, 0.0, 1.0], [0.25, 0.5, 0.75, 2.0]]
        );
    }

    #[test]
    fn two_key_vector_and_float_tracks_are_their_lo_and_extent() {
        let c = Clip {
            formats: [1, 1, 1, 1],
            vectors: vec![vector(2, 0, 30, [1.0, 2.0, 3.0], [4.0, 5.0, 6.0])],
            floats: vec![scalar(2, 7, 70, -1.0, 0.5)],
            ..Clip::default()
        };
        let e = decode(&c.bytes()).unwrap();
        assert_eq!(e.translations[0].frames, [0, 30]);
        assert_eq!(e.translations[0].values, [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
        assert_eq!(e.scalars[0].frames, [7, 70]);
        assert_eq!(e.scalars[0].values, [-1.0, 0.5]);
    }

    /// One vector track and one float track of three keys at frames 1, 4, 9 (u8 times), with `vector_values` and
    /// `float_values` as the stream's key values.
    fn quantised(
        vector_format: u32,
        float_format: u32,
        vector_values: &[u8],
        float_values: &[u8],
    ) -> Eclipse {
        let mut c = Clip {
            formats: [1, vector_format, float_format, 0],
            ..Clip::default()
        };
        let kt = c.put(&[1, 4, 9]);
        let (vkv, fkv) = (c.put(vector_values), c.put(float_values));
        c.vectors = vec![vector(3, kt, vkv, [1.0, -2.0, 0.0], [255.0, 255.0, 1.0])];
        c.floats = vec![scalar(3, kt, fkv, 10.0, 5.0)];
        decode(&c.bytes()).unwrap()
    }

    #[test]
    fn quantised_u8_vectors_and_floats() {
        let e = quantised(0, 0, &[0, 0, 0, 255, 255, 255, 51, 102, 153], &[0, 51, 255]);
        assert_eq!(e.translations[0].frames, [1, 4, 9]);
        near(&e.translations[0].values[0], &[1.0, -2.0, 0.0], 0.0);
        near(&e.translations[0].values[1], &[256.0, 253.0, 1.0], 1e-4);
        near(&e.translations[0].values[2], &[52.0, 100.0, 0.6], 1e-4);
        assert_eq!(e.scalars[0].frames, [1, 4, 9]);
        near(&e.scalars[0].values, &[10.0, 11.0, 15.0], 1e-5);
    }

    #[test]
    fn quantised_u16_vectors_and_floats() {
        let e = quantised(
            1,
            1,
            &be(&[0, 0, 0, 65_535, 65_535, 65_535, 0, 65_535, 0]),
            &be(&[0, 65_535, 32_768]),
        );
        near(&e.translations[0].values[0], &[1.0, -2.0, 0.0], 0.0);
        near(&e.translations[0].values[1], &[256.0, 253.0, 1.0], 1e-4);
        near(&e.translations[0].values[2], &[1.0, 253.0, 0.0], 1e-4);
        near(&e.scalars[0].values, &[10.0, 15.0, 12.5], 1e-4);
    }

    #[test]
    fn f32_vectors_and_floats_ignore_lo_and_extent() {
        let e = quantised(
            2,
            2,
            &f32s(&[1.0, 2.0, 3.0, -1.5, 0.0, 9.0, 0.5, 0.5, 0.5]),
            &f32s(&[0.25, -8.0, 3.0]),
        );
        assert_eq!(e.translations[0].values[1], [-1.5, 0.0, 9.0]);
        assert_eq!(e.scalars[0].values, [0.25, -8.0, 3.0]);
    }

    #[test]
    fn unknown_formats_are_refused() {
        let times = [0u8, 1];
        for f in [0, 4] {
            let e = refused(&rotation_clip(f, 0, &times, &[0; 32]).bytes());
            assert!(e.contains(&format!("unsupported quaternion format {f}")), "{e}");
        }
        let e = refused(&rotation_clip(2, 2, &times, &[0; 16]).bytes());
        assert!(e.contains("unknown time format 2"), "{e}");
        let mut c = Clip {
            formats: [1, 3, 0, 0],
            ..Clip::default()
        };
        let kt = c.put(&[0, 1, 2]);
        let kv = c.put(&[0; 36]);
        c.vectors = vec![vector(3, kt, kv, [0.0; 3], [0.0; 3])];
        assert!(refused(&c.bytes()).contains("unsupported vector format 3"));
        c.vectors.clear();
        c.formats = [1, 0, 3, 0];
        c.floats = vec![scalar(3, kt, kv, 0.0, 0.0)];
        assert!(refused(&c.bytes()).contains("unsupported float format 3"));
    }

    #[test]
    fn structure_outside_the_bytes_is_refused() {
        let good = rotation_clip(2, 0, &[0, 1], &[half64(0), half64(0)].concat()).bytes();
        decode(&good).unwrap();
        // the stream runs past the bytes
        let mut b = good.clone();
        b[56..60].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(refused(&b).contains("encoded stream"));
        // a record table past the bytes (a count that would overflow the table's size too)
        for count in [1000u32, u32::MAX] {
            let mut b = good.clone();
            b[0..4].copy_from_slice(&count.to_le_bytes());
            assert!(refused(&b).contains("rotation record table"));
        }
        // key times and values past the bytes
        let mut c = rotation_clip(2, 1, &be(&[0, 1]), &[0; 16]);
        c.rotations[0][1] = u32::MAX;
        assert!(refused(&c.bytes()).contains("key times"));
        let mut c = rotation_clip(2, 1, &be(&[0, 1]), &[0; 16]);
        c.rotations[0][2] = 1000;
        assert!(refused(&c.bytes()).contains("run past"));
        // a short header
        assert!(refused(&good[..40]).contains("header"));
    }

    #[test]
    fn unknown_layout_is_refused() {
        let mut b = rotation_clip(2, 0, &[0, 1], &[half64(0), half64(0)].concat()).bytes();
        for word in [2usize, 5, 8, 13] {
            let mut r = b.clone();
            r[word * 4] = 1;
            assert!(refused(&r).contains(&format!("header word {word}")));
        }
        // a track of no keys
        b[64..68].copy_from_slice(&0u32.to_le_bytes());
        assert!(refused(&b).contains("0 keys"));
    }

    #[test]
    fn non_increasing_key_frames_are_refused() {
        for times in [[5u8, 5], [6, 5]] {
            let e = refused(&rotation_clip(2, 0, &times, &[half64(0), half64(0)].concat()).bytes());
            assert!(e.contains("not strictly increasing"), "{e}");
        }
        let c = Clip {
            formats: [1, 1, 1, 1],
            vectors: vec![vector(2, 9, 3, [0.0; 3], [0.0; 3])],
            ..Clip::default()
        };
        assert!(refused(&c.bytes()).contains("not strictly increasing"));
    }

    #[test]
    fn non_unit_and_non_finite_values_are_refused() {
        // three kept components of 1.0 each: squared norm 3
        let loud = pack64(2_097_151, 2_097_151, 1_048_575, 3);
        let e = refused(&rotation_clip(2, 0, &[0, 1], &[loud, loud].concat()).bytes());
        assert!(e.contains("squared norm"), "{e}");
        let nan = f32s(&[0.0, 0.0, 0.0, f32::NAN, 0.0, 0.0, 0.0, 1.0]);
        assert!(refused(&rotation_clip(3, 0, &[0, 1], &nan).bytes()).contains("non-finite"));
        let c = Clip {
            formats: [1, 1, 1, 1],
            vectors: vec![vector(2, 0, 1, [f32::INFINITY, 0.0, 0.0], [0.0; 3])],
            ..Clip::default()
        };
        assert!(refused(&c.bytes()).contains("non-finite"));
    }
}
