//! The preview manifest's types, its JSON form and its checks. The schema is documented in `docs/PREVIEW-MANIFEST.md`;
//! this module is its one implementation, written and read with the same types, so a manifest written here reads back
//! as the same value ([`Manifest::from_slice`]).

use std::collections::{BTreeMap, BTreeSet};

use primase_core::convert::{Notice, NoticeKind};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// `format` of every manifest.
pub const FORMAT: &str = "primase.preview-manifest";

/// The schema version this crate writes and reads. A version bump is a change of meaning or of fields; a reader
/// refuses a version it does not know rather than guess.
pub const VERSION: u32 = 1;

/// The manifest's file name beside the GLBs.
pub const FILE_NAME: &str = "manifest.json";

/// The preview manifest: what a website needs, beside the GLB files, to offer choices the files leave unresolved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// [`FORMAT`].
    pub format: String,
    /// [`VERSION`].
    pub version: u32,
    /// The writer that made it (`primase-gltf <version>`).
    pub generator: String,
    /// Every GLB the preview wrote, in the order written.
    pub files: Vec<File>,
    /// The state the preview leaves open: each a variable with named values (outfit, accessory, skin tone).
    pub choices: Vec<Choice>,
    /// The assets (characters, props), each in one file.
    pub assets: Vec<Asset>,
    /// What the preview approximates, leaves out or chose: the writer's own and the passes' ([`Manifest::add_notices`]).
    pub fidelity: Vec<Fidelity>,
}

/// One GLB.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    /// The file's name, beside the manifest.
    pub name: String,
    pub bytes: u64,
}

/// A variable a program can set: a world state axis (`core.state_axis`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    /// The axis's key: stable and unique.
    pub id: String,
    /// The axis entity's `core.name`, else the id.
    pub label: String,
    /// What kind of state it is (`outfit`, `accessory`, ...), so a site can group choices.
    pub class: String,
    /// Where it came from (the source's own words).
    pub source: String,
    /// The values, by name: each name is unique in the choice and serves as its label (`off` and `on` for a bool).
    pub options: Vec<String>,
    /// The value the source states when nothing is chosen; absent when it states none (nothing is invented).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// "`choice` is one of `options`".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Literal {
    pub choice: String,
    pub options: Vec<String>,
}

/// When something is present: an OR of clauses, each an AND of literals. No clause: never present.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Condition(pub Vec<Vec<Literal>>);

/// An asset: a character or prop, written to one file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    /// The asset root's `core.source`, else its `core.name`: stable and unique.
    pub id: String,
    pub label: String,
    /// The file ([`File::name`]) its nodes and meshes are in.
    pub file: String,
    pub members: Vec<Member>,
    pub rigs: Vec<Rig>,
}

/// What a member is to its asset (`core.asset`'s roles).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Part,
    Rig,
    Clip,
    Alternative,
}

/// A part, alternative, rig or clip of an asset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    /// Unique in its asset: the member's `core.name` when it has one that no other member has, else `member-<n>`, `n`
    /// its place in the asset's member list.
    pub id: String,
    pub label: String,
    pub role: Role,
    /// When the member exists; absent: always. Resolving a choice that fails it removes the member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Condition>,
    /// The nodes that draw it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub draws: Vec<Draw>,
}

/// One node of the GLB that draws a member's mesh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draw {
    /// `<member id>@<n>`: unique in the manifest; `n` counts the member's draws in the order written.
    pub id: String,
    pub label: String,
    /// The glTF node, and the glTF mesh it draws, in the asset's file.
    pub node: usize,
    pub mesh: usize,
    /// When this node is drawn, besides its member's condition; absent: whenever the member exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Condition>,
    /// Primitives of the mesh (by index in `meshes[mesh].primitives`) drawn only when their condition holds; the
    /// others are always drawn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub primitives: Vec<PrimitiveRule>,
    /// The mesh to draw for each material variant this node can take (the node's own among them); each mesh shares
    /// its accessors with `mesh` and differs in materials.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Variant>,
    /// Changes the node takes in states, in order: the first whose `when` holds applies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<Override>,
    /// The rig and binding it is skinned with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin: Option<SkinBinding>,
    /// The rig socket it follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<AttachmentBinding>,
}

