//! The strict reader's scene, skins and animations: a document's nodes, its default scene, its skins and its
//! animations as typed values, checked against glTF 2.0's rules, for a reader that maps them onto a world.
//!
//! - **Nodes** (5.25): `children`, `mesh`, `skin`, `weights`, `camera` (counted by the caller, not read here), and
//!   the local transform, a `matrix` (column-major, its last row `0 0 0 1`: glTF requires an affine node matrix) or
//!   `translation`/`rotation`/`scale` (defaults: none, identity, one). Every child in range, no node the child of
//!   two parents or of itself (the nodes form a forest), `skin` only with `mesh`. A node with `matrix` and any of the
//!   three is refused (glTF forbids it).
//! - **Scene** (3.5): the document's `scene`, else its first; its root nodes, each a root of the forest.
//! - **Skins** (5.28): `joints` (at least one, each once, in range), `inverseBindMatrices` (a FLOAT MAT4 accessor of
//!   one matrix a joint, each affine; absent, identities), `skeleton` (a hint with no effect on deformation, given
//!   for the caller to report).
//! - **Animations** (5.6 and 3.11): each channel's target node and path (`translation`, `rotation`, `scale`,
//!   `weights`) and its sampler's interpolation (`STEP`, `LINEAR` default, `CUBICSPLINE`), key times (a FLOAT
//!   SCALAR accessor, the first at or after 0, strictly increasing) and values flattened (FLOAT, or for `rotation`
//!   and `weights` normalized integers dequantised as glTF 3.11 states). The values' width per key is checked for
//!   the paths whose width the document fixes (3, 4, 3); for `weights` it is the target mesh's morph target count,
//!   which the caller checks. A channel without a target node (only an extension gives one) is refused.
use crate::json::{Obj, invalid};
use crate::read::{ComponentType, ElementType, Gltf};
use crate::{Error, Result};
use serde_json::Value;

/// A node's local transform as stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Local {
    /// Column-major, the last row `0 0 0 1`: the 12 values of its first three rows, by column.
    Matrix([[f32; 3]; 4]),
    /// `T * R * S`; rotation a quaternion `[x, y, z, w]`.
    Trs {
        translation: [f32; 3],
        rotation: [f32; 4],
        scale: [f32; 3],
    },
}

/// One node as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadNode {
    pub name: Option<String>,
    pub children: Vec<usize>,
    pub local: Local,
    pub mesh: Option<usize>,
    pub skin: Option<usize>,
    /// The node's own morph weights, overriding its mesh's.
    pub weights: Option<Vec<f32>>,
    pub camera: Option<usize>,
    pub extras: Option<Value>,
}

/// One skin as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadSkin {
    pub name: Option<String>,
    pub joints: Vec<usize>,
    /// One a joint, column-major's first three rows by column; identities when the document gives none.
    pub inverse_binds: Vec<[[f32; 3]; 4]>,
    /// The document's skeleton root hint (no effect on deformation).
    pub skeleton: Option<usize>,
    /// The skin's `extras`, as stored.
    pub extras: Option<Value>,
}

/// What an animation channel drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationPath {
    Translation,
    Rotation,
    Scale,
    Weights,
}

impl AnimationPath {
    /// Values a key for the paths whose width the document fixes.
    pub fn width(self) -> Option<usize> {
        match self {
            AnimationPath::Translation | AnimationPath::Scale => Some(3),
            AnimationPath::Rotation => Some(4),
            AnimationPath::Weights => None,
        }
    }
}

/// A sampler's interpolation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationInterpolation {
    Step,
    Linear,
    /// Values per key: in-tangent, value, out-tangent.
    CubicSpline,
}

/// One channel with its sampler's data.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadChannel {
    pub node: usize,
    pub path: AnimationPath,
    pub interpolation: AnimationInterpolation,
    /// Seconds, strictly increasing, the first at or after 0.
    pub times: Vec<f32>,
    /// Per key, the path's width of values (three times that for cubic splines), flattened.
    pub values: Vec<f32>,
}

