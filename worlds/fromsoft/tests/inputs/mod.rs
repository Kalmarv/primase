//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Elden = "elden",
        Sekiro = "sekiro",
    }
    installs {
        Elden {
            about: "Elden Ring's `Game` folder (Data0 to Data3, `sd/sd`, DLC archives when installed)",
            source: "the user's own install",
            gate: false,
        },
        Sekiro {
            about: "Sekiro's install folder (Data1 to Data5 BHD/BDT pairs, the Oodle 6 DLL)",
            source: "the user's own install",
            gate: true,
        },
    }
    keys {
        Elden {
            about: "the user's Elden Ring archive keys, one PKCS#1 `RSA PUBLIC KEY` PEM per archive named after its stem, and `regulation.key`, the key of `regulation.bin` as 64 hexadecimal digits (held to the SHA-256 worlds/fromsoft/src/elden/mod.rs states)",
            source: "the community unpacker's published keys (UXM's ArchiveKeys.cs) and SoulsFormats' RegulationDecryptor key for the game, written out by a script kept with the game's private tools",
            gate: false,
        },
        Sekiro {
            about: "the user's Sekiro archive keys: one PKCS#1 `RSA PUBLIC KEY` PEM per archive, Data1.pem to Data5.pem",
            source: "the community unpacker's published keys (UXM's ArchiveKeys.cs), written out as PEM files",
            gate: true,
        },
    }
    tools {
        Dotnet = "dotnet" {
            about: "the .NET host (`dotnet`, with the .NET 8 runtime) that runs the C# programs given as assemblies: `scripts.sekiro/dds`'s DdsWorker.dll and a `tools.scene_skate` that is a .dll",
            source: "third-party: Microsoft's .NET 8 runtime",
            gate: true,
        },
        Oodle6 = "oodle6" {
            about: "an Oodle version-6 DLL (`oo2core_6_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured",
            source: "the game's own DLL from the user's install",
            gate: true,
        },
        PythonPillow = "python_pillow" {
            about: "a Python 3 interpreter with Pillow 11.0.0, which runs the Pillow oracle worker (toolkit/texcodec/tests/oracles/worker.py)",
            source: "third-party: Pillow 11.0.0",
            gate: true,
        },
    }
    oracle_sets {
        Baseline = "baseline" {
            about: "our own earlier outputs kept to detect change (an earlier exporter's scene, a map our writer built, a frozen stage result)",
            source: "regression baselines: our own earlier outputs, not independent oracles",
        },
        Corpus = "corpus" {
            about: "the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames)",
            source: "the user's own installs: files extracted as stored",
        },
        Scripts = "scripts" {
            about: "measurements by our own scripts over game data, separate from the readers they check (normal curl, texture statistics, shadow and state records, the eclipse reference)",
            source: "our own scripts kept with the private oracle tools: separate from the readers under test, not third-party; some producers are not recorded yet",
        },
        SoulsFormats = "soulsformats" {
            about: "SoulsFormats' readings of FromSoftware archives and files (rigid parts, skins, parts, census, frames, formats)",
            source: "independent: SoulsFormats 9c0da6b (Sekiro) and SoulsFormatsNEXT ee1dd61 (Elden Ring), through a thin dump harness kept with the private oracle tools",
        },
        Uxm = "uxm" {
            about: "UXM's path dictionaries (`dictionary.txt`, UTF-8, one archive path per line)",
            source: "third-party: UXM-Selective-Unpack 9501be8's dictionaries",
        },
    }
    declared {
        Query::Oracle(OracleSet::Baseline, Game::Elden, "malenia-binding-report.json") => {
            status: Live,
            gate: false,
            about: r#"the lab's report of a read of Elden Ring's c2120 (Malenia) with game shaders (its `views[*].game_shader.bound` rows: per bound program the material constants as the lab bound them); worlds/fromsoft/tests/malenia_visibility.rs executes the petal program on those constants"#,
            source: r#"regression baseline: the primase-lab smoke report of an earlier run of our own lab, frozen; not an independent oracle"#,
        },
        Query::Oracle(OracleSet::Baseline, Game::Sekiro, "layer-pilot") => {
            status: Live,
            gate: true,
            about: r#"layer-bake pipeline states of an area's layered materials (cases.json; per case, size cap and weight variant a state folder) with WARP's outputs for each (warp/); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/layer_pilot.rs"#,
            source: r#"the states: our own bake's (its stated policy), written by worlds/fromsoft/examples/layer_pilot_states.rs over the user's install (m11_01, region -170,180,-50,300); independent: Microsoft's WARP (d3d10warp 6.2.26100.9711) running the game's programs on them, through a thin quad host kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::Baseline, Game::Sekiro, "outfits") => {
            status: Live,
            gate: false,
            about: r#"a folder of the old C# character exporter's manifests, one `<id>.json` per character (`c1010`): the NPC row chosen, the display masks it showed (`options.DisplayMasks`), and per source mesh its material, display mask and whether it was included, hidden by its mask or left out as equipment; worlds/fromsoft/tests/npc_outfits.rs"#,
            source: r#"regression baseline: the manifests our earlier C# exporter (sekiro-skate SekiroExtract, CharacterExport.cs) wrote for the Sekiro character pack 0.2.x; our own output, so it detects a change of the selection and checks no correctness"#,
        },
        Query::Oracle(OracleSet::Baseline, Game::Sekiro, "scene") => {
            status: Live,
            gate: true,
            about: r#"our earlier exporter's skate-scene folder of an area: scene.json (region, materials), its collision tables (a regression baseline, not an independent reference) and textures; checks/tests/web_texture_decode.rs, worlds/fromsoft/tests/material_assignments.rs, shadow_census.rs"#,
            source: r#"regression baseline: our earlier C# exporter's scene of the area, not an independent reference; placements are checked against the SoulsFormats dump instead"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "dcx") => {
            status: Live,
            gate: true,
            about: r#"folder recursively containing loose KRAK DCX files"#,
            source: r#"the game's own files from the user's install: worlds/fromsoft/examples/corpus_sekiro_dcx.rs (KRAK envelopes as stored, the first 256 in hash order up to 4 MiB)"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "lznib") => {
            status: Live,
            gate: true,
            about: r#"a folder of Sekiro's LZB16 streams (Oodle decoder type 2, `8C 02`): `<hash>.payload`, the stream as the archive stores it in its DCX envelope from the block header on, and `<hash>.decoded`, the game's Oodle library's output for it; worlds/fromsoft/tests/lznib_gate.rs"#,
            source: r#"the game's own library's output for the game's own streams: worlds/fromsoft/tests/krak_archive.rs with the debug key `oodle.dump` on a machine with the Oodle 6 library, the files it keeps for the streams oozextract cannot decode or decodes differently"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "shaders") => {
            status: Live,
            gate: false,
            about: r#"a folder (searched recursively) of the DXBC containers of Sekiro's extracted pixel and vertex programs (`.ppo`, `.vpo`), lifted, validated and written as WGSL, the refusals counted by reason; https://github.com/Kalmarv/iridia/blob/main/iridia-dxbc/tests/lift_corpus.rs; the detail-blend programs' equations derived and proven, worlds/fromsoft/tests/detail_derive.rs"#,
            source: r#"the user's own install: the programs extracted from its shader archives"#,
        },
        Query::Oracle(OracleSet::Scripts, Game::Sekiro, "dds") => {
            status: Live,
            gate: true,
            about: r#"a folder holding the DDS decoding worker `DdsWorker.dll` and the libraries it loads; protocol in worlds/fromsoft/README.md; worlds/fromsoft/tests/tpf_pixels.rs"#,
            source: r#"independent: BCnEncoder.Net 2.2.1 (MIT OR Unlicense), through a thin decoding worker kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::Scripts, Game::Sekiro, "normal-curl.json") => {
            status: Live,
            gate: true,
            about: r#"a JSON list of curl measurements of Sekiro normal maps by a script separate from the reader (`name`, `verdict` per map); `worlds/fromsoft/tests/normal_basis.rs`"#,
            source: r#"our own measurement script over the game's normal maps (separate from the reader under test, not a third-party reader)"#,
        },
        Query::Oracle(OracleSet::Scripts, Game::Sekiro, "pbr") => {
            status: Live,
            gate: true,
            about: r#"source DDS in `census/dds` plus independent `texture-statistics.json` channel extrema and means; `worlds/fromsoft/tests/pbr_census.rs`"#,
            source: r#"the install's own DDS with statistics computed by a script kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::Scripts, Game::Sekiro, "reflectance") => {
            status: Live,
            gate: true,
            about: r#"a folder: `reflectance.json`, naming one layered material's G-buffer program (path, sha256, the dielectric literal's offset and bits) and its DDS inputs (paths, sha256), every path relative to the folder, and the program and DDS files it names; the static bake must reproduce the traced reflectance at every texel while the other lowered channels stay unchanged; `worlds/fromsoft/src/sekiro/passes/bake_assembly.rs`"#,
            source: r#"the game's own shader program and textures from the user's install, with the literal's offset read from the program's bytes; the file only names and hashes them"#,
        },
        Query::Oracle(OracleSet::Scripts, Game::Sekiro, "shadow") => {
            status: Live,
            gate: true,
            about: r#"original decompressed `map.msb` and independent `shadows.json` (source SHA-256 and ordered part names plus six raw shadow fields); `worlds/fromsoft/tests/shadow_census.rs`"#,
            source: r#"the install's map.msb with shadows.json; the JSON's producer is not recorded yet"#,
        },
        Query::Oracle(OracleSet::Scripts, Game::Sekiro, "state") => {
            status: Live,
            gate: true,
            about: r#"independent MSB part entity IDs and groups (`entities.jsonl`) and event-script decode (`emevd.jsonl`); schema in worlds/fromsoft/README.md"#,
            source: r#"a private dump tool's MSB/EMEVD reading; only its build output is retained, so its independence is not yet shown"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Elden, "census") => {
            status: Live,
            gate: false,
            about: r#"an independent census of installs.elden in oracles.soulsformats.sekiro/census's schema (the archives' decrypted headers as `<name>.plain`, entries.tsv, binders.tsv, errors.tsv, samples; hashes 16 hex digits; an archive's shard is its index in UXM's Elden Ring archive list); worlds/fromsoft/tests/census.rs, and asset_collision.rs (the asset collision binders entries.tsv lists)"#,
            source: r#"independent: SoulsFormatsNEXT ee1dd61 with UXM 9501be8's keys, RSA and path hash, the Elden Ring harness's `census` mode (kept with the private oracle tools)"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Elden, "format") => {
            status: Live,
            gate: false,
            about: r#"an independent reader's field-by-field reading of Elden Ring files read from installs.elden (JSON lines, floats as hex bit patterns): `msb.jsonl` (every map layout: its models, every part with kind, model, transform, entity IDs, shadow controls and hit filter, and its spawn point regions with name, position and rotation), `btl.jsonl` (every light bank: its version and lights, or the reference's refusal), `havok.jsonl` (chosen tiles' map collision and assets' collision: every compressed mesh shape's stored sections, vertices, primitives, shape keys and extern shape instances), `matbin.jsonl` (every material binder's members in member order: shader and source paths, key, every parameter's name, key, type and value, every sampler's type, path, key and two trailing floats), `metaparam.jsonl` (every shader bundle's metaparam: binder, bundle and member names, header words, texture slots, render passes, CAParams and MtdParams); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/msb_census.rs, btl_census.rs, havok_census.rs, matbin_census.rs, metaparam_census.rs"#,
            source: r#"independent: SoulsFormatsNEXT ee1dd61 with UXM 9501be8's keys and path hash, and HKLib ac40bc9 for Havok, the Elden Ring harness's format modes (kept with the private oracle tools)"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Elden, "npc.jsonl") => {
            status: Live,
            gate: false,
            about: r#"JSON lines: one of the NPC param's header facts (`kind` param: member, regulation version, paramType, dataVersion, detected and definition row size, format flags, unk06, row count), one per row (`kind` row: id, name, the 32 modelDispMask cells), then one per Enemy and DummyEnemy part of every map the UXM dictionary names (`kind` enemy or dummy: map path, part name, model, NPCParamID); worlds/fromsoft/tests/npc_outfits.rs"#,
            source: r#"independent: SoulsFormatsNEXT ee1dd61 (RegulationDecryptor, PARAM with Paramdex's ER/Defs/NpcParam.xml as Smithbox cbd477a8 ships it, applied at the regulation's version; MSBE), the Elden Ring harness's `npc` mode (kept with the private oracle tools)"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "census") => {
            status: Live,
            gate: true,
            about: r#"reference archive outputs; schema in worlds/fromsoft/README.md"#,
            source: r#"independent: SoulsFormats 9c0da6b with UXM's Sekiro dictionary, the dump harness's census mode (kept with the private oracle tools)"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "frames") => {
            status: Live,
            gate: true,
            about: r#"a folder holding `frames.jsonl`: SoulsFormats' reading of a few Sekiro models as stored (per mesh its LOD0 face set, per vertex position, normal, tangents and UV0); `worlds/fromsoft/tests/handedness.rs`"#,
            source: r#"independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (frames mode)"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "m11_01") => {
            status: Live,
            gate: true,
            about: r#"a SoulsFormats dump of an area: parts.jsonl (every MSB map piece and object part with its model and matrix), models.jsonl, missing-rigid.jsonl (placed models it could not dump, with why) and per placed model a binary of mesh/vertex counts followed by position and normal f32 triples in model space; worlds/fromsoft/tests/rigid_census.rs, sekiro_scene.rs"#,
            source: r#"independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`rigid` mode)"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "npc") => {
            status: Live,
            gate: false,
            about: r#"a folder of SoulsFormats dumps of the Sekiro NPC param and of characters' materials: npcparam.json (NpcParam.param of gameparam.parambnd.dcx read by PARAM with Paramdex's NpcParam.xml: header facts, row size, every row's id, name and every cell by its internal name), per character id `<id>.materials.json` (the FLVER's materials by index, name and MTD, and each mesh's material index), and `enemies.jsonl` with `maps.json` (every Enemy and DummyEnemy part of the install's ten MSBS maps: map, model name, NPCParamID, ThinkParamID); worlds/fromsoft/tests/npc_outfits.rs"#,
            source: r#"independent: SoulsFormats 9c0da6b with Paramdex's SDT/Defs/NpcParam.xml (sha256 dc4be2da), through a thin dump harness kept with the private oracle tools (`params`, `chrmaterials` and `enemies` modes)"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "objects") => {
            status: Live,
            gate: true,
            about: r#"the game's object main .hkx files (179, castle) with Havoc's reading of each (`<name>.hkx.json`: every object, its class and fields as stored, floats as bits); bodies, shape classes, convex cores and poses checked exactly, compressed meshes counted as uncovered; worlds/fromsoft/tests/object_collision.rs"#,
            source: r#"independent: Havoc (SoulsAssetPipeline d11caf9), through a thin dump harness kept with the private oracle tools (`havok-json` mode); no independent decoder reads Sekiro's compressed meshes (HKLib supports Havok 2018 only)"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "parts") => {
            status: Live,
            gate: false,
            about: r#"a folder of SoulsFormats dumps of the Wolf's parts, one folder per part name (`bd_m_9120_l`): the files of oracles.soulsformats.sekiro/skin's skeleton.json, meshes.jsonl, meshes/<i>.bin and chrbnd.json (the partsbnd's members), for the part's FLVER; worlds/fromsoft/tests/sekiro_parts.rs"#,
            source: r#"independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`skin` mode, area `part:<name>`)"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "skin") => {
            status: Live,
            gate: false,
            about: r#"a folder of SoulsFormats dumps of Sekiro characters, one folder per character id (`c1010`): skeleton.json (every FLVER bone's name, parent, stored transform and ComputeLocalTransform), meshes.jsonl (each mesh's dynamic flag, palette, face sets and layouts), meshes/<i>.bin (per vertex position, normal, bone indices and weights as stored), dummies.json, anibnd.json; worlds/fromsoft/tests/character_skin.rs"#,
            source: r#"independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`skin` mode)"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "tpf") => {
            status: Live,
            gate: true,
            about: r#"independent texture-package census (packages.jsonl, area-dds/, errors.tsv); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/tpf_census.rs, tpf_pixels.rs"#,
            source: r#"independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`tpf` mode); area-dds/ holds every distinct DDS under `/map/`"#,
        },
        Query::Oracle(OracleSet::Uxm, Game::Elden, "dictionary.txt") => {
            status: Live,
            gate: false,
            about: r#"caller-provided UTF-8 Elden Ring path dictionary"#,
            source: r#"third-party: UXM's Elden Ring path dictionary"#,
        },
        Query::Oracle(OracleSet::Uxm, Game::Sekiro, "dictionary.txt") => {
            status: Live,
            gate: true,
            about: r#"caller-provided UTF-8 Sekiro path dictionary"#,
            source: r#"third-party: UXM's Sekiro path dictionary"#,
        },
    }
    debug {
        "fromsoft.bake_digest" => "the digest the baked layer textures of Sekiro's m11_01 must have (printed by an earlier run)",
        "fromsoft.census_areas" => "which areas the FromSoft memory census reads: a comma list of `<game>/<area>` or `<area>` (default every area it lists; one to a process so an abort loses no other)",
        "fromsoft.detail_census" => "which characters the detail-blend census samples: `<game>:<ids>;<game>:<ids>`, each id `cNNNN` (default `elden:c2120,c3251,c2030,c2010;sekiro:c1070`; worlds/fromsoft/tests/detail_census.rs)",
        "fromsoft.memory_cap_gb" => "the most GiB the FromSoft memory census lets a read hold before it aborts naming the stage (default 10; worlds/fromsoft/tests/memory_census.rs)",
        "oodle.dump" => "a folder under the scratch root: the Oodle gate writes there the payload and the game library's output of each stream oozextract refuses or decodes differently",
    }
}