/// A primitive drawn only under a condition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrimitiveRule {
    pub primitive: usize,
    pub requires: Condition,
}

/// The glTF mesh of one material variant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    /// `core.instance`'s variant value.
    pub variant: u32,
    pub mesh: usize,
}

/// A change to a node in the states its `when` holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Override {
    /// An AND of literals.
    pub when: Vec<Literal>,
    pub set: Set,
}

/// What an [`Override`] changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Set {
    /// The node draws the mesh of this variant ([`Draw::variants`]).
    Variant(u32),
    /// The node's placement: a column-major 4x4 matrix, metres, glTF's frame.
    Transform([f32; 16]),
    /// The node draws this member (an id in the asset) instead.
    Prototype(String),
}

/// How a node is skinned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkinBinding {
    /// A [`Rig::id`] of the asset.
    pub rig: String,
    /// `core.skin`'s binding id.
    pub binding: u32,
}

/// A node that follows a rig's socket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentBinding {
    /// A [`Rig::id`] of the asset.
    pub rig: String,
    /// A [`Socket::id`] of that rig.
    pub socket: u32,
    /// The object's place in the socket's space: a column-major 4x4 matrix.
    pub offset: [f32; 16],
}

/// A skeleton as written: its node tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rig {
    /// `<member id>@<n>`, the skeleton member and the rig's place among its rigs.
    pub id: String,
    /// The skeleton's member id.
    pub skeleton: String,
    /// The rig's root node.
    pub node: usize,
    pub joints: Vec<Joint>,
    pub sockets: Vec<Socket>,
}

/// A joint's node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Joint {
    /// The skeleton's joint id.
    pub id: u32,
    pub name: String,
    pub node: usize,
}

/// A named place on a joint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Socket {
    pub id: u32,
    pub name: String,
    /// A [`Joint::id`].
    pub joint: u32,
    /// The socket's space in its joint's: a column-major 4x4 matrix.
    pub offset: [f32; 16],
}

/// What a conversion stage told the program about the preview: a [`Notice`] in the file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fidelity {
    /// The pass (as in its pipeline), reader or writer that said it.
    pub stage: String,
    pub kind: Kind,
    pub reason: String,
    pub count: u64,
    /// A few examples of what it concerns (file names, source names).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
}

/// What a [`Fidelity`] entry says, as [`NoticeKind`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Something in the source the stage does not support, left out.
    Unsupported,
    /// Something kept, but not exactly.
    Approximation,
    /// Something left out or chosen deliberately.
    Selection,
    /// What a stage was doing when it failed.
    Failure,
    /// What a diagnostic found.
    Diagnostic,
}

impl From<&Notice> for Fidelity {
    fn from(n: &Notice) -> Fidelity {
        Fidelity {
            stage: n.stage.clone(),
            kind: match n.kind {
                NoticeKind::Unsupported => Kind::Unsupported,
                NoticeKind::Approximation => Kind::Approximation,
                NoticeKind::Selection => Kind::Selection,
                NoticeKind::FailureContext => Kind::Failure,
                NoticeKind::Diagnostic => Kind::Diagnostic,
            },
            reason: n.reason.clone(),
            count: n.count,
            examples: n.examples.clone(),
        }
    }
}

fn invalid(item: impl Into<String>, detail: impl Into<String>) -> Error {
    Error::Invalid {
        item: item.into(),
        detail: detail.into(),
    }
}

impl Manifest {
    /// An empty manifest of this writer.
    pub fn new() -> Manifest {
        Manifest {
            format: FORMAT.to_owned(),
            version: VERSION,
            generator: concat!("primase-gltf ", env!("CARGO_PKG_VERSION")).to_owned(),
            files: Vec::new(),
            choices: Vec::new(),
            assets: Vec::new(),
            fidelity: Vec::new(),
        }
    }

