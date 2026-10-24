//! `prepare-web-textures`: the web tiles' textures at the sizes the program chooses, as a pass before the web writer,
//! which writes what it is given and never lowers a resolution.
//!
//! Each texture a material binds is sampled in one role ([`Role`]): colour (base colour, emissive, specular colour),
//! normal, or mask (metallic-roughness, occlusion, specular strength, a layered material's weight masks and height
//! maps). A texture
//! bound in several roles gets a copy per further role (`<name>~mask`, `<name>~normal`), its bindings in that role
//! moved to the copy, so each is filtered and encoded as what it is. Data textures (`TexRole::Data`) are left as
//! stored.
//!
//! Each texture is snapped to the largest power of two no larger than its own size or its role's cap, at least 4 on a
//! side: colour box-filtered in linear light, masks in stored values, normal maps decoded, averaged and renormalized
//! ([`primase_image::resize_normals`]). With a byte budget for colour or mask images, the pass also encodes them as
//! the writer does (UASTC LDR 4x4, Zstandard), halving an image until its encoding fits the budget (each halving an
//! approximation, counted), and checks a colour image's alpha against the materials that cut it out or blend it (an
//! encoding that changes it too much is kept as PNG). What it chose (every prepared image's source size and cap, and
//! the encodings) goes into `web.prepared_images`, keyed by the image's PNG digest and role, so the writer writes it as
//! chosen and never chooses again.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use primase_core::layered::{Channel, LayeredMaterial, Source};
use primase_core::parallel::Threads;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef};
use primase_core::{Component, Entity, Name, Resource, TexRole, Texture, World};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ktx2::{AlphaPolicy, Role};

/// A texture's largest edge on the web, in pixels: a power of two, at least 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WebCap(u32);

impl WebCap {
    /// `pixels`: an error unless it is a power of two of at least 4.
    pub fn new(pixels: u32) -> Result<WebCap, String> {
        if pixels >= 4 && pixels.is_power_of_two() {
            Ok(WebCap(pixels))
        } else {
            Err(format!("{pixels} is not a power of two of at least 4 pixels"))
        }
    }
    fn parse(key: &str, text: &str) -> Result<WebCap, String> {
        let n = text
            .parse::<u32>()
            .map_err(|_| format!("{key}={text} is not a number of pixels"))?;
        WebCap::new(n).map_err(|e| format!("{key}: {e}"))
    }
    pub fn get(self) -> u32 {
        self.0
    }
}

/// The largest encoding of one image, in bytes: above 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteBudget(usize);

impl ByteBudget {
    /// `bytes`: an error when 0.
    pub fn new(bytes: usize) -> Result<ByteBudget, String> {
        if bytes > 0 {
            Ok(ByteBudget(bytes))
        } else {
            Err("a byte budget is above 0".into())
        }
    }
    fn parse(key: &str, text: &str) -> Result<ByteBudget, String> {
        let n = text
            .parse::<usize>()
            .map_err(|_| format!("{key}={text} is not a number of bytes"))?;
        ByteBudget::new(n).map_err(|e| format!("{key}: {e}"))
    }
    pub fn get(self) -> usize {
        self.0
    }
}

/// The cap of each role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoleCaps {
    pub color: WebCap,
    pub normal: WebCap,
    pub mask: WebCap,
}

impl RoleCaps {
    pub fn get(&self, role: Role) -> u32 {
        match role {
            Role::Colour => self.color.get(),
            Role::Normal => self.normal.get(),
            Role::Mask => self.mask.get(),
        }
    }
    /// `C/N/M`.
    fn parse(key: &str, text: &str) -> Result<RoleCaps, String> {
        let parts: Vec<&str> = text.split('/').collect();
        let [c, n, m] = parts[..] else {
            return Err(format!("{key}: {text} is not colour/normal/mask caps C/N/M"));
        };
        Ok(RoleCaps {
            color: WebCap::parse(key, c)?,
            normal: WebCap::parse(key, n)?,
            mask: WebCap::parse(key, m)?,
        })
    }
    fn text(&self) -> String {
        format!("{}/{}/{}", self.color.get(), self.normal.get(), self.mask.get())
    }
}

/// `prepare-web-textures{color=N,normal=N,mask=N[,names=PREFIX:C/N/M;...][,color-bytes=N][,mask-bytes=N][,memo-mib=N]}`:
/// see the module documentation. The three caps are required (the program's quality choice); `names` gives the caps
/// of textures whose name starts with a prefix, the first that matches; without a byte budget an image is not encoded
/// here and never halved. `memo-mib` keeps the preparations in memory across the pass's runs, at most N MiB (a
/// conversion runs it once per part, and parts share textures; the same output either way); without it, none kept.
/// Like every pass it does no I/O: an encoding it measures against a budget is made in memory, and nothing is kept
/// across runs of the program (the web writer's KTX2 disk cache is the writer's, at the sink).
#[derive(Clone)]
pub struct PrepareWebTextures {
    pub caps: RoleCaps,
    pub caps_by_name: Vec<(String, RoleCaps)>,
    pub color_bytes: Option<ByteBudget>,
    pub mask_bytes: Option<ByteBudget>,
    /// Preparations kept in memory between runs, owned by the program that hands it to the pass (it runs the pass
    /// once per cell, and cells share textures); `None`: none kept. The same output either way.
    pub memo: Option<Arc<std::sync::Mutex<Memo>>>,
}

