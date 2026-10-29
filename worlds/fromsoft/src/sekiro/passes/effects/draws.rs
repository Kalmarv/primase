//! The appearances of an effect's configs (the actions of the project's Appearance slot) as core draws.
//!
//! What the project says about a parameter is what the lowering does with it; what it does not say, or the files store as a
//! word it has no name for, is dropped and the count says so. The properties are the files' [`Property`] values lowered by
//! [`super::value`]; the arguments each is read at are the project's.

use super::cx::Cx;
use super::units;
use super::value;
use fromsoft_formats::fxr::Property;
use fromsoft_formats::fxr::actions as a;
use fromsoft_formats::fxr::actions::Typed;
use fromsoft_formats::fxr::resources::TextureKind;
use primase_core::particles::{
    Argument, Atlas, Blend, Channels, DistanceFade, Draw, Flare, FlareLayer, ForceField, ForceKind,
    ForceVolume, Layer, LayeredQuad, LightDraw, LightKind, Lit, Map, MeshClip, MeshParticle, Orientation,
    Point, Quad, RadialBlur, Reflection, Refraction, RefractionMode, RefractionShape, Ripples, SoftEdge,
    Streak, Term, Trail, TrailDistortion, Value,
};

/// 255 stored levels to a fraction.
const BYTE: f32 = 1.0 / 255.0;

