//! A program's WGSL as one pipeline binds it ([`wgsl_for`]): where its resources are bound, where a vertex program's
//! outputs land in the program that reads them, and what WebGPU without its optional features cannot take. Each is a
//! change to a copy of the program's module, written once it validates: the text is never edited, and the
//! [`Program`] keeps its own interface and bindings.
use std::collections::{BTreeMap, BTreeSet};

use naga::{
    Binding, Block, Expression, Handle, Interpolation, ResourceBinding, Span, Statement, StructMember, Type,
    TypeInner,
};

use crate::program::module_info;
use crate::{EntryIo, Interface, IoType, Label, Program, ProgramError as Error, ResourceId, Stage, entry_io};

/// Clip distances carried as plain varyings: `count` scalars at the locations from `first`, interpolated
/// perspective-correct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipVaryings {
    pub first: u32,
    pub count: u32,
}

/// The pipeline a program's WGSL is written for. The default changes nothing: the text is [`super::wgsl`]'s.
#[derive(Clone, Debug, Default)]
pub struct Pipeline<'a> {
    /// The group and binding of each resource named here, in place of the program's own.
    pub bindings: BTreeMap<ResourceId, (u32, u32)>,
    /// The program a vertex program's outputs feed. Each output the consumer reads, matched as Direct3D links stages
    /// (by semantic: the label's name without case, and its index), takes the consumer's location and interpolation
    /// for it ([`entry_io`]); the others take the locations after the consumer's. A consumer input no output carries
    /// is an error. The value types are each program's own: a consumer that reads fewer lanes than its producer
    /// writes is the pipeline's to accept or refuse.
    pub feeds: Option<&'a Interface>,
    /// A vertex program's clip distances as plain varyings ([`ClipVaryings`], after every other output), for a
    /// pipeline without WebGPU's `clip-distances` feature. A consumer discards where one is negative
    /// ([`Pipeline::clip_discard`]): a distance linear in the clip position interpolates perspective-correct to its
    /// value at the pixel, so this keeps what a clipper keeps, decided at pixel centres.
    pub clip_varyings: bool,
    /// A pixel program's inputs for its producer's [`ClipVaryings`], and a discard before each return where one of
    /// them is negative (last, after every sample the program takes: a derivative after a discard is not uniform).
    pub clip_discard: Option<ClipVaryings>,
    /// Each sampler heap (an array of samplers indexed at run time, WebGPU's binding arrays) as one sampler: every
    /// index reads the same sampler, so a program whose samplers differ by index is not drawn as written (noted).
    pub single_samplers: bool,
}

/// A program's WGSL for a [`Pipeline`], and where the pipeline finds what the text holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    pub text: String,
    /// The group and binding of every resource of the program in the text.
    pub bindings: BTreeMap<ResourceId, (u32, u32)>,
    /// A vertex program's clip distances as varyings, when [`Pipeline::clip_varyings`] made them.
    pub clip: Option<ClipVaryings>,
    /// The first location after every one the text's outputs (a vertex program) or inputs (a pixel program) use.
    pub next_location: u32,
    /// What the text approximates of the program, each once.
    pub notes: Vec<String>,
}

/// The program as WGSL for `pipeline` ([`Pipeline`]). The program is validated first and refused as
/// [`super::wgsl`] refuses it; a pipeline it cannot meet (a resource that is not the program's, two resources at
/// one binding, a link or clip option for a stage it does not apply to, a consumer input with no output) is an
/// [`Error::Pipeline`].
pub fn wgsl_for(program: &Program, pipeline: &Pipeline) -> Result<Written, Error> {
    super::writable(program)?;
    program.validated()?;
    let mut module = program.module.clone();
    let mut notes = Vec::new();
    rebind(program, &mut module, &pipeline.bindings)?;
    if pipeline.single_samplers {
        let heaps = single_samplers(&mut module);
        if heaps > 0 {
            notes.push(format!(
                "{heaps} sampler heap{} written as one sampler: every index reads the same sampler",
                if heaps == 1 { "" } else { "s" }
            ));
        }
    }
    let io = entry_io(&program.interface).map_err(Error::Invalid)?;
    let vertex = program.stage == Stage::Vertex;
    if !vertex && (pipeline.feeds.is_some() || pipeline.clip_varyings) {
        return Err(Error::Pipeline(format!(
            "a {:?} program feeds no consumer and has no clip distances to carry",
            program.stage
        )));
    }
    if program.stage != Stage::Pixel && pipeline.clip_discard.is_some() {
        return Err(Error::Pipeline(format!(
            "a {:?} program does not discard by clip distances",
            program.stage
        )));
    }
    let (clip, next_location) = if vertex {
        outputs(program, &io, &mut module, pipeline)?
    } else {
        let next = next_after(io.arguments.iter().map(|a| &a.binding));
        match pipeline.clip_discard {
            Some(clips) => (None, discard_clipped(&io, &mut module, clips, next)?),
            None => (None, next),
        }
    };
    let info = module_info(&module).map_err(|e| Error::Pipeline(format!("the lowered module: {e}")))?;
    let text = naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
        .map_err(|e| Error::Unsupported {
            writer: "wgsl",
            detail: e.to_string(),
        })?;
    let bindings = program
        .bindings
        .iter()
        .filter_map(|(id, g)| {
            let b = module.global_variables[*g].binding.as_ref()?;
            Some((*id, (b.group, b.binding)))
        })
        .collect();
    Ok(Written {
        text,
        bindings,
        clip,
        next_location,
        notes,
    })
}