    /// Add what stages told the program (a conversion's reported notices) to the fidelity list, so the manifest says
    /// what the preview approximates beyond what the writer itself left out. Each is added as it is; the same notice
    /// twice is one entry (counts are not summed: a notice is a total).
    pub fn add_notices<'a>(&mut self, notices: impl IntoIterator<Item = &'a Notice>) {
        for n in notices {
            let f = Fidelity::from(n);
            if !self.fidelity.contains(&f) {
                self.fidelity.push(f);
            }
        }
    }

    /// The manifest as JSON, indented, after [`Manifest::check`] (a manifest that fails its own checks is not written).
    pub fn to_json(&self) -> Result<String> {
        self.check()?;
        let mut text = serde_json::to_string_pretty(self).map_err(|e| invalid("manifest", e.to_string()))?;
        text.push('\n');
        Ok(text)
    }

    /// Read a manifest, refusing a format or version this crate does not know, unknown fields, and a manifest that
    /// fails [`Manifest::check`].
    pub fn from_slice(bytes: &[u8]) -> Result<Manifest> {
        let m: Manifest = serde_json::from_slice(bytes).map_err(|e| invalid("manifest", e.to_string()))?;
        m.check()?;
        Ok(m)
    }

    /// Write the manifest to `path` ([`Manifest::to_json`]).
    #[allow(
        clippy::disallowed_methods,
        clippy::disallowed_types,
        reason = "writer I/O: writes the manifest file"
    )]
    pub fn write_file(&self, path: &std::path::Path) -> Result<()> {
        let text = self.to_json()?;
        std::fs::write(path, text).map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })
    }

    /// Read the manifest at `path` ([`Manifest::from_slice`]).
    #[allow(
        clippy::disallowed_methods,
        clippy::disallowed_types,
        reason = "reader I/O: reads the manifest file"
    )]
    pub fn read_file(path: &std::path::Path) -> Result<Manifest> {
        let bytes = std::fs::read(path).map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })?;
        Manifest::from_slice(&bytes)
    }

    /// The choice with this id.
    pub fn choice(&self, id: &str) -> Option<&Choice> {
        self.choices.iter().find(|c| c.id == id)
    }

    /// The manifest's own consistency, as a reader needs it to trust the ids it follows:
    /// - the format and version are known;
    /// - ids are unique (choices; assets; members within an asset; draws and rigs over the manifest) and each choice's
    ///   options are unique, non-empty, its default one of them;
    /// - every file an asset names is listed;
    /// - every literal names a choice and options of it, with no option repeated and at least one;
    /// - an override's prototype is a member of its draw's asset; a skin or attachment names a rig of the asset, an
    ///   attachment a socket of it; a rig's skeleton is a member of the asset, its sockets name its joints;
    /// - a draw's primitives, variants and overrides' variants are listed once.
    pub fn check(&self) -> Result<()> {
        if self.format != FORMAT {
            return Err(invalid(
                "manifest",
                format!("format {:?}, not {FORMAT}", self.format),
            ));
        }
        if self.version != VERSION {
            return Err(invalid(
                "manifest",
                format!("version {}, this reader knows {VERSION}", self.version),
            ));
        }
        let unique = |what: &str, ids: Vec<&str>| -> Result<()> {
            let mut seen = BTreeSet::new();
            match ids.into_iter().find(|id| id.is_empty() || !seen.insert(*id)) {
                Some(id) => Err(invalid(what, format!("id {id:?} is empty or used twice"))),
                None => Ok(()),
            }
        };
        unique("files", self.files.iter().map(|f| f.name.as_str()).collect())?;
        unique("choices", self.choices.iter().map(|c| c.id.as_str()).collect())?;
        unique("assets", self.assets.iter().map(|a| a.id.as_str()).collect())?;
        let options: BTreeMap<&str, &Choice> = self.choices.iter().map(|c| (c.id.as_str(), c)).collect();
        for c in &self.choices {
            unique(
                &format!("choice {}", c.id),
                c.options.iter().map(String::as_str).collect(),
            )?;
            if let Some(d) = &c.default
                && !c.options.contains(d)
            {
                return Err(invalid(
                    format!("choice {}", c.id),
                    format!("default {d:?} is not one of its options"),
                ));
            }
        }
        let literals = |what: &str, ls: &[Literal]| -> Result<()> {
            for l in ls {
                let c = options.get(l.choice.as_str()).ok_or_else(|| {
                    invalid(
                        what,
                        format!("a literal names the choice {:?}, which there is not", l.choice),
                    )
                })?;
                if l.options.is_empty() {
                    return Err(invalid(
                        what,
                        format!("a literal on {} lists no option", l.choice),
                    ));
                }
                let mut seen = BTreeSet::new();
                if let Some(o) = l
                    .options
                    .iter()
                    .find(|o| !c.options.contains(o) || !seen.insert(*o))
                {
                    return Err(invalid(
                        what,
                        format!(
                            "{}: option {o:?} is not one of its options, or listed twice",
                            l.choice
                        ),
                    ));
                }
            }
            Ok(())
        };
        let condition = |what: &str, c: &Condition| -> Result<()> {
            c.0.iter().try_for_each(|clause| literals(what, clause))
        };
        let mut draw_ids = BTreeSet::new();
        let mut rig_ids = BTreeSet::new();
        for a in &self.assets {
            let what = format!("asset {}", a.id);
            if !self.files.iter().any(|f| f.name == a.file) {
                return Err(invalid(&what, format!("file {:?} is not listed", a.file)));
            }
            unique(
                &format!("{what} members"),
                a.members.iter().map(|m| m.id.as_str()).collect(),
            )?;
            for m in &a.members {
                if let Some(c) = &m.requires {
                    condition(&format!("{what}, member {}", m.id), c)?;
                }
            }
            for r in &a.rigs {
                if !rig_ids.insert(r.id.as_str()) {
                    return Err(invalid(&what, format!("rig id {:?} used twice", r.id)));
                }
                if !a.members.iter().any(|m| m.id == r.skeleton) {
                    return Err(invalid(
                        &what,
                        format!("rig {}: skeleton {:?} is no member", r.id, r.skeleton),
                    ));
                }
                let joints: BTreeSet<u32> = r.joints.iter().map(|j| j.id).collect();
                if joints.len() != r.joints.len() {
                    return Err(invalid(&what, format!("rig {}: a joint id twice", r.id)));
                }
                if let Some(s) = r.sockets.iter().find(|s| !joints.contains(&s.joint)) {
                    return Err(invalid(
                        &what,
                        format!(
                            "rig {}: socket {} is on joint {}, which it has not",
                            r.id, s.id, s.joint
                        ),
                    ));
                }
            }
            let rig = |id: &str| a.rigs.iter().find(|r| r.id == id);
            for m in &a.members {
                for d in &m.draws {
                    let what = format!("{what}, draw {}", d.id);
                    if !draw_ids.insert(d.id.as_str()) {
                        return Err(invalid(&what, "id used twice"));
                    }
                    if let Some(c) = &d.requires {
                        condition(&what, c)?;
                    }
                    let mut seen = BTreeSet::new();
                    for p in &d.primitives {
                        condition(&what, &p.requires)?;
                        if !seen.insert(p.primitive) {
                            return Err(invalid(&what, format!("primitive {} listed twice", p.primitive)));
                        }
                    }
                    let mut seen = BTreeSet::new();
                    if let Some(v) = d.variants.iter().find(|v| !seen.insert(v.variant)) {
                        return Err(invalid(&what, format!("variant {} listed twice", v.variant)));
                    }
                    for o in &d.overrides {
                        literals(&what, &o.when)?;
                        match &o.set {
                            Set::Variant(v) if !seen.contains(v) => {
                                return Err(invalid(
                                    &what,
                                    format!("an override to variant {v}, which has no mesh"),
                                ));
                            }
                            Set::Prototype(p) if !a.members.iter().any(|m| &m.id == p) => {
                                return Err(invalid(
                                    &what,
                                    format!("an override to member {p:?}, which there is not"),
                                ));
                            }
                            _ => {}
                        }
                    }
                    if let Some(s) = &d.skin
                        && rig(&s.rig).is_none()
                    {
                        return Err(invalid(
                            &what,
                            format!("skinned to rig {:?}, which the asset has not", s.rig),
                        ));
                    }
                    if let Some(at) = &d.attachment {
                        let r = rig(&at.rig).ok_or_else(|| {
                            invalid(
                                &what,
                                format!("attached to rig {:?}, which the asset has not", at.rig),
                            )
                        })?;
                        if !r.sockets.iter().any(|s| s.id == at.socket) {
                            return Err(invalid(
                                &what,
                                format!(
                                    "attached to socket {} of rig {}, which it has not",
                                    at.socket, r.id
                                ),
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl Default for Manifest {
    fn default() -> Manifest {
        Manifest::new()
    }
}
