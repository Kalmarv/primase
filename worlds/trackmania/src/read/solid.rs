//! A solid (`CPlugSolid`) as geometry in its own space ([`read_solid`]): its tree (`CPlugTree`) walked from the root,
//! each tree's location composed onto its parent's (`world = parent * location`, the iso4's matrix applied to column
//! vectors with its rows as stored), each indexed triangle visual a [`Part`] with the material its tree names, and each
//! tree's collision surface (`CPlugSurface`) a [`CollisionPart`]. A mip tree (`CPlugTreeVisualMip`) holds its levels'
//! trees: the walk for level `k` takes each mip tree's level `k`, or its last when it has fewer.
use std::sync::Arc;

use gbx_formats::plug::solid::SolidContent;
use gbx_formats::plug::surface::{Surf, SurfMaterial, material_id_name};
use gbx_formats::plug::tree::TreeData;
use gbx_formats::plug::visual::{VertexColors, VertexNormals, unpack_color, unpack_normal};
use gbx_formats::{Node, NodeIndex};

use crate::Result;
use crate::read::library::{At, Doc, Library};

/// An affine map: a 3x3 matrix by rows and a translation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub rows: [[f32; 3]; 3],
    pub translation: [f32; 3],
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        rows: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        translation: [0.0; 3],
    };

    pub fn point(&self, v: [f32; 3]) -> [f32; 3] {
        let m = &self.rows;
        std::array::from_fn(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2] + self.translation[r])
    }

    /// `self * other`: `other` applied first.
    pub fn then_local(&self, other: &Affine) -> Affine {
        let (p, l) = (&self.rows, &other.rows);
        Affine {
            rows: std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| p[r][k] * l[k][c]).sum())),
            translation: self.point(other.translation),
        }
    }

    fn determinant(&self) -> f32 {
        let m = &self.rows;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// A normal mapped by the matrix's cofactors (its inverse transpose up to scale), normalised.
    fn normal(&self, n: [f32; 3]) -> [f32; 3] {
        let m = &self.rows;
        let c = |r: usize, k: usize| {
            let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
            let (k1, k2) = ((k + 1) % 3, (k + 2) % 3);
            m[r1][k1] * m[r2][k2] - m[r1][k2] * m[r2][k1]
        };
        let v: [f32; 3] = std::array::from_fn(|r| c(r, 0) * n[0] + c(r, 1) * n[1] + c(r, 2) * n[2]);
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if len > 0.0 { v.map(|x| x / len) } else { n }
    }
}

/// Which material a part draws with: a material file, or a material node inside a file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MaterialRef {
    File(String),
    Node(String, u32),
}

/// One indexed triangle visual, in the solid's space.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Part {
    /// The tree's name (its plug id), when it has one.
    pub tree: Option<String>,
    pub material: Option<MaterialRef>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub colors: Option<Vec<[f32; 4]>>,
    pub texcoords: Vec<Vec<[f32; 2]>>,
    /// Triangles, wound as stored (reversed under a mirroring location).
    pub indices: Vec<u32>,
}

/// One collision shape, in the solid's space, each triangle with its physics surface's name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CollisionPart {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub surfaces: Vec<String>,
}

/// A solid's geometry: per level of detail its parts, and its collision.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SolidGeometry {
    /// The mip trees' far distances, in the order met at level 0.
    pub mip_far: Vec<Vec<f32>>,
    /// Level `k`'s parts, level 0 the finest; one level when the solid has no mip tree.
    pub levels: Vec<Vec<Part>>,
    pub collision: Vec<CollisionPart>,
    /// What was met and not read (a visual class, a collision shape, a visual without vertices), counted.
    pub skipped: std::collections::BTreeMap<String, u32>,
}

/// The solid at `at`, following its model (`CPlugSolid` content `Model`) to the solid that holds a tree.
pub fn read_solid(lib: &mut Library<'_>, at: &At) -> Result<Option<(At, SolidGeometry)>> {
    let mut at = at.clone();
    for _ in 0..16 {
        let Node::Solid(solid) = at.node() else {
            return Ok(None);
        };
        match solid.content {
            Some(SolidContent::Model { model, .. }) => match lib.follow(&at.doc, model)? {
                Some(next) => at = next,
                None => return Ok(None),
            },
            Some(SolidContent::Tree(tree)) => {
                let Some(root) = lib.follow(&at.doc, tree)? else {
                    return Ok(Some((at, SolidGeometry::default())));
                };
                let mut g = SolidGeometry::default();
                let mut walk = Walk {
                    level: 0,
                    parts: Vec::new(),
                    mips: Vec::new(),
                    collision: Some(Vec::new()),
                    skipped: Default::default(),
                };
                walk.tree(lib, &root, Affine::IDENTITY, 0)?;
                g.mip_far = std::mem::take(&mut walk.mips);
                g.collision = walk.collision.take().unwrap_or_default();
                g.levels.push(std::mem::take(&mut walk.parts));
                let levels = g.mip_far.iter().map(Vec::len).max().unwrap_or(1);
                for level in 1..levels {
                    walk.level = level;
                    walk.tree(lib, &root, Affine::IDENTITY, 0)?;
                    g.levels.push(std::mem::take(&mut walk.parts));
                }
                g.skipped = walk.skipped;
                return Ok(Some((at, g)));
            }
            None => return Ok(Some((at, SolidGeometry::default()))),
        }
    }
    Ok(None)
}