/// One texture as prepared, compactly: its level 0 as PNG (empty when the pass left it as stored), the digest of
/// that PNG, what was chosen, and what the preparation logged.
type Remembered = (Vec<u8>, [u8; 32], PreparedImage, PassLog);

/// [`PrepareWebTextures::memo`]: preparations by everything they read (the key of
/// `ktx2::preparation_key`: format version, code and encoders, the stored level 0 and description, role, cap,
/// budget, alpha policy), holding at most `cap` bytes of PNGs and encodings, the oldest dropped first.
#[derive(Debug)]
pub struct Memo {
    entries: BTreeMap<primase_cache::Key, (Remembered, usize)>,
    order: std::collections::VecDeque<primase_cache::Key>,
    bytes: usize,
    cap: usize,
}

impl Memo {
    /// The most bytes of PNGs and encodings it holds.
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Holding at most `cap` bytes of PNGs and encodings.
    pub fn new(cap: usize) -> Memo {
        Memo {
            entries: BTreeMap::new(),
            order: std::collections::VecDeque::new(),
            bytes: 0,
            cap,
        }
    }
    fn holds(&self, key: &primase_cache::Key) -> bool {
        self.entries.contains_key(key)
    }
    fn get(&self, key: &primase_cache::Key) -> Option<Remembered> {
        self.entries.get(key).map(|(r, _)| r.clone())
    }
    /// Keep `r`; returns how many older entries were dropped to stay within the bound.
    fn put(&mut self, key: primase_cache::Key, r: Remembered) -> u64 {
        let size = r.0.len() + r.2.encoded.as_ref().map_or(0, |(b, _)| b.len());
        self.bytes += size;
        if let Some((_, old)) = self.entries.insert(key, (r, size)) {
            self.bytes -= old;
        } else {
            self.order.push_back(key);
        }
        let mut evicted = 0;
        while self.bytes > self.cap {
            let Some(k) = self.order.pop_front() else {
                break;
            };
            if let Some((_, n)) = self.entries.remove(&k) {
                self.bytes -= n;
                evicted += 1;
            }
        }
        evicted
    }
}

impl std::fmt::Debug for PrepareWebTextures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrepareWebTextures")
            .field("caps", &self.caps)
            .field("caps_by_name", &self.caps_by_name)
            .field("color_bytes", &self.color_bytes)
            .field("mask_bytes", &self.mask_bytes)
            .finish()
    }
}

impl PrepareWebTextures {
    /// Every texture capped by its role, no image encoded here.
    pub fn new(caps: RoleCaps) -> Self {
        PrepareWebTextures {
            caps,
            caps_by_name: Vec::new(),
            color_bytes: None,
            mask_bytes: None,
            memo: None,
        }
    }

    /// From its declared options.
    fn from_given(g: &Given) -> Result<Self, String> {
        let cap = |key: &str| WebCap::parse(key, g.required(key));
        let mut pass = PrepareWebTextures::new(RoleCaps {
            color: cap("color")?,
            normal: cap("normal")?,
            mask: cap("mask")?,
        });
        if let Some(v) = g.optional("color-bytes") {
            pass.color_bytes = Some(ByteBudget::parse("color-bytes", v)?);
        }
        if let Some(v) = g.optional("mask-bytes") {
            pass.mask_bytes = Some(ByteBudget::parse("mask-bytes", v)?);
        }
        if let Some(v) = g.optional("memo-mib") {
            let mib = v
                .parse::<usize>()
                .ok()
                .filter(|m| *m > 0)
                .ok_or_else(|| format!("memo-mib={v} is not a number of MiB above 0"))?;
            pass.memo = Some(Arc::new(std::sync::Mutex::new(Memo::new(mib << 20))));
        }
        if let Some(v) = g.optional("names") {
            for rule in v.split(';').filter(|r| !r.is_empty()) {
                let (prefix, caps) = rule
                    .rsplit_once(':')
                    .ok_or_else(|| format!("names: {rule} is not PREFIX:C/N/M"))?;
                pass.caps_by_name
                    .push((prefix.to_owned(), RoleCaps::parse("names", caps)?));
            }
        }
        Ok(pass)
    }

    /// The caps of a texture named `name`: the first prefix rule that matches, else the roles'.
    fn caps_for(&self, name: Option<&str>) -> RoleCaps {
        name.and_then(|n| self.caps_by_name.iter().find(|(p, _)| n.starts_with(p.as_str())))
            .map_or(self.caps, |(_, c)| *c)
    }

    fn budget(&self, role: Role) -> Option<usize> {
        match role {
            Role::Colour => self.color_bytes.map(ByteBudget::get),
            Role::Mask => self.mask_bytes.map(ByteBudget::get),
            // normal maps are written as PNG: no encoding to fit
            Role::Normal => None,
        }
    }
}

