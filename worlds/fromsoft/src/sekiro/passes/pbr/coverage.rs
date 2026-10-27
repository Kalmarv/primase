//! What a material binary's own G-buffer programs say its preview is drawn with ([`crate::passes::program_reads`]): the
//! slot whose alpha reaches the pixel program's discard (the coverage), the slot its albedo is made of with the others
//! blended over it (the base), where each is sampled (which UV set of the mesh) and whether a vertex colour's alpha
//! multiplies the coverage. Elden Ring's fur and feather cards test one texture on one UV set and colour from another;
//! its `C[DetailBlend]` materials colour from their own albedo (`Texture2D_7`) with shared detail maps blended over it
//! from lower slots, so neither the lowest nor the eighth slot is the one to draw with.
use crate::layer_bake::resolve::{ENGINE_DITHER_NEUTRAL, ENGINE_DITHER_TEXTURE};
use crate::materials::{Definition, Material, engine_alpha_reference};
use crate::passes::program_reads::{self, Reads, SlotRead};
use crate::sekiro::source::{MaterialShader, ShaderBundle, ShaderStage};
use fromsoft_formats::metaparam::texture_number;
use iridia::output_uses::{AlphaExpr, AlphaTest, Known, alpha_tests};
use iridia_interface::{ResourceId, ResourceKind};
use primase_core::{Entity, World};
use std::collections::BTreeMap;

/// A slot the programs read and the lowered UV set (`TexCoord(n)`) they sample it with.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Sampled {
    /// The key of the material's binding of the slot.
    pub binding: String,
    pub texcoord: u8,
}

/// The slot the alpha test reads and how it is sampled.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Coverage {
    pub slot: Sampled,
    /// Whether the first vertex colour's alpha multiplies the coverage.
    pub vertex_alpha: bool,
    /// What else decides the discard, which the preview does not follow.
    pub left_out: Vec<&'static str>,
    /// The program's own expression for the test, evaluated with the values the material and the engine state, was one
    /// varying alpha of the slot: its exact cutoff (`None`: the test discards nothing). `None` where the slot is the
    /// one dataflow names and the test is not followed exactly.
    pub solved: Option<Solved>,
}

/// A test solved to one cutoff on a slot's alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Solved {
    /// The lowest alpha kept; `None`: nothing is discarded.
    pub cut: Option<f32>,
    /// The expression holds the engine's dither noise with a nonzero coefficient: the cut replaces a stochastic test.
    pub dithered: bool,
}

/// The slot the albedo is made of and whether other slots are blended over it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Base {
    pub slot: Sampled,
    /// Other material slots reach the albedo too (detail maps, masks): the preview leaves them out.
    pub blended: bool,
    /// The pixel program it was read from.
    pub program: String,
    /// The program's albedo is several layers and no one of them is its base (a blend of two or three layers whose
    /// weights are made of vertex colours, Elden Ring's `Mb2` and `Mb3` programs): the slot is the lowest of the layers,
    /// the first the program lists, drawn alone.
    pub lowest_layer: bool,
}

/// Why a material binary's programs could not say what they read: the pixel program the material draws with (the
/// bundle's name where the material names none) and the reason.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Unread {
    pub program: String,
    pub why: String,
}

/// What a material's G-buffer programs read.
#[derive(Clone)]
struct Programs {
    /// The pixel program's file name.
    name: String,
    reads: Reads,
    /// The vertex program's UV inputs ([`program_reads::vertex_uv_inputs`]).
    uv_inputs: Vec<(u32, u8)>,
    /// The program's discards against constant words, as its graph states them ([`alpha_tests`]); none where the graph
    /// could not be followed.
    tests: Vec<AlphaTest>,
    /// What each texture of the program is: a slot of the metaparam, or the engine's.
    texels: BTreeMap<ResourceId, Texel>,
    /// The metaparam's slots, by name.
    slots: BTreeMap<String, SlotFacts>,
}

/// What a texture resource of a program is.
#[derive(Clone, Debug, PartialEq)]
enum Texel {
    /// A slot of the shader's metaparam, by its name (the key of the material's binding of it).
    Slot(String),
    /// A texture the engine binds, by the program's name for it.
    Engine(String),
}

/// What the metaparam says of a slot that decides what the program samples there.
#[derive(Clone, Debug, PartialEq)]
struct SlotFacts {
    /// The `N` of its `Texture2D_N`, by which a material's binding is matched to it.
    number: Option<u32>,
    /// The stem of the texture the engine binds where the material binds none.
    default_stem: Option<String>,
}

/// The material binaries' G-buffer programs as read so far in one run, by the shader's bundle and the pixel program's
/// name: materials share programs, and lifting one is the cost.
#[derive(Default)]
pub(super) struct Reader {
    read: BTreeMap<(Entity, String), Result<Programs, Unread>>,
}

