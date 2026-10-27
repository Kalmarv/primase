//! A rigid MeshSet and its geometry chunk built from triangle meshes ([`build`]): one LOD, one section a part, laid out
//! as the game's own one-LOD Quick Drop meshes are (the 73 of `Win32/buildkititems.toc`, each one `default_mat`
//! section; 60 store these three streams, 13 add a sub-material index, usage 51). `tests/rigid.rs` rebuilds each of
//! the 60 from its decoded geometry: the chunk comes out byte-identical but for tangent frames with two encodings
//! (the same frame either way), and every record field but the measured ones below byte-identical.
//!
//! A section's vertices are three streams: half-float positions (w 1), half-float UV0 and the packed tangent frame
//! ([`tangent`]), 8, 4 and 4 bytes. Sections lie back to back in the vertex buffer (16-aligned at its end), their
//! indices (each part's own vertices) back to back after it, 2 bytes each (4 when a part has more than 65,536
//! vertices), padded to 4 (the LOD's index bytes), the chunk then padded to 16.
//!
//! Measured from the parts' own (unquantised) data:
//! - bounds: section +0x150 / +0x160 its vertices' (w 0), header 0x10 / 0x20 every section's;
//! - section +0x14c: the surface area in square metres (measured from the decoded triangles of the game's 1,222
//!   rigid Quick Drop sections with UV0 against the stored value: 1st..99th percentile 0.996..1.003);
//! - section +0x58: the median over the triangles with area of `sqrt(UV area / area)`, UV0 units a metre (the same
//!   sections: 25th..75th percentile 0.996..1.001, 5th..95th 0.849..1.024; it does not fit sections whose UVs fold
//!   many triangles to nothing, as the 60 `default_mat` meshes'); 1.0 when no triangle has area; +0x5c the same for
//!   UV1, which these sections have not: 1.0.
//!
//! Named by [`djb2`]: header 0x78 the MeshSet's name, LOD +0xa4 its path part (`<name>_lod0`), section +0x144 the
//! material's name lowercased. Each section is in the LOD's lists 0, 3 and 4 and has flags 0x0e, as the game's opaque
//! shadow-casting sections (its decal sections are in list 2 alone with flags 0x0a, its shadow proxies in 3 and 4
//! with flags 0x04). Every other byte is the value all 73 store.

use half::f16;

use crate::meshset::{self, Decl, Lod, MeshSet, Section, format, usage};
use crate::{Result, bad, djb2, tangent};

const W: &str = "rigid mesh";
/// The section flags of an opaque, shadow-casting section.
pub const OPAQUE: u8 = 0x0e;
/// Bytes a vertex: position, UV0, tangent frame.
const STRIDES: [u8; 3] = [8, 4, 4];

/// One section: a material's triangles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Part {
    /// The material's name (section +0x08), its key (+0x138) and its index in the mesh asset's `Materials` (+0x1c).
    pub material: String,
    pub key: u64,
    pub slot: u8,
    pub positions: Vec<[f32; 3]>,
    /// Per vertex: the unit normal; the tangent and, in w, the bitangent's sign (the bitangent, `w * (normal x
    /// tangent)`, points where V grows, as in the game's meshes).
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 4]>,
    pub uv: Vec<[f32; 2]>,
    /// Indices into this part's vertices.
    pub triangles: Vec<[u32; 3]>,
}

/// A built mesh: the MeshSet (write it with [`meshset::write`]) and its LOD's geometry chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    pub meshset: MeshSet,
    pub chunk: Vec<u8>,
}

/// The declaration of the three streams.
pub fn declaration() -> Decl {
    let mut b = [0u8; meshset::DECL_SIZE];
    let elements = [
        (usage::POSITION, format::HALF4),
        (usage::TEXCOORD0, format::HALF2),
        (usage::TANGENT_SPACE, format::UBYTE4N),
    ];
    for e in 0..16 {
        b[4 * e..4 * e + 4].copy_from_slice(&match elements.get(e) {
            Some(&(u, f)) => [u, f, 0, e as u8],
            None => [0, 0, 0xff, 0],
        });
    }
    for (j, &s) in STRIDES.iter().enumerate() {
        b[64 + 2 * j] = s;
    }
    b[96] = elements.len() as u8;
    b[97] = STRIDES.len() as u8;
    Decl {
        elements: elements
            .iter()
            .enumerate()
            .map(|(e, &(usage, format))| meshset::Element {
                usage,
                format,
                offset: 0,
                stream: e as u8,
            })
            .collect(),
        strides: STRIDES.to_vec(),
        bytes: b,
    }
}