struct Walk {
    level: usize,
    parts: Vec<Part>,
    mips: Vec<Vec<f32>>,
    /// Collected on the level-0 walk only.
    collision: Option<Vec<CollisionPart>>,
    skipped: std::collections::BTreeMap<String, u32>,
}

impl Walk {
    fn skip(&mut self, what: impl Into<String>) {
        *self.skipped.entry(what.into()).or_default() += 1;
    }

    fn tree(&mut self, lib: &mut Library<'_>, at: &At, parent: Affine, depth: u32) -> Result<()> {
        if depth > 64 {
            self.skip("a tree nested past 64 levels");
            return Ok(());
        }
        let (data, mip): (&TreeData, Option<NodeIndex>) = match at.node() {
            Node::Tree(t) => (&t.tree, None),
            Node::TreeVisualMip(t) => {
                let levels = t.levels.as_deref().unwrap_or_default();
                if self.level == 0 {
                    self.mips.push(levels.iter().map(|l| l.far_z).collect());
                }
                let chosen = levels.get(self.level.min(levels.len().saturating_sub(1)));
                (&t.tree, chosen.and_then(|l| l.tree))
            }
            other => {
                self.skip(format!("tree class {}", other.class_name()));
                return Ok(());
            }
        };
        let here = data.location.map_or(parent, |l| {
            parent.then_local(&Affine {
                rows: l.matrix,
                translation: l.translation,
            })
        });
        let name = data.plug_id.as_ref().map(|id| format!("{id:?}"));
        if let Some(content) = data.content.clone() {
            if let Some(visual) = lib.follow(&at.doc, content.visual)? {
                let material = material_ref(&at.doc, content.shader);
                self.visual(&visual, name.clone(), material, &here);
            }
            if let Some(surface) = lib.follow(&at.doc, content.surface)?
                && self.collision.is_some()
            {
                self.surface(lib, &surface, &here)?;
            }
        }
        let children = data.children.clone().unwrap_or_default();
        if let Some(mip) = mip
            && let Some(child) = lib.follow(&at.doc, Some(mip))?
        {
            self.tree(lib, &child, here, depth + 1)?;
        }
        for child in children {
            if let Some(child) = lib.follow(&at.doc, child)? {
                self.tree(lib, &child, here, depth + 1)?;
            }
        }
        Ok(())
    }

    fn visual(&mut self, at: &At, tree: Option<String>, material: Option<MaterialRef>, m: &Affine) {
        let Node::VisualIndexedTriangles(v) = at.node() else {
            self.skip(format!("visual class {}", at.node().class_name()));
            return;
        };
        let Some(vertices) = &v.visual_3d.vertices else {
            self.skip("a visual without vertices in place");
            return;
        };
        let n = vertices.positions.len();
        let normals = match &vertices.normals {
            Some(VertexNormals::Floats(f)) => Some(f.clone()),
            Some(VertexNormals::Packed(p)) => Some(p.iter().map(|&x| unpack_normal(x)).collect::<Vec<_>>()),
            None => None,
        };
        let colors = match &vertices.colors {
            Some(VertexColors::Floats(f)) => Some(f.clone()),
            Some(VertexColors::Packed(p)) => Some(p.iter().map(|&x| unpack_color(x)).collect::<Vec<_>>()),
            None => None,
        };
        let texcoords: Vec<Vec<[f32; 2]>> = v
            .visual
            .geometry
            .as_ref()
            .map(|g| g.texcoords.iter().map(|t| t.uv.clone()).collect())
            .unwrap_or_default();
        if normals.as_ref().is_some_and(|x| x.len() != n)
            || colors.as_ref().is_some_and(|x| x.len() != n)
            || texcoords.iter().any(|t| t.len() != n)
        {
            self.skip("a visual whose vertex attributes differ in count");
            return;
        }
        let mut indices: Vec<u32> = match &v.indexed.index_buffer {
            Some(ib) => ib.indices.iter().map(|&i| u32::from(i)).collect(),
            None => (0..n as u32).collect(),
        };
        if !indices.len().is_multiple_of(3) || indices.iter().any(|&i| i as usize >= n) {
            self.skip("a visual whose indices are not triangles of its vertices");
            return;
        }
        if m.determinant() < 0.0 {
            indices
                .as_chunks_mut::<3>()
                .0
                .iter_mut()
                .for_each(|t| t.swap(1, 2));
        }
        self.parts.push(Part {
            tree,
            material,
            positions: vertices.positions.iter().map(|&p| m.point(p)).collect(),
            normals: normals.map(|ns| ns.into_iter().map(|x| m.normal(x)).collect()),
            colors,
            texcoords,
            indices,
        });
    }