/// The inputs of a quad the billboard and the multi-texture billboard share (the project's parameters of the same names).
struct QuadIn<'a> {
    map: Option<Map>,
    blend: &'a Property,
    orientation: i32,
    width: &'a Property,
    height: &'a Property,
    offset: [&'a Property; 3],
    rotation: [&'a Property; 3],
    spin: [(&'a Property, &'a Property); 3],
    colours: [&'a Property; 3],
    rgb: &'a Property,
    alpha: &'a Property,
    fade_threshold: &'a Property,
    cutoff: &'a Property,
    depth_offset: Option<&'a Property>,
    frame: (&'a Property, &'a Property),
    size_variance: [f32; 2],
    uniform: bool,
    scale_before_rotation: bool,
    columns: i32,
    total_frames: i32,
    interpolate: bool,
    depth_blend: bool,
    octagonal: bool,
    lighting: i32,
    normal_map: i32,
    specular: i32,
    glossiness: f32,
    specularity: f32,
    shadow: f32,
    bloom: Option<[f32; 4]>,
    distance: DistanceFade,
    premultiply: bool,
}

impl Cx<'_> {
    fn atlas(
        &mut self,
        columns: i32,
        total: i32,
        frame: (&Property, &Property),
        interpolate: bool,
    ) -> Option<Atlas> {
        if columns <= 1 && total <= 1 {
            return None;
        }
        let index = self.age(frame.0);
        let offset = self.age(frame.1);
        let cell = value::sum(index, offset, &mut self.notes);
        Some(Atlas {
            columns: u32::try_from(columns.max(1)).unwrap_or(1),
            cells: u32::try_from(total.max(1)).unwrap_or(1),
            cell,
            blend_cells: interpolate,
        })
    }

    fn lit(
        &mut self,
        lighting: i32,
        normal_map: i32,
        specular: i32,
        glossiness: f32,
        specularity: f32,
        shadow: f32,
    ) -> Option<Lit> {
        if !units::lit(lighting, &mut self.notes) {
            return None;
        }
        Some(Lit {
            normal_map: self.map_word(TextureKind::Normal, normal_map),
            specular_map: self.map_word(TextureKind::Specular, specular),
            glossiness,
            specularity,
            shadow,
        })
    }

    fn soft(&mut self, depth_blend: bool) -> Option<SoftEdge> {
        depth_blend.then(|| {
            self.notes.approximate("soft-particle depth taken as a fixed distance (the file's two depth-blend numbers are not read)");
            SoftEdge { distance: units::SOFT_DISTANCE }
        })
    }

    fn quad(&mut self, q: QuadIn<'_>) -> Quad {
        let blend = units::blend(value::number(q.blend).round() as i32, &mut self.notes);
        let orientation = units::orientation(q.orientation, &mut self.notes);
        let (width, height) = (self.age(q.width), self.age(q.height));
        let offset = [self.age(q.offset[0]), self.age(q.offset[1]), {
            let z = self.age(q.offset[2]);
            value::negated(z)
        }];
        let rotation = self.rotation(q.rotation);
        let spin = self.spin(q.spin);
        let tint = self.tint(q.colours[0], q.colours[1], q.colours[2], q.rgb, q.alpha);
        let fade = self.age(q.fade_threshold);
        let cutoff = self.active(q.cutoff);
        let depth_bias = match q.depth_offset {
            Some(p) => self.age(p),
            None => self.scalar(0.0),
        };
        let atlas = self.atlas(q.columns, q.total_frames, q.frame, q.interpolate);
        let lit = self.lit(
            q.lighting,
            q.normal_map,
            q.specular,
            q.glossiness,
            q.specularity,
            q.shadow,
        );
        let soft = self.soft(q.depth_blend);
        Quad {
            map: q.map,
            blend,
            orientation,
            size: [width, height],
            uniform: q.uniform,
            size_variance: q.size_variance,
            offset,
            rotation,
            spin,
            scale_before_rotation: q.scale_before_rotation,
            atlas,
            tint,
            alpha_fade_threshold: value::scaled(fade, BYTE),
            alpha_cutoff: value::scaled(cutoff, BYTE),
            soft,
            distance: units::limit(q.distance),
            octagonal: q.octagonal,
            lit,
            bloom: q.bloom,
            premultiply_alpha: q.premultiply,
            depth_bias,
        }
    }

    /// The draw of one appearance action, or `None` for an action that is not one or whose files are absent (counted).
    pub fn appearance(&mut self, action: &Typed) -> Option<Draw> {
        Some(match action {
            Typed::BillboardEx(b) => {
                let map = self.map_prop(TextureKind::Albedo, &b.texture);
                let q = self.quad(QuadIn {
                    map,
                    blend: &b.blend_mode,
                    orientation: b.orientation,
                    width: &b.width,
                    height: &b.height,
                    offset: [&b.offset_x, &b.offset_y, &b.offset_z],
                    rotation: [&b.rotation_x, &b.rotation_y, &b.rotation_z],
                    spin: [
                        (&b.angular_speed_x, &b.angular_speed_multiplier_x),
                        (&b.angular_speed_y, &b.angular_speed_multiplier_y),
                        (&b.angular_speed_z, &b.angular_speed_multiplier_z),
                    ],
                    colours: [&b.color1, &b.color2, &b.color3],
                    rgb: &b.rgb_multiplier,
                    alpha: &b.alpha_multiplier,
                    fade_threshold: &b.alpha_fade_threshold,
                    cutoff: &b.alpha_threshold,
                    depth_offset: Some(&b.depth_offset),
                    frame: (&b.frame_index, &b.frame_index_offset),
                    size_variance: [b.scale_variation_x, b.scale_variation_y],
                    uniform: b.uniform_scale,
                    scale_before_rotation: b.scale_before_rotation,
                    columns: b.columns,
                    total_frames: b.total_frames,
                    interpolate: b.interpolate_frames,
                    depth_blend: b.depth_blend,
                    octagonal: b.octagonal,
                    lighting: b.lighting,
                    normal_map: b.normal_map,
                    specular: b.specular,
                    glossiness: b.glossiness,
                    specularity: b.specularity,
                    shadow: b.shadow_darkness,
                    bloom: b.bloom.then_some(b.bloom_color),
                    distance: units::distance_fade(
                        b.min_fade_distance,
                        b.min_distance,
                        b.max_fade_distance,
                        b.max_distance,
                        b.min_distance_threshold,
                        b.max_distance_threshold,
                    ),
                    premultiply: b.premultiply_alpha,
                });
                Draw::Quad(q)
            }
            Typed::MultiTextureBillboardEx(b) => {
                let q = self.quad(QuadIn {
                    map: None,
                    blend: &b.blend_mode,
                    orientation: b.orientation,
                    width: &b.width,
                    height: &b.height,
                    offset: [&b.offset_x, &b.offset_y, &b.offset_z],
                    rotation: [&b.rotation_x, &b.rotation_y, &b.rotation_z],
                    spin: [
                        (&b.angular_speed_x, &b.angular_speed_multiplier_x),
                        (&b.angular_speed_y, &b.angular_speed_multiplier_y),
                        (&b.angular_speed_z, &b.angular_speed_multiplier_z),
                    ],
                    colours: [&b.color1, &b.color2, &b.color3],
                    rgb: &b.rgb_multiplier,
                    alpha: &b.alpha_multiplier,
                    fade_threshold: &b.alpha_fade_threshold,
                    cutoff: &b.alpha_threshold,
                    depth_offset: None,
                    frame: (&b.frame_index, &b.frame_index_offset),
                    size_variance: [1.0, 1.0],
                    uniform: b.uniform_scale,
                    scale_before_rotation: b.scale_before_rotation,
                    columns: b.columns,
                    total_frames: b.total_frames,
                    interpolate: b.interpolate_frames,
                    depth_blend: b.depth_blend,
                    octagonal: b.octagonal,
                    lighting: b.lighting,
                    normal_map: 0,
                    specular: b.specular,
                    glossiness: b.glossiness,
                    // the action stores no specularity (only `specular` and `glossiness`)
                    specularity: 0.0,
                    shadow: b.shadow_darkness,
                    bloom: b.bloom.then_some(b.bloom_color),
                    distance: units::distance_fade(
                        b.min_fade_distance,
                        b.min_distance,
                        b.max_fade_distance,
                        b.max_distance,
                        b.min_distance_threshold,
                        b.max_distance_threshold,
                    ),
                    premultiply: b.premultiply_alpha,
                });
                let mut layers = Vec::new();
                let sources = [
                    (
                        b.layer1,
                        &b.layer1_color,
                        [&b.layer1_speed_u, &b.layer1_speed_v],
                        [&b.layer1_offset_u, &b.layer1_offset_v],
                        [&b.layer1_scale_u, &b.layer1_scale_v],
                    ),
                    (
                        b.layer2,
                        &b.layer2_color,
                        [&b.layer2_speed_u, &b.layer2_speed_v],
                        [&b.layer2_offset_u, &b.layer2_offset_v],
                        [&b.layer2_scale_u, &b.layer2_scale_v],
                    ),
                    (
                        b.layer3,
                        &b.layer3_color,
                        [&b.layer3_speed_u, &b.layer3_speed_v],
                        [&b.layer3_offset_u, &b.layer3_offset_v],
                        [&b.layer3_scale_u, &b.layer3_scale_v],
                    ),
                ];
                for (texture, colour, speed, offset, scale) in sources {
                    let map = self.map_word(TextureKind::Albedo, texture);
                    layers.push(Layer {
                        map,
                        tint: self.age(colour),
                        scroll: [self.age(speed[0]), self.age(speed[1])],
                        offset: [self.fixed(offset[0]), self.fixed(offset[1])],
                        scale: [self.age(scale[0]), self.age(scale[1])],
                    });
                }
                Draw::LayeredQuad(LayeredQuad { quad: q, layers })
            }
            Typed::PointSprite(p) => {
                let map = self.map_prop(TextureKind::Albedo, &p.texture);
                let blend = units::blend(value::number(&p.blend_mode).round() as i32, &mut self.notes);
                let size = self.age(&p.size);
                let tint = self.tint(
                    &p.color1,
                    &p.color2,
                    &p.color3,
                    &p.rgb_multiplier,
                    &p.alpha_multiplier,
                );
                let cutoff = self.active(&p.alpha_threshold);
                Draw::Point(Point {
                    map,
                    blend,
                    size,
                    tint,
                    alpha_cutoff: value::scaled(cutoff, BYTE),
                    soft: None,
                    distance: units::limit(units::distance_fade(
                        p.min_fade_distance,
                        p.min_distance,
                        p.max_fade_distance,
                        p.max_distance,
                        p.min_distance_threshold,
                        p.max_distance_threshold,
                    )),
                    bloom: p.bloom.then_some(p.bloom_color),
                })
            }
            Typed::Line(l) => {
                let blend = units::blend(value::number(&l.blend_mode).round() as i32, &mut self.notes);
                let (length, mult) = (self.emitted(&l.length), self.age(&l.length_multiplier));
                let length = value::product(length, mult, &mut self.notes);
                let tint = self.line_tint(
                    &l.color1,
                    &l.color2,
                    &l.color3,
                    &l.rgb_multiplier,
                    &l.alpha_multiplier,
                );
                let (start, end) = (self.active(&l.start_color), self.active(&l.end_color));
                let cutoff = self.active(&l.alpha_threshold);
                Draw::Streak(Streak {
                    map: None,
                    blend,
                    width: None,
                    length,
                    start_colour: start,
                    end_colour: end,
                    tint,
                    alpha_cutoff: value::scaled(cutoff, BYTE),
                    soft: None,
                    distance: units::distance_fade(
                        l.min_fade_distance,
                        l.min_distance,
                        l.max_fade_distance,
                        l.max_distance,
                        l.min_distance_threshold,
                        l.max_distance_threshold,
                    ),
                    bloom: l.bloom.then_some(l.bloom_color),
                })
            }
            Typed::QuadLine(l) => {
                let blend = units::blend(value::number(&l.blend_mode).round() as i32, &mut self.notes);
                let (width, wm) = (self.emitted(&l.width), self.age(&l.width_multiplier));
                let width = value::product(width, wm, &mut self.notes);
                let (length, lm) = (self.emitted(&l.length), self.age(&l.length_multiplier));
                let length = value::product(length, lm, &mut self.notes);
                let tint = self.line_tint(
                    &l.color1,
                    &l.color2,
                    &l.color3,
                    &l.rgb_multiplier,
                    &l.alpha_multiplier,
                );
                let (start, end) = (self.active(&l.start_color), self.active(&l.end_color));
                let cutoff = self.active(&l.alpha_threshold);
                Draw::Streak(Streak {
                    map: None,
                    blend,
                    width: Some(width),
                    length,
                    start_colour: start,
                    end_colour: end,
                    tint,
                    alpha_cutoff: value::scaled(cutoff, BYTE),
                    soft: None,
                    distance: units::distance_fade(
                        l.min_fade_distance,
                        l.min_distance,
                        l.max_fade_distance,
                        l.max_distance,
                        l.min_distance_threshold,
                        l.max_distance_threshold,
                    ),
                    bloom: l.bloom.then_some(l.bloom_color),
                })
            }
            Typed::Model(m) => {
                let Some(mesh) = value::id(&m.model).and_then(|id| self.resolver.model(id)) else {
                    self.notes.lose("a model particle whose model is not read");
                    return None;
                };
                let clip = (m.anibnd > 0)
                    .then(|| self.resolver.animation(m.anibnd))
                    .flatten()
                    .map(|clip| MeshClip {
                        clip,
                        speed: m.animation_speed,
                        looped: m.loop_animation,
                    });
                let blend = units::blend(value::number(&m.blend_mode).round() as i32, &mut self.notes);
                let size = self.size3(&m.size_x, &m.size_y, &m.size_z, m.uniform_scale);
                let rotation = self.rotation([&m.rotation_x, &m.rotation_y, &m.rotation_z]);
                let spin = self.spin([
                    (&m.angular_speed_x, &m.angular_speed_multiplier_x),
                    (&m.angular_speed_y, &m.angular_speed_multiplier_y),
                    (&m.angular_speed_z, &m.angular_speed_multiplier_z),
                ]);
                let tint = self.tint(
                    &m.color1,
                    &m.color2,
                    &m.color3,
                    &m.rgb_multiplier,
                    &m.alpha_multiplier,
                );
                let scroll = [
                    self.scroll(&m.speed_u, &m.speed_multiplier_u),
                    self.scroll(&m.speed_v, &m.speed_multiplier_v),
                ];
                let offset = [self.fixed(&m.offset_u), self.fixed(&m.offset_v)];
                let atlas = self.atlas(
                    m.columns,
                    m.total_frames,
                    (&m.frame_index, &m.frame_index_offset),
                    false,
                );
                Draw::Mesh(MeshParticle {
                    mesh,
                    clip,
                    blend,
                    orientation: units::model_orientation(m.orientation, &mut self.notes),
                    size,
                    size_variance: [m.scale_variation_x, m.scale_variation_y, m.scale_variation_z],
                    uniform: m.uniform_scale,
                    rotation,
                    spin,
                    tint,
                    scroll,
                    offset,
                    atlas,
                    soft: None,
                    distance: units::limit(units::distance_fade(
                        m.min_fade_distance,
                        m.min_distance,
                        m.max_fade_distance,
                        m.max_distance,
                        m.min_distance_threshold,
                        m.max_distance_threshold,
                    )),
                    lit: false,
                })
            }
            Typed::RichModel(m) => {
                let Some(mesh) = value::id(&m.model).and_then(|id| self.resolver.model(id)) else {
                    self.notes.lose("a model particle whose model is not read");
                    return None;
                };
                let clip = (m.anibnd > 0)
                    .then(|| self.resolver.animation(m.anibnd))
                    .flatten()
                    .map(|clip| MeshClip {
                        clip,
                        speed: m.animation_speed,
                        looped: m.loop_animation,
                    });
                self.notes
                    .approximate("a rich model's blend mode taken as normal (its blend word is unnamed)");
                let size = self.size3(&m.size_x, &m.size_y, &m.size_z, m.uniform_scale);
                let rotation = self.rotation([&m.rotation_x, &m.rotation_y, &m.rotation_z]);
                let spin = self.spin([
                    (&m.angular_speed_x, &m.angular_speed_multiplier_x),
                    (&m.angular_speed_y, &m.angular_speed_multiplier_y),
                    (&m.angular_speed_z, &m.angular_speed_multiplier_z),
                ]);
                let tint = self.tint(
                    &m.color1,
                    &m.color2,
                    &m.color3,
                    &m.rgb_multiplier,
                    &m.alpha_multiplier,
                );
                let scroll = [
                    self.scroll(&m.speed_u, &m.speed_multiplier_u),
                    self.scroll(&m.speed_v, &m.speed_multiplier_v),
                ];
                let offset = [self.fixed(&m.offset_u), self.fixed(&m.offset_v)];
                Draw::Mesh(MeshParticle {
                    mesh,
                    clip,
                    blend: Blend::Alpha,
                    orientation: units::rich_model_orientation(m.orientation, &mut self.notes),
                    size,
                    size_variance: [m.scale_variation_x, m.scale_variation_y, m.scale_variation_z],
                    uniform: m.uniform_scale,
                    rotation,
                    spin,
                    tint,
                    scroll,
                    offset,
                    atlas: None,
                    soft: None,
                    distance: units::limit(units::distance_fade(
                        m.min_fade_distance,
                        m.min_distance,
                        m.max_fade_distance,
                        m.max_distance,
                        m.min_distance_threshold,
                        m.max_distance_threshold,
                    )),
                    lit: false,
                })
            }
            Typed::LegacyTracer(t) => {
                let map = self.map_prop(TextureKind::Albedo, &t.texture);
                self.tracer(TracerIn {
                    map,
                    blend: &t.blend_mode,
                    orientation: t.orientation,
                    width: &t.width,
                    width_multiplier: &t.width_multiplier,
                    fade: (&t.start_fade_endpoint, &t.end_fade_endpoint),
                    colours: [&t.color1, &t.color2, &t.color3],
                    rgb: &t.rgb_multiplier,
                    alpha: &t.alpha_multiplier,
                    fade_threshold: &t.alpha_fade_threshold,
                    cutoff: &t.alpha_threshold,
                    frame: (&t.frame_index, &t.frame_index_offset),
                    texture_fraction: &t.texture_fraction,
                    speed_u: &t.speed_u,
                    variance_u: 0.0,
                    variance_v: &t.variance_v,
                    distortion_intensity: &t.distortion_intensity,
                    normal_map: t.normal_map,
                    segment_interval: t.segment_interval,
                    segment_duration: t.segment_duration,
                    concurrent_segments: t.concurrent_segments,
                    subdivision: t.segment_subdivision,
                    fade_out: t.fade_out_time,
                    columns: t.columns,
                    total_frames: t.total_frames,
                    attached_uv: t.attached_uv,
                    dynamic_opacity: false,
                    lighting: t.lighting,
                    specular: t.specular,
                    glossiness: t.glossiness,
                    specularity: t.specularity,
                    shadow: t.shadow_darkness,
                    bloom: t.bloom.then_some(t.bloom_color),
                    distance: units::distance_fade(
                        t.min_fade_distance,
                        t.min_distance,
                        t.max_fade_distance,
                        t.max_distance,
                        t.min_distance_threshold,
                        t.max_distance_threshold,
                    ),
                    premultiply: t.premultiply_alpha,
                })
            }
            Typed::Tracer(t) => {
                let map = self.map_prop(TextureKind::Albedo, &t.texture);
                self.tracer(TracerIn {
                    map,
                    blend: &t.blend_mode,
                    orientation: t.orientation,
                    width: &t.width,
                    width_multiplier: &t.width_multiplier,
                    fade: (&t.start_fade_endpoint, &t.end_fade_endpoint),
                    colours: [&t.color1, &t.color2, &t.color3],
                    rgb: &t.rgb_multiplier,
                    alpha: &t.alpha_multiplier,
                    fade_threshold: &t.alpha_fade_threshold,
                    cutoff: &t.alpha_threshold,
                    frame: (&t.frame_index, &t.frame_index_offset),
                    texture_fraction: &t.texture_fraction,
                    speed_u: &t.speed_u,
                    variance_u: t.variance_u,
                    variance_v: &t.variance_v,
                    distortion_intensity: &t.distortion_intensity,
                    normal_map: t.normal_map,
                    segment_interval: t.segment_interval,
                    segment_duration: t.segment_duration,
                    concurrent_segments: t.concurrent_segments,
                    subdivision: t.segment_subdivision,
                    fade_out: t.fade_out_time,
                    columns: t.columns,
                    total_frames: t.total_frames,
                    attached_uv: t.attached_uv,
                    dynamic_opacity: t.dynamic_opacity,
                    lighting: t.lighting,
                    specular: t.specular,
                    glossiness: t.glossiness,
                    specularity: t.specularity,
                    shadow: t.shadow_darkness,
                    bloom: t.bloom.then_some(t.bloom_color),
                    distance: units::distance_fade(
                        t.min_fade_distance,
                        t.min_distance,
                        t.max_fade_distance,
                        t.max_distance,
                        t.min_distance_threshold,
                        t.max_distance_threshold,
                    ),
                    premultiply: t.premultiply_alpha,
                })
            }
            Typed::Distortion(d) => self.distortion(d),
            Typed::RadialBlur(b) => {
                let mask = self.map_prop(TextureKind::Albedo, &b.mask);
                let blend = units::blend(value::number(&b.blend_mode).round() as i32, &mut self.notes);
                let offset = self.offset3(&b.offset_x, &b.offset_y, &b.offset_z);
                let (width, height) = (self.age(&b.width), self.age(&b.height));
                let tint = self.single_colour(&b.color, &b.rgb_multiplier, &b.alpha_multiplier);
                let radius = self.age(&b.blur_radius);
                let cutoff = self.active(&b.alpha_threshold);
                Draw::RadialBlur(RadialBlur {
                    mask,
                    blend,
                    offset,
                    size: [width, height],
                    uniform: b.uniform_scale,
                    radius,
                    iterations: u32::try_from(b.iterations.max(1)).unwrap_or(1),
                    tint,
                    alpha_cutoff: value::scaled(cutoff, BYTE),
                    distance: units::distance_fade(
                        b.min_fade_distance,
                        b.min_distance,
                        b.max_fade_distance,
                        b.max_distance,
                        b.min_distance_threshold,
                        b.max_distance_threshold,
                    ),
                })
            }
            Typed::PointLight(l) => {
                let colour = self.light_colour(&l.diffuse_color);
                let intensity = self.active(&l.diffuse_multiplier);
                let range = self.active(&l.radius);
                if l.jitter_and_flicker {
                    self.notes.lose("the flicker and jitter of an effect light");
                }
                Draw::Light(LightDraw {
                    kind: LightKind::Point,
                    colour,
                    intensity,
                    range,
                    shadows: l.shadows,
                    fade_out: units::frames(l.fade_out_time),
                })
            }
            Typed::SpotLight(l) => {
                let colour = self.light_colour(&l.diffuse_color);
                let intensity = self.active(&l.diffuse_multiplier);
                let (far, rx, ry) = (
                    value::number(&l.far),
                    value::number(&l.radius_x),
                    value::number(&l.radius_y),
                );
                let range = self.active(&l.far);
                let half = if far > 0.0 {
                    rx.max(ry).atan2(far).to_degrees()
                } else {
                    45.0
                };
                self.notes
                    .approximate("an effect spot light's cone as the wider half-angle of its elliptic base");
                self.notes
                    .approximate("an effect spot light's inner cone as a fixed share of the outer");
                if l.jitter_and_flicker {
                    self.notes.lose("the flicker and jitter of an effect light");
                }
                Draw::Light(LightDraw {
                    kind: LightKind::Spot {
                        inner: half * 0.85,
                        outer: half,
                    },
                    colour,
                    intensity,
                    range,
                    shadows: l.shadows,
                    fade_out: units::frames(l.fade_out_time),
                })
            }
            Typed::WindForce(f) => self.field(
                ForceKind::Wind,
                &f.force,
                FieldShape::of(
                    f.shape,
                    f.sphere_radius,
                    f.box_size,
                    f.cylinder_height,
                    f.cylinder_radius,
                    f.square_prism_height,
                    f.square_prism_apothem,
                ),
                f.force_multiplier,
                [f.force_random_multiplier_min, f.force_random_multiplier_max],
                f.fade_out_time,
            ),
            Typed::GravityForce(f) => self.field(
                ForceKind::Gravity,
                &f.force,
                FieldShape::of(
                    f.shape,
                    f.sphere_radius,
                    f.box_size,
                    f.cylinder_height,
                    f.cylinder_radius,
                    f.square_prism_height,
                    f.square_prism_apothem,
                ),
                f.force_multiplier,
                [f.force_random_multiplier_min, f.force_random_multiplier_max],
                f.fade_out_time,
            ),
            Typed::WaterInteraction(w) => Draw::Ripples(Ripples {
                map: self.map_word(TextureKind::Detail, w.texture),
                depth: w.depth,
                size: w.size,
                descent: w.descent,
                duration: w.duration,
            }),
            Typed::LensFlare(f) => self.flare(f),
            _ => return None,
        })
    }

    /// A line's colour factors: its two clamped multipliers and its third (all read at the age for the first two, at the
    /// active time for the third, which the project also clamps), and the brightness and alpha multipliers.
    fn line_tint(
        &mut self,
        c1: &Property,
        c2: &Property,
        c3: &Property,
        rgb: &Property,
        alpha: &Property,
    ) -> Vec<Term> {
        let terms = vec![
            self.term(c1, Argument::Age, true, Channels::Rgba),
            self.term(c2, Argument::Age, true, Channels::Rgba),
            self.term(c3, Argument::ActiveTime, true, Channels::Rgba),
            self.term(rgb, Argument::ActiveTime, false, Channels::Rgb),
            self.term(alpha, Argument::ActiveTime, false, Channels::Alpha),
        ];
        super::cx::without_identities(terms)
    }

    /// A single clamped colour (distortion, blur) with the brightness and alpha multipliers.
    fn single_colour(&mut self, colour: &Property, rgb: &Property, alpha: &Property) -> Vec<Term> {
        let terms = vec![
            self.term(colour, Argument::Age, true, Channels::Rgba),
            self.term(rgb, Argument::ActiveTime, false, Channels::Rgb),
            self.term(alpha, Argument::ActiveTime, false, Channels::Alpha),
        ];
        super::cx::without_identities(terms)
    }

    fn size3(&mut self, x: &Property, y: &Property, z: &Property, uniform: bool) -> [Value; 3] {
        let first = self.age(x);
        if uniform {
            return [first.clone(), first.clone(), first];
        }
        [first, self.age(y), self.age(z)]
    }

    fn offset3(&mut self, x: &Property, y: &Property, z: &Property) -> [Value; 3] {
        let z = self.age(z);
        [self.age(x), self.age(y), value::negated(z)]
    }

    /// A texture scroll speed: the speed times its multiplier, both read at the age.
    fn scroll(&mut self, speed: &Property, multiplier: &Property) -> Value {
        let (s, m) = (self.age(speed), self.age(multiplier));
        value::product(s, m, &mut self.notes)
    }

    /// An effect light's colour: red, green, blue and alpha at the active time, with the game's transfer (the colour squared,
    /// read back from a running game for BTL and effect lights alike) applied at the keys of the red, green and blue.
    fn light_colour(&mut self, p: &Property) -> Value {
        let mut v = self.active(p);
        for key in &mut v.curve.keys {
            for c in 0..3 {
                key.value[c] *= key.value[c];
                key.handle_in[c] = 0.0;
                key.handle_out[c] = 0.0;
            }
        }
        self.notes
            .approximate("an effect light's colour squared at the keys of its curve (the game's transfer)");
        v
    }

    fn distortion(&mut self, d: &a::Distortion) -> Draw {
        let mode = match d.mode {
            0 => RefractionMode::Stir,
            1 | 3 => RefractionMode::Push,
            2 => RefractionMode::Twist,
            _ => {
                self.notes
                    .lose("a distortion mode the project does not name, taken as a normal-map push");
                RefractionMode::Push
            }
        };
        let shape = match d.shape {
            0 => RefractionShape::Rectangle,
            1 => RefractionShape::Hemiellipsoid,
            2 => RefractionShape::Ellipsoid,
            _ => {
                self.notes
                    .lose("a distortion shape the project does not name, taken as a rectangle");
                RefractionShape::Rectangle
            }
        };
        let blend = units::blend(value::number(&d.blend_mode).round() as i32, &mut self.notes);
        let orientation: Orientation = units::orientation(d.orientation, &mut self.notes);
        let normal_map = self.map_word(TextureKind::Normal, d.normal_map);
        let mask = self.map_word(TextureKind::Albedo, d.mask);
        let offset = self.offset3(&d.offset_x, &d.offset_y, &d.offset_z);
        let size = self.size3(&d.size_x, &d.size_y, &d.size_z, d.uniform_scale);
        let intensity = self.age(&d.intensity);
        // the file stores the stir speed in radians per second; the project shows degrees
        let stir = self.age(&d.stir_speed);
        let stir_speed = value::scaled(stir, 180.0 / std::f32::consts::PI);
        let radius = self.age(&d.radius);
        let normal_offset = [
            self.fixed(&d.normal_map_offset_u),
            self.fixed(&d.normal_map_offset_v),
        ];
        let normal_scroll = [self.age(&d.normal_map_speed_u), self.age(&d.normal_map_speed_v)];
        let normal_scale = [
            self.fixed(&d.normal_map_scale_u),
            self.fixed(&d.normal_map_scale_v),
        ];
        let tint = self.single_colour(&d.color, &d.rgb_multiplier, &d.alpha_multiplier);
        let cutoff = self.active(&d.alpha_threshold);
        Draw::Refraction(Refraction {
            mode,
            shape,
            orientation,
            normal_map,
            mask,
            offset,
            size,
            uniform: d.uniform_scale,
            size_variance: [d.scale_variation_x, d.scale_variation_y, d.scale_variation_z],
            intensity,
            stir_speed,
            radius,
            normal_offset,
            normal_scroll,
            normal_scale,
            tint,
            blend,
            alpha_cutoff: value::scaled(cutoff, BYTE),
            distance: units::distance_fade(
                d.min_fade_distance,
                d.min_distance,
                d.max_fade_distance,
                d.max_distance,
                d.min_distance_threshold,
                d.max_distance_threshold,
            ),
        })
    }

    fn tracer(&mut self, t: TracerIn<'_>) -> Draw {
        let blend = units::blend(value::number(t.blend).round() as i32, &mut self.notes);
        let (width, width_scale) = (self.age(t.width), self.emitted(t.width_multiplier));
        let (fade_start, fade_end) = (self.age(t.fade.0), self.age(t.fade.1));
        let tint = self.tint(t.colours[0], t.colours[1], t.colours[2], t.rgb, t.alpha);
        let (threshold, cutoff) = (self.age(t.fade_threshold), self.active(t.cutoff));
        let atlas = self.atlas(t.columns, t.total_frames, t.frame, false);
        let texture_fraction = self.age(t.texture_fraction);
        let scroll_u = self.age(t.speed_u);
        let variance_v = self.age(t.variance_v);
        let intensity = self.active(t.distortion_intensity);
        let normal_map = self.map_word(TextureKind::Normal, t.normal_map);
        let distortion =
            (!intensity.is_fixed() || value::number_of(&intensity) != 0.0).then_some(TrailDistortion {
                normal_map,
                intensity,
            });
        let lit = self.lit(t.lighting, 0, t.specular, t.glossiness, t.specularity, t.shadow);
        Draw::Trail(Trail {
            map: t.map,
            blend,
            orientation: units::tracer_orientation(t.orientation, &mut self.notes),
            width,
            width_scale,
            segment_interval: t.segment_interval,
            segment_life: t.segment_duration,
            max_segments: u32::try_from(t.concurrent_segments.max(1)).unwrap_or(1),
            subdivisions: u32::try_from(t.subdivision.max(0)).unwrap_or(0),
            texture_fraction,
            scroll_u,
            variance_u: t.variance_u,
            variance_v,
            attached_uv: t.attached_uv,
            dynamic_opacity: t.dynamic_opacity,
            fade_start,
            fade_end,
            alpha_fade_threshold: value::scaled(threshold, BYTE),
            alpha_cutoff: value::scaled(cutoff, BYTE),
            atlas,
            tint,
            fade_out: t.fade_out,
            distortion,
            lit,
            premultiply_alpha: t.premultiply,
            soft: None,
            distance: t.distance,
            bloom: t.bloom,
        })
    }

    fn field(
        &mut self,
        kind: ForceKind,
        force: &Property,
        dims: FieldShape,
        multiplier: f32,
        random: [f32; 2],
        fade_out: i32,
    ) -> Draw {
        let volume = match dims.shape {
            0 => ForceVolume::Boundless,
            1 => ForceVolume::Sphere {
                radius: dims.sphere_radius,
            },
            2 => ForceVolume::Box {
                half: dims.box_size.map(|s| s * 0.5),
            },
            3 => ForceVolume::Cylinder {
                radius: dims.cylinder_radius,
                half_height: dims.cylinder_height * 0.5,
            },
            4 => {
                // the project's apothem is the half-width of a square; a prism is a box
                self.notes.approximate("a square-prism force volume as a box");
                ForceVolume::Box {
                    half: [dims.prism_apothem, dims.prism_height * 0.5, dims.prism_apothem],
                }
            }
            _ => {
                self.notes
                    .lose("a force volume shape the project does not name, taken as boundless");
                ForceVolume::Boundless
            }
        };
        let strength = self.active(force);
        Draw::Field(ForceField {
            kind,
            volume,
            direction: units::FORWARD,
            strength,
            multiplier,
            random_multiplier: random,
            fade_out: units::frames(fade_out),
        })
    }

    fn flare(&mut self, f: &a::LensFlare) -> Draw {
        let blend = units::blend(f.blend_mode, &mut self.notes);
        let layers = [
            (
                f.layer1,
                &f.layer1_width,
                &f.layer1_height,
                &f.layer1_color,
                f.layer1_count,
                [f.layer1_scale_variation_x, f.layer1_scale_variation_y],
                f.layer1_uniform_scale,
                f.layer1_color_multiplier,
                f.layer1_reflection,
                f.layer1_offset,
                f.layer1_offset_variation,
                f.layer1_attenuation_radius,
                f.layer1_bloom_color,
            ),
            (
                f.layer2,
                &f.layer2_width,
                &f.layer2_height,
                &f.layer2_color,
                f.layer2_count,
                [f.layer2_scale_variation_x, f.layer2_scale_variation_y],
                f.layer2_uniform_scale,
                f.layer2_color_multiplier,
                f.layer2_reflection,
                f.layer2_offset,
                f.layer2_offset_variation,
                f.layer2_attenuation_radius,
                f.layer2_bloom_color,
            ),
            (
                f.layer3,
                &f.layer3_width,
                &f.layer3_height,
                &f.layer3_color,
                f.layer3_count,
                [f.layer3_scale_variation_x, f.layer3_scale_variation_y],
                f.layer3_uniform_scale,
                f.layer3_color_multiplier,
                f.layer3_reflection,
                f.layer3_offset,
                f.layer3_offset_variation,
                f.layer3_attenuation_radius,
                f.layer3_bloom_color,
            ),
            (
                f.layer4,
                &f.layer4_width,
                &f.layer4_height,
                &f.layer4_color,
                f.layer4_count,
                [f.layer4_scale_variation_x, f.layer4_scale_variation_y],
                f.layer4_uniform_scale,
                f.layer4_color_multiplier,
                f.layer4_reflection,
                f.layer4_offset,
                f.layer4_offset_variation,
                f.layer4_attenuation_radius,
                f.layer4_bloom_color,
            ),
        ];
        let mut out = Vec::new();
        for (
            texture,
            width,
            height,
            colour,
            count,
            variation,
            uniform,
            multiplier,
            reflection,
            offset,
            offset_variation,
            attenuation,
            bloom,
        ) in layers
        {
            if texture <= 0 {
                continue;
            }
            let map = self.map_word(TextureKind::Albedo, texture);
            let reflection = match reflection {
                0 => Reflection::None,
                1 => Reflection::Radial,
                2 => Reflection::Horizontal,
                _ => {
                    self.notes
                        .lose("a flare reflection mode the project does not name, taken as none");
                    Reflection::None
                }
            };
            let (width, height, colour) = (self.fixed(width), self.fixed(height), self.fixed(colour));
            out.push(FlareLayer {
                map,
                count: u32::try_from(count.max(1)).unwrap_or(1),
                width,
                height,
                uniform,
                scale_variation: variation,
                colour,
                colour_multiplier: multiplier,
                reflection,
                offset,
                offset_variation,
                attenuation_radius: attenuation,
                bloom: f.bloom.then_some(bloom),
            });
        }
        if out.is_empty() {
            self.notes.lose("a lens flare without a layer");
        }
        Draw::Flare(Flare {
            layers: out,
            blend,
            source_size: f.source_size,
            opacity_transition: f.opacity_transition_duration,
        })
    }
}

/// The parameters of a tracer the legacy tracer and the tracer share.
struct TracerIn<'a> {
    map: Option<Map>,
    blend: &'a Property,
    orientation: i32,
    width: &'a Property,
    width_multiplier: &'a Property,
    fade: (&'a Property, &'a Property),
    colours: [&'a Property; 3],
    rgb: &'a Property,
    alpha: &'a Property,
    fade_threshold: &'a Property,
    cutoff: &'a Property,
    frame: (&'a Property, &'a Property),
    texture_fraction: &'a Property,
    speed_u: &'a Property,
    variance_u: f32,
    variance_v: &'a Property,
    distortion_intensity: &'a Property,
    normal_map: i32,
    segment_interval: f32,
    segment_duration: f32,
    concurrent_segments: i32,
    subdivision: i32,
    fade_out: f32,
    columns: i32,
    total_frames: i32,
    attached_uv: bool,
    dynamic_opacity: bool,
    lighting: i32,
    specular: i32,
    glossiness: f32,
    specularity: f32,
    shadow: f32,
    bloom: Option<[f32; 4]>,
    distance: DistanceFade,
    premultiply: bool,
}

/// The size fields of a force volume.
struct FieldShape {
    /// The volume kind (0 boundless, 1 sphere, 2 box, 3 cylinder, 4 square prism).
    shape: i32,
    sphere_radius: f32,
    box_size: [f32; 3],
    cylinder_height: f32,
    cylinder_radius: f32,
    prism_height: f32,
    prism_apothem: f32,
}

impl FieldShape {
    fn of(
        shape: i32,
        sphere_radius: f32,
        box_size: [f32; 3],
        cylinder_height: f32,
        cylinder_radius: f32,
        prism_height: f32,
        prism_apothem: f32,
    ) -> Self {
        FieldShape {
            sphere_radius,
            box_size,
            cylinder_height,
            cylinder_radius,
            prism_height,
            prism_apothem,
            shape,
        }
    }
}