impl Reader {
    /// What the material's own G-buffer pixel program ([`crate::materials::MaterialView::gbuffer_program`]: its
    /// shader, static switch and alpha reference select it) and the shader's G-buffer vertex program read, with the
    /// vertex program's UV inputs and the pixel program's name. `Ok(None)`: not a material binary's, or the material
    /// has no bundle; `Err`: the programs could not be read (counted by the caller).
    fn read(
        &mut self,
        world: &World,
        entity: Entity,
        material: &Material,
    ) -> Result<Option<Programs>, Unread> {
        if !matches!(material.definition, Some(Definition::Matbin(_))) {
            return Ok(None);
        }
        let Some(view) = material.view() else {
            return Ok(None);
        };
        let Some(bundle_entity) = world.get::<MaterialShader>(entity).map(|m| m.0) else {
            return Ok(None);
        };
        let Some(bundle) = world.get::<ShaderBundle>(bundle_entity) else {
            return Ok(None);
        };
        let name = view.gbuffer_program();
        self.read
            .entry((bundle_entity, name.clone()))
            .or_insert_with(|| programs(bundle, name))
            .clone()
            .map(Some)
    }

    /// What a material's program says about its alpha test, or why it could not say it (`Err`, counted by the caller).
    /// `Ok(None)`: a material this does not apply to (not a material binary's, or a program whose discards test no
    /// material slot's alpha).
    ///
    /// The program's own expression for the test ([`alpha_tests`]) is evaluated first, with the values the material
    /// states (`g_AlphaRef`), the engine states (the dither texture's neutral answer) and the metaparam states (a slot the
    /// material binds nothing to is its default texture, whose alpha `default_alpha` reads, stem and lane, where it is
    /// one value everywhere): where that is one varying slot alpha times and plus constants the cutoff is solved
    /// ([`Coverage::solved`]). Otherwise the slot dataflow names, where it names one, is cut at the material's reference
    /// as an approximation, and where it names several the reason is the evaluation's.
    pub(super) fn coverage(
        &mut self,
        world: &World,
        entity: Entity,
        material: &Material,
        default_alpha: &mut dyn FnMut(&str) -> Result<Option<f64>, String>,
    ) -> Result<Option<Coverage>, Unread> {
        let Some(Programs {
            name,
            reads,
            uv_inputs,
            tests,
            texels,
            slots,
        }) = self.read(world, entity, material)?
        else {
            return Ok(None);
        };
        if reads.coverage.is_empty() {
            return Ok(None);
        }
        let mut not_exact = None;
        match solve(&tests, &texels, &slots, material, default_alpha) {
            Ok(Some((texture, cut, dithered))) => {
                let slot = reads.coverage.iter().find(|s| s.texture == texture).ok_or_else(|| Unread {
                    program: name.clone(),
                    why: format!("the slot {texture}, whose alpha the test is solved for, is not one the dataflow reads"),
                })?;
                return Ok(Some(Coverage {
                    slot: sampled(&name, slot, &uv_inputs, "coverage")?,
                    vertex_alpha: false,
                    left_out: reads.left_out.into_iter().collect(),
                    solved: Some(Solved { cut, dithered }),
                }));
            }
            Ok(None) => {}
            Err(why) => not_exact = Some(why),
        }
        let [slot] = &reads.coverage[..] else {
            return Err(Unread {
                why: format!(
                    "{} slots reach the alpha test, not one{}",
                    reads.coverage.len(),
                    not_exact.map_or_else(String::new, |why| format!(
                        "; its expression is not solved: {why}"
                    ))
                ),
                program: name,
            });
        };
        Ok(Some(Coverage {
            slot: sampled(&name, slot, &uv_inputs, "coverage")?,
            vertex_alpha: reads.coverage_vertex_alpha,
            left_out: reads.left_out.into_iter().collect(),
            solved: None,
        }))
    }

    /// What a material's G-buffer program says its albedo is made of: the base slot and how it is sampled, or why it
    /// could not say (`Err`, counted by the caller). `Ok(None)`: not a material binary's.
    pub(super) fn base(
        &mut self,
        world: &World,
        entity: Entity,
        material: &Material,
    ) -> Result<Option<Base>, Unread> {
        let Some(Programs {
            name,
            reads,
            uv_inputs,
            ..
        }) = self.read(world, entity, material)?
        else {
            return Ok(None);
        };
        // no one slot is the base where the program blends layers by weights of its own making: the layers are listed
        // by slot, and the lowest is drawn, a counted approximation, rather than none
        let (slot, lowest_layer) = match (reads.base.as_ref(), reads.colour.first()) {
            (Some(slot), _) => (slot, false),
            (None, Some(lowest)) => (lowest, true),
            (None, None) => {
                return Err(Unread {
                    program: name,
                    why: "no slot's colour reaches the albedo".into(),
                });
            }
        };
        Ok(Some(Base {
            slot: sampled(&name, slot, &uv_inputs, "base")?,
            blended: reads.colour.len() > 1,
            program: name,
            lowest_layer,
        }))
    }
}

