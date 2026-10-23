//! The dialect's text read and lowered in one pass: declarations, then `main`'s statements, each lowered as it is read
//! (an `if` and a loop open their blocks in the builder).

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use naga::{
    AddressSpace, ArraySize, BinaryOperator as Bo, Block, DerivativeAxis, DerivativeControl, Expression as E,
    GlobalVariable, Handle, ImageClass, ImageDimension, LocalVariable, MathFunction as Mf,
    RelationalFunction, ResourceBinding, SampleLevel, ScalarKind, Statement, TypeInner, UnaryOperator as Uo,
};
use primase_shader::build::{Builder, Kind, Val};
use primase_shader::{
    BufferLayout, Constant as LayoutConstant, ConstantType, Conventions, D3dBinding, D3dClass, D3dModes,
    D3dSignature, Interface, InterpMode, InterpSampling, Interpolation, Label, Program, Provenance, Resource,
    ResourceId, ResourceKind, ResourceProvenance, SampleType, ScalarType, ShaderFormat, Stage, StageFacts,
    SystemValue, TextureDim, Value, Varying, VaryingProvenance, entry_io,
};

use crate::lex::{self, Spanned, Token};
use crate::{Error, Hints, LIFTER_VERSION, Result};

/// What a name stands for.
#[derive(Clone, Debug)]
enum Sym {
    /// A register or a varying written: a local of this kind and width.
    Local(Handle<LocalVariable>, Kind, u8),
    /// A constant (`const vec4 ps_c5 = vec4(...)`): its value, and its lanes where they are integer literals (a loop's
    /// count is read from them).
    Const(Val, Option<[i64; 4]>),
    /// The uniform array of constants, of this many rows.
    Constants(u32),
    /// A sampler, by its index in the program's samplers.
    Sampler(usize),
    /// An argument of the entry point: an attribute, or a varying a pixel program reads.
    Argument(usize),
    /// `posFixup`, which only the dropped epilogue reads.
    PosFixup,
    /// A loop's counter.
    Loop(Val),
}

/// A value, and its number if it is an integer literal or a sum, difference or product of them.
#[derive(Clone, Copy, Debug)]
struct V {
    v: Val,
    literal: Option<i64>,
}

impl From<Val> for V {
    fn from(v: Val) -> V {
        V { v, literal: None }
    }
}

struct Sampler {
    glsl_name: String,
    slot: u32,
    name: String,
    texture: Handle<GlobalVariable>,
    sampler: Handle<GlobalVariable>,
}

struct Lower<'a> {
    b: Builder,
    tokens: Vec<Spanned>,
    pos: usize,
    stage: Stage,
    hints: &'a Hints,
    symbols: BTreeMap<String, Sym>,
    /// The uniform array's global and rows.
    constants: Option<(Handle<GlobalVariable>, u32)>,
    samplers: Vec<Sampler>,
    inputs: Vec<Varying>,
    /// The outputs in the order the entry point returns them, and the local that holds each.
    outputs: Vec<(Varying, Handle<LocalVariable>)>,
    notes: Vec<String>,
    loops: u32,
}

fn error(line: usize, detail: impl Into<String>) -> Error {
    Error {
        line,
        detail: detail.into(),
    }
}

fn float_value(width: u8) -> Value {
    Value {
        scalar: ScalarType::Float,
        width,
    }
}

/// The semantic a varying of the dialect stands for: `vFrontColor` is colour 0, `vTexCoord<n>` texture coordinate n.
fn varying_semantic(name: &str) -> Option<(&'static str, u32)> {
    match name {
        "vFrontColor" => Some(("COLOR", 0)),
        "vBackColor" => Some(("COLOR", 1)),
        "vFogFactor" => Some(("FOG", 0)),
        _ => {
            let n = name.strip_prefix("vTexCoord")?;
            Some(("TEXCOORD", n.parse().ok()?))
        }
    }
}

const EPILOGUE: [&str; 3] = [
    "gl_Position . y = gl_Position . y * posFixup . y ;",
    "gl_Position . xy += posFixup . zw * gl_Position . ww ;",
    "gl_Position . z = gl_Position . z * 2.0 - gl_Position . w ;",
];

pub(crate) fn lift(source: &str, stage: Stage, hints: &Hints) -> Result<Program> {
    if !matches!(stage, Stage::Vertex | Stage::Pixel) {
        return Err(error(0, format!("a {stage:?} program is not in the dialect")));
    }
    let lexed = lex::lex(source)?;
    let mut notes = Vec::new();
    for e in &lexed.extensions {
        if e != "GL_OES_standard_derivatives : enable" {
            return Err(error(0, format!("the extension {e}")));
        }
    }
    let mut l = Lower {
        b: Builder::new(),
        tokens: lexed.tokens,
        pos: 0,
        stage,
        hints,
        symbols: BTreeMap::new(),
        constants: None,
        samplers: Vec::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        notes: Vec::new(),
        loops: 0,
    };
    l.prepare_outputs()?;
    l.declarations()?;
    l.main()?;
    notes.append(&mut l.notes);
    l.finish(notes)
}

