//! # `window_linecastroom`
//!
//! Evidence, over the 51 GBuffer permutations of the family in the Tokyo track's `bin.zip` (each render target channel
//! followed back through the `CBMP` and `TXMP` maps; the texture indices of this family's parameter buffer start at
//! dword 124): the albedo (target 0 XYZ) is
//! `lerp(t, FrameDiffuseandMask.rgb, FrameDiffuseandMask.a) * (1 - (1 - Fittings.a)(1 - FrameDiffuseandMask.a))` with
//! `t` the fittings' colour (`FittingsDiffAlphaTex.r`, `uv1` in its atlas) tinted by `WindowTintColour`, so where neither
//! the frame nor a fitting covers the pane the albedo is 0; the frame atlas is read at `uv2` (the `_FT` permutations;
//! `FrameUseAtlasOnOff` picks the cell `(FrameAtlasIndex + uv2) / FrameAtlasCount`); the metalness (target 0 W) is the
//! constant `Metalness`; the roughness (target 1 W) is `max(FrameGlassRoughnessTexture(uv2).r, lerp(1 - COLOR.a,
//! GlassRoughnessTexture(uv0 * GlassRoughnessUVTiling).r, COLOR.a))`; the normal (target 2) is the standard decode of
//! `FrameGlassNormalTexture(uv2)`. The room behind the pane (`BoxDiffRoughTex`, `BoxEmissiveTex`, up to three
//! furniture layers with parallax) is an interior mapping done in the pixel shader from the world position, and
//! reaches the lit colour (target 1 XYZ), not the albedo. The only `discard` compares a constant of the draw with one.
//!
//! The room's atlas (`BoxUseAtlas`, `RoomAtlasCount` cells, the cell of `RoomAtlasIndex`, or any cell where
//! `RoomRandomisationOnOff`) holds in each cell four columns of tiles in two rows: the two side walls in the first
//! column, the back wall in the lower tile of the second (its upper tile, the front, is black), the floor and the
//! ceiling in the next two. Without `BoxUseAtlas` the whole texture is one cell.
//!
//! The rule: the frame is the surface, sampled on `TEXCOORD2` alone (a material is sampled through one coordinate set, and the
//! frame is the one that carries the glass normal and roughness): the base colour is `FrameDiffuseandMask.rgb` times
//! its mask, baked opaque (black where the mask is 0, as the shader's albedo is); the roughness the frame glass
//! roughness texture's red (the map's blue is 1, so that the constant `Metalness` is the metallic factor); the normal
//! `FrameGlassNormalTexture`. The room is drawn as one colour, the mean of its back wall (over the cells the material
//! uses: a randomised room over all its cells), composited into the baked frame where the mask leaves the pane empty
//! (`room * (1 - mask) + colour * mask`), one baked texture per distinct (frame, room colour as sRGB bytes); a
//! material without a frame is that colour alone. Left out, named per material and triangle: the room's parallax,
//! furniture and per-placement extents, the fittings (`uv1`), the glass roughness texture tiled on `uv0`, the
//! fittings' tint, and wetness and lighting.
//!
//! The room's glow (`BoxEmissiveTex`, `Emissive` on) is the room term of the lit colour, which the shader scales by
//! `ExposureValues * COLOR.x * saturate((y - EmissiveVariationMin) / (EmissiveVariationMax - EmissiveVariationMin))`
//! with `y` the placement's instance variation, by `WindowTintColour` and by a warm or cold hue chosen from the world
//! position; no permutation reads `LightSwitchOnTime` or `LightSwitchOffTime`, so nothing gates the glow by the time of
//! day. The rule: the glow is baked like the room colour, its factor is `WindowTintColour * ExposureValues` (the
//! largest channel above 1 goes to the emissive strength), and the material carries the ramp
//! ([`crate::components::EmissionVariation`]) for `apply-forzatech-emission-variation`, which gives each placement its
//! step. Evidence, over the Shibuya box (3,259 window materials, 1,361 window primitives, 22,662 placements of them,
//! 22,654 of them by rule): `ExposureValues` is 1.4 in 2,908 materials, 0.352 in 183, 0.704 in 12 and 11.2 in 156; the
//! ramp starts at 0.6 or more in 98% of the placements' primitives (7,512 of 30,355 have an empty ramp at 1.0) while the
//! placements' variation is spread over 0 to 1, so its step is 0 in 73% of the rule placements' triangles, 1 in 11% and
//! between in 16%; `COLOR.x` is 1 on 96.4% of the placements' triangles, 0 on 1.3% and between on 2.3%. A
//! material whose every triangle has a `COLOR.x` of 0 emits nothing (counted as `window emission off`); on a material
//! with some such triangles the glow is drawn there too (a vertex colour does not scale glTF's emission), counted
//! with those triangles, as is the hue.
use super::{Binding, Derived, Lowering, Params, approximate, left_out, seen};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{ColorSpace, Entity, Name, TexRole, Texture, World};
use primase_foundations::color::srgb_to_linear;
use primase_image::{TextureMetadata, map_rgba8_levels};
use std::collections::btree_map::Entry;