/// The first location after the ones `bindings` use (0 when they use none).
fn next_after<'b>(bindings: impl Iterator<Item = &'b Binding>) -> u32 {
    bindings
        .filter_map(|b| match b {
            Binding::Location { location, .. } => Some(location + 1),
            Binding::BuiltIn(_) => None,
        })
        .max()
        .unwrap_or(0)
}

/// Each resource of `bindings` at its group and binding; two resources at one place are refused.
fn rebind(
    program: &Program,
    module: &mut naga::Module,
    bindings: &BTreeMap<ResourceId, (u32, u32)>,
) -> Result<(), Error> {
    for (id, &(group, binding)) in bindings {
        let g = program
            .global(*id)
            .ok_or_else(|| Error::Pipeline(format!("resource {} is not one of the program's", id.0)))?;
        module.global_variables[g].binding = Some(ResourceBinding { group, binding });
    }
    let mut taken: BTreeMap<(u32, u32), Option<String>> = BTreeMap::new();
    for (_, v) in module.global_variables.iter() {
        let Some(b) = &v.binding else { continue };
        if let Some(other) = taken.insert((b.group, b.binding), v.name.clone()) {
            return Err(Error::Pipeline(format!(
                "{} and {} are both at group {} binding {}",
                other.as_deref().unwrap_or("a global"),
                v.name.as_deref().unwrap_or("a global"),
                b.group,
                b.binding
            )));
        }
    }
    Ok(())
}

/// Each global that is an array of samplers made one sampler, every access to it the sampler itself. Returns how
/// many there were.
fn single_samplers(module: &mut naga::Module) -> usize {
    let heaps: BTreeMap<Handle<naga::GlobalVariable>, Handle<Type>> = module
        .global_variables
        .iter()
        .filter_map(|(g, v)| match module.types[v.ty].inner {
            TypeInner::BindingArray { base, .. }
                if matches!(module.types[base].inner, TypeInner::Sampler { .. }) =>
            {
                Some((g, base))
            }
            _ => None,
        })
        .collect();
    if heaps.is_empty() {
        return 0;
    }
    for (g, base) in &heaps {
        module.global_variables[*g].ty = *base;
    }
    let functions = module
        .functions
        .iter_mut()
        .map(|(_, f)| f)
        .chain(module.entry_points.iter_mut().map(|e| &mut e.function));
    for function in functions {
        let accesses: Vec<(Handle<Expression>, Handle<Expression>)> = function
            .expressions
            .iter()
            .filter_map(|(h, e)| match e {
                Expression::Access { base, .. } | Expression::AccessIndex { base, .. } => matches!(
                    function.expressions[*base],
                    Expression::GlobalVariable(g) if heaps.contains_key(&g)
                )
                .then_some((h, *base)),
                _ => None,
            })
            .collect();
        if accesses.is_empty() {
            continue;
        }
        // an access becomes the global it accessed: a global is no emitted expression, so it leaves its emit range
        let mut flattened = BTreeSet::new();
        for (access, base) in accesses {
            function.expressions[access] = function.expressions[base].clone();
            function.named_expressions.shift_remove(&access);
            flattened.insert(access);
        }
        let body = std::mem::take(&mut function.body);
        function.body = unemit(body, &flattened, &function.expressions);
    }
    heaps.len()
}

