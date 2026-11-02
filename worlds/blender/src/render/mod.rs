//! The stage after saving: Cycles renders a `.blend` in the background from a camera the program states, every choice
//! stated ([`RenderOptions`]): the image (`render.png`, the Standard view transform), the render as a multilayer EXR
//! (`render.exr`) and each pass as raw floats a test reads without an EXR reader ([`Rendered::pass`]), the passes
//! every material writes among them ([`crate::graph::aov`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::save::Blender;
use crate::{Error, Result};

/// How the camera projects.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    /// `yfov`: the vertical field of view, radians.
    Perspective { yfov: f32 },
    /// `height`: the view's height, metres.
    Orthographic { height: f32 },
}

/// A camera in the world frame (glTF's: +Y up).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Camera {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub up: [f32; 3],
    pub projection: Projection,
    /// Clip distances, metres.
    pub near: f32,
    pub far: f32,
}

/// Where Cycles renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Device {
    Cpu,
    Optix,
    Cuda,
}

/// Every choice of a render.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RenderOptions {
    pub camera: Camera,
    /// Width and height, pixels.
    pub size: [u32; 2],
    /// Samples per pixel (no adaptive sampling, no denoising).
    pub samples: u32,
    pub seed: u32,
    /// The pixel filter's width, pixels (Blender's 1.5 by default; a small width makes a pixel its centre's value).
    pub filter_width: f32,
    pub device: Device,
}

/// A pass written as raw floats.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Pass {
    /// In the render folder.
    pub file: String,
    /// Its channels in order (`R`, `G`, `B`, `A`; `X` for a value).
    pub channels: Vec<String>,
}

/// A render.
#[derive(Clone, Debug, PartialEq)]
pub struct Rendered {
    pub folder: PathBuf,
    pub size: [u32; 2],
    pub passes: BTreeMap<String, Pass>,
    /// Blender's output (`render.log` in the folder).
    pub log: PathBuf,
    pub seconds: f64,
}

#[derive(Deserialize)]
struct Summary {
    size: [u32; 2],
    passes: BTreeMap<String, Pass>,
}

/// The script the stage runs, written into the render folder as `render.py`.
pub const RENDER_PY: &str = include_str!("render.py");

/// The line `render.py` prints what it wrote on.
const WROTE: &str = "primase-blender-render: ";

/// Render `blend` with `blender` by `options` into the folder `out` (absent or empty: one with anything in it is
/// refused, never emptied here). A script that fails is an error naming the log.
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "writer I/O: runs Blender's Cycles render of a .blend into the render folder, and writes its options, \
              script and log"
)]
pub fn render(blender: &Blender, blend: &Path, options: &RenderOptions, out: &Path) -> Result<Rendered> {
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| Error::Io { path, source }
    };
    if out.exists() && std::fs::read_dir(out).map_err(io(out))?.next().is_some() {
        return Err(Error::Stage(format!(
            "{}: the render folder is not empty (empty it first if it is to be replaced)",
            out.display()
        )));
    }
    std::fs::create_dir_all(out).map_err(io(out))?;
    let script = out.join("render.py");
    std::fs::write(&script, RENDER_PY).map_err(io(&script))?;
    let stated = out.join("render.json");
    let json =
        serde_json::to_vec_pretty(options).map_err(|e| Error::Stage(format!("render options: {e}")))?;
    std::fs::write(&stated, json).map_err(io(&stated))?;
    let log = out.join("render.log");
    let started = Instant::now();
    let run = std::process::Command::new(&blender.executable)
        .args(["--background", "--factory-startup"])
        .arg(blend)
        .args(["--python-exit-code", "1", "--python"])
        .arg(&script)
        .arg("--")
        .arg(&stated)
        .arg(out)
        .output()
        .map_err(io(&blender.executable))?;
    let text = [run.stdout, run.stderr].concat();
    std::fs::write(&log, &text).map_err(io(&log))?;
    let wrote = String::from_utf8_lossy(&text)
        .lines()
        .find_map(|l| l.strip_prefix(WROTE).map(str::to_owned));
    let (true, Some(wrote)) = (run.status.success(), wrote) else {
        return Err(Error::Stage(format!(
            "Blender's render of {} failed ({}); its output: {}",
            blend.display(),
            run.status,
            log.display()
        )));
    };
    let summary: Summary = serde_json::from_str(&wrote).map_err(|e| {
        Error::Stage(format!(
            "{}: what render.py wrote is not its summary: {e}",
            log.display()
        ))
    })?;
    Ok(Rendered {
        folder: out.to_owned(),
        size: summary.size,
        passes: summary.passes,
        log,
        seconds: started.elapsed().as_secs_f64(),
    })
}

impl Rendered {
    /// Pass `name`'s values: rows from the top, its channels interleaved.
    #[allow(
        clippy::disallowed_methods,
        reason = "writer I/O: reads a pass the render stage wrote"
    )]
    pub fn pass(&self, name: &str) -> Result<Vec<f32>> {
        let pass = self
            .passes
            .get(name)
            .ok_or_else(|| Error::Stage(format!("the render has no pass {name}")))?;
        let path = self.folder.join(&pass.file);
        let bytes = std::fs::read(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        let want = self.size[0] as usize * self.size[1] as usize * pass.channels.len() * 4;
        if bytes.len() != want {
            return Err(Error::Stage(format!(
                "{}: {} bytes, not {want}",
                path.display(),
                bytes.len()
            )));
        }
        Ok(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect())
    }
}
