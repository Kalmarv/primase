//! The stable text form of a program, for a reader (the lab's program panel, a diff in review): a header with
//! everything the module cannot say, then the module as WGSL.

use std::fmt::Write;

use super::{ClipDepth, Error, Origin, Program};
use crate::{BufferKind, InterpMode, InterpSampling, ResourceKind, Stage, StageFacts, SystemValue, Varying};

fn lanes(v: &Varying) -> String {
    format!("{:?}{}", v.value.scalar, v.value.width).to_lowercase()
}

fn varying(out: &mut String, direction: &str, v: &Varying) {
    let _ = write!(
        out,
        "// {direction} {} {}",
        Program::varying_name(&v.label),
        lanes(v)
    );
    if let Some(system) = v.system {
        match system {
            SystemValue::Target(n) => {
                let _ = write!(out, " system=target{n}");
            }
            s => {
                let _ = write!(out, " system={s:?}");
            }
        }
    }
    if let Some(i) = v.interp {
        let mode = match i.mode {
            InterpMode::Perspective => "perspective",
            InterpMode::Linear => "linear",
            InterpMode::Flat => "flat",
        };
        let sampling = match i.sampling {
            InterpSampling::Center => "center",
            InterpSampling::Centroid => "centroid",
            InterpSampling::Sample => "sample",
        };
        let _ = write!(out, " {mode}/{sampling}");
    }
    if !v.used {
        out.push_str(" unused");
    }
    if let Some(d) = v.provenance.d3d {
        let _ = write!(out, " d3d=r{}.{:04b}", d.register, d.mask.reverse_bits() >> 4);
    }
    out.push('\n');
}

pub(super) fn text(p: &Program) -> Result<String, Error> {
    let wgsl = crate::emit::module_text(p)?;
    let mut out = String::new();
    let _ = writeln!(out, "// primase shader program v{}", super::VERSION);
    let _ = writeln!(
        out,
        "// stage {} entry {}",
        match p.stage {
            Stage::Vertex => "vertex",
            Stage::Pixel => "pixel",
            Stage::Compute => "compute",
            Stage::Hull => "hull",
            Stage::Domain => "domain",
            Stage::Geometry => "geometry",
            Stage::RayGeneration => "ray_generation",
            Stage::Intersection => "intersection",
            Stage::AnyHit => "any_hit",
            Stage::ClosestHit => "closest_hit",
            Stage::Miss => "miss",
            Stage::Callable => "callable",
            _ => "other",
        },
        p.entry
    );
    let _ = writeln!(
        out,
        "// source {:?} {} lifted by {} {}{}",
        p.provenance.format,
        p.provenance.target,
        p.provenance.lifter,
        p.provenance.lifter_version,
        p.provenance
            .source_hash
            .as_ref()
            .map_or(String::new(), |h| format!(" from {h}"))
    );
    let c = p.conventions;
    let _ = writeln!(
        out,
        "// conventions clip_depth={} fragment_origin={} texture_origin={}",
        match c.clip_depth {
            ClipDepth::ZeroToOne => "0..1",
            ClipDepth::MinusOneToOne => "-1..1",
        },
        origin(c.fragment_origin),
        origin(c.texture_origin)
    );
    let m = p.modes;
    let _ = writeln!(
        out,
        "// d3d divide_by_zero={} float_to_int={} min_max_nan={} saturate_nan={} flush_denormals={}",
        m.divide_by_zero, m.float_to_int, m.min_max_nan, m.saturate_nan, m.flush_denormals
    );
    match &p.interface.facts {
        StageFacts::None => {}
        StageFacts::VertexWithoutPosition => {
            let _ = writeln!(out, "// feeds a tessellator: functions, no entry point");
        }
        StageFacts::Hull(h) => {
            let _ = writeln!(
                out,
                "// hull domain={:?} partitioning={:?} topology={:?} control_points={}->{} max_tess_factor={}",
                h.domain,
                h.partitioning,
                h.topology,
                h.input_control_points,
                h.output_control_points,
                f32::from_bits(h.max_tess_factor_bits)
            );
            for v in &h.patch_constants {
                varying(&mut out, "patch_constant", v);
            }
        }
        StageFacts::Domain(d) => {
            let _ = writeln!(
                out,
                "// domain domain={:?} control_points={}",
                d.domain, d.input_control_points
            );
            for v in &d.patch_constants {
                varying(&mut out, "patch_constant", v);
            }
        }
        StageFacts::Geometry(g) => {
            let _ = writeln!(
                out,
                "// geometry input={:?} output={:?} max_vertices={} instances={}",
                g.input, g.output, g.max_vertices, g.instances
            );
        }
        other => {
            let _ = writeln!(out, "// facts {other:?}");
        }
    }
    for note in &p.notes {
        let _ = writeln!(out, "// note {note}");
    }
    for v in &p.interface.inputs {
        varying(&mut out, "input", v);
    }
    for v in &p.interface.outputs {
        varying(&mut out, "output", v);
    }
    for r in &p.interface.resources {
        let _ = write!(out, "// resource {} {:?}", r.id.0, r.provenance.name);
        match &r.kind {
            ResourceKind::AccelerationStructure => {
                let _ = write!(out, " acceleration_structure");
            }
            ResourceKind::ConstantBuffer { layout } => {
                let _ = write!(out, " constant_buffer size={}", layout.size);
            }
            ResourceKind::Texture {
                dim,
                arrayed,
                multisampled,
                sample,
            } => {
                let _ = write!(
                    out,
                    " texture {dim:?}{}{} {sample:?}",
                    if *arrayed { " array" } else { "" },
                    if *multisampled { " ms" } else { "" }
                );
            }
            ResourceKind::Sampler { comparison } => {
                let _ = write!(out, " sampler{}", if *comparison { " comparison" } else { "" });
            }
            ResourceKind::Buffer(k) => {
                let _ = write!(out, " buffer {}", buffer(*k));
            }
            ResourceKind::Storage { target, access } => {
                let _ = write!(out, " storage {target:?} {access:?}");
            }
        }
        if let Some(b) = r.provenance.d3d {
            let _ = write!(out, " d3d={:?}{}", b.class, b.register);
            if b.space != 0 {
                let _ = write!(out, " space{}", b.space);
            }
        }
        if let Some(c) = r.provenance.counter_of {
            let _ = write!(out, " counter_of={}", c.0);
        }
        out.push('\n');
    }
    out.push_str(&wgsl);
    Ok(out)
}

fn origin(o: Origin) -> &'static str {
    match o {
        Origin::TopLeft => "top_left",
        Origin::BottomLeft => "bottom_left",
    }
}

fn buffer(k: BufferKind) -> String {
    match k {
        BufferKind::Typed(v) => format!("typed {:?}{}", v.scalar, v.width),
        BufferKind::Structured { stride } => format!("structured {stride}"),
        BufferKind::Raw => "raw".into(),
    }
}