/// The coordinate set of the window frame.
const FRAME_SET: u8 = 2;

/// The back wall's tile in a room cell: a cell holds four columns of tiles in two rows, the walls in the first two
/// columns and the floor and ceiling in the next two; the back wall is the lower tile of the second column. The upper
/// tile of that column is the front, which no view shows. `[u0, u1, v0, v1]`, `v` down the image.
const BACK_WALL: [f64; 4] = [0.25, 0.5, 0.5, 1.0];

/// The mip the room means are read from: a room cell is averaged, so the small mip says the same.
const ROOM_LEVEL: usize = 2;

/// A linear colour as sRGB bytes.
fn bytes(c: [f64; 3]) -> [u8; 3] {
    c.map(|x| (primase_foundations::color::linear_to_srgb(x.clamp(0.0, 1.0)) * 255.0).round() as u8)
}

impl Lowering {
    /// The mean linear colour of the back wall of each cell of `room`, an atlas of `grid` columns and rows of cells
    /// (row by row); `None` when it has no pixels. Made once per texture and grid.
    fn room_cells(
        &mut self,
        w: &World,
        room: Entity,
        grid: [u32; 2],
    ) -> Result<Option<&Vec<[f64; 3]>>, String> {
        if let Entry::Vacant(slot) = self.rooms.entry((room, grid)) {
            let cells = match w.get::<Texture>(room) {
                Some(t) if !t.levels.is_empty() => {
                    let space = t.color_space;
                    let count = (grid[0] * grid[1]) as usize;
                    let mut sums = vec![([0.0f64; 3], 0u64); count];
                    let level = ROOM_LEVEL.min(t.levels.len() - 1);
                    map_rgba8_levels(
                        t,
                        level..level + 1,
                        TextureMetadata {
                            color_space: space,
                            role: t.role,
                            normal: None,
                        },
                        |texel| {
                            let cu = (f64::from(texel.x) + 0.5) / f64::from(texel.width) * f64::from(grid[0]);
                            let cv =
                                (f64::from(texel.y) + 0.5) / f64::from(texel.height) * f64::from(grid[1]);
                            let (column, row) = (cu.floor(), cv.floor());
                            let (tu, tv) = (cu - column, cv - row);
                            if (BACK_WALL[0]..BACK_WALL[1]).contains(&tu)
                                && (BACK_WALL[2]..BACK_WALL[3]).contains(&tv)
                                && let Some((sum, n)) =
                                    sums.get_mut(row as usize * grid[0] as usize + column as usize)
                            {
                                for (s, c) in sum.iter_mut().zip(&texel.rgba[..3]) {
                                    *s += seen(space, *c);
                                }
                                *n += 1;
                            }
                            Ok(texel.rgba)
                        },
                    )
                    .map_err(|e| format!("reading a room texture: {e}"))?;
                    Some(
                        sums.into_iter()
                            .map(|(sum, n)| sum.map(|s| s / n.max(1) as f64))
                            .collect(),
                    )
                }
                _ => None,
            };
            slot.insert(cells);
        }
        Ok(self.rooms.get(&(room, grid)).and_then(Option::as_ref))
    }

    /// The colour the room shows: the mean of the back wall over the cells the material's placements use (the cell
    /// of `RoomAtlasIndex` in an atlas of `RoomAtlasCount` cells, every cell where the room is randomised, the whole
    /// texture when it is no atlas); `None` when the material has no room texture or it has no pixels.
    fn room_colour(&mut self, w: &World, p: &Params<'_>, name: &str) -> Result<Option<[f64; 3]>, String> {
        let Binding::Texture(room) = p.texture(name) else {
            return Ok(None);
        };
        let grid = match (p.on("BoxUseAtlas"), p.vec2("RoomAtlasCount")) {
            (true, Some(n)) if n.iter().all(|x| x.is_finite() && x.trunc() >= 1.0) => {
                [n[0].trunc() as u32, n[1].trunc() as u32]
            }
            _ => [1, 1],
        };
        let randomised = p.on("RoomRandomisationOnOff");
        let index = p
            .vec2("RoomAtlasIndex")
            .filter(|i| i.iter().all(|x| x.is_finite()))
            .unwrap_or([0.0; 2]);
        let Some(cells) = self.room_cells(w, room, grid)? else {
            return Ok(None);
        };
        let chosen: Vec<[f64; 3]> = if randomised || grid == [1, 1] {
            cells.clone()
        } else {
            let column = index[0].max(0.0).trunc() as u32 % grid[0];
            let row = index[1].max(0.0).trunc() as u32 % grid[1];
            vec![cells[(row * grid[0] + column) as usize]]
        };
        let n = chosen.len().max(1) as f64;
        let sum = chosen
            .iter()
            .fold([0.0; 3], |a, c| [a[0] + c[0], a[1] + c[1], a[2] + c[2]]);
        Ok(Some(sum.map(|x| x / n)))
    }