/// `block` with the `removed` expressions left out of its emit ranges, in nested blocks too.
fn unemit(block: Block, removed: &BTreeSet<Handle<Expression>>, arena: &naga::Arena<Expression>) -> Block {
    let removed_index: BTreeSet<u32> = removed.iter().map(|h| h.index() as u32).collect();
    let nested = |b: Block| unemit(b, removed, arena);
    let mut out = Block::with_capacity(block.len());
    for (statement, span) in block.span_into_iter() {
        match statement {
            Statement::Emit(range) => {
                // the range in runs between the removed expressions
                let mut start = None;
                let indices = range.index_range();
                for i in indices.clone().chain([indices.end]) {
                    let kept = i < indices.end && !removed_index.contains(&i);
                    match (kept, start) {
                        (true, None) => start = Some(i),
                        (false, Some(s)) => {
                            out.push(Statement::Emit(naga::Range::from_index_range(s..i, arena)), span);
                            start = None;
                        }
                        _ => {}
                    }
                }
            }
            Statement::Block(b) => out.push(Statement::Block(nested(b)), span),
            Statement::If {
                condition,
                accept,
                reject,
            } => out.push(
                Statement::If {
                    condition,
                    accept: nested(accept),
                    reject: nested(reject),
                },
                span,
            ),
            Statement::Switch { selector, cases } => out.push(
                Statement::Switch {
                    selector,
                    cases: cases
                        .into_iter()
                        .map(|c| naga::SwitchCase {
                            body: nested(c.body),
                            ..c
                        })
                        .collect(),
                },
                span,
            ),
            Statement::Loop {
                body,
                continuing,
                break_if,
            } => out.push(
                Statement::Loop {
                    body: nested(body),
                    continuing: nested(continuing),
                    break_if,
                },
                span,
            ),
            other => out.push(other, span),
        }
    }
    out
}

/// Whether two labels are one semantic, as Direct3D links stages: the name without case, and the index.
fn same_label(a: &Label, b: &Label) -> bool {
    a.index == b.index && a.name.eq_ignore_ascii_case(&b.name)
}

/// A vertex program's result bindings for `pipeline` (its consumer's locations, its clip distances as varyings),
/// the result structure rebuilt with them and every return made of it. Returns the clip varyings and the first location
/// after the outputs'.
fn outputs(
    program: &Program,
    io: &EntryIo,
    module: &mut naga::Module,
    pipeline: &Pipeline,
) -> Result<(Option<ClipVaryings>, u32), Error> {
    let mut bindings: Vec<Binding> = io.results.iter().map(|r| r.binding.clone()).collect();
    if let Some(consumer) = pipeline.feeds {
        link(program, io, consumer, &mut bindings)?;
    }
    let mut next = next_after(bindings.iter());
    let clip_member = io
        .results
        .iter()
        .position(|r| matches!(r.ty, IoType::ClipDistances(_)))
        .filter(|_| pipeline.clip_varyings);
    let clip = clip_member.map(|k| {
        let IoType::ClipDistances(count) = io.results[k].ty else {
            unreachable!("the clip member")
        };
        let clips = ClipVaryings { first: next, count };
        next += count;
        clips
    });
    let unchanged = clip.is_none() && bindings.iter().zip(&io.results).all(|(b, r)| *b == r.binding);
    if unchanged {
        return Ok((None, next));
    }
    let entry = module
        .entry_points
        .first()
        .ok_or_else(|| Error::Invalid("no entry point".into()))?;
    let old = entry
        .function
        .result
        .as_ref()
        .ok_or_else(|| Error::Invalid("a vertex program with no result".into()))?
        .ty;
    let TypeInner::Struct { members, span } = module.types[old].inner.clone() else {
        return Err(Error::Invalid(
            "a vertex program's result is not a structure".into(),
        ));
    };
    if members.len() != bindings.len() {
        return Err(Error::Invalid(format!(
            "the result structure has {} members, the interface {}",
            members.len(),
            bindings.len()
        )));
    }
    let f32_ty = module.types.insert(
        Type {
            name: None,
            inner: TypeInner::Scalar(naga::Scalar::F32),
        },
        Span::UNDEFINED,
    );
    let member_count = members.len();
    let mut new_members = Vec::with_capacity(member_count);
    for (k, (member, binding)) in members.into_iter().zip(bindings).enumerate() {
        match clip {
            Some(clips) if Some(k) == clip_member => {
                // the array's elements, at its offsets
                for j in 0..clips.count {
                    new_members.push(StructMember {
                        name: Some(format!("clip_distance{j}")),
                        ty: f32_ty,
                        binding: Some(Binding::Location {
                            location: clips.first + j,
                            interpolation: Some(Interpolation::Perspective),
                            sampling: None,
                            blend_src: None,
                            per_primitive: false,
                        }),
                        offset: member.offset + 4 * j,
                    });
                }
            }
            _ => new_members.push(StructMember {
                binding: Some(binding),
                ..member
            }),
        }
    }
    let name = module.types[old].name.clone();
    let new = module.types.insert(
        Type {
            name: name.clone(),
            inner: TypeInner::Struct {
                members: new_members,
                span,
            },
        },
        Span::UNDEFINED,
    );
    // the old structure stays as the value each return is made of, without its bindings: a built-in it binds (the
    // clip distances) would otherwise still be in the module, and in the text's `enable`s
    let TypeInner::Struct { members, span } = module.types[old].inner.clone() else {
        unreachable!("checked above")
    };
    let plain = Type {
        name,
        inner: TypeInner::Struct {
            members: members
                .into_iter()
                .map(|m| StructMember { binding: None, ..m })
                .collect(),
            span,
        },
    };
    if module.types.get(&plain).is_some() {
        return Err(Error::Pipeline(
            "the result structure without its bindings is already a type of the module".into(),
        ));
    }
    module.types.replace(old, plain);
    let entry = module
        .entry_points
        .first_mut()
        .ok_or_else(|| Error::Invalid("no entry point".into()))?;
    if let Some(result) = entry.function.result.as_mut() {
        result.ty = new;
    }
    // each return's value, member by member (each clip distance an element of the array), as the new structure
    let function = &mut entry.function;
    let body = std::mem::take(&mut function.body);
    function.body = returns(body, &mut function.expressions, &mut |value, expressions| {
        // a function with a result returns a value (the program validated)
        let Some(value) = value else {
            return (Vec::new(), None);
        };
        let start = expressions.len();
        let mut components = Vec::new();
        for k in 0..member_count {
            let part = expressions.append(
                Expression::AccessIndex {
                    base: value,
                    index: k as u32,
                },
                Span::UNDEFINED,
            );
            match clip {
                Some(clips) if Some(k) == clip_member => {
                    for j in 0..clips.count {
                        components.push(
                            expressions
                                .append(Expression::AccessIndex { base: part, index: j }, Span::UNDEFINED),
                        );
                    }
                }
                _ => components.push(part),
            }
        }
        let compose = expressions.append(Expression::Compose { ty: new, components }, Span::UNDEFINED);
        let emitted = naga::Range::from_index_range(start as u32..expressions.len() as u32, expressions);
        (vec![Statement::Emit(emitted)], Some(compose))
    });
    Ok((clip, next))
}