    fn surface(&mut self, lib: &mut Library<'_>, at: &At, m: &Affine) -> Result<()> {
        let Node::Surface(surface) = at.node() else {
            self.skip(format!("surface class {}", at.node().class_name()));
            return Ok(());
        };
        let Some(geom) = lib.follow(&at.doc, surface.geom.flatten())? else {
            self.skip("a surface without geometry");
            return Ok(());
        };
        let Node::SurfaceGeom(geom_node) = geom.node() else {
            self.skip(format!("surface geometry class {}", geom.node().class_name()));
            return Ok(());
        };
        // each material slot's surface name
        let mut names = Vec::new();
        for material in surface.materials.clone().unwrap_or_default() {
            let id = match material {
                SurfMaterial::SurfaceId(id) => Some(id),
                SurfMaterial::Material(r) => match lib.follow(&at.doc, r)? {
                    Some(mat) => match mat.node() {
                        Node::Material(mat) => mat.surface_id,
                        _ => None,
                    },
                    None => None,
                },
            };
            names.push(surface_name(id));
        }
        let fallback = surface_name(geom_node.surface_id);
        let flips = m.determinant() < 0.0;
        match &geom_node.surf {
            Some(Surf::Mesh(mesh)) => {
                let n = mesh.vertices.len();
                let mut part = CollisionPart {
                    positions: mesh.vertices.iter().map(|&p| m.point(p)).collect(),
                    ..CollisionPart::default()
                };
                for t in &mesh.triangles {
                    let Ok(i) = t
                        .indices
                        .map(u32::try_from)
                        .into_iter()
                        .collect::<Result<Vec<u32>, _>>()
                    else {
                        self.skip("a collision triangle with a negative index");
                        continue;
                    };
                    if i.iter().any(|&x| x as usize >= n) {
                        self.skip("a collision triangle past its vertices");
                        continue;
                    }
                    let tri = if flips {
                        [i[0], i[2], i[1]]
                    } else {
                        [i[0], i[1], i[2]]
                    };
                    part.triangles.push(tri);
                    let name = usize::try_from(t.surface_index)
                        .ok()
                        .and_then(|k| names.get(k))
                        .cloned()
                        .unwrap_or_else(|| fallback.clone());
                    part.surfaces.push(name);
                }
                if let Some(c) = self.collision.as_mut() {
                    c.push(part);
                }
            }
            Some(Surf::Box { bounds }) => {
                let (c, h) = (bounds.center, bounds.half_size);
                let corner = |k: usize| -> [f32; 3] {
                    std::array::from_fn(|a| c[a] + if k >> a & 1 == 1 { h[a] } else { -h[a] })
                };
                // the six faces, each two triangles counter-clockwise seen from outside (right-handed reading)
                let faces: [[usize; 4]; 6] = [
                    [0, 2, 3, 1],
                    [4, 5, 7, 6],
                    [0, 1, 5, 4],
                    [2, 6, 7, 3],
                    [0, 4, 6, 2],
                    [1, 3, 7, 5],
                ];
                let mut part = CollisionPart {
                    positions: (0..8).map(|k| m.point(corner(k))).collect(),
                    ..CollisionPart::default()
                };
                for f in faces {
                    for tri in [[f[0], f[1], f[2]], [f[0], f[2], f[3]]] {
                        let t = tri.map(|x| x as u32);
                        part.triangles.push(if flips { [t[0], t[2], t[1]] } else { t });
                        part.surfaces.push(fallback.clone());
                    }
                }
                if let Some(col) = self.collision.as_mut() {
                    col.push(part);
                }
            }
            Some(Surf::Ellipsoid { .. }) => self.skip("an ellipsoid collision shape"),
            None => self.skip("a surface geometry without a shape"),
        }
        Ok(())
    }
}

/// A physics surface's name: GBX.NET's name for the id, `MaterialId<n>` for one past its list, `None` stored as
/// `Unknown`.
fn surface_name(id: Option<i16>) -> String {
    match id {
        Some(id) => material_id_name(id).map_or_else(|| format!("MaterialId{id}"), str::to_owned),
        None => "Unknown".to_owned(),
    }
}

/// The material a tree names: a file, or a node of the tree's own file.
fn material_ref(doc: &Arc<Doc>, shader: Option<NodeIndex>) -> Option<MaterialRef> {
    let index = shader?;
    if doc.node(index).is_some() {
        return Some(MaterialRef::Node(doc.path.clone(), index.0));
    }
    Library::file_of(doc, Some(index)).map(MaterialRef::File)
}