/// What the pass chose for one image ([`PreparedImages`]).
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedImage {
    /// The texture's size before the pass.
    pub source_dimensions: [u32; 2],
    pub cap: u32,
    /// Normal-map texels whose average was near zero, set flat.
    pub near_zero_to_flat: u64,
    /// The chosen encoding and its metadata, when the pass encoded it (a byte budget).
    pub encoded: Option<(Vec<u8>, Value)>,
}

/// `web.prepared_images`: what `prepare-web-textures` chose for each image, by the digest of the image's PNG (as the
/// glTF writer embeds it) and role. Not serialised.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedImages(pub BTreeMap<([u8; 32], Role), PreparedImage>);

impl Resource for PreparedImages {
    const NAME: &'static str = "web.prepared_images";
}

/// A texture slot of a material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Slot {
    BaseColor,
    Emissive,
    Normal,
    MetallicRoughness,
    Occlusion,
    Specular,
    SpecularColor,
    /// A layered material's weight channel `channel`, its `source`-th source.
    Mask {
        channel: usize,
        source: usize,
    },
    /// A layered material's height source of slot `slot` (the height composition).
    Height {
        slot: usize,
    },
}

impl Slot {
    fn role(self) -> Role {
        match self {
            Slot::BaseColor | Slot::Emissive | Slot::SpecularColor => Role::Colour,
            Slot::Normal => Role::Normal,
            Slot::MetallicRoughness
            | Slot::Occlusion
            | Slot::Specular
            | Slot::Mask { .. }
            | Slot::Height { .. } => Role::Mask,
        }
    }
}

fn sources_mut(c: &mut Channel) -> Vec<&mut Source> {
    match c {
        Channel::Source(s) => vec![s],
        Channel::Lerp { a, b, t } => vec![a, b, t],
        Channel::Threshold { v, m, .. } => vec![v, m],
        Channel::Power { v, .. } => vec![v],
    }
}

fn pbr_slots(p: &mut PbrMaterial) -> Vec<(Slot, &mut Option<TextureRef>)> {
    let mut out = vec![
        (Slot::BaseColor, &mut p.base_color_texture),
        (Slot::Emissive, &mut p.emissive_texture),
        (Slot::Normal, &mut p.normal_texture),
        (Slot::MetallicRoughness, &mut p.metallic_roughness_texture),
        (Slot::Occlusion, &mut p.occlusion_texture),
    ];
    if let Some(s) = &mut p.specular {
        out.push((Slot::Specular, &mut s.texture));
        out.push((Slot::SpecularColor, &mut s.color_texture));
    }
    out
}

/// Every texture binding: (material, slot, texture).
fn bindings(w: &World) -> Vec<(Entity, Slot, Entity)> {
    let mut out = Vec::new();
    for (e, p) in w.iter::<PbrMaterial>() {
        let mut p = p.clone();
        for (slot, r) in pbr_slots(&mut p) {
            if let Some(r) = r {
                out.push((e, slot, r.texture));
            }
        }
    }
    for (e, l) in w.iter::<LayeredMaterial>() {
        let mut l = l.clone();
        for (channel, c) in l.weights.channels.iter_mut().flatten().enumerate() {
            for (source, s) in sources_mut(c).into_iter().enumerate() {
                if let Source::Mask { texture, .. } = s {
                    out.push((e, Slot::Mask { channel, source }, texture.texture));
                }
            }
        }
        for (slot, s) in l.weights.heights.iter().enumerate() {
            if let Source::Mask { texture, .. } = s {
                out.push((e, Slot::Height { slot }, texture.texture));
            }
        }
    }
    out
}

/// The error of an entity a pass listed from the world and then did not find in it.
fn unbound(what: &str, e: Entity) -> String {
    format!("{what} of {e:?} was listed from the world and is not in it")
}

/// Bind `slot` of `material` to `texture`.
fn rebind(w: &mut World, material: Entity, slot: Slot, texture: Entity) -> Result<(), String> {
    if let Slot::Mask { .. } | Slot::Height { .. } = slot {
        let mut l = w
            .get::<LayeredMaterial>(material)
            .ok_or_else(|| unbound("the layered material", material))?
            .clone();
        let s = match slot {
            Slot::Mask { channel, source } => {
                let channels = l
                    .weights
                    .channels
                    .as_mut()
                    .and_then(|c| c.get_mut(channel))
                    .ok_or_else(|| unbound("the weight channel", material))?;
                sources_mut(channels).swap_remove(source)
            }
            Slot::Height { slot } => l
                .weights
                .heights
                .get_mut(slot)
                .ok_or_else(|| unbound("the height slot", material))?,
            _ => return Err(format!("{slot:?} is not a layered slot")),
        };
        if let Source::Mask { texture: r, .. } = s {
            r.texture = texture;
        }
        w.insert(material, l);
    } else {
        let mut p = w
            .get::<PbrMaterial>(material)
            .ok_or_else(|| unbound("the material", material))?
            .clone();
        for (s, r) in pbr_slots(&mut p) {
            if s == slot
                && let Some(r) = r
            {
                r.texture = texture;
            }
        }
        w.insert(material, p);
    }
    Ok(())
}