/// Link a vertex program's result `bindings` to `consumer`'s inputs (see [`Pipeline::feeds`]).
fn link(
    program: &Program,
    io: &EntryIo,
    consumer: &Interface,
    bindings: &mut [Binding],
) -> Result<(), Error> {
    let wanted = entry_io(consumer).map_err(|e| Error::Pipeline(format!("the consumer: {e}")))?;
    let mut consumed = vec![false; bindings.len()];
    let mut next = 0;
    for argument in &wanted.arguments {
        let Binding::Location { location, .. } = argument.binding else {
            continue;
        };
        next = next.max(location + 1);
        let label = &consumer.inputs[argument.varyings[0]].label;
        let k = io
            .results
            .iter()
            .position(|r| {
                matches!(r.binding, Binding::Location { .. })
                    && same_label(&program.interface.outputs[r.varyings[0]].label, label)
            })
            .ok_or_else(|| {
                let written: Vec<String> = io
                    .results
                    .iter()
                    .filter(|r| matches!(r.binding, Binding::Location { .. }))
                    .map(|r| r.name.clone())
                    .collect();
                Error::Pipeline(format!(
                    "the consumer reads {}{}, which the vertex program does not write (it writes {})",
                    label.name,
                    label.index,
                    written.join(", ")
                ))
            })?;
        bindings[k] = argument.binding.clone();
        consumed[k] = true;
    }
    for (binding, consumed) in bindings.iter_mut().zip(consumed) {
        if let (Binding::Location { location, .. }, false) = (binding, consumed) {
            *location = next;
            next += 1;
        }
    }
    Ok(())
}

