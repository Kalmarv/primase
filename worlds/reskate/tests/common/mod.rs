//! Read-back helpers shared by the post-compile gates.

#![allow(clippy::unwrap_used, dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_reskate::postcompile::fb::bundle;
use primase_reskate::postcompile::fb::folder::Source;
use primase_reskate::postcompile::fb::{MESHSET, lookup};
use primase_reskate::postcompile::meshset;

pub type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub const BAM: &str = "Win32/levels/game/bam_levelroot/bam_levelroot.toc";
pub const ROOT: &str = "Win32/levels/game/dingolevel_root/dingolevel_root.toc";

/// Every MeshSet of every TOC of a Patch: (bundle, name) -> (payload, LOD chunk payloads by GUID).
pub type Meshes = BTreeMap<(String, String), (Vec<u8>, BTreeMap<[u8; 16], Vec<u8>>)>;

pub fn meshes(src: &Source) -> R<Meshes> {
    let mut out = BTreeMap::new();
    for rel in &src.folder.tocs {
        let t = src.folder.toc(rel)?;
        for (name, region) in &t.bundles {
            let (files, inline) = bundle::read_region(region)?;
            let Some((m, first, _)) = src.manifest(&files, &inline)? else {
                continue;
            };
            if first != 1 || !files[0].0.patch {
                continue;
            }
            for (i, a) in m.res.iter().enumerate().filter(|(_, a)| a.rtype == MESHSET) {
                let d = src.payload(files[m.res_file(i)], a.size as usize)?;
                let mut chunks = BTreeMap::new();
                for l in meshset::read(&d)?.lods {
                    // a LOD's chunk is in the bundle or, for content copied from the base game, in the TOC's table
                    let g = match m.chunks.iter().position(|c| c.guid == l.chunk) {
                        Some(ci) => src.payload(files[m.chunk_file(ci)], m.chunks[ci].lsize as usize)?,
                        None => {
                            let c = t
                                .chunks
                                .iter()
                                .find(|c| c.guid == l.chunk)
                                .ok_or("LOD chunk in neither bundle nor TOC")?;
                            let f = c.file.ok_or("removed chunk")?;
                            let raw = src.raw(f)?;
                            let n = primase_compress::cas::blocks(&raw)?
                                .iter()
                                .map(|b| b.decoded_size)
                                .sum();
                            src.payload(f, n)?
                        }
                    };
                    chunks.insert(l.chunk, g);
                }
                out.insert((name.clone(), a.name.clone()), (d, chunks));
            }
        }
    }
    Ok(out)
}

/// Program rows and streamable-texture-set rows of every lookup table of a Patch.
pub type Rows = (BTreeMap<u64, Vec<u32>>, BTreeMap<u64, u64>);

pub fn rows(src: &Source) -> R<Rows> {
    let (mut p, mut t) = (BTreeMap::new(), BTreeMap::new());
    for rel in &src.folder.tocs {
        let toc = src.folder.toc(rel)?;
        for (_, region) in &toc.bundles {
            let (files, inline) = bundle::read_region(region)?;
            let Some((m, first, _)) = src.manifest(&files, &inline)? else {
                continue;
            };
            for (i, a) in m.res.iter().enumerate() {
                let f = files[first + m.ebx.len() + i];
                if a.rtype == lookup::PROGRAM_LOOKUP {
                    p.extend(lookup::read_programs(&src.payload(f, a.size as usize)?, &a.rmeta)?.rows);
                } else if a.rtype == lookup::TEXTURE_LOOKUP {
                    t.extend(lookup::read_texture_sets(&src.payload(f, a.size as usize)?)?);
                }
            }
        }
    }
    Ok((p, t))
}

pub fn all_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut e: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    e.sort();
    for p in e {
        if p.is_dir() {
            all_files(&p, out)
        } else {
            out.push(p)
        }
    }
}

/// A section's stored stream `j`.
pub fn stream(s: &meshset::Section, g: &[u8], j: usize) -> Vec<u8> {
    let o = s.stream_offset(j);
    g[o..o + s.decl.strides[j] as usize * s.vertices as usize].to_vec()
}

/// The LOD's index buffer.
pub fn index_buffer(l: &meshset::Lod, g: &[u8]) -> Vec<u8> {
    g[l.vertex_bytes as usize..l.vertex_bytes as usize + l.index_bytes as usize].to_vec()
}