impl Pass for PrepareWebTextures {
    fn name(&self) -> &'static str {
        "prepare-web-textures"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([
            ("color".to_owned(), self.caps.color.get().to_string()),
            ("normal".to_owned(), self.caps.normal.get().to_string()),
            ("mask".to_owned(), self.caps.mask.get().to_string()),
        ]);
        if let Some(b) = self.color_bytes {
            o.insert("color-bytes".into(), b.get().to_string());
        }
        if let Some(b) = self.mask_bytes {
            o.insert("mask-bytes".into(), b.get().to_string());
        }
        if let Some(cap) = self.memo.as_ref().and_then(|m| m.lock().ok().map(|m| m.cap())) {
            o.insert("memo-mib".into(), (cap >> 20).to_string());
        }
        if !self.caps_by_name.is_empty() {
            o.insert(
                "names".into(),
                self.caps_by_name
                    .iter()
                    .map(|(p, c)| format!("{p}:{}", c.text()))
                    .collect::<Vec<_>>()
                    .join(";"),
            );
        }
        o
    }
    fn provides(&self) -> &[&'static str] {
        &[Texture::NAME, Name::NAME, PreparedImages::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[
            Texture::NAME,
            PbrMaterial::NAME,
            LayeredMaterial::NAME,
            PreparedImages::NAME,
        ]
    }
    fn approximates(&self) -> &'static str {
        "textures snapped to a power of two within their role's cap; with a byte budget, images halved until their encoding fits"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        self.prepare(w, log)
    }
}

/// A preparation made: the prepared texture, its image, the prepared texture's PNG and that PNG's digest, and what
/// making it logged.
type Made = (Texture, PreparedImage, Vec<u8>, [u8; 32], PassLog);

impl PrepareWebTextures {
    /// Texture `t`'s preparation input as `role` ([`crate::ktx2::preparation_key`]).
    fn input(
        &self,
        w: &World,
        policies: &BTreeMap<Entity, AlphaPolicy>,
        t: Entity,
        role: Role,
    ) -> Result<primase_cache::Key, String> {
        let name = w.get::<Name>(t).map(|n| n.0.as_str());
        let policy = policies.get(&t).cloned().unwrap_or_default();
        let texture = w.get::<Texture>(t).ok_or_else(|| unbound("the texture", t))?;
        Ok(crate::ktx2::preparation_key(
            texture,
            role,
            self.caps_for(name).get(role),
            self.budget(role),
            &policy,
        ))
    }

    /// Texture `t` prepared as `role`: a pure function of the texture, its name's cap, the role's budget and its alpha
    /// policy.
    fn make(
        &self,
        w: &World,
        policies: &BTreeMap<Entity, AlphaPolicy>,
        t: Entity,
        role: Role,
    ) -> Result<Made, String> {
        let name = w.get::<Name>(t).map(|n| n.0.as_str());
        let policy = policies.get(&t).cloned().unwrap_or_default();
        let texture = w.get::<Texture>(t).ok_or_else(|| unbound("the texture", t))?;
        let mut made = PassLog::default();
        let job = crate::ktx2::Preparation {
            role,
            cap: self.caps_for(name).get(role),
            policy: &policy,
            budget: self.budget(role),
        };
        let (prepared, image) = crate::ktx2::prepare_image(texture, &job, &mut made)
            .map_err(|e| format!("{}: {e}", name.unwrap_or("texture")))?;
        let png = primase_image::texture_level_png(&prepared, 0).map_err(|e| e.to_string())?;
        let key: [u8; 32] = Sha256::digest(&png).into();
        Ok((prepared, image, png, key, made))
    }