/// A pixel program's inputs for `clips` and a discard before each return where one is negative. `next` is the first
/// location after the program's own inputs; the clip varyings must not be below it. Returns the first location after
/// them.
fn discard_clipped(
    io: &EntryIo,
    module: &mut naga::Module,
    clips: ClipVaryings,
    next: u32,
) -> Result<u32, Error> {
    if clips.first < next {
        return Err(Error::Pipeline(format!(
            "the clip varyings at {} meet the pixel program's own inputs (locations below {next})",
            clips.first
        )));
    }
    let f32_ty = module.types.insert(
        Type {
            name: None,
            inner: TypeInner::Scalar(naga::Scalar::F32),
        },
        Span::UNDEFINED,
    );
    let entry = module
        .entry_points
        .first_mut()
        .ok_or_else(|| Error::Invalid("no entry point".into()))?;
    let function = &mut entry.function;
    debug_assert_eq!(function.arguments.len(), io.arguments.len());
    let mut distances = Vec::with_capacity(clips.count as usize);
    for j in 0..clips.count {
        let index = function.arguments.len() as u32;
        function.arguments.push(naga::FunctionArgument {
            name: Some(format!("clip_distance{j}")),
            ty: f32_ty,
            binding: Some(Binding::Location {
                location: clips.first + j,
                interpolation: Some(Interpolation::Perspective),
                sampling: None,
                blend_src: None,
                per_primitive: false,
            }),
        });
        distances.push(
            function
                .expressions
                .append(Expression::FunctionArgument(index), Span::UNDEFINED),
        );
    }
    let zero = function
        .expressions
        .append(Expression::Literal(naga::Literal::F32(0.0)), Span::UNDEFINED);
    // the test, made afresh where it is used: an expression is emitted once in its scope
    let test = |expressions: &mut naga::Arena<Expression>| -> Vec<Statement> {
        let start = expressions.len();
        let mut condition = None;
        for &d in &distances {
            let below = expressions.append(
                Expression::Binary {
                    op: naga::BinaryOperator::Less,
                    left: d,
                    right: zero,
                },
                Span::UNDEFINED,
            );
            condition = Some(match condition {
                None => below,
                Some(earlier) => expressions.append(
                    Expression::Binary {
                        op: naga::BinaryOperator::LogicalOr,
                        left: earlier,
                        right: below,
                    },
                    Span::UNDEFINED,
                ),
            });
        }
        let Some(condition) = condition else {
            return Vec::new();
        };
        let emitted = naga::Range::from_index_range(start as u32..expressions.len() as u32, expressions);
        vec![
            Statement::Emit(emitted),
            Statement::If {
                condition,
                accept: Block::from_vec(vec![Statement::Kill]),
                reject: Block::new(),
            },
        ]
    };
    let body = std::mem::take(&mut function.body);
    let mut body = returns(body, &mut function.expressions, &mut |value, expressions| {
        (test(expressions), value)
    });
    // a program with no result may end without a return: the end is one
    if function.result.is_none() && !matches!(body.last(), Some(Statement::Return { .. })) {
        for statement in test(&mut function.expressions) {
            body.push(statement, Span::UNDEFINED);
        }
    }
    function.body = body;
    Ok(clips.first + clips.count)
}

/// `block` with each return (in nested blocks too) preceded by the statements `at` makes and returning the value it
/// gives, from the value it returned.
#[allow(clippy::type_complexity)]
fn returns(
    block: Block,
    expressions: &mut naga::Arena<Expression>,
    at: &mut dyn FnMut(
        Option<Handle<Expression>>,
        &mut naga::Arena<Expression>,
    ) -> (Vec<Statement>, Option<Handle<Expression>>),
) -> Block {
    let mut out = Block::with_capacity(block.len());
    for (statement, span) in block.span_into_iter() {
        match statement {
            Statement::Return { value } => {
                let (before, value) = at(value, expressions);
                for s in before {
                    out.push(s, span);
                }
                out.push(Statement::Return { value }, span);
            }
            Statement::Block(b) => out.push(Statement::Block(returns(b, expressions, at)), span),
            Statement::If {
                condition,
                accept,
                reject,
            } => {
                let accept = returns(accept, expressions, at);
                let reject = returns(reject, expressions, at);
                out.push(
                    Statement::If {
                        condition,
                        accept,
                        reject,
                    },
                    span,
                );
            }
            Statement::Switch { selector, cases } => {
                let cases = cases
                    .into_iter()
                    .map(|c| naga::SwitchCase {
                        body: returns(c.body, expressions, at),
                        ..c
                    })
                    .collect();
                out.push(Statement::Switch { selector, cases }, span);
            }
            Statement::Loop {
                body,
                continuing,
                break_if,
            } => {
                let body = returns(body, expressions, at);
                let continuing = returns(continuing, expressions, at);
                out.push(
                    Statement::Loop {
                        body,
                        continuing,
                        break_if,
                    },
                    span,
                );
            }
            other => out.push(other, span),
        }
    }
    out
}