/// What the pixel program `name` of `bundle` and the bundle's G-buffer vertex program read, both lifted.
fn programs(bundle: &ShaderBundle, name: String) -> Result<Programs, Unread> {
    let unread = |why: String| Unread {
        program: name.clone(),
        why,
    };
    let Some(meta) = &bundle.parameters else {
        return Err(unread(format!("{}: no metaparam read", bundle.name)));
    };
    let pixel = bundle
        .program(&name)
        .ok_or_else(|| unread("not in the bundle".into()))?;
    let vertex = bundle
        .program(&format!("{}_Gbuf.vpo", bundle.name))
        .or_else(|| bundle.program(&format!("{}_Gbuf_Skin.vpo", bundle.name)))
        .filter(|p| p.stage == ShaderStage::Vertex)
        .ok_or_else(|| unread(format!("{}: no G-buffer vertex program", bundle.name)))?;
    let pixel = program_reads::lift(pixel.format, &pixel.code).map_err(unread)?;
    let vertex = program_reads::lift(vertex.format, &vertex.code).map_err(unread)?;
    let reads = program_reads::read(&pixel, &vertex, meta).map_err(unread)?;
    // the program's own discards, where its graph can be followed
    let (tests, interface) = match pixel.graph() {
        Ok(graph) => (alpha_tests(&graph), graph.interface),
        Err(_) => (Vec::new(), pixel.interface.clone()),
    };
    let texels = interface
        .resources
        .iter()
        .filter(|r| matches!(r.kind, ResourceKind::Texture { .. }))
        .map(|r| {
            let slot = program_reads::slot_of(r, meta)
                .and_then(|(slot, _)| meta.textures.iter().find(|t| u32::from(t.slot) == slot));
            (
                r.id,
                match slot {
                    Some(t) => Texel::Slot(t.name.clone()),
                    None => Texel::Engine(r.provenance.name.clone()),
                },
            )
        })
        .collect();
    let slots = meta
        .textures
        .iter()
        .map(|t| {
            (
                t.name.clone(),
                SlotFacts {
                    number: texture_number(&t.name),
                    default_stem: (!t.default_texture.is_empty())
                        .then(|| fromsoft_formats::names::stem(&t.default_texture)),
                },
            )
        })
        .collect();
    Ok(Programs {
        uv_inputs: program_reads::vertex_uv_inputs(&vertex),
        reads,
        name,
        tests,
        texels,
        slots,
    })
}