impl Lower<'_> {
    fn line(&self) -> usize {
        self.tokens
            .get(self.pos)
            .or_else(|| self.tokens.last())
            .map_or(0, |t| t.line)
    }

    fn fail<T>(&self, detail: impl Into<String>) -> Result<T> {
        Err(error(self.line(), detail))
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|t| &t.token)
    }

    fn next(&mut self) -> Result<Token> {
        match self.tokens.get(self.pos) {
            Some(t) => {
                self.pos = self.pos.saturating_add(1);
                Ok(t.token.clone())
            }
            None => self.fail("the program ends in the middle of a statement"),
        }
    }

    fn at(&self, punct: &str) -> bool {
        self.peek().is_some_and(|t| t.is(punct))
    }

    fn at_ident(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Ident(s)) if s == word)
    }

    fn accept(&mut self, punct: &str) -> bool {
        let found = self.at(punct);
        if found {
            self.pos = self.pos.saturating_add(1);
        }
        found
    }

    fn expect(&mut self, punct: &str) -> Result<()> {
        if self.accept(punct) {
            Ok(())
        } else {
            let found = self.peek().map_or("the end".to_owned(), Token::text);
            self.fail(format!("{punct} expected, found {found}"))
        }
    }

    fn ident(&mut self) -> Result<String> {
        match self.next()? {
            Token::Ident(s) => Ok(s),
            other => self.fail(format!("a name expected, found {}", other.text())),
        }
    }

    fn word(&mut self, word: &str) -> Result<()> {
        let found = self.ident()?;
        if found == word {
            Ok(())
        } else {
            self.fail(format!("{word} expected, found {found}"))
        }
    }

    fn integer(&mut self) -> Result<i64> {
        match self.next()? {
            Token::Int(i) => Ok(i),
            other => self.fail(format!("an integer expected, found {}", other.text())),
        }
    }

    // ---- the entry point's edges ----

    /// A pixel program writes `gl_FragColor` or `gl_FragData[n]`, which it never declares: the targets it writes are
    /// found in its text. A vertex program writes `gl_Position`.
    fn prepare_outputs(&mut self) -> Result<()> {
        let v4 = self.b.ty_of(Kind::F, 4);
        match self.stage {
            Stage::Vertex => {
                let local = self.b.local("position", v4);
                self.symbols
                    .insert("gl_Position".into(), Sym::Local(local, Kind::F, 4));
                self.outputs.push((
                    Varying {
                        label: Label {
                            name: "POSITION".into(),
                            index: 0,
                        },
                        system: Some(SystemValue::ClipPosition),
                        value: float_value(4),
                        interp: None,
                        used: true,
                        provenance: VaryingProvenance::default(),
                    },
                    local,
                ));
            }
            _ => {
                let mut targets = std::collections::BTreeSet::new();
                for (i, t) in self.tokens.iter().enumerate() {
                    match &t.token {
                        Token::Ident(s) if s == "gl_FragColor" => {
                            targets.insert(0u32);
                        }
                        Token::Ident(s) if s == "gl_FragData" => {
                            let n = match (
                                self.tokens.get(i.saturating_add(1)).map(|t| &t.token),
                                self.tokens.get(i.saturating_add(2)).map(|t| &t.token),
                            ) {
                                (Some(open), Some(Token::Int(n))) if open.is("[") => *n,
                                _ => {
                                    return Err(error(
                                        t.line,
                                        "gl_FragData indexed by something that is not a number",
                                    ));
                                }
                            };
                            let n = u32::try_from(n)
                                .ok()
                                .filter(|n| *n < 4)
                                .ok_or_else(|| error(t.line, format!("gl_FragData[{n}]")))?;
                            targets.insert(n);
                        }
                        _ => {}
                    }
                }
                for n in targets {
                    let local = self.b.local(&format!("target{n}"), v4);
                    // `gl_FragColor` is target 0
                    self.symbols
                        .insert(format!("gl_FragData[{n}]"), Sym::Local(local, Kind::F, 4));
                    if n == 0 {
                        self.symbols
                            .insert("gl_FragColor".into(), Sym::Local(local, Kind::F, 4));
                    }
                    self.outputs.push((
                        Varying {
                            label: Label {
                                name: "SV_Target".into(),
                                index: n,
                            },
                            system: Some(SystemValue::Target(n)),
                            value: float_value(4),
                            interp: None,
                            used: true,
                            provenance: VaryingProvenance::default(),
                        },
                        local,
                    ));
                }
            }
        }
        Ok(())
    }

    fn has_ident(&self, word: &str) -> bool {
        self.tokens
            .iter()
            .any(|t| matches!(&t.token, Token::Ident(s) if s == word))
    }

    // ---- declarations ----

    fn declarations(&mut self) -> Result<()> {
        let prefix = if self.stage == Stage::Vertex { "vs_" } else { "ps_" };
        loop {
            if self.at_ident("void") {
                return Ok(());
            }
            let storage = self.ident()?;
            match storage.as_str() {
                "uniform" => {
                    let ty = self.ident()?;
                    let name = self.ident()?;
                    match ty.as_str() {
                        "vec4" if self.accept("[") => {
                            let rows = self.integer()?;
                            self.expect("]")?;
                            self.expect(";")?;
                            if name != format!("{prefix}uniforms_vec4") {
                                return self.fail(format!("the uniform array {name}"));
                            }
                            let rows = u32::try_from(rows).ok().filter(|r| *r > 0).ok_or_else(|| {
                                error(self.line(), format!("a uniform array of {rows} rows"))
                            })?;
                            self.declare_constants(&name, rows);
                        }
                        "vec4" if name == "posFixup" => {
                            self.expect(";")?;
                            self.symbols.insert(name, Sym::PosFixup);
                        }
                        "sampler2D" => {
                            self.expect(";")?;
                            self.declare_sampler(&name)?;
                        }
                        other => return self.fail(format!("a uniform {other} {name}")),
                    }
                }
                "attribute" if self.stage == Stage::Vertex => {
                    self.word("vec4")?;
                    let name = self.ident()?;
                    self.expect(";")?;
                    self.declare_attribute(&name)?;
                }
                "varying" => {
                    self.word("vec4")?;
                    let name = self.ident()?;
                    self.expect(";")?;
                    self.declare_varying(&name)?;
                }
                "const" => {
                    let ty = self.ident()?;
                    let name = self.ident()?;
                    self.expect("=")?;
                    self.declare_const(&ty, &name)?;
                }
                "vec4" | "ivec4" => {
                    let name = self.ident()?;
                    self.expect(";")?;
                    let (kind, ty) = if storage == "vec4" {
                        (Kind::F, self.b.ty_of(Kind::F, 4))
                    } else {
                        (Kind::I, self.b.ty_of(Kind::I, 4))
                    };
                    let local = self.b.local(&name, ty);
                    self.symbols.insert(name, Sym::Local(local, kind, 4));
                }
                other => return self.fail(format!("a declaration beginning {other}")),
            }
        }
    }

    fn declare_constants(&mut self, name: &str, rows: u32) {
        let u4 = self.b.ty_of(Kind::U, 4);
        let ty = self.b.ty(TypeInner::Array {
            base: u4,
            size: ArraySize::Constant(NonZeroU32::new(rows).unwrap_or(NonZeroU32::MIN)),
            stride: 16,
        });
        let g = self.b.global(GlobalVariable {
            name: Some("cb0".into()),
            space: AddressSpace::Uniform,
            binding: Some(ResourceBinding { group: 0, binding: 0 }),
            ty,
            init: None,
            memory_decorations: Default::default(),
        });
        self.constants = Some((g, rows));
        self.symbols.insert(name.to_owned(), Sym::Constants(rows));
    }

    fn declare_sampler(&mut self, name: &str) -> Result<()> {
        let Some(hint) = self.hints.samplers.iter().find(|s| s.glsl_name == name) else {
            return self.fail(format!("the sampler {name}, which the caller does not describe"));
        };
        let (slot, texture_name) = (hint.slot, hint.name.clone());
        let image = self.b.ty(TypeInner::Image {
            dim: ImageDimension::D2,
            arrayed: false,
            class: ImageClass::Sampled {
                kind: ScalarKind::Float,
                multi: false,
            },
        });
        let sampler_ty = self.b.ty(TypeInner::Sampler { comparison: false });
        let texture = self.b.global(GlobalVariable {
            name: Some(format!("t{slot}")),
            space: AddressSpace::Handle,
            binding: Some(ResourceBinding {
                group: 1,
                binding: slot,
            }),
            ty: image,
            init: None,
            memory_decorations: Default::default(),
        });
        let sampler = self.b.global(GlobalVariable {
            name: Some(format!("s{slot}")),
            space: AddressSpace::Handle,
            binding: Some(ResourceBinding {
                group: 2,
                binding: slot,
            }),
            ty: sampler_ty,
            init: None,
            memory_decorations: Default::default(),
        });
        let index = self.samplers.len();
        self.samplers.push(Sampler {
            glsl_name: name.to_owned(),
            slot,
            name: texture_name,
            texture,
            sampler,
        });
        self.symbols.insert(name.to_owned(), Sym::Sampler(index));
        Ok(())
    }

    fn declare_attribute(&mut self, name: &str) -> Result<()> {
        let Some(hint) = self.hints.attributes.iter().find(|a| a.glsl_name == name) else {
            return self.fail(format!(
                "the attribute {name}, which the caller does not describe"
            ));
        };
        let register = name
            .strip_prefix("vs_v")
            .and_then(|n| n.parse::<u32>().ok())
            .map(|register| D3dSignature {
                register,
                mask: 0xF,
                read_write_mask: 0,
            });
        let index = self.inputs.len();
        self.inputs.push(Varying {
            label: Label {
                name: hint.semantic.clone(),
                index: hint.index,
            },
            system: None,
            value: float_value(4),
            interp: None,
            used: true,
            provenance: VaryingProvenance { d3d: register },
        });
        self.symbols.insert(name.to_owned(), Sym::Argument(index));
        Ok(())
    }

    fn declare_varying(&mut self, name: &str) -> Result<()> {
        let Some((semantic, index)) = varying_semantic(name) else {
            return self.fail(format!("the varying {name}"));
        };
        let label = Label {
            name: semantic.into(),
            index,
        };
        if self.stage == Stage::Pixel {
            let argument = self.inputs.len();
            self.inputs.push(Varying {
                label,
                system: None,
                value: float_value(4),
                interp: Some(Interpolation {
                    mode: InterpMode::Perspective,
                    sampling: InterpSampling::Center,
                }),
                used: true,
                provenance: VaryingProvenance::default(),
            });
            self.symbols.insert(name.to_owned(), Sym::Argument(argument));
        } else {
            let v4 = self.b.ty_of(Kind::F, 4);
            let local = self.b.local(name, v4);
            self.outputs.push((
                Varying {
                    label,
                    system: None,
                    value: float_value(4),
                    interp: None,
                    used: true,
                    provenance: VaryingProvenance::default(),
                },
                local,
            ));
            self.symbols
                .insert(name.to_owned(), Sym::Local(local, Kind::F, 4));
        }
        Ok(())
    }

    fn declare_const(&mut self, ty: &str, name: &str) -> Result<()> {
        match ty {
            "ivec4" => {
                self.word("ivec4")?;
                self.expect("(")?;
                let mut lanes = [0i64; 4];
                let mut parts = Vec::new();
                for (k, lane) in lanes.iter_mut().enumerate() {
                    if k > 0 {
                        self.expect(",")?;
                    }
                    let negative = self.accept("-");
                    let n = self.integer()?;
                    *lane = if negative { -n } else { n };
                    let value = i32::try_from(*lane)
                        .map_err(|_| error(self.line(), "an integer constant over 32 bits"))?;
                    parts.push(self.b.lit_i(value));
                }
                self.expect(")")?;
                self.expect(";")?;
                let v = self.b.compose(&parts);
                self.symbols.insert(name.to_owned(), Sym::Const(v, Some(lanes)));
            }
            "vec4" => {
                let v = self.expression()?;
                self.expect(";")?;
                self.symbols.insert(name.to_owned(), Sym::Const(v.v, None));
            }
            other => return self.fail(format!("a constant of type {other}")),
        }
        Ok(())
    }

    // ---- main ----

    fn main(&mut self) -> Result<()> {
        self.word("void")?;
        self.word("main")?;
        self.expect("(")?;
        self.expect(")")?;
        self.expect("{")?;
        // the end of the body: its closing brace is the last token
        if self.tokens.last().is_none_or(|t| !t.token.is("}")) {
            return self.fail("main does not end with a closing brace");
        }
        let end = self.tokens.len().saturating_sub(1);
        // the clip-space epilogue MojoShader appends to a vertex program
        if self.stage == Stage::Vertex && self.has_ident("posFixup") {
            let tail: Vec<String> = self.tokens[..end].iter().map(|t| t.token.text()).collect();
            let mut matched = 0usize;
            let joined = tail.join(" ");
            let epilogue = EPILOGUE.join(" ");
            if joined.ends_with(&epilogue) {
                matched = EPILOGUE.iter().map(|s| s.split(' ').count()).sum();
            }
            if matched == 0 {
                return self
                    .fail("posFixup used outside the clip-space fix MojoShader ends a vertex program with");
            }
            let keep = end.saturating_sub(matched);
            self.tokens.drain(keep..end);
            self.notes.push(
                "the clip-space fix for OpenGL that MojoShader ends a vertex program with (posFixup: flip y, \
                 half-pixel offset, depth 0..1 to -1..1) is dropped: the program keeps Direct3D's clip space"
                    .into(),
            );
        }
        let end = self.tokens.len().saturating_sub(1);
        while self.pos < end {
            self.statement()?;
        }
        Ok(())
    }

    fn block_or_statement(&mut self) -> Result<()> {
        if self.accept("{") {
            while !self.at("}") {
                if self.peek().is_none() {
                    return self.fail("a block that is not closed");
                }
                self.statement()?;
            }
            self.expect("}")
        } else {
            self.statement()
        }
    }

    fn statement(&mut self) -> Result<()> {
        if self.at_ident("if") {
            self.pos = self.pos.saturating_add(1);
            self.expect("(")?;
            let c = self.expression()?;
            self.expect(")")?;
            if c.v.kind != Kind::B || c.v.width != 1 {
                return self.fail("an if on something that is not one boolean");
            }
            self.b.blocks.push(Block::new());
            self.block_or_statement()?;
            let accept = self.b.blocks.pop().expect("the block just pushed");
            let mut reject = Block::new();
            if self.at_ident("else") {
                self.pos = self.pos.saturating_add(1);
                self.b.blocks.push(Block::new());
                self.block_or_statement()?;
                reject = self.b.blocks.pop().expect("the block just pushed");
            }
            self.b.push(Statement::If {
                condition: c.v.expr,
                accept,
                reject,
            });
            return Ok(());
        }
        if self.at_ident("for") {
            return self.for_loop();
        }
        if self.at_ident("discard") {
            if self.stage != Stage::Pixel {
                return self.fail("discard in a vertex program");
            }
            self.pos = self.pos.saturating_add(1);
            self.expect(";")?;
            self.b.push(Statement::Kill);
            return Ok(());
        }
        if self.at_ident("break") || self.at_ident("return") || self.at_ident("while") {
            let word = self.ident()?;
            return self.fail(word);
        }
        self.assignment()
    }

    /// `for (int repN = A; repN < ps_iM.x; repN++) { .. }`, a `rep` loop of constant count.
    fn for_loop(&mut self) -> Result<()> {
        self.pos = self.pos.saturating_add(1);
        self.expect("(")?;
        self.word("int")?;
        let counter = self.ident()?;
        self.expect("=")?;
        let start = self.integer()?;
        self.expect(";")?;
        if self.ident()? != counter {
            return self.fail("a loop that tests something other than its counter");
        }
        self.expect("<")?;
        let bound = self.ident()?;
        self.expect(".")?;
        let lane = self.ident()?;
        self.expect(";")?;
        if self.ident()? != counter {
            return self.fail("a loop that steps something other than its counter");
        }
        self.expect("++")?;
        self.expect(")")?;
        let Some(Sym::Const(_, Some(lanes))) = self.symbols.get(&bound).cloned() else {
            return self.fail(format!(
                "a loop bounded by {bound}, which is not a constant integer vector"
            ));
        };
        let lane = swizzle_lanes(&lane).filter(|l| l.len() == 1).map(|l| l[0]);
        let Some(lane) = lane else {
            return self.fail("a loop bound that is not one lane");
        };
        let count = lanes
            .get(usize::from(lane))
            .and_then(|end| end.checked_sub(start))
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| error(self.line(), "a loop that does not count up"))?;
        let (i, local) = self.b.begin_repeat(count);
        let i = self.b.bitcast(i, Kind::I);
        let start = i32::try_from(start).map_err(|_| error(self.line(), "a loop start over 32 bits"))?;
        let offset = self.b.lit_i(start);
        let value = self.b.binary(Bo::Add, i, offset);
        self.loops = self.loops.saturating_add(1);
        let saved = self.symbols.insert(counter.clone(), Sym::Loop(value));
        self.block_or_statement()?;
        match saved {
            Some(s) => self.symbols.insert(counter, s),
            None => self.symbols.remove(&counter),
        };
        self.b.end_repeat(local);
        Ok(())
    }

    fn assignment(&mut self) -> Result<()> {
        let line = self.line();
        let name = self.ident()?;
        // `gl_FragData[n]` is one name
        let name = if name == "gl_FragData" {
            self.expect("[")?;
            let n = self.integer()?;
            self.expect("]")?;
            format!("gl_FragData[{n}]")
        } else {
            name
        };
        let lanes = if self.accept(".") {
            let w = self.ident()?;
            swizzle_lanes(&w).ok_or_else(|| error(line, format!("the swizzle {w}")))?
        } else {
            Vec::new()
        };
        let Some(Sym::Local(local, kind, width)) = self.symbols.get(&name).cloned() else {
            return Err(error(line, format!("an assignment to {name}")));
        };
        let lanes = if lanes.is_empty() {
            (0..width).collect()
        } else {
            lanes
        };
        if lanes.iter().any(|l| *l >= width) || (1..lanes.len()).any(|k| lanes[..k].contains(&lanes[k])) {
            return Err(error(
                line,
                format!("a write mask of {name} that repeats or leaves it"),
            ));
        }
        if !self.accept("=") {
            let op = self.next()?.text();
            return Err(error(line, format!("the assignment {op}")));
        }
        let value = self.expression()?;
        self.expect(";")?;
        let mut value = value.v;
        if value.kind != kind {
            return Err(error(
                line,
                format!("a {:?} stored in a {kind:?} register", value.kind),
            ));
        }
        if value.width == 1 && lanes.len() > 1 {
            value = self.b.splat(value, lanes.len() as u8);
        }
        if usize::from(value.width) != lanes.len() {
            return Err(error(
                line,
                format!("{} lanes stored in {} of {name}", value.width, lanes.len()),
            ));
        }
        let pointer = self.b.local_ptr(local);
        if lanes.len() == usize::from(width) && lanes.iter().enumerate().all(|(k, l)| usize::from(*l) == k) {
            self.b.store(pointer, value);
        } else {
            for (k, lane) in lanes.iter().enumerate() {
                let part = self.b.lane(value, k as u8);
                let target = self.b.app(E::AccessIndex {
                    base: pointer,
                    index: u32::from(*lane),
                });
                self.b.store(target, part);
            }
        }
        Ok(())
    }

    // ---- expressions ----

    fn expression(&mut self) -> Result<V> {
        self.ternary()
    }

    fn ternary(&mut self) -> Result<V> {
        let c = self.or()?;
        if !self.accept("?") {
            return Ok(c);
        }
        let a = self.expression()?;
        self.expect(":")?;
        let b = self.expression()?;
        if c.v.kind != Kind::B || c.v.width != 1 {
            return self.fail("a condition that is not one boolean");
        }
        if a.v.kind != b.v.kind {
            return self.fail("the two sides of ?: differ in type");
        }
        Ok(self.b.select(c.v, a.v, b.v).into())
    }

    fn or(&mut self) -> Result<V> {
        let mut a = self.and()?;
        while self.accept("||") {
            let b = self.and()?;
            a = self.logical(Bo::LogicalOr, a, b)?;
        }
        Ok(a)
    }

    fn and(&mut self) -> Result<V> {
        let mut a = self.equality()?;
        while self.accept("&&") {
            let b = self.equality()?;
            a = self.logical(Bo::LogicalAnd, a, b)?;
        }
        Ok(a)
    }

    fn logical(&mut self, op: Bo, a: V, b: V) -> Result<V> {
        if a.v.kind != Kind::B || b.v.kind != Kind::B || a.v.width != 1 || b.v.width != 1 {
            return self.fail("a logical operator on something that is not one boolean");
        }
        Ok(self.b.binary(op, a.v, b.v).into())
    }

    fn equality(&mut self) -> Result<V> {
        let mut a = self.relational()?;
        loop {
            let op = if self.at("==") {
                Bo::Equal
            } else if self.at("!=") {
                Bo::NotEqual
            } else {
                return Ok(a);
            };
            self.pos = self.pos.saturating_add(1);
            let b = self.relational()?;
            a = self.compare(op, a, b)?;
        }
    }

    fn relational(&mut self) -> Result<V> {
        let mut a = self.additive()?;
        loop {
            let op = if self.at("<") {
                Bo::Less
            } else if self.at(">") {
                Bo::Greater
            } else if self.at("<=") {
                Bo::LessEqual
            } else if self.at(">=") {
                Bo::GreaterEqual
            } else {
                return Ok(a);
            };
            self.pos = self.pos.saturating_add(1);
            let b = self.additive()?;
            a = self.compare(op, a, b)?;
        }
    }

    fn compare(&mut self, op: Bo, a: V, b: V) -> Result<V> {
        if a.v.kind != b.v.kind || a.v.width != 1 || b.v.width != 1 {
            return self.fail("a comparison of values that are not scalars of one type");
        }
        Ok(self.b.binary(op, a.v, b.v).into())
    }

    fn additive(&mut self) -> Result<V> {
        let mut a = self.multiplicative()?;
        loop {
            let op = if self.at("+") {
                Bo::Add
            } else if self.at("-") {
                Bo::Subtract
            } else {
                return Ok(a);
            };
            self.pos = self.pos.saturating_add(1);
            let b = self.multiplicative()?;
            a = self.arithmetic(op, a, b)?;
        }
    }

    fn multiplicative(&mut self) -> Result<V> {
        let mut a = self.unary()?;
        loop {
            let op = if self.at("*") {
                Bo::Multiply
            } else if self.at("/") {
                Bo::Divide
            } else {
                return Ok(a);
            };
            self.pos = self.pos.saturating_add(1);
            let b = self.unary()?;
            a = self.arithmetic(op, a, b)?;
        }
    }

    fn arithmetic(&mut self, op: Bo, a: V, b: V) -> Result<V> {
        if a.v.kind != b.v.kind || !matches!(a.v.kind, Kind::F | Kind::I) {
            return self.fail("arithmetic on values of different types (or booleans)");
        }
        if a.v.kind == Kind::I && op == Bo::Divide {
            return self.fail("integer division");
        }
        let literal = match (a.literal, b.literal, op) {
            (Some(x), Some(y), Bo::Add) => x.checked_add(y),
            (Some(x), Some(y), Bo::Subtract) => x.checked_sub(y),
            (Some(x), Some(y), Bo::Multiply) => x.checked_mul(y),
            _ => None,
        };
        let v = self.b.binary(op, a.v, b.v);
        Ok(V { v, literal })
    }

    fn unary(&mut self) -> Result<V> {
        if self.accept("-") {
            let a = self.unary()?;
            if !matches!(a.v.kind, Kind::F | Kind::I) {
                return self.fail("a negation of a boolean");
            }
            let v = self.b.unary(Uo::Negate, a.v);
            return Ok(V {
                v,
                literal: a.literal.and_then(i64::checked_neg),
            });
        }
        if self.accept("!") {
            let a = self.unary()?;
            if a.v.kind != Kind::B {
                return self.fail("! on something that is not a boolean");
            }
            return Ok(self.b.unary(Uo::LogicalNot, a.v).into());
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<V> {
        let mut v = self.primary()?;
        loop {
            if self.accept(".") {
                let w = self.ident()?;
                let Some(lanes) = swizzle_lanes(&w) else {
                    return self.fail(format!("the swizzle {w}"));
                };
                if lanes.iter().any(|l| *l >= v.v.width) {
                    return self.fail(format!("the swizzle {w} of a value {} lanes wide", v.v.width));
                }
                let r = if lanes.len() == 1 {
                    self.b.lane(v.v, lanes[0])
                } else {
                    self.b.swizzle(v.v, &lanes)
                };
                v = r.into();
            } else {
                return Ok(v);
            }
        }
    }

    fn primary(&mut self) -> Result<V> {
        match self.next()? {
            Token::Float(f) => Ok(self.b.lit_f(f).into()),
            Token::Int(i) => {
                let value = i32::try_from(i).map_err(|_| error(self.line(), "an integer over 32 bits"))?;
                Ok(V {
                    v: self.b.lit_i(value),
                    literal: Some(i),
                })
            }
            Token::Punct("(") => {
                let v = self.expression()?;
                self.expect(")")?;
                Ok(v)
            }
            Token::Ident(name) => self.name(&name),
            Token::Punct(p) => self.fail(format!("{p} where a value was expected")),
        }
    }

    fn name(&mut self, name: &str) -> Result<V> {
        if self.at("(") {
            return self.call(name);
        }
        if name == "gl_FragData" {
            return self.fail("gl_FragData read");
        }
        if matches!(
            name,
            "gl_FragCoord" | "gl_FrontFacing" | "gl_PointCoord" | "gl_FragDepth"
        ) {
            return self.fail(name);
        }
        let Some(sym) = self.symbols.get(name).cloned() else {
            return self.fail(format!("the name {name}"));
        };
        match sym {
            Sym::Local(local, kind, width) => {
                let p = self.b.local_ptr(local);
                Ok(self.b.load(p, kind, width).into())
            }
            Sym::Const(v, lanes) => {
                let _ = lanes;
                Ok(v.into())
            }
            Sym::Argument(index) => {
                let expr = self.b.app(E::FunctionArgument(index as u32));
                Ok(Val {
                    expr,
                    kind: Kind::F,
                    width: 4,
                }
                .into())
            }
            Sym::Loop(v) => Ok(v.into()),
            Sym::Constants(rows) => {
                self.expect("[")?;
                let index = self.expression()?;
                self.expect("]")?;
                self.constant_row(rows, index)
            }
            Sym::Sampler(_) => self.fail(format!("the sampler {name} used as a value")),
            Sym::PosFixup => {
                self.fail("posFixup used outside the clip-space fix MojoShader ends a vertex program with")
            }
        }
    }

    /// Row `index` of the uniform array as four floats; past the array, zero (D3D9's relative addressing).
    fn constant_row(&mut self, rows: u32, index: V) -> Result<V> {
        let Some((global, _)) = self.constants else {
            return self.fail("the uniform array is not declared");
        };
        if index.v.kind != Kind::I || index.v.width != 1 {
            return self.fail("a constant row indexed by something that is not an integer");
        }
        let pointer = self.b.global_ptr(global);
        if let Some(row) = index.literal {
            return match u32::try_from(row).ok().filter(|r| *r < rows) {
                Some(row) => {
                    let element = self.b.app(E::AccessIndex {
                        base: pointer,
                        index: row,
                    });
                    let words = self.b.load(element, Kind::U, 4);
                    Ok(self.b.bitcast(words, Kind::F).into())
                }
                None => self.fail(format!("constant row {row} of {rows}")),
            };
        }
        let row = self.b.bitcast(index.v, Kind::U);
        let length = self.b.lit_u(rows);
        let inside = self.b.binary(Bo::Less, row, length);
        let zero = self.b.lit_u(0);
        let safe = self.b.select(inside, row, zero);
        let element = self.b.app(E::Access {
            base: pointer,
            index: safe.expr,
        });
        let words = self.b.load(element, Kind::U, 4);
        let nothing = self.b.splat(zero, 4);
        let words = self.b.select(inside, words, nothing);
        Ok(self.b.bitcast(words, Kind::F).into())
    }

    fn arguments(&mut self) -> Result<Vec<V>> {
        self.expect("(")?;
        let mut out = Vec::new();
        if self.accept(")") {
            return Ok(out);
        }
        loop {
            out.push(self.expression()?);
            if self.accept(")") {
                return Ok(out);
            }
            self.expect(",")?;
        }
    }

    fn call(&mut self, name: &str) -> Result<V> {
        if name == "texture2D" {
            self.expect("(")?;
            let sampler = self.ident()?;
            self.expect(",")?;
            let coordinate = self.expression()?;
            self.expect(")")?;
            let Some(Sym::Sampler(index)) = self.symbols.get(&sampler).cloned() else {
                return self.fail(format!("texture2D of {sampler}, which is not a sampler"));
            };
            if coordinate.v.kind != Kind::F || coordinate.v.width != 2 {
                return self.fail("texture2D at a coordinate that is not two floats");
            }
            let (texture, sampler) = (self.samplers[index].texture, self.samplers[index].sampler);
            let image = self.b.global_ptr(texture);
            let sampler = self.b.global_ptr(sampler);
            let expr = self.b.app(E::ImageSample {
                image,
                sampler,
                gather: None,
                coordinate: coordinate.v.expr,
                array_index: None,
                offset: None,
                level: SampleLevel::Auto,
                depth_ref: None,
                clamp_to_edge: false,
            });
            return Ok(Val {
                expr,
                kind: Kind::F,
                width: 4,
            }
            .into());
        }
        let args = self.arguments()?;
        match name {
            "vec2" | "vec3" | "vec4" | "ivec2" | "ivec3" | "ivec4" => self.construct(name, &args),
            "float" | "int" => {
                let [a] = args.as_slice() else {
                    return self.fail(format!("{name}() of {} arguments", args.len()));
                };
                if a.v.width != 1 {
                    return self.fail(format!("{name}() of a vector"));
                }
                let kind = if name == "float" { Kind::F } else { Kind::I };
                Ok(if a.v.kind == kind {
                    *a
                } else {
                    self.b.convert(a.v, kind).into()
                })
            }
            "dFdx" | "dFdy" => {
                let [a] = args.as_slice() else {
                    return self.fail(format!("{name} of {} arguments", args.len()));
                };
                if self.stage != Stage::Pixel {
                    return self.fail(format!("{name} in a vertex program"));
                }
                let expr = self.b.app(E::Derivative {
                    axis: if name == "dFdx" {
                        DerivativeAxis::X
                    } else {
                        DerivativeAxis::Y
                    },
                    ctrl: DerivativeControl::None,
                    expr: a.v.expr,
                });
                Ok(Val { expr, ..a.v }.into())
            }
            "any" | "all" => {
                let [a] = args.as_slice() else {
                    return self.fail(format!("{name} of {} arguments", args.len()));
                };
                if a.v.kind != Kind::B {
                    return self.fail(format!("{name} of something that is not boolean"));
                }
                if a.v.width == 1 {
                    return Ok(*a);
                }
                let expr = self.b.app(E::Relational {
                    fun: if name == "any" {
                        RelationalFunction::Any
                    } else {
                        RelationalFunction::All
                    },
                    argument: a.v.expr,
                });
                Ok(Val {
                    expr,
                    kind: Kind::B,
                    width: 1,
                }
                .into())
            }
            "lessThan" | "lessThanEqual" | "greaterThan" | "greaterThanEqual" | "equal" | "notEqual" => {
                let [a, b] = args.as_slice() else {
                    return self.fail(format!("{name} of {} arguments", args.len()));
                };
                if a.v.kind != b.v.kind || a.v.width != b.v.width || a.v.kind == Kind::B {
                    return self.fail(format!("{name} of values that differ in type or width"));
                }
                let op = match name {
                    "lessThan" => Bo::Less,
                    "lessThanEqual" => Bo::LessEqual,
                    "greaterThan" => Bo::Greater,
                    "greaterThanEqual" => Bo::GreaterEqual,
                    "equal" => Bo::Equal,
                    _ => Bo::NotEqual,
                };
                Ok(self.b.binary(op, a.v, b.v).into())
            }
            _ => {
                let Some((fun, arity)) = builtin(name) else {
                    return self.fail(format!("the function {name}"));
                };
                if args.len() != arity {
                    return self.fail(format!("{name} of {} arguments", args.len()));
                }
                if args.iter().any(|a| a.v.kind != Kind::F) {
                    return self.fail(format!("{name} of something that is not a float"));
                }
                let vals: Vec<Val> = args.iter().map(|a| a.v).collect();
                Ok(self.b.math(fun, &vals).into())
            }
        }
    }

    /// `vec4(..)`, `ivec4(..)`: the lanes of the arguments in order, or one scalar splat; a float vector converted to an
    /// integer one, as GLSL's constructor does (truncating).
    fn construct(&mut self, name: &str, args: &[V]) -> Result<V> {
        let width: u8 = name
            .chars()
            .last()
            .and_then(|c| c.to_digit(10))
            .and_then(|d| u8::try_from(d).ok())
            .unwrap_or(4);
        let kind = if name.starts_with('i') { Kind::I } else { Kind::F };
        let mut lanes = Vec::new();
        for a in args {
            let v = if a.v.kind == kind {
                a.v
            } else if matches!(a.v.kind, Kind::F | Kind::I) {
                self.b.convert(a.v, kind)
            } else {
                return self.fail(format!("{name}() of a boolean"));
            };
            for k in 0..v.width {
                lanes.push(self.b.lane(v, k));
            }
        }
        match lanes.len() {
            1 => {
                let v = self.b.splat(lanes[0], width);
                Ok(v.into())
            }
            n if n == usize::from(width) => Ok(self.b.compose(&lanes).into()),
            n => self.fail(format!("{name}() of {n} lanes")),
        }
    }

    // ---- the program ----

    fn finish(mut self, notes: Vec<String>) -> Result<Program> {
        let interface = Interface {
            stage: self.stage,
            inputs: self.inputs.clone(),
            outputs: self.outputs.iter().map(|(v, _)| v.clone()).collect(),
            resources: Vec::new(),
            facts: StageFacts::None,
        };
        // the resources: the constants, then each sampler's texture and sampler
        let mut resources = Vec::new();
        let mut bindings = Vec::new();
        if let Some((global, rows)) = self.constants {
            let size = rows.saturating_mul(16);
            let mut constants = Vec::new();
            for c in &self.hints.constants {
                // the program declares the rows it reads: a constant beyond them is not its, one that reaches past them
                // is cut to the rows it has
                if c.offset >= size {
                    continue;
                }
                let bytes = c.bytes.min(size.saturating_sub(c.offset));
                let ty = if bytes == 16 {
                    if c.width <= 1 {
                        ConstantType::Scalar(ScalarType::Float)
                    } else {
                        ConstantType::Vector(float_value(c.width.min(4)))
                    }
                } else {
                    ConstantType::Array {
                        element: Box::new(ConstantType::Vector(float_value(4))),
                        count: (bytes / 16).max(1),
                        stride: 16,
                    }
                };
                constants.push(LayoutConstant {
                    name: c.name.clone(),
                    offset: c.offset,
                    ty,
                    default: None,
                });
            }
            let id = ResourceId(resources.len() as u32);
            resources.push(Resource {
                id,
                kind: ResourceKind::ConstantBuffer {
                    layout: BufferLayout { size, constants },
                },
                count: 1,
                provenance: ResourceProvenance {
                    name: if self.stage == Stage::Vertex {
                        "vs_uniforms_vec4"
                    } else {
                        "ps_uniforms_vec4"
                    }
                    .into(),
                    d3d: Some(D3dBinding {
                        class: D3dClass::ConstantBuffer,
                        register: 0,
                        space: 0,
                    }),
                    counter_of: None,
                },
            });
            bindings.push((id, global));
        }
        for s in &self.samplers {
            let id = ResourceId(resources.len() as u32);
            resources.push(Resource {
                id,
                kind: ResourceKind::Texture {
                    dim: TextureDim::D2,
                    arrayed: false,
                    multisampled: false,
                    sample: SampleType::Float,
                },
                count: 1,
                provenance: ResourceProvenance {
                    name: s.name.clone(),
                    d3d: Some(D3dBinding {
                        class: D3dClass::ShaderResource,
                        register: s.slot,
                        space: 0,
                    }),
                    counter_of: None,
                },
            });
            bindings.push((id, s.texture));
            let id = ResourceId(resources.len() as u32);
            resources.push(Resource {
                id,
                kind: ResourceKind::Sampler { comparison: false },
                count: 1,
                provenance: ResourceProvenance {
                    name: format!("{} ({})", s.name, s.glsl_name),
                    d3d: Some(D3dBinding {
                        class: D3dClass::Sampler,
                        register: s.slot,
                        space: 0,
                    }),
                    counter_of: None,
                },
            });
            bindings.push((id, s.sampler));
        }
        let interface = Interface {
            resources,
            ..interface
        };
        let io = entry_io(&interface).map_err(|detail| error(0, detail))?;
        let decl = self.b.declare_entry(&io).map_err(|detail| error(0, detail))?;
        // the result: every output's local, in the interface's order
        let mut parts = Vec::new();
        for (varying, local) in &self.outputs {
            let p = self.b.local_ptr(*local);
            parts.push(self.b.load(p, Kind::F, varying.value.width).expr);
        }
        let out = decl
            .result
            .ok_or_else(|| error(0, "a program that returns nothing"))?;
        let result = self.b.app(E::Compose {
            ty: out,
            components: parts,
        });
        self.b.push(Statement::Return { value: Some(result) });
        let module = self.b.finish(
            "main",
            if self.stage == Stage::Vertex {
                naga::ShaderStage::Vertex
            } else {
                naga::ShaderStage::Fragment
            },
            decl.arguments,
            Some(out),
            false,
        );
        let mut notes = notes;
        notes.extend(primase_shader::interface_notes(&interface));
        Ok(Program {
            module,
            stage: self.stage,
            entry: "main".into(),
            interface,
            bindings,
            conventions: Conventions::D3D,
            modes: D3dModes::default(),
            provenance: Provenance {
                format: ShaderFormat::Glsl,
                target: self.hints.target.clone(),
                lifter: "primase-mojoshader".into(),
                lifter_version: LIFTER_VERSION,
                source_hash: None,
            },
            notes,
        })
    }
}

/// The lanes a swizzle names (`xyzw`, `rgba`, `stpq`), none when it is not one.
fn swizzle_lanes(word: &str) -> Option<Vec<u8>> {
    if word.is_empty() || word.len() > 4 {
        return None;
    }
    word.chars()
        .map(|c| match c {
            'x' | 'r' | 's' => Some(0),
            'y' | 'g' | 't' => Some(1),
            'z' | 'b' | 'p' => Some(2),
            'w' | 'a' | 'q' => Some(3),
            _ => None,
        })
        .collect()
}

/// The built-in functions of the dialect that are one naga math function, and their number of arguments.
fn builtin(name: &str) -> Option<(Mf, usize)> {
    Some(match name {
        "dot" => (Mf::Dot, 2),
        "abs" => (Mf::Abs, 1),
        "fract" => (Mf::Fract, 1),
        "inversesqrt" => (Mf::InverseSqrt, 1),
        "sqrt" => (Mf::Sqrt, 1),
        "clamp" => (Mf::Clamp, 3),
        "sin" => (Mf::Sin, 1),
        "cos" => (Mf::Cos, 1),
        "exp2" => (Mf::Exp2, 1),
        "log2" => (Mf::Log2, 1),
        "mix" => (Mf::Mix, 3),
        "normalize" => (Mf::Normalize, 1),
        "min" => (Mf::Min, 2),
        "max" => (Mf::Max, 2),
        "sign" => (Mf::Sign, 1),
        "floor" => (Mf::Floor, 1),
        "ceil" => (Mf::Ceil, 1),
        "pow" => (Mf::Pow, 2),
        "length" => (Mf::Length, 1),
        "cross" => (Mf::Cross, 2),
        _ => return None,
    })
}