    fn prepare(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // each texture's roles, data textures and unloaded files aside
        let binds = bindings(w);
        let mut roles = BTreeMap::<Entity, BTreeSet<Role>>::new();
        for &(_, slot, t) in &binds {
            match w.get::<Texture>(t) {
                Some(tex) if tex.role == TexRole::Data => {}
                // no exact 8-bit form to filter or encode: left as stored, for the writer to report
                Some(tex) if !primase_image::rgba8_exact(tex.format) => log.count(
                    "texture bindings whose format an 8-bit PNG cannot hold exactly, not prepared \
                     (lower-textures-rgba8 first)",
                    1,
                ),
                Some(_) => {
                    roles.entry(t).or_default().insert(slot.role());
                }
                None => log.count("texture bindings to unloaded files, not prepared", 1),
            }
        }
        // a copy for each further role, its bindings moved to it
        let mut of_role = BTreeMap::<(Entity, Role), Entity>::new();
        for (&t, rs) in &roles {
            for (k, &role) in rs.iter().enumerate() {
                if k == 0 {
                    of_role.insert((t, role), t);
                    continue;
                }
                let copy = w.spawn();
                let original = w
                    .get::<Texture>(t)
                    .ok_or_else(|| unbound("the texture", t))?
                    .clone();
                w.insert(copy, original);
                if let Some(Name(n)) = w.get::<Name>(t).cloned() {
                    w.insert(copy, Name(format!("{n}~{}", role.key())));
                }
                of_role.insert((t, role), copy);
                log.count("textures copied for a further role", 1);
            }
        }
        for &(material, slot, t) in &binds {
            if let Some(&copy) = of_role.get(&(t, slot.role()))
                && copy != t
            {
                rebind(w, material, slot, copy)?;
            }
        }
        // the alpha a colour image must keep: the materials that cut it out or blend it
        let mut policies = BTreeMap::<Entity, AlphaPolicy>::new();
        for &(material, slot, t) in &binds {
            if slot != Slot::BaseColor {
                continue;
            }
            let Some(&t) = of_role.get(&(t, Role::Colour)) else {
                continue;
            };
            let p = w
                .get::<PbrMaterial>(material)
                .ok_or_else(|| unbound("the material", material))?;
            let policy = policies.entry(t).or_default();
            match p.alpha_mode {
                AlphaMode::Blend => policy.blend = true,
                AlphaMode::Mask { cutoff } if p.base_color[3] > 0. => {
                    policy
                        .cutoffs
                        .insert((cutoff.to_bits(), p.base_color[3].to_bits()));
                }
                _ => {}
            }
        }
        // a world (or part) without textures has nothing to prepare: counted as such
        log.count("textures prepared", 0);
        let mut prepared = w.remove_resource::<PreparedImages>().unwrap_or_default();
        // the preparations the memo does not hold, made ahead on the world's threads (`core.threads`) a batch at a time:
        // pure, so each is the one the loop would make where it makes one (it takes it from the batch, else makes it
        // then). A batch is two preparations per thread, so what is held at once (each with its PNG, its texture and its
        // encoding) is bounded by the threads, not by the world's textures.
        let threads = Threads::of(w);
        let batch = if threads.get() < 2 { 1 } else { 2 * threads.get() };
        let entries: Vec<((Entity, Role), Entity)> = of_role.iter().map(|(&k, &t)| (k, t)).collect();
        let result = (|| -> Result<(), String> {
            for chunk in entries.chunks(batch) {
                let mut ahead = if threads.get() < 2 {
                    BTreeMap::new()
                } else {
                    let held = |t: Entity, role: Role| {
                        self.memo.as_ref().is_some_and(|m| {
                            // a texture that is not there is `make`'s error to report
                            self.input(w, &policies, t, role)
                                .is_ok_and(|input| m.lock().is_ok_and(|m| m.holds(&input)))
                        })
                    };
                    let jobs: Vec<&((Entity, Role), Entity)> =
                        chunk.iter().filter(|&&((_, role), t)| !held(t, role)).collect();
                    crate::ktx2::init();
                    let made = primase_core::parallel::map(threads, &jobs, |_, &&((_, role), t)| {
                        self.make(w, &policies, t, role)
                    })
                    .map_err(|e| e.to_string())?;
                    jobs.into_iter()
                        .map(|&(k, _)| k)
                        .zip(made)
                        .collect::<BTreeMap<_, _>>()
                };
                for &((texture_entity, role), t) in chunk {
                    let name = w.get::<Name>(t).map(|n| n.0.clone());
                    let cap = self.caps_for(name.as_deref()).get(role);
                    let policy = policies.get(&t).cloned().unwrap_or_default();
                    let texture = w
                        .get::<Texture>(t)
                        .ok_or_else(|| unbound("the texture", t))?
                        .clone();
                    let budget = self.budget(role);
                    let input = crate::ktx2::preparation_key(&texture, role, cap, budget, &policy);
                    let held = match &self.memo {
                        Some(m) => m.lock().map_err(|_| "web texture memo poisoned")?.get(&input),
                        None => None,
                    };
                    let (texture, key, image, made) = match held {
                        Some((png, key, image, made)) => {
                            log.count("texture preparations remembered", 1);
                            let texture = if png.is_empty() {
                                texture
                            } else {
                                let pixels = primase_image::read_png(&png).map_err(|e| e.to_string())?.image;
                                let primase_image::Samples::U8(rgba) = pixels.samples else {
                                    return Err("a remembered web texture is not RGBA8".into());
                                };
                                Texture {
                                    width: pixels.width,
                                    height: pixels.height,
                                    format: primase_core::TexFormat::Rgba8Unorm,
                                    levels: vec![rgba],
                                    ..texture
                                }
                            };
                            (texture, key, image, made)
                        }
                        None => {
                            let (prepared, image, png, key, made) =
                                match ahead.remove(&(texture_entity, role)) {
                                    Some(made) => made?,
                                    None => self.make(w, &policies, t, role)?,
                                };
                            if let Some(m) = &self.memo {
                                log.count("texture preparations not remembered, made", 1);
                                // left as stored: the digest is all there is to keep
                                let kept = if prepared == texture { Vec::new() } else { png };
                                let evicted = m
                                    .lock()
                                    .map_err(|_| "web texture memo poisoned")?
                                    .put(input, (kept, key, image.clone(), made.clone()));
                                log.count("texture preparations dropped from memory", evicted);
                            }
                            (prepared, key, image, made)
                        }
                    };
                    log.absorb(made);
                    if prepared.0.insert((key, role), image).is_some() {
                        log.count("images of the same pixels and role prepared again", 1);
                    }
                    w.insert(t, texture);
                    log.count("textures prepared", 1);
                }
            }
            Ok(())
        })();
        w.insert_resource(prepared);
        result
    }
}

