//! Elden Ring's far tiles (`m60_X_Z_02`) hold environment probe groups (`GIIV`/`GILM` 09xx) that no box of the tile's
//! layout names: one, or on high ground a second stacked on top of it. The reader records each as stored
//! (`FarTileEnvironmentGroup`), and `place-fromsoft-far-tile-environment` gives each a region placed by a box fitted
//! to the groups' own volumes (`Placement::FarTile`, how `Fitted`); these tests hold that rule over every
//! installed far tile, and hold the boxes against the volumes: their `W` coefficients are dark in cells buried under
//! the collision of the tile and its neighbours and bright in open ones, and the boxes separate them better than the
//! boxes around them.
mod common;
mod inputs;
use fromsoft_formats::{Archive, Binder, Game, dcx::Decoder, mapid::MapId, msb::MapLayout, tpf::Package};
use primase_core::World;
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_fromsoft::sekiro::environment;
use primase_fromsoft::sekiro::environment_data::{
    EnvironmentRegion, FarTileEnvironmentGroup, FarTilePlacement, Placement,
};
use primase_texcodec::TexDecoder;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Runs `test` with Elden Ring's archive and an Oodle decoder; passes where the install or Oodle is absent.
fn with_install(test: impl FnOnce(&Archive, &Decoder<'_>) -> Result<()>) -> Result<()> {
    let (Some(archive), Some(dll)) = (
        common::archive(Game::EldenRing).map_err(|e| e.to_string())?,
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll).map_err(|e| e.to_string())?;
    test(
        &archive,
        &Decoder {
            oodle: Some(&dll),
            max_output: 1 << 30,
        },
    )
}

/// Every far tile name of the overworld grids (`m60` and `m61`, variants `_02` and `_12`), installed or not.
fn far_tile_names() -> impl Iterator<Item = String> {
    [60u8, 61].into_iter().flat_map(|area| {
        (0..=40u8).flat_map(move |x| {
            (0..=40u8).flat_map(move |z| {
                [0x02u8, 0x12]
                    .into_iter()
                    .filter_map(move |variant| MapId { area, x, z, variant }.name())
            })
        })
    })
}

/// The tile's environment report, and its regions once `place-fromsoft-far-tile-environment` has placed its far-tile
/// groups, or `None` where the tile has no layout. The reader itself places none: it records each group as stored.
fn far_regions(
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
) -> Result<Option<(environment::Report, Vec<EnvironmentRegion>)>> {
    let path = fromsoft_formats::paths::map_layout(area);
    if !archive.contains(&path) {
        return Ok(None);
    }
    let layout = MapLayout::read(&archive.read(&path, decoder)?)?;
    let mut world = World::new();
    let report = environment::read(archive, decoder, area, &layout, &mut world)?;
    assert_eq!(
        world.iter::<FarTileEnvironmentGroup>().count(),
        report.far_tile_groups,
        "{area}"
    );
    assert!(
        world
            .iter::<EnvironmentRegion>()
            .all(|(_, r)| matches!(r.placement, Placement::Authored(_))),
        "{area}: the reader placed a far-tile group"
    );
    let mut registry = PassRegistry::new();
    primase_fromsoft::sekiro::passes::register(&mut registry);
    let manager = PassManager::new(registry).with_pass_checks(true);
    manager.run(
        &mut world,
        &manager.registry.parse("place-fromsoft-far-tile-environment")?,
        &mut RunReport::default(),
    )?;
    let regions = world
        .iter::<EnvironmentRegion>()
        .map(|(_, r)| r.clone())
        .collect();
    Ok(Some((report, regions)))
}

/// One far-tile region, as the placement pass placed it.
struct Layer {
    layer: u8,
    group: u32,
    centre: [f32; 3],
    half: [f32; 3],
    how: FarTilePlacement,
    packages: usize,
}

/// The far-tile regions among `regions`, by layer from the lowest.
fn layers(regions: &[EnvironmentRegion]) -> Vec<Layer> {
    let mut out: Vec<_> = regions
        .iter()
        .filter_map(|r| match r.placement {
            Placement::FarTile {
                layer,
                centre,
                half_extents,
                how,
            } => Some(Layer {
                layer,
                group: r.group,
                centre,
                half: half_extents,
                how,
                packages: r.available_packages.len(),
            }),
            Placement::Authored(_) => None,
        })
        .collect();
    out.sort_by_key(|l| l.layer);
    out
}

/// Every installed far tile with environment packages (48 in the installed build, all `m60`: 46 `_02` and two `_12`)
/// gets one far-tile region for each group of its packages that no box names, stacked from the ground up in the order
/// of their numbers (layer 0 the lowest), each placed by the fitted box; a far tile without packages gets none.
#[test]
fn every_far_tile_group_is_a_fitted_layer() -> Result<()> {
    with_install(|archive, decoder| {
        let mut tiles_by_layers = std::collections::BTreeMap::<usize, usize>::new();
        let mut without = 0;
        for name in far_tile_names() {
            let Some((report, regions)) = far_regions(archive, decoder, &name)? else {
                continue;
            };
            if report.packages == 0 {
                assert!(regions.is_empty(), "{name}: regions without packages");
                without += 1;
                continue;
            }
            let far = layers(&regions);
            assert!(!far.is_empty(), "{name}: no far-tile region");
            assert_eq!(report.far_tile_groups, far.len(), "{name}");
            for (i, l) in far.iter().enumerate() {
                assert_eq!(usize::from(l.layer), i, "{name}: layers");
                assert_eq!(l.how, FarTilePlacement::Fitted);
                assert!((900..1000).contains(&l.group), "{name}: group {:04}", l.group);
                assert_eq!(l.packages, report.packages, "{name}: group {:04}", l.group);
                if i > 0 {
                    let below = &far[i - 1];
                    assert!(below.group < l.group, "{name}: layers out of group order");
                    assert_eq!(
                        below.centre[1] + below.half[1],
                        l.centre[1] - l.half[1],
                        "{name}: layer {i} not on the one below"
                    );
                }
            }
            *tiles_by_layers.entry(far.len()).or_default() += 1;
        }
        println!("far tiles by layers: {tiles_by_layers:?}; {without} without packages");
        assert!(
            tiles_by_layers.values().sum::<usize>() >= 48,
            "{tiles_by_layers:?}"
        );
        assert!(tiles_by_layers.contains_key(&2), "no far tile with two layers");
        Ok(())
    })
}

/// The highest collision surface over a 4 m grid of +-768 m about the far tile's origin (rows along +Z, columns along
/// +X): the tile's 16 small tiles and the ring around them, every triangle of their `h` collision rasterised.
fn heights(archive: &Archive, decoder: &Decoder<'_>, far: MapId) -> Result<Vec<f32>> {
    use fromsoft_formats::overworld::tile_origin;
    const CELL: f64 = 4.;
    const HALF: f64 = 768.;
    let n = (2. * HALF / CELL) as usize;
    let mut hi = vec![f32::NAN; n * n];
    let (_, origin) = tile_origin(far).ok_or("not a far tile")?;
    for dx in -1..5i32 {
        for dz in -1..5i32 {
            let (x, z) = (i32::from(far.x) * 4 + dx, i32::from(far.z) * 4 + dz);
            let small = MapId {
                area: far.area,
                x: x as u8,
                z: z as u8,
                variant: 0,
            };
            let (Some(name), Some((_, at))) = (small.name(), tile_origin(small)) else {
                continue;
            };
            let (ox, oz) = (at[0] - origin[0], at[1] - origin[1]);
            let path = format!("/map/m{}/{name}/h{}.hkxbhd", far.area, &name[1..]);
            let (Ok(header), Ok(payload)) = (
                archive.read(&path, decoder),
                archive.read(&path.replace(".hkxbhd", ".hkxbdt"), decoder),
            ) else {
                continue;
            };
            let binder = Binder::read_split(&header, &payload, decoder)?;
            let mut compendium = None;
            for f in &binder.files {
                let bytes = decoder.decode_if_dcx(&f.bytes)?;
                if bytes.get(4..8) == Some(b"TCM0") {
                    compendium = Some(havok_formats::Compendium::read(&bytes)?);
                }
            }
            for f in &binder.files {
                let file = f.name.as_deref().unwrap_or("");
                if !file.ends_with(".hkx") && !file.ends_with(".hkx.dcx") {
                    continue;
                }
                let soup = fromsoft_formats::collision::read_map(
                    &decoder.decode_if_dcx(&f.bytes)?,
                    compendium.as_ref(),
                )?;
                for t in &soup.triangles {
                    let p = t.map(|v| [f64::from(v[0]) + ox, f64::from(v[1]), f64::from(v[2]) + oz]);
                    let det =
                        (p[1][0] - p[0][0]) * (p[2][2] - p[0][2]) - (p[2][0] - p[0][0]) * (p[1][2] - p[0][2]);
                    if det.abs() < 1e-9 {
                        continue;
                    }
                    let cell = |a: f64| ((a + HALF) / CELL).floor().clamp(0., (n - 1) as f64) as usize;
                    let (x0, x1) = (
                        p.iter().map(|v| v[0]).fold(f64::MAX, f64::min),
                        p.iter().map(|v| v[0]).fold(f64::MIN, f64::max),
                    );
                    let (z0, z1) = (
                        p.iter().map(|v| v[2]).fold(f64::MAX, f64::min),
                        p.iter().map(|v| v[2]).fold(f64::MIN, f64::max),
                    );
                    for cz in cell(z0)..=cell(z1) {
                        for cx in cell(x0)..=cell(x1) {
                            let (x, z) = ((cx as f64 + 0.5) * CELL - HALF, (cz as f64 + 0.5) * CELL - HALF);
                            let u = ((x - p[0][0]) * (p[2][2] - p[0][2])
                                - (p[2][0] - p[0][0]) * (z - p[0][2]))
                                / det;
                            let v = ((p[1][0] - p[0][0]) * (z - p[0][2])
                                - (x - p[0][0]) * (p[1][2] - p[0][2]))
                                / det;
                            if u >= 0. && v >= 0. && u + v <= 1. {
                                let y = (p[0][1] + u * (p[1][1] - p[0][1]) + v * (p[2][1] - p[0][1])) as f32;
                                let k = cz * n + cx;
                                if hi[k].is_nan() || y > hi[k] {
                                    hi[k] = y;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // the plane far below the land at the map's western edge is not ground a probe sits on
    for h in &mut hi {
        if *h < -300. {
            *h = f32::NAN;
        }
    }
    Ok(hi)
}

/// The `W` volumes of state 05, high tier, of `groups` stacked by rows in that order (the first at the bottom), decoded:
/// per texel the mean of its three coefficients `(rgb - 0.5) * 2 * 2^((a - 0.5) * 20)`, as `[slice][row][column]` with
/// the stack's size.
fn stacked(
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
    groups: &[u32],
) -> Result<(Vec<f64>, [usize; 3])> {
    let paths = fromsoft_formats::envmap::packages(archive, area)?;
    let package = paths
        .iter()
        .find(|p| p.state == 5 && p.tier == fromsoft_formats::envmap::Tier::High)
        .ok_or("no state 05 high package")?;
    let binder = Binder::read(&archive.read(&package.path, decoder)?, decoder)?;
    let codec = primase_texcodec::Decoder::default();
    let mut found = std::collections::BTreeMap::new();
    for member in &binder.files {
        for t in Package::read(&member.bytes)?.textures {
            let Some(group) = groups
                .iter()
                .find(|g| t.name.ends_with(&format!("GIIV{g:04}_05_W")))
            else {
                continue;
            };
            let layers = primase_dds::read_volume(&t.dds)?;
            let primase_core::TextureShape::Volume { depth } = layers.shape else {
                return Err(format!("{area}: {} not a volume", t.name).into());
            };
            let mut w = Vec::new();
            for slice in &layers.levels[0] {
                let rgba = codec.decode_rgba8(layers.format, layers.width, layers.height, slice)?;
                w.extend(rgba.as_chunks::<4>().0.iter().map(|c| {
                    let scale = 2f64.powf((f64::from(c[3]) / 255. - 0.5) * 20.);
                    c[..3]
                        .iter()
                        .map(|v| (f64::from(*v) / 255. - 0.5) * 2. * scale)
                        .sum::<f64>()
                        / 3.
                }));
            }
            found.insert(
                *group,
                (w, [layers.width as usize, layers.height as usize, depth as usize]),
            );
        }
    }
    let mut out = Vec::new();
    let mut size = [0usize; 3];
    for group in groups {
        let (v, [nx, ny, nz]) = found
            .remove(group)
            .ok_or_else(|| format!("{area}: no GIIV{group:04}_05_W"))?;
        if size[1] > 0 && [nx, nz] != [size[0], size[2]] {
            return Err(format!(
                "{area}: group {group:04} is {nx}x{nz} columns, the stack {}x{}",
                size[0], size[2]
            )
            .into());
        }
        // this layer's rows above the stack's
        let rows = size[1];
        let mut next = vec![0.; nx * (rows + ny) * nz];
        for k in 0..nz {
            for j in 0..rows + ny {
                for i in 0..nx {
                    next[(k * (rows + ny) + j) * nx + i] = if j < rows {
                        out[(k * rows + j) * nx + i]
                    } else {
                        v[(k * ny + j - rows) * nx + i]
                    };
                }
            }
        }
        out = next;
        size = [nx, rows + ny, nz];
    }
    Ok((out, size))
}

/// The probability that a buried cell's `W` is below an open cell's (the area under the ROC curve), over the cells of
/// the box `centre +- half` (each axis flipped where `flip` says) whose centre lies more than 1.5 rows from the surface.
fn separation(
    w: &[f64],
    [nx, ny, nz]: [usize; 3],
    heights: &[f32],
    centre: [f32; 3],
    half: [f32; 3],
    flip: [bool; 2],
) -> f64 {
    let n = (heights.len() as f64).sqrt() as usize;
    let at = |i: usize, count: usize, c: f32, h: f32, flip: bool| {
        let t = (i as f32 + 0.5) / count as f32 * 2. - 1.;
        c + if flip { -t } else { t } * h
    };
    let row = 2. * half[1] / ny as f32;
    let (mut buried, mut open) = (Vec::new(), Vec::new());
    for k in 0..nz {
        let z = at(k, nz, centre[2], half[2], flip[1]);
        for i in 0..nx {
            let x = at(i, nx, centre[0], half[0], flip[0]);
            let (cx, cz) = (((x + 768.) / 4.).floor(), ((z + 768.) / 4.).floor());
            if cx < 0. || cz < 0. || cx >= n as f32 || cz >= n as f32 {
                continue;
            }
            let ground = heights[cz as usize * n + cx as usize];
            if ground.is_nan() {
                continue;
            }
            for j in 0..ny {
                let y = at(j, ny, centre[1], half[1], false);
                let v = w[(k * ny + j) * nx + i];
                if y < ground - 1.5 * row {
                    buried.push(v);
                } else if y > ground + 1.5 * row {
                    open.push(v);
                }
            }
        }
    }
    let mut all: Vec<(f64, bool)> = buried
        .iter()
        .map(|v| (*v, true))
        .chain(open.iter().map(|v| (*v, false)))
        .collect();
    all.sort_by(|a, b| a.0.total_cmp(&b.0));
    // equal values share their mean rank: the decoded coefficients take few distinct values, so a tie counts one half
    let (mut rank_sum, mut start) = (0., 0);
    for tie in all.chunk_by(|a, b| a.0 == b.0) {
        let rank = start as f64 + (tie.len() as f64 - 1.) / 2.;
        rank_sum += rank * tie.iter().filter(|(_, b)| *b).count() as f64;
        start += tie.len();
    }
    let (b, o) = (buried.len() as f64, open.len() as f64);
    1. - (rank_sum - b * (b - 1.) / 2.) / (b * o)
}

/// A far tile's stack of layers: the collision heights about it, its volumes stacked in the pass's layer order, and
/// the pass's box around the whole stack.
struct Tile {
    heights: Vec<f32>,
    w: Vec<f64>,
    size: [usize; 3],
    groups: Vec<u32>,
    stack: Stack,
}

/// A box around a stack of layers: its bottom, one layer's height, the half span, its centre's X and Z, flips along X
/// and Z.
#[derive(Clone, Copy)]
struct Stack {
    bottom: f32,
    layer: f32,
    span: f32,
    centre: [f32; 2],
    flip: [bool; 2],
}
impl Stack {
    fn separation(&self, w: &[f64], size: [usize; 3], heights: &[f32], layers: usize) -> f64 {
        let height = self.layer * layers as f32;
        let centre = [self.centre[0], self.bottom + height / 2., self.centre[1]];
        separation(
            w,
            size,
            heights,
            centre,
            [self.span, height / 2., self.span],
            self.flip,
        )
    }
}

/// The far tile `name` stacked as the pass places its `layers` (at least one, by layer from the lowest).
fn tile(archive: &Archive, decoder: &Decoder<'_>, name: &str, layers: &[Layer]) -> Result<Tile> {
    let first = layers
        .first()
        .ok_or_else(|| format!("{name}: no far-tile region"))?;
    let groups: Vec<_> = layers.iter().map(|l| l.group).collect();
    let (w, size) = stacked(archive, decoder, name, &groups)?;
    let id = MapId::from_name(name).ok_or("map name")?;
    Ok(Tile {
        heights: heights(archive, decoder, id)?,
        w,
        size,
        stack: Stack {
            bottom: first.centre[1] - first.half[1],
            layer: 2. * first.half[1],
            span: first.half[0],
            centre: [first.centre[0], first.centre[2]],
            flip: [false, false],
        },
        groups,
    })
}

/// Far tiles the box fit is held over: low and high relief, eight of them with two layers.
const FIT_TILES: [&str; 12] = [
    "m60_08_10_02",
    "m60_08_11_02",
    "m60_09_10_02",
    "m60_11_09_02",
    "m60_10_12_02",
    "m60_10_13_02",
    "m60_11_12_02",
    "m60_11_13_02",
    "m60_12_12_02",
    "m60_12_13_02",
    "m60_13_13_02",
    "m60_13_14_02",
];

/// The pass's far-tile boxes separate buried from open cells of the groups' volumes, the layers of a tile stacked
/// (mean AUC at least 0.97 over [`FIT_TILES`]; the lower layer of high tiles separates worst, presumably because a
/// highest-surface height field mislabels cells under cliffs), and better than the boxes around them: flipped along X
/// or Z, 2.5 % narrower or wider, moved two cells east, the stack's bottom 40 m lower or higher, its layers 10 %
/// shorter or taller, or with cubic cells.
#[test]
fn far_tile_boxes_fit_their_volumes_against_collision() -> Result<()> {
    with_install(|archive, decoder| {
        let mut tiles = Vec::new();
        for name in FIT_TILES {
            let (_, regions) =
                far_regions(archive, decoder, name)?.ok_or_else(|| format!("{name}: no layout"))?;
            tiles.push(tile(archive, decoder, name, &layers(&regions))?);
        }
        let score = |change: &dyn Fn(&mut Stack)| {
            let sum: f64 = tiles
                .iter()
                .map(|t| {
                    let mut stack = t.stack;
                    change(&mut stack);
                    stack.separation(&t.w, t.size, &t.heights, t.groups.len())
                })
                .sum();
            sum / tiles.len() as f64
        };
        let fitted = score(&|_| {});
        println!("fitted boxes: mean AUC {fitted:.5} over {} tiles", tiles.len());
        assert!(fitted >= 0.97, "fitted boxes AUC {fitted}");
        let others: [(&str, f64); 11] = [
            ("flipped along X", score(&|b| b.flip[0] = true)),
            ("flipped along Z", score(&|b| b.flip[1] = true)),
            ("2.5 % narrower", score(&|b| b.span *= 0.975)),
            ("2.5 % wider", score(&|b| b.span *= 1.025)),
            (
                "moved two cells east",
                score(&|b| b.centre[0] += 2. * b.span / 42.),
            ),
            ("bottom 40 m lower", score(&|b| b.bottom -= 40.)),
            ("bottom 40 m higher", score(&|b| b.bottom += 40.)),
            ("layers 10 % shorter", score(&|b| b.layer *= 0.9)),
            ("layers 10 % taller", score(&|b| b.layer *= 1.1)),
            ("cubic cells", score(&|b| b.layer = 2. * b.span)),
            (
                "bottom 40 m lower, layers 5 % taller",
                score(&|b| {
                    b.bottom -= 40.;
                    b.layer *= 1.05;
                }),
            ),
        ];
        for (name, auc) in others {
            println!("{name}: mean AUC {auc:.5}");
            assert!(
                auc < fitted,
                "boxes {name} separate as well ({auc} against {fitted})"
            );
        }
        Ok(())
    })
}

/// On every installed far tile with two layers (15 in the installed build: 14 `_02` tiles and `m60_11_13_12`), the
/// volumes stacked in the pass's order (the lower group number at the bottom) separate buried from open cells better
/// than stacked the other way round, in the same box.
#[test]
fn far_tile_layers_stack_in_group_order() -> Result<()> {
    with_install(|archive, decoder| {
        let mut stacked_tiles = 0;
        for name in far_tile_names() {
            let Some((_, regions)) = far_regions(archive, decoder, &name)? else {
                continue;
            };
            let layers = layers(&regions);
            if layers.len() < 2 {
                continue;
            }
            assert_eq!(layers.len(), 2, "{name}: more than two layers");
            let t = tile(archive, decoder, &name, &layers)?;
            let ordered = t.stack.separation(&t.w, t.size, &t.heights, 2);
            let (swapped, size) = stacked(archive, decoder, &name, &[t.groups[1], t.groups[0]])?;
            let reversed = t.stack.separation(&swapped, size, &t.heights, 2);
            println!(
                "{name}: group {:04} under {:04}: AUC {ordered:.5}, swapped {reversed:.5}",
                t.groups[0], t.groups[1]
            );
            assert!(
                ordered > reversed,
                "{name}: the swapped layers separate as well ({reversed} against {ordered})"
            );
            stacked_tiles += 1;
        }
        println!("{stacked_tiles} far tiles with two layers, each in group order");
        assert!(stacked_tiles >= 15, "{stacked_tiles} far tiles with two layers");
        Ok(())
    })
}