fn put(b: &mut [u8], at: usize, v: &[u8]) {
    b[at..at + v.len()].copy_from_slice(v);
}

fn f32s(b: &mut [u8], at: usize, v: &[f32]) {
    for (k, x) in v.iter().enumerate() {
        put(b, at + 4 * k, &x.to_le_bytes());
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// The area of the triangle `p`.
fn area(p: [[f64; 3]; 3]) -> f64 {
    let (u, v) = (sub(p[1], p[0]), sub(p[2], p[0]));
    let c = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    0.5 * (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt()
}

/// What [`build`] measures of a part (the module documentation's measures).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measures {
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// Surface area, square metres.
    pub area: f32,
    /// UV units a metre: the median over the triangles with area of `sqrt(UV area / area)`; 1.0 when none has area.
    pub uv_density: f32,
}

/// The part's bounds, surface area and UV0 density.
pub fn measure(p: &Part) -> Measures {
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in &p.positions {
        for k in 0..3 {
            min[k] = min[k].min(v[k]);
            max[k] = max[k].max(v[k]);
        }
    }
    let (mut total, mut density) = (0.0f64, Vec::with_capacity(p.triangles.len()));
    for t in &p.triangles {
        let a = area(t.map(|i| p.positions[i as usize].map(f64::from)));
        total += a;
        if a > 0.0 {
            let uv = area(t.map(|i| {
                let w = p.uv[i as usize];
                [f64::from(w[0]), f64::from(w[1]), 0.0]
            }));
            density.push((uv / a).sqrt());
        }
    }
    density.sort_by(f64::total_cmp);
    Measures {
        min,
        max,
        area: total as f32,
        uv_density: density.get(density.len() / 2).copied().unwrap_or(1.0) as f32,
    }
}

/// The MeshSet named `name` (`<path>/<short name>`, as the game names them: `world/.../<x>_mesh`) of `parts`, its
/// LOD's geometry in the chunk `chunk`. Refused: no part, more than 255 parts, a part without triangles, streams of
/// unequal length, an index past its part's vertices, a value that is not finite, a normal without length.
pub fn build(name: &str, chunk: [u8; 16], parts: &[Part]) -> Result<Built> {
    if parts.is_empty() || parts.len() > 255 {
        return Err(bad(W, 0, format!("{} parts (1..255)", parts.len())));
    }
    for (i, p) in parts.iter().enumerate() {
        let n = p.positions.len();
        let refuse = |what: String| Err(bad(W, i, format!("part {i} ({}): {what}", p.material)));
        if p.normals.len() != n || p.tangents.len() != n || p.uv.len() != n {
            return refuse("streams of unequal length".into());
        }
        if p.triangles.is_empty() || n == 0 {
            return refuse("no triangles".into());
        }
        if let Some(t) = p.triangles.iter().find(|t| t.iter().any(|&x| x as usize >= n)) {
            return refuse(format!("triangle {t:?} past its {n} vertices"));
        }
        let finite = p.positions.iter().flatten().chain(p.normals.iter().flatten());
        if !finite
            .chain(p.tangents.iter().flatten())
            .chain(p.uv.iter().flatten())
            .all(|v| v.is_finite())
        {
            return refuse("a value that is not finite".into());
        }
    }
    let index_size: u8 = if parts.iter().all(|p| p.positions.len() <= 1 << 16) {
        2
    } else {
        4
    };
    let mut data = Vec::new();
    let mut sections = Vec::with_capacity(parts.len());
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    let decl = declaration();
    for (i, p) in parts.iter().enumerate() {
        let vertex_offset = u32::try_from(data.len()).map_err(|_| bad(W, i, "vertex buffer past 4 GiB"))?;
        for v in &p.positions {
            for c in [v[0], v[1], v[2], 1.0] {
                data.extend(f16::from_f32(c).to_bits().to_le_bytes());
            }
        }
        for v in &p.uv {
            for c in v {
                data.extend(f16::from_f32(*c).to_bits().to_le_bytes());
            }
        }
        for (k, (n, t)) in p.normals.iter().zip(&p.tangents).enumerate() {
            let b = tangent::encode(*n, [t[0], t[1], t[2]], t[3])
                .ok_or_else(|| bad(W, i, format!("part {i}: vertex {k}'s normal has no length")))?;
            data.extend(b);
        }
        let Measures {
            min: plo,
            max: phi,
            area: surface,
            uv_density: density,
        } = measure(p);
        for k in 0..3 {
            lo[k] = lo[k].min(plo[k]);
            hi[k] = hi[k].max(phi[k]);
        }
        let mut record = vec![0u8; meshset::SECTION_SIZE];
        put(&mut record, 0x1a, &[0x30]);
        put(&mut record, 0x1c, &[p.slot, 0]);
        record[0x1f] = 3;
        put(&mut record, 0x48, &[0xff; 8]);
        f32s(&mut record, 0x58, &[density, 1.0, 1.0, 1.0, 1.0, 1.0]);
        put(
            &mut record,
            0x144,
            &djb2(&p.material.to_lowercase()).to_le_bytes(),
        );
        f32s(&mut record, 0x14c, &[surface]);
        f32s(
            &mut record,
            0x150,
            &[plo[0], plo[1], plo[2], 0.0, phi[0], phi[1], phi[2], 0.0],
        );
        sections.push(Section {
            record,
            name: p.material.clone(),
            key: p.key,
            decl: decl.clone(),
            draw_decl: decl.clone(),
            flags: OPAQUE,
            stride: STRIDES.iter().sum(),
            primitives: p.triangles.len() as u32,
            vertex_offset,
            vertices: p.positions.len() as u32,
            ..Section::default()
        });
    }
    data.resize(crate::align(data.len(), 16), 0);
    let vertex_bytes = u32::try_from(data.len()).map_err(|_| bad(W, 0, "vertex buffer past 4 GiB"))?;
    let mut first = 0u32;
    for (p, s) in parts.iter().zip(&mut sections) {
        s.start_index = first;
        for &i in p.triangles.iter().flatten() {
            match index_size {
                2 => data.extend((i as u16).to_le_bytes()),
                _ => data.extend(i.to_le_bytes()),
            }
        }
        first += 3 * p.triangles.len() as u32;
    }
    data.resize(crate::align(data.len(), 4), 0);
    let index_bytes = data.len() as u32 - vertex_bytes;
    // the chunk ends 16-aligned
    data.resize(crate::align(data.len(), 16), 0);
    let short_name = name.rfind('/').map_or(0, |i| i + 1);
    let lod_name = format!("Mesh:{name}_lod0");
    let mut record = vec![0u8; meshset::LOD_PLAIN];
    put(&mut record, 0x04, &[0xff, 0xff, 0, 0]);
    // the last (only) LOD
    put(&mut record, 0x50, &[0x41]);
    put(&mut record, 0x84, &[0xff; 8]);
    put(&mut record, 0xa4, &djb2(&lod_name["Mesh:".len()..]).to_le_bytes());
    let every: Vec<u8> = (0..parts.len() as u8).collect();
    let lod = Lod {
        record,
        name: lod_name,
        name_parts: [
            meshset::NamePart::At("Mesh:".len()),
            meshset::NamePart::At("Mesh:".len() + short_name),
        ],
        lists: [every.clone(), Vec::new(), Vec::new(), every.clone(), every],
        chunk,
        index_size,
        index_bytes,
        vertex_bytes,
        sections,
        ..Lod::default()
    };
    let mut header = vec![0u8; meshset::HEADER];
    for (at, w) in [
        (0, meshset::HEADER - 16),
        (4, meshset::LOD_PLAIN),
        (8, meshset::SECTION_SIZE),
    ] {
        put(&mut header, at, &(w as u32).to_le_bytes());
    }
    f32s(
        &mut header,
        0x10,
        &[lo[0], lo[1], lo[2], 0.0, hi[0], hi[1], hi[2], 0.0],
    );
    put(&mut header, 0x78, &djb2(name).to_le_bytes());
    put(&mut header, 0x7c, &[0x00, 0x03, 0x00, 0xff]);
    put(&mut header, 0x9c, &[0x00, 0x00, 0x01, 0x00, 0x01]);
    put(&mut header, 0xa8, &[0x01, 0xff, 0xff, 0x80, 0x1e]);
    put(&mut header, 0xb0, &[0x1f, 0x7f]);
    // sections in all, and LOD 0's first (0)
    put(&mut header, 0xb6, &(parts.len() as u16).to_le_bytes());
    Ok(Built {
        meshset: MeshSet {
            header,
            name: name.to_owned(),
            short_name: meshset::NamePart::At(short_name),
            lods: vec![lod],
            ..MeshSet::default()
        },
        chunk: data,
    })
}