/// `prepare-web-textures`' declaration ([`primase_core::pass::PassRegistry::add`]): the largest texture of each role
/// is the program's choice, no default.
pub const PREPARE_WEB_TEXTURES: PassSpec = PassSpec {
    name: "prepare-web-textures",
    options: &[
        OptionSpec::required(
            "color",
            "512",
            "pixels, a power of two of at least 4: the largest colour texture",
        ),
        OptionSpec::required(
            "normal",
            "512",
            "pixels, a power of two of at least 4: the largest normal map",
        ),
        OptionSpec::required(
            "mask",
            "256",
            "pixels, a power of two of at least 4: the largest mask texture",
        ),
        OptionSpec::optional(
            "color-bytes",
            "1000000",
            "bytes above 0: the most a colour texture's prepared image may take, halved until it fits",
            "no byte budget: a colour image is capped by its pixels only",
        ),
        OptionSpec::optional(
            "mask-bytes",
            "1000000",
            "bytes above 0: the most a mask texture's prepared image may take, halved until it fits",
            "no byte budget: a mask image is capped by its pixels only",
        ),
        OptionSpec::optional(
            "memo-mib",
            "64",
            "MiB above 0: preparations kept in memory between runs (the same output either way)",
            "no memo: every preparation made afresh",
        ),
        OptionSpec::optional(
            "names",
            "bake_:16/8/4",
            "PREFIX:C/N/M rules joined by ;: a texture named with the first matching prefix takes those caps",
            "every texture takes its role's caps",
        ),
    ],
    make: |g| Ok(Box::new(PrepareWebTextures::from_given(g)?)),
};