    /// `frame` as the shader's albedo sees it, `rgb * a`, with `room` (sRGB bytes) where the mask leaves the pane
    /// empty (black without one): opaque, sRGB. With `emission`, the frame emits nothing: the room's colour where
    /// the mask leaves the pane empty and black where it covers it. Made once per source, room colour and kind.
    fn window_frame_colour(
        &mut self,
        w: &mut World,
        frame: Entity,
        room: Option<[u8; 3]>,
        emission: bool,
        log: &mut PassLog,
    ) -> Result<Option<Entity>, String> {
        let key = room.map_or_else(
            || vec![u32::from(emission)],
            |r| vec![u32::from_le_bytes([r[0], r[1], r[2], 1]), u32::from(emission)],
        );
        if let Some(&e) = self.baked.get(&(frame, key.clone(), None)) {
            return Ok(Some(e));
        }
        let Some(t) = w.get::<Texture>(frame) else {
            log.count("window frame maps left out: no pixels", 1);
            return Ok(None);
        };
        let space = t.color_space;
        let behind = room.map_or([0.0; 3], |r| r.map(|x| srgb_to_linear(f64::from(x) / 255.0)));
        let texture = map_rgba8_levels(
            t,
            0..t.levels.len(),
            TextureMetadata {
                color_space: ColorSpace::Srgb,
                role: TexRole::Color,
                normal: None,
            },
            |texel| {
                let [r, g, b, a] = texel.rgba;
                let mask = f64::from(a) / 255.0;
                let mixed = |x: u8, room: f64| {
                    let covered = if emission { 0.0 } else { seen(space, x) };
                    let linear = covered * mask + room * (1.0 - mask);
                    (primase_foundations::color::linear_to_srgb(linear) * 255.0).round() as u8
                };
                Ok([mixed(r, behind[0]), mixed(g, behind[1]), mixed(b, behind[2]), 255])
            },
        )
        .map_err(|e| format!("masking a window frame: {e}"))?;
        let name = w.get::<Name>(frame).map(|n| n.0.clone()).unwrap_or_default();
        let e = w.spawn();
        w.insert(
            e,
            Name(format!(
                "{name} ({})",
                if emission { "room emission" } else { "frame colour" }
            )),
        );
        w.insert(e, texture);
        log.count(
            if emission {
                "window frame textures baked as emission maps"
            } else {
                "window frame textures baked as colour maps"
            },
            1,
        );
        self.baked.insert((frame, key, None), e);
        Ok(Some(e))
    }