/// The program's alpha test evaluated: the slot whose alpha it varies with and the exact cutoff on it (`None`: nothing
/// is discarded), or `Ok(None)` where no discard compares a material slot's alpha with a constant word, or why the
/// expression is not one varying slot alpha with stated constants.
///
/// Every leaf is stated by what the program, the material and the metaparam say, never by a name: the reference word is
/// the one the discard compares (the material's `g_AlphaRef`, [`engine_alpha_reference`]) and the word after it the
/// scale (the engine's second word of the same constant); a slot the material binds is the varying sample; a slot it
/// binds nothing to is its default texture's alpha, where that is one value; the engine's dither texture is its neutral
/// answer ([`ENGINE_DITHER_NEUTRAL`]), as the layer bind binds it.
fn solve(
    tests: &[AlphaTest],
    texels: &BTreeMap<ResourceId, Texel>,
    slots: &BTreeMap<String, SlotFacts>,
    material: &Material,
    default_alpha: &mut dyn FnMut(&str) -> Result<Option<f64>, String>,
) -> Result<Option<(String, Option<f32>, bool)>, String> {
    let slot_alpha = |t: &AlphaTest| {
        t.tested.leaves().iter().any(
            |l| matches!(l, AlphaExpr::Texel { resource, lane: 3, .. } if matches!(texels.get(resource), Some(Texel::Slot(_)))),
        )
    };
    let mine: Vec<&AlphaTest> = tests.iter().filter(|t| slot_alpha(t)).collect();
    let Some(test) = mine.first() else {
        return Ok(None);
    };
    if let Some(other) = mine
        .iter()
        .find(|t| (t.buffer, t.word) != (test.buffer, test.word))
    {
        return Err(format!(
            "the program compares texture alphas against two constants (word {} and word {})",
            test.word, other.word
        ));
    }
    if mine.len() > 1 {
        return Err(format!(
            "the program discards on {} different expressions against one constant: {}; {}",
            mine.len(),
            mine[0].tested,
            mine[1].tested
        ));
    }
    let (buffer, word) = (test.buffer, test.word);
    let [reference, scale] = engine_alpha_reference(material);
    let bound = |slot: &str| {
        let number = slots.get(slot).and_then(|s| s.number);
        material
            .bindings
            .keys()
            .any(|k| k == slot || (number.is_some() && texture_number(k) == number))
    };
    // the dither noise's answer: its neutral value, then once another, to see whether the test depends on it
    let dither = std::cell::Cell::new(ENGINE_DITHER_NEUTRAL);
    // a default texture that could not be read is `Unknown` to the evaluation, and its reason is kept for the error
    let unread = std::cell::RefCell::new(Vec::<String>::new());
    let mut known = |leaf: &AlphaExpr| match leaf {
        AlphaExpr::Word { buffer: b, word: w } if *b == buffer && *w == word => {
            Known::Value(f64::from(reference))
        }
        AlphaExpr::Word { buffer: b, word: w } if *b == buffer && *w == word + 1 => {
            Known::Value(f64::from(scale))
        }
        AlphaExpr::Texel { resource, lane, .. } => match texels.get(resource) {
            Some(Texel::Slot(slot)) if bound(slot) => Known::Varies,
            // a default texture's alpha, where it is one value everywhere
            Some(Texel::Slot(slot)) if *lane == 3 => slots
                .get(slot)
                .and_then(|s| s.default_stem.as_deref())
                .and_then(|stem| match default_alpha(stem) {
                    Ok(alpha) => alpha,
                    Err(why) => {
                        unread
                            .borrow_mut()
                            .push(format!("default texture {stem} could not be read: {why}"));
                        None
                    }
                })
                .map_or(Known::Unknown, Known::Value),
            Some(Texel::Engine(name)) if name == ENGINE_DITHER_TEXTURE && *lane == 3 => {
                Known::Value(dither.get())
            }
            _ => Known::Unknown,
        },
        _ => Known::Unknown,
    };
    let linear = test.tested.linear(&mut known).map_err(|why| {
        let named: Vec<String> = test
            .tested
            .leaves()
            .iter()
            .filter_map(|l| match l {
                AlphaExpr::Texel { resource, lane, .. } => Some(match texels.get(resource) {
                    Some(Texel::Slot(s)) => format!("{l} is {s}.{lane}"),
                    Some(Texel::Engine(n)) => format!("{l} is the engine's {n}.{lane}"),
                    None => format!("{l} is a texture of no name"),
                }),
                _ => None,
            })
            .collect();
        let mut unread = unread.borrow().clone();
        unread.dedup();
        let unread = if unread.is_empty() {
            String::new()
        } else {
            format!("; {}", unread.join("; "))
        };
        format!(
            "{why} (the test is {} < reference; {}){unread}",
            test.tested,
            named.join(", ")
        )
    })?;
    let Some((_, resource, lane)) = linear.sample else {
        return Err(format!("no sample varies in {}", test.tested));
    };
    if lane != 3 {
        return Err(format!(
            "the varying sample of {} is lane {lane}, not an alpha",
            test.tested
        ));
    }
    let Some(Texel::Slot(texture)) = texels.get(&resource) else {
        return Err(format!(
            "the varying sample of {} is no material slot",
            test.tested
        ));
    };
    // alpha is not below 0: a cutoff there discards nothing
    let cut = linear
        .cutoff(f64::from(reference))?
        .filter(|c| *c > 0.)
        .map(|c| c as f32);
    // the test depends on the noise (a nonzero folded coefficient: the scale word is not 0) exactly when another answer
    // moves the expression; a hard cutoff at the noise mean is then not the stochastic test
    dither.set(ENGINE_DITHER_NEUTRAL + 1.);
    let dithered = test.tested.linear(&mut known).map_or(true, |other| {
        other.scale != linear.scale || other.offset != linear.offset
    });
    Ok(Some((texture.clone(), cut, dithered)))
}

/// The UV set a slot is sampled with: its `u` and `v` made of one vertex input's pair.
fn sampled(name: &str, slot: &SlotRead, uv_inputs: &[(u32, u8)], what: &str) -> Result<Sampled, Unread> {
    let unread = |why: String| Unread {
        program: name.to_owned(),
        why,
    };
    let (Some(u), Some(v)) = (&slot.u, &slot.v) else {
        return Err(unread(format!(
            "the {what} slot's coordinates are not made of one vertex input"
        )));
    };
    if (u.name.as_str(), u.index, u.component / 2) != (v.name.as_str(), v.index, v.component / 2) {
        return Err(unread(format!(
            "the {what} slot's u and v come from different UV pairs"
        )));
    }
    let texcoord = program_reads::uv_set(uv_inputs, u)
        .ok_or_else(|| unread(format!("the {what} slot is not sampled with a UV set")))?;
    Ok(Sampled {
        binding: slot.texture.clone(),
        texcoord,
    })
}
