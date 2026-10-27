//! How the stages of a draw's variants go together in the stored data. Unity compiles each stage of a pass for the
//! keyword sets that stage uses (a keyword a stage never reads is not in its variants), so a draw's keyword set `S`
//! (the material's keywords and the renderer's) selects, per stage, the variant whose keywords are exactly `S`
//! restricted to the keywords some variant of that stage uses. A fragment variant's keywords alone do not determine
//! its vertex variant (a vertex stage reading `LOD_FADE_PERCENTAGE` is compiled with it though the fragment stage
//! never is): the draw's `S` does.
//!
//! Tested for every pass of every shader of Sledding Game and SkateBIRD, per hardware tier and per shader model
//! (vertex types 15 and 16 go with pixel types 17 and 18): every fragment variant `F` has a vertex variant `V` with
//! `S = F ∪ V` selecting exactly `F` and `V` (`S` restricted to the fragment stage's keywords is `F`'s, restricted to
//! the vertex stage's is `V`'s), and every vertex variant has such a fragment variant: the stages' variants form
//! one consistent family. Prints the counts. Duplicate variants (the same blob listed twice) are one.
#![allow(clippy::unwrap_used)]

mod inputs;

use std::collections::BTreeSet;

use inputs::{Game, Tool, install_file, tool};
use primase_unity::TypeTreePackage;
use primase_unity::listing;
use primase_unity::player::Player;
use primase_unity::program::{D3D11_PROGRAM_TYPES, PassProgram, ShaderPrograms, Variant};

type Keywords = BTreeSet<String>;

fn keywords(v: &Variant) -> Keywords {
    v.keywords.iter().cloned().collect()
}

/// The D3D11 variants of a stage, each blob once.
fn distinct(p: &PassProgram) -> Vec<&Variant> {
    let mut seen = BTreeSet::new();
    p.variants
        .iter()
        .filter(|v| D3D11_PROGRAM_TYPES.contains(&v.gpu_program_type) && seen.insert((v.blob_index, v.group)))
        .collect()
}

/// (fragment variants, those with no vertex partner, vertex variants, those with no fragment partner)
fn family(fragment: &PassProgram, vertex: &PassProgram) -> (usize, usize, usize, usize) {
    let (frag, vert) = (distinct(fragment), distinct(vertex));
    let used = |vs: &[&Variant]| -> Keywords { vs.iter().flat_map(|v| v.keywords.clone()).collect() };
    let (used_f, used_v) = (used(&frag), used(&vert));
    let restrict = |s: &Keywords, used: &Keywords| -> Keywords { s.intersection(used).cloned().collect() };
    let partner = |f: &Variant, v: &Variant| -> bool {
        let s: Keywords = keywords(f).union(&keywords(v)).cloned().collect();
        f.group == v.group
            && f.gpu_program_type == v.gpu_program_type + 2
            && restrict(&s, &used_f) == keywords(f)
            && restrict(&s, &used_v) == keywords(v)
    };
    let f_alone = frag
        .iter()
        .filter(|f| !vert.iter().any(|v| partner(f, v)))
        .count();
    let v_alone = vert
        .iter()
        .filter(|v| !frag.iter().any(|f| partner(f, v)))
        .count();
    (frag.len(), f_alone, vert.len(), v_alone)
}

fn census(label: &str, data: &std::path::Path, tpk: &std::path::Path) {
    let player = Player::open(
        data,
        TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let shaders = listing::list(&player, Some(&["Shader"])).unwrap();
    let mut total = (0, 0, 0, 0);
    let (mut unreadable, mut passes) = (0, 0);
    for s in &shaders.entries {
        let Ok(programs) = ShaderPrograms::read(&player, &s.object) else {
            unreadable += 1;
            continue;
        };
        for pass in programs.passes.iter().flatten() {
            if pass.fragment.variants.is_empty() || pass.vertex.variants.is_empty() {
                continue;
            }
            passes += 1;
            let (a, b, c, d) = family(&pass.fragment, &pass.vertex);
            total = (total.0 + a, total.1 + b, total.2 + c, total.3 + d);
        }
    }
    println!(
        "{label}: {} shaders ({unreadable} unreadable), {passes} passes with both stages; fragment D3D11 variants {} \
         ({} without a vertex partner), vertex variants {} ({} without a fragment partner)",
        shaders.entries.len(),
        total.0,
        total.1,
        total.2,
        total.3
    );
    assert_eq!(
        (total.1, total.3),
        (0, 0),
        "{label}: stages' variants do not form one family"
    );
}

#[test]
fn the_stages_variants_of_a_pass_form_one_family_by_the_draws_keywords() {
    let Some(tpk) = tool(Tool::UnityTpk) else {
        return;
    };
    let players = [
        (
            "Sledding Game",
            install_file(Game::Sledding, "Sledding Game_Data"),
        ),
        ("SkateBIRD", install_file(Game::Skatebird, "SkateBIRD_Data")),
    ];
    for (label, data) in players {
        if let Some(data) = data {
            census(label, &data, &tpk);
        }
    }
}