    /// The `window_linecastroom` family's material.
    pub(super) fn window_linecastroom(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<PbrMaterial, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        // the room behind the pane, as the colour the pane shows where no frame covers it
        let room = self.room_colour(w, &p, "BoxDiffRoughTex")?.map(bytes);
        let glow = self
            .room_colour(w, &p, "BoxEmissiveTex")?
            .map(bytes)
            .filter(|c| *c != [0; 3]);
        let mut lit = false;
        // the shader's room term is scaled by the red of the vertex colour: a material whose every triangle has 0 emits
        // nothing
        let red = self.colour_red.get(&e).copied().unwrap_or_default();
        let dark = red[0] > 0 && red[1] == 0 && red[2] == 0;
        let mut pbr = PbrMaterial {
            base_color: room.map_or([0.0, 0.0, 0.0, 1.0], |r| {
                let c = r.map(|x| srgb_to_linear(f64::from(x) / 255.0) as f32);
                [c[0], c[1], c[2], 1.0]
            }),
            metallic: p.float("Metalness").unwrap_or(0.0).clamp(0.0, 1.0),
            roughness: 1.0,
            ..PbrMaterial::default()
        };

        // the frame atlas cell, `(index + uv2) / count`, on the frame's coordinate set
        let cell = if p.on("FrameUseAtlasOnOff") {
            match (p.vec2("FrameAtlasIndex"), p.vec2("FrameAtlasCount")) {
                (Some(i), Some(n))
                    if n.iter().all(|x| x.trunc() >= 1.0) && i.iter().all(|x| x.is_finite()) =>
                {
                    let (nx, ny) = (n[0].trunc(), n[1].trunc());
                    Some(UvTransform {
                        offset: [i[0].trunc() / nx, i[1].trunc() / ny],
                        scale: [1.0 / nx, 1.0 / ny],
                        rotation: 0.0,
                    })
                }
                _ => {
                    approximate(log, e, "no frame atlas cell: the whole atlas", triangles);
                    None
                }
            }
        } else {
            None
        };
        let slot = |texture: Entity| TextureRef {
            texcoord: FRAME_SET,
            wrap: [Wrap::Clamp; 2],
            transform: cell.filter(|t| *t != UvTransform::IDENTITY),
            ..TextureRef::new(texture)
        };

        if !self.has_set(e, FRAME_SET) {
            approximate(
                log,
                e,
                "a primitive without the frame's coordinates (TEXCOORD2): no frame textures, the room colour alone",
                triangles,
            );
        } else if p.on("FrameTex") {
            match p.texture("FrameDiffuseandMask") {
                Binding::Texture(t) => {
                    if let Some(colour) = self.window_frame_colour(w, t, room, false, log)? {
                        // the baked frame holds the whole colour, the room where the frame leaves the pane empty
                        pbr.base_color = [1.0; 4];
                        pbr.base_color_texture = Some(slot(colour));
                    }
                }
                Binding::Absent | Binding::None => approximate(
                    log,
                    e,
                    "no frame texture: the pane is the room colour alone",
                    triangles,
                ),
            }
            if p.on("Emissive")
                && !dark
                && let Some(glow) = glow
                && let Binding::Texture(t) = p.texture("FrameDiffuseandMask")
                && let Some(map) = self.window_frame_colour(w, t, Some(glow), true, log)?
            {
                // the room's glow where the frame leaves the pane empty, scaled by the window tint and the exposure
                // constant: the map holds the glow, the factor the rest
                let tint = p.vec4("WindowTintColour").unwrap_or([1.0; 4]);
                let exposure = p.float("ExposureValues").unwrap_or(1.0).max(0.0);
                let radiance = [tint[0], tint[1], tint[2]].map(|c| c.max(0.0) * exposure);
                let strength = radiance.into_iter().fold(1.0, f32::max);
                pbr.emissive = radiance.map(|c| c / strength);
                pbr.emissive_strength = strength;
                pbr.emissive_texture = Some(slot(map));
                lit = true;
                Self::vary_emission(w, e, &p, &pbr);
                if red[0] > 0 {
                    approximate(
                        log,
                        e,
                        "room emission drawn where the vertex colour's red (COLOR.x) is 0",
                        red[0],
                    );
                }
                if red[2] > 0 {
                    approximate(
                        log,
                        e,
                        "room emission drawn at full strength where the vertex colour's red (COLOR.x) is between 0 and 1",
                        red[2],
                    );
                }
                left_out(
                    log,
                    &p,
                    e,
                    "EmissiveTempVariationOnOff",
                    "room emission's warm and cold hue (from the world position) left out",
                    triangles,
                );
            }
            if let Binding::Texture(t) = p.texture("FrameGlassRoughnessTexture")
                && let Some(rough) = self.derive(w, t, Derived::RedRoughnessMetal, log)?
            {
                // the map's G holds the roughness and its B is 1: `Metalness` stays the metallic factor
                pbr.metallic_roughness_texture = Some(slot(rough));
                pbr.roughness = 1.0;
            }
            if let Binding::Texture(t) = p.texture("FrameGlassNormalTexture")
                && let Some(map) = self.normal_slot(w, e, t, triangles, log, slot)?
            {
                pbr.normal_texture = Some(map);
            }
        } else {
            approximate(
                log,
                e,
                "no frame (FrameTex off): the pane is the room colour alone",
                triangles,
            );
        }

        approximate(
            log,
            e,
            if room.is_some() {
                "room drawn as its back wall's mean colour (no parallax, furniture and per-placement room extents left out)"
            } else {
                "room behind the pane (no room texture) left out: black"
            },
            triangles,
        );
        if dark {
            log.count(
                "window emission off: the vertex colour's red (COLOR.x) is 0 on every triangle",
                1,
            );
        } else if !lit {
            approximate(log, e, "room emission left out", triangles);
        }
        left_out(
            log,
            &p,
            e,
            "Fittings",
            "fittings (TEXCOORD1) and their tint left out",
            triangles,
        );
        approximate(
            log,
            e,
            "glass roughness texture (TEXCOORD0), wetness and lighting left out",
            triangles,
        );
        Ok(pbr)
    }
}