/// One animation as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadAnimation {
    pub name: Option<String>,
    pub channels: Vec<ReadChannel>,
    /// The animation's `extras`, as stored.
    pub extras: Option<Value>,
}

/// The `key` array of the document, empty when absent.
fn array<'a>(json: &'a Value, key: &str) -> Result<&'a [Value]> {
    match json.get(key) {
        None => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(_) => Err(invalid(key, "not an array")),
    }
}

/// An array of indices under `key`, each below `bound`.
fn indices(o: &Obj<'_>, key: &str, bound: usize, what: &str) -> Result<Option<Vec<usize>>> {
    let Some(v) = o.get(key) else {
        return Ok(None);
    };
    let a = v
        .as_array()
        .ok_or_else(|| invalid(o.path, &format!("{key} is not an array")))?;
    a.iter()
        .map(|x| {
            x.as_u64()
                .and_then(|i| usize::try_from(i).ok())
                .filter(|&i| i < bound)
                .ok_or_else(|| {
                    invalid(
                        o.path,
                        &format!("{key} holds {x}, not a {what} index below {bound}"),
                    )
                })
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

/// `n` numbers under `key`, if present.
fn numbers<const N: usize>(o: &Obj<'_>, key: &str) -> Result<Option<[f32; N]>> {
    o.f32s_opt(key)?
        .map(|v| {
            <[f32; N]>::try_from(v.as_slice())
                .map_err(|_| invalid(o.path, &format!("{key} has {} values, not {N}", v.len())))
        })
        .transpose()
}

/// A column-major 4x4 matrix's first three rows, refused unless its last row is `0 0 0 1`.
fn affine(m: &[f32], path: &str) -> Result<[[f32; 3]; 4]> {
    if m.len() != 16 {
        return Err(invalid(path, &format!("a matrix of {} values, not 16", m.len())));
    }
    if [m[3], m[7], m[11], m[15]] != [0.0, 0.0, 0.0, 1.0] {
        return Err(invalid(
            path,
            &format!(
                "a matrix whose last row is {:?}, not 0 0 0 1 (not affine)",
                [m[3], m[7], m[11], m[15]]
            ),
        ));
    }
    Ok(std::array::from_fn(|c| [m[4 * c], m[4 * c + 1], m[4 * c + 2]]))
}

impl Gltf {
    /// The document's nodes (see the module documentation).
    pub fn nodes(&self) -> Result<Vec<ReadNode>> {
        let json = self.json();
        let nodes = array(json, "nodes")?;
        let (meshes, skins, cameras) = (
            array(json, "meshes")?.len(),
            array(json, "skins")?.len(),
            array(json, "cameras")?.len(),
        );
        let mut out = Vec::with_capacity(nodes.len());
        let mut parent: Vec<Option<usize>> = vec![None; nodes.len()];
        for (i, n) in nodes.iter().enumerate() {
            let path = format!("nodes[{i}]");
            let o = Obj::new(n, &path)?;
            o.only(&[
                "name",
                "children",
                "matrix",
                "translation",
                "rotation",
                "scale",
                "mesh",
                "skin",
                "weights",
                "camera",
                "extras",
            ])?;
            let children = indices(&o, "children", nodes.len(), "node")?.unwrap_or_default();
            for &c in &children {
                if c == i {
                    return Err(invalid(&path, "is its own child"));
                }
                if let Some(p) = parent[c].replace(i) {
                    return Err(invalid(&path, &format!("node {c} is also a child of node {p}")));
                }
            }
            let trs = [numbers::<3>(&o, "translation")?, None, numbers::<3>(&o, "scale")?];
            let rotation = numbers::<4>(&o, "rotation")?;
            let local = match o.f32s_opt("matrix")? {
                Some(m) => {
                    if trs[0].is_some() || rotation.is_some() || trs[2].is_some() {
                        return Err(invalid(&path, "both a matrix and translation, rotation or scale"));
                    }
                    Local::Matrix(affine(&m, &format!("{path}.matrix"))?)
                }
                None => Local::Trs {
                    translation: trs[0].unwrap_or([0.0; 3]),
                    rotation: rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]),
                    scale: trs[2].unwrap_or([1.0; 3]),
                },
            };
            let bounded = |key: &str, bound: usize| -> Result<Option<usize>> {
                o.usize_opt(key)?
                    .map(|v| {
                        if v < bound {
                            Ok(v)
                        } else {
                            Err(invalid(&path, &format!("{key} {v} does not exist")))
                        }
                    })
                    .transpose()
            };
            let mesh = bounded("mesh", meshes)?;
            let skin = bounded("skin", skins)?;
            if skin.is_some() && mesh.is_none() {
                return Err(invalid(&path, "a skin without a mesh"));
            }
            out.push(ReadNode {
                name: o.str_opt("name")?.map(str::to_owned),
                children,
                local,
                mesh,
                skin,
                weights: o.f32s_opt("weights")?,
                camera: bounded("camera", cameras)?,
                extras: o.get("extras").cloned(),
            });
        }
        // a forest: following parents from any node ends at a root. Each node is walked once: a walk stops at a node an
        // earlier walk finished (`Done`) and is a cycle when it comes back to one of its own (`Walking`).
        #[derive(Clone, Copy, PartialEq)]
        enum Seen {
            No,
            Walking,
            Done,
        }
        let mut seen = vec![Seen::No; out.len()];
        for start in 0..out.len() {
            let mut at = start;
            let mut walked = Vec::new();
            while seen[at] == Seen::No {
                seen[at] = Seen::Walking;
                walked.push(at);
                match parent[at] {
                    Some(p) => at = p,
                    None => break,
                }
            }
            if seen[at] == Seen::Walking && parent[at].is_some() {
                return Err(invalid(&format!("nodes[{start}]"), "is in a cycle of children"));
            }
            for n in walked {
                seen[n] = Seen::Done;
            }
        }
        Ok(out)
    }

    /// The default scene (`scene`, else the first) and its index; `None` when the document has no scene.
    fn default_scene(&self) -> Result<Option<(usize, &Value)>> {
        let json = self.json();
        let scenes = array(json, "scenes")?;
        let chosen = match json.get("scene") {
            None => 0,
            Some(v) => v
                .as_u64()
                .and_then(|s| usize::try_from(s).ok())
                .filter(|&s| s < scenes.len())
                .ok_or_else(|| invalid("scene", &format!("{v} is not a scene index")))?,
        };
        Ok(scenes.get(chosen).map(|s| (chosen, s)))
    }

    /// The default scene's `extras`, as stored; `None` when it has none or the document has no scene.
    pub fn scene_extras(&self) -> Result<Option<Value>> {
        Ok(self.default_scene()?.and_then(|(_, s)| s.get("extras").cloned()))
    }

    /// The default scene's root nodes (`scene`, else the first scene); `None` when the document has no scene.
    pub fn scene_roots(&self) -> Result<Option<Vec<usize>>> {
        let nodes = array(self.json(), "nodes")?.len();
        let Some((chosen, s)) = self.default_scene()? else {
            return Ok(None);
        };
        let path = format!("scenes[{chosen}]");
        let o = Obj::new(s, &path)?;
        o.only(&["nodes", "name", "extras"])?;
        let roots = indices(&o, "nodes", nodes, "node")?.unwrap_or_default();
        let mut listed = std::collections::BTreeSet::new();
        if let Some(r) = roots.iter().find(|r| !listed.insert(**r)) {
            return Err(invalid(&path, &format!("root node {r} is listed twice")));
        }
        let parented: std::collections::BTreeSet<usize> = self
            .nodes()?
            .iter()
            .flat_map(|n| n.children.iter().copied())
            .collect();
        if let Some(r) = roots.iter().find(|r| parented.contains(r)) {
            return Err(invalid(&path, &format!("root node {r} is another node's child")));
        }
        Ok(Some(roots))
    }

    /// The document's skins (see the module documentation).
    pub fn skins(&self) -> Result<Vec<ReadSkin>> {
        let json = self.json();
        let nodes = array(json, "nodes")?.len();
        array(json, "skins")?
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let path = format!("skins[{i}]");
                let o = Obj::new(s, &path)?;
                o.only(&["name", "joints", "inverseBindMatrices", "skeleton", "extras"])?;
                let joints = indices(&o, "joints", nodes, "node")?
                    .filter(|j| !j.is_empty())
                    .ok_or_else(|| invalid(&path, "no joints"))?;
                let mut seen = std::collections::BTreeSet::new();
                if let Some(j) = joints.iter().find(|j| !seen.insert(**j)) {
                    return Err(invalid(&path, &format!("joint {j} listed twice")));
                }
                let inverse_binds = match o.usize_opt("inverseBindMatrices")? {
                    None => vec![[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]]; joints.len()],
                    Some(a) => {
                        let apath = format!("{path}.inverseBindMatrices");
                        let meta = self.accessor_data(a).map_err(|e| e.context(&apath))?;
                        if (meta.component, meta.normalized, meta.element)
                            != (ComponentType::F32, false, ElementType::Mat4)
                        {
                            return Err(invalid(&apath, "not a FLOAT MAT4 accessor"));
                        }
                        if meta.count != joints.len() {
                            return Err(invalid(
                                &apath,
                                &format!("{} matrices for {} joints", meta.count, joints.len()),
                            ));
                        }
                        let (_, v) = self.floats(a).map_err(|e| e.context(&apath))?;
                        v.as_chunks::<16>()
                            .0
                            .iter()
                            .enumerate()
                            .map(|(k, m)| affine(m, &format!("{apath}[{k}]")))
                            .collect::<Result<Vec<_>>>()?
                    }
                };
                Ok(ReadSkin {
                    name: o.str_opt("name")?.map(str::to_owned),
                    extras: o.get("extras").cloned(),
                    joints,
                    inverse_binds,
                    skeleton: o
                        .usize_opt("skeleton")?
                        .map(|s| {
                            if s < nodes {
                                Ok(s)
                            } else {
                                Err(invalid(&path, &format!("skeleton {s} does not exist")))
                            }
                        })
                        .transpose()?,
                })
            })
            .collect()
    }

    /// The document's animations (see the module documentation).
    pub fn animations(&self) -> Result<Vec<ReadAnimation>> {
        let json = self.json();
        let nodes = array(json, "nodes")?.len();
        array(json, "animations")?
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let path = format!("animations[{i}]");
                let o = Obj::new(a, &path)?;
                o.only(&["name", "channels", "samplers", "extras"])?;
                let samplers = match o.get("samplers") {
                    Some(Value::Array(s)) if !s.is_empty() => s,
                    _ => return Err(invalid(&path, "samplers missing or empty")),
                };
                let channels = match o.get("channels") {
                    Some(Value::Array(c)) if !c.is_empty() => c,
                    _ => return Err(invalid(&path, "channels missing or empty")),
                };
                let mut targets = std::collections::BTreeSet::new();
                let channels = channels
                    .iter()
                    .enumerate()
                    .map(|(k, c)| {
                        let cpath = format!("{path}.channels[{k}]");
                        let c = Obj::new(c, &cpath)?;
                        c.only(&["sampler", "target"])?;
                        let sampler = c.usize("sampler")?;
                        let tpath = format!("{cpath}.target");
                        let t = Obj::new(
                            c.get("target").ok_or_else(|| invalid(&cpath, "no target"))?,
                            &tpath,
                        )?;
                        t.only(&["node", "path"])?;
                        let node = t.usize_opt("node")?.ok_or_else(|| {
                            invalid(&tpath, "no node (only an extension targets otherwise)")
                        })?;
                        if node >= nodes {
                            return Err(invalid(&tpath, &format!("node {node} does not exist")));
                        }
                        let p = match t.str_opt("path")? {
                            Some("translation") => AnimationPath::Translation,
                            Some("rotation") => AnimationPath::Rotation,
                            Some("scale") => AnimationPath::Scale,
                            Some("weights") => AnimationPath::Weights,
                            other => return Err(invalid(&tpath, &format!("path {other:?}"))),
                        };
                        if !targets.insert((node, p as u8)) {
                            return Err(invalid(&tpath, "a second channel for this node and path"));
                        }
                        let spath = format!("{path}.samplers[{sampler}]");
                        let s = Obj::new(
                            samplers.get(sampler).ok_or_else(|| {
                                invalid(&cpath, &format!("sampler {sampler} does not exist"))
                            })?,
                            &spath,
                        )?;
                        s.only(&["input", "output", "interpolation"])?;
                        let interpolation = match s.str_opt("interpolation")? {
                            None | Some("LINEAR") => AnimationInterpolation::Linear,
                            Some("STEP") => AnimationInterpolation::Step,
                            Some("CUBICSPLINE") => AnimationInterpolation::CubicSpline,
                            Some(other) => {
                                return Err(invalid(&spath, &format!("interpolation {other}")));
                            }
                        };
                        let (times, values) = self.sampler(&s, p, &spath)?;
                        let keys = times.len();
                        let per = if interpolation == AnimationInterpolation::CubicSpline {
                            3
                        } else {
                            1
                        };
                        if let Some(w) = p.width()
                            && values.len() != keys * per * w
                        {
                            return Err(invalid(
                                &spath,
                                &format!("{} output values for {keys} keys of {w}", values.len()),
                            ));
                        }
                        if values.len() % (keys * per) != 0 {
                            return Err(invalid(
                                &spath,
                                &format!("{} output values do not divide into {keys} keys", values.len()),
                            ));
                        }
                        Ok(ReadChannel {
                            node,
                            path: p,
                            interpolation,
                            times,
                            values,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(ReadAnimation {
                    name: o.str_opt("name")?.map(str::to_owned),
                    channels,
                    extras: o.get("extras").cloned(),
                })
            })
            .collect()
    }

    /// A sampler's key times and values, checked for `path` (see the module documentation).
    fn sampler(&self, s: &Obj<'_>, path: AnimationPath, spath: &str) -> Result<(Vec<f32>, Vec<f32>)> {
        let input = s.usize("input")?;
        let ipath = format!("{spath}.input");
        let meta = self.accessor_data(input).map_err(|e| e.context(&ipath))?;
        if (meta.component, meta.normalized, meta.element) != (ComponentType::F32, false, ElementType::Scalar)
        {
            return Err(invalid(&ipath, "not a FLOAT SCALAR accessor"));
        }
        let (_, times) = self.floats(input).map_err(|e| e.context(&ipath))?;
        if times.is_empty() {
            return Err(invalid(&ipath, "no keys"));
        }
        if times[0] < 0.0 {
            return Err(invalid(
                &ipath,
                &format!("the first key is at {} s, before 0", times[0]),
            ));
        }
        if let Some(k) = times.windows(2).position(|w| w[1] <= w[0]) {
            return Err(invalid(&ipath, &format!("key {} is not after key {k}", k + 1)));
        }
        let output = s.usize("output")?;
        let opath = format!("{spath}.output");
        let meta = self.accessor_data(output).map_err(|e| e.context(&opath))?;
        let integer = meta.normalized
            && matches!(
                meta.component,
                ComponentType::I8 | ComponentType::U8 | ComponentType::I16 | ComponentType::U16
            );
        let float = meta.component == ComponentType::F32 && !meta.normalized;
        let fits = match path {
            AnimationPath::Translation | AnimationPath::Scale => float && meta.element == ElementType::Vec3,
            AnimationPath::Rotation => (float || integer) && meta.element == ElementType::Vec4,
            AnimationPath::Weights => (float || integer) && meta.element == ElementType::Scalar,
        };
        if !fits {
            return Err(invalid(
                &opath,
                &format!(
                    "{:?} {:?} (normalized {}) is not allowed for {path:?}",
                    meta.element, meta.component, meta.normalized
                ),
            ));
        }
        let (_, values) = self.floats(output).map_err(|e: Error| e.context(&opath))?;
        Ok((times, values))
    }
}