/// Register this module's pass by its declaration.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.add(&PREPARE_WEB_TEXTURES);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use primase_core::{ColorSpace, TexFormat};

    fn texture(w: &mut World, name: &str, width: u32, height: u32) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            Texture {
                width,
                height,
                format: TexFormat::Rgba8Unorm,
                levels: vec![[128, 128, 255, 255].repeat((width * height) as usize)],
                color_space: ColorSpace::Srgb,
                role: TexRole::Color,
                normal: None,
            },
        );
        w.insert(e, Name(name.into()));
        e
    }

    fn pass(options: &[(&str, &str)]) -> Box<dyn Pass> {
        PREPARE_WEB_TEXTURES.instance(&opts(options)).unwrap()
    }

    fn opts(options: &[(&str, &str)]) -> Options {
        options
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// Colour and normal maps capped at 512, masks at 256, with these byte budgets: a pass a test hands a memo.
    fn budgeted(color_bytes: usize, mask_bytes: Option<usize>) -> PrepareWebTextures {
        let cap = |pixels| WebCap::new(pixels).unwrap();
        PrepareWebTextures {
            color_bytes: Some(ByteBudget::new(color_bytes).unwrap()),
            mask_bytes: mask_bytes.map(|b| ByteBudget::new(b).unwrap()),
            ..PrepareWebTextures::new(RoleCaps {
                color: cap(512),
                normal: cap(512),
                mask: cap(256),
            })
        }
    }

    /// `wall` (513 x 3) as base colour, metallic-roughness and normal map of one material; `bake_wall` (64 x 64) as
    /// another's base colour.
    fn world() -> (World, Entity, Entity, Entity, Entity) {
        let mut w = World::new();
        let wall = texture(&mut w, "wall", 513, 3);
        let bake = texture(&mut w, "bake_wall", 64, 64);
        let m = w.spawn();
        w.insert(
            m,
            PbrMaterial {
                base_color_texture: Some(TextureRef::new(wall)),
                metallic_roughness_texture: Some(TextureRef::new(wall)),
                normal_texture: Some(TextureRef::new(wall)),
                alpha_mode: AlphaMode::Mask { cutoff: 0.5 },
                ..PbrMaterial::default()
            },
        );
        let n = w.spawn();
        w.insert(
            n,
            PbrMaterial {
                base_color_texture: Some(TextureRef::new(bake)),
                ..PbrMaterial::default()
            },
        );
        (w, wall, bake, m, n)
    }

    #[test]
    fn roles_get_copies_and_caps_snap_to_powers_of_two() {
        let (mut w, wall, bake, m, n) = world();
        let mut log = PassLog::default();
        pass(&[
            ("color", "512"),
            ("normal", "512"),
            ("mask", "256"),
            ("names", "bake_:16/8/4"),
        ])
        .run(&mut w, &mut log)
        .unwrap();
        assert_eq!(log.counts["textures copied for a further role"], 2);
        let p = w.get::<PbrMaterial>(m).unwrap();
        let (colour, mask, normal) = (
            p.base_color_texture.unwrap().texture,
            p.metallic_roughness_texture.unwrap().texture,
            p.normal_texture.unwrap().texture,
        );
        assert_eq!(colour, wall, "the first role keeps the texture");
        let size = |e: Entity| {
            let t = w.get::<Texture>(e).unwrap();
            (t.width, t.height, w.get::<Name>(e).unwrap().0.clone())
        };
        assert_eq!(size(colour), (512, 4, "wall".into()));
        assert_eq!(size(mask), (256, 4, "wall~mask".into()));
        assert_eq!(size(normal), (512, 4, "wall~normal".into()));
        assert_eq!(
            w.get::<PbrMaterial>(n)
                .unwrap()
                .base_color_texture
                .unwrap()
                .texture,
            bake
        );
        assert_eq!(
            size(bake),
            (16, 16, "bake_wall".into()),
            "the name rule's colour cap"
        );
        let prepared = w.resource::<PreparedImages>().unwrap();
        assert_eq!(prepared.0.len(), 4);
        assert!(
            prepared.0.values().all(|p| p.encoded.is_none()),
            "no budget: nothing encoded"
        );
        assert!(
            prepared
                .0
                .values()
                .any(|p| p.source_dimensions == [513, 3] && p.cap == 256)
        );
    }

    /// A height map of the height composition is a mask: capped by the mask cap, and given a copy of its own where
    /// the same texture is also a base colour, the height binding moved to the copy.
    #[test]
    fn height_maps_are_prepared_as_masks() {
        use primase_core::layered::{Composition, Layer, LayerWeights};
        use primase_core::{Semantic, WeightMapping};
        let (mut w, wall, _, m, _) = world();
        let layered = w.spawn();
        let layer = Some(Layer {
            material: m,
            texcoord: None,
            transform: None,
        });
        w.insert(
            layered,
            LayeredMaterial {
                layers: vec![layer.clone(), layer],
                weights: LayerWeights {
                    composition: Composition::Height { contrast: 0.5 },
                    heights: vec![
                        Source::Mask {
                            texture: TextureRef::new(wall),
                            channels: [1., 0., 0., 0.],
                            bias: 0.,
                        },
                        Source::Vertex {
                            semantic: Semantic::Color(0),
                            channel: 1,
                        },
                    ],
                    ..LayerWeights::new(Semantic::Color(0), WeightMapping::Identity)
                },
            },
        );
        pass(&[("color", "512"), ("normal", "512"), ("mask", "64")])
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        let l = w.get::<LayeredMaterial>(layered).unwrap();
        let Source::Mask { texture, .. } = &l.weights.heights[0] else {
            panic!("a mask height");
        };
        let mr = w
            .get::<PbrMaterial>(m)
            .unwrap()
            .metallic_roughness_texture
            .unwrap()
            .texture;
        assert_eq!(texture.texture, mr, "the height map shares the mask copy of wall");
        let t = w.get::<Texture>(texture.texture).unwrap();
        assert_eq!(
            (
                t.width,
                t.height,
                w.get::<Name>(texture.texture).unwrap().0.as_str()
            ),
            (64, 4, "wall~mask")
        );
    }

    #[test]
    fn a_byte_budget_halves_until_the_encoding_fits() {
        let (mut w, wall, _, _, _) = world();
        // noise, so a smaller image encodes smaller; the budget: what 256 x 4 takes, less than 512 x 4
        let mut seed = 7u32;
        let noise: Vec<u8> = (0..513 * 3 * 4)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        w.get_mut::<Texture>(wall).unwrap().levels = vec![noise];
        let at = |width: u32| {
            crate::ktx2::encode_rgba8(width, 4, &vec![200; (width * 16) as usize], true)
                .unwrap()
                .len()
        };
        let budget = crate::ktx2::encode_rgba8(
            256,
            4,
            &{
                let mut s = 9u32;
                (0..256 * 16)
                    .map(|_| {
                        s ^= s << 13;
                        s ^= s >> 17;
                        s ^= s << 5;
                        s as u8
                    })
                    .collect::<Vec<u8>>()
            },
            true,
        )
        .unwrap()
        .len()
            + 64;
        assert!(at(4) < budget);
        let mut log = PassLog::default();
        pass(&[
            ("color", "512"),
            ("normal", "512"),
            ("mask", "256"),
            ("color-bytes", &budget.to_string()),
            ("mask-bytes", "1000000"),
        ])
        .run(&mut w, &mut log)
        .unwrap();
        let t = w.get::<Texture>(wall).unwrap();
        assert!(t.width < 512, "halved: {}", t.width);
        assert_eq!(
            log.approximations["images halved until their encoding fits its byte budget"],
            1
        );
        let png = primase_image::texture_level_png(t, 0).unwrap();
        let key: [u8; 32] = Sha256::digest(&png).into();
        let prepared = &w.resource::<PreparedImages>().unwrap().0[&(key, Role::Colour)];
        let (bytes, meta) = prepared.encoded.as_ref().unwrap();
        assert!(bytes.len() <= budget || meta["codec"] == "PNG_RGBA8", "{meta}");
        assert_eq!(meta["dimensions"], serde_json::json!([t.width, t.height]));
        assert_eq!(meta["source_dimensions"], serde_json::json!([513, 3]));
        // the mask's budget is met at its cap: encoded, not halved
        let mask = w
            .resource::<PreparedImages>()
            .unwrap()
            .0
            .iter()
            .find(|((_, r), _)| *r == Role::Mask)
            .unwrap()
            .1;
        assert_eq!(
            mask.encoded.as_ref().unwrap().1["dimensions"],
            serde_json::json!([256, 4])
        );
    }

    /// A second run of the same pass on the same content takes what the first prepared: the same world, the same
    /// log.
    #[test]
    fn a_remembered_preparation_is_the_same_as_a_fresh_one() {
        let mut p = budgeted(1000000, None);
        let fresh = |p: &PrepareWebTextures| {
            let (mut w, wall, ..) = world();
            let mut log = PassLog::default();
            p.run(&mut w, &mut log).unwrap();
            let memo: Vec<String> = log
                .counts
                .keys()
                .filter(|k| k.contains("preparations"))
                .cloned()
                .collect();
            for k in memo {
                log.counts.remove(&k);
            }
            (
                w.get::<Texture>(wall).unwrap().clone(),
                w.resource::<PreparedImages>().unwrap().clone(),
                log,
            )
        };
        let without = fresh(&p);
        p.memo = Some(Arc::new(std::sync::Mutex::new(Memo::new(1 << 30))));
        assert_eq!(fresh(&p), without, "first run, memo filling");
        assert_eq!(p.memo.as_ref().unwrap().lock().unwrap().entries.len(), 4);
        assert_eq!(fresh(&p), without, "second run, from the memo");
        // memo-mib in the options gives the pass a memo of its own, of that size, and prints back
        let o = Options::from([
            ("color".to_owned(), "512".to_owned()),
            ("normal".to_owned(), "512".to_owned()),
            ("mask".to_owned(), "256".to_owned()),
            ("memo-mib".to_owned(), "64".to_owned()),
        ]);
        // the printed size is the memo's own cap
        assert_eq!(
            PREPARE_WEB_TEXTURES.instance(&o).unwrap().options()["memo-mib"],
            "64"
        );
        assert!(
            PREPARE_WEB_TEXTURES
                .instance(&opts(&[
                    ("color", "512"),
                    ("normal", "512"),
                    ("mask", "256"),
                    ("memo-mib", "0"),
                ]))
                .is_err()
        );
        // a memo too small to hold anything: every preparation made again, the drops counted
        p.memo = Some(Arc::new(std::sync::Mutex::new(Memo::new(1))));
        let (mut w, ..) = world();
        let mut log = PassLog::default();
        p.run(&mut w, &mut log).unwrap();
        assert_eq!(log.counts["texture preparations dropped from memory"], 4);
        assert!(!log.counts.contains_key("texture preparations remembered"));
    }

    /// On several threads (`core.threads`): every texture, the prepared images and the log (memo counts included) are
    /// the one-thread run's, without a memo, with one filling and serving, and with one too small to hold anything.
    #[test]
    fn on_any_thread_count_the_preparations_are_the_serial_ones() {
        let run = |threads: usize, memo: Option<usize>, runs: usize| {
            let mut p = budgeted(2000, Some(1000));
            p.memo = memo.map(|cap| Arc::new(std::sync::Mutex::new(Memo::new(cap))));
            let mut out = Vec::new();
            for _ in 0..runs {
                let (mut w, ..) = world();
                let mut seed = 3u32;
                let all: Vec<Entity> = w.iter::<Texture>().map(|(e, _)| e).collect();
                for e in all {
                    for b in &mut w.get_mut::<Texture>(e).unwrap().levels[0] {
                        seed ^= seed << 13;
                        seed ^= seed >> 17;
                        seed ^= seed << 5;
                        *b = seed as u8;
                    }
                }
                w.insert_resource(primase_core::parallel::Threads::new(threads).unwrap());
                let mut log = PassLog::default();
                p.run(&mut w, &mut log).unwrap();
                let textures: Vec<(Entity, Texture)> =
                    w.iter::<Texture>().map(|(e, t)| (e, t.clone())).collect();
                out.push((textures, w.resource::<PreparedImages>().unwrap().clone(), log));
            }
            out
        };
        for (memo, runs) in [(None, 1), (Some(1 << 30), 2), (Some(1), 1)] {
            let serial = run(1, memo, runs);
            for n in [2, 4] {
                assert!(run(n, memo, runs) == serial, "{n} threads, memo {memo:?}");
            }
        }
    }

    #[test]
    fn the_caps_are_the_programs_and_powers_of_two() {
        assert!(PREPARE_WEB_TEXTURES.instance(&Options::new()).is_err());
        assert!(
            PREPARE_WEB_TEXTURES
                .instance(&opts(&[("color", "300"), ("normal", "512"), ("mask", "256")]))
                .is_err()
        );
        assert!(WebCap::new(2).is_err() && ByteBudget::new(0).is_err());
        let p = pass(&[
            ("color", "512"),
            ("normal", "512"),
            ("mask", "256"),
            ("names", "a:4/4/4;b:8/8/8"),
        ]);
        assert_eq!(p.options()["names"], "a:4/4/4;b:8/8/8");
    }

    /// The module registers its pass by its declaration, made only with its required caps, saying what it takes.
    #[test]
    fn the_web_pass_registers_by_name() {
        let mut reg = primase_core::pass::PassRegistry::new();
        register(&mut reg);
        assert_eq!(format!("{reg:?}"), r#"["prepare-web-textures"]"#);
        assert!(reg.requires_options("prepare-web-textures"));
        assert!(reg.describe("prepare-web-textures").unwrap().starts_with(
            "prepare-web-textures{color*,normal*,mask*,color-bytes,mask-bytes,memo-mib,names}\n"
        ));
        assert!(reg.make("prepare-web-textures", &Options::new()).is_err());
    }
}
