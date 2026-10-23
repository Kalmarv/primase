#![allow(clippy::unwrap_used)]
//! The old word of an atomic operation on a texture view (`imm_atomic_*` on a typed unordered-access view) in a program
//! lifted from DXBC, run by the executor against textures that apply the operations (tests/fixtures/
//! image_atomic_results.hlsl: four threads in order, each adding, exchanging and maxing into texels of one texture and
//! storing the old words it reads to another).
use primase_dxbc::lift;
use primase_shader::D3dClass;
use primase_shader::ResourceId;
use primase_shader_exec::{
    Backend, Element, Executable, ImageAtomic, ImageAtomicOp, ImageSample, ImageStore, Resources,
};
use std::collections::BTreeMap;
use std::path::Path;

struct Results {
    words: BTreeMap<(ResourceId, [u32; 3]), u32>,
    stores: BTreeMap<(ResourceId, [u32; 3]), u32>,
}
impl Backend for Results {
    fn sample(&mut self, _: ImageSample<Element>) -> Result<[f32; 4], String> {
        Err("no sampling in this fixture".into())
    }
    fn atomic_image(&mut self, r: ImageAtomic<ResourceId>) -> Result<u32, String> {
        let word = self.words.entry((r.view, r.texel)).or_insert(0);
        let old = *word;
        *word = match r.op {
            ImageAtomicOp::Add => word.wrapping_add(r.value),
            ImageAtomicOp::Max => (*word).max(r.value),
            ImageAtomicOp::Exchange => r.value,
            other => return Err(format!("{other:?} is not in this fixture")),
        };
        Ok(old)
    }
    fn store_image(&mut self, r: ImageStore<ResourceId>) -> Result<(), String> {
        self.stores.insert((r.view, r.texel), r.value[0]);
        Ok(())
    }
}

/// Four threads in order, each adding x + 1 to counts[(0, 0)], exchanging x + 10 into counts[(1, 0)] (preset 5) and
/// maxing x * 3 into counts[(2, 0)], storing each old word to olds[(x, request)]: the old words are the running sum 0, 1,
/// 3, 6, the words exchanged out 5, 10, 11, 12 and the running maximum 0, 0, 3, 6.
#[test]
fn the_old_word_of_an_atomic_on_a_texture_reaches_the_program() {
    for variant in ["", ".od"] {
        let name = format!("image_atomic_results{variant}.dxbc");
        let bytes = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(&name),
        )
        .unwrap();
        let program = lift(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        program.validate().unwrap_or_else(|p| panic!("{name}: {p:?}"));
        assert!(
            primase_shader::emit::wgsl(&program)
                .unwrap_err()
                .to_string()
                .contains("texture atomic result"),
            "{name}"
        );
        let exe = Executable::new(&program).unwrap_or_else(|e| panic!("{name}: {e}"));
        let uav = |register: u32| {
            program
                .interface
                .resources
                .iter()
                .find(|r| {
                    r.provenance
                        .d3d
                        .is_some_and(|d| d.class == D3dClass::UnorderedAccess && d.register == register)
                })
                .unwrap_or_else(|| panic!("{name}: no u{register}"))
                .id
        };
        let (counts, olds) = (uav(0), uav(1));
        let mut backend = Results {
            words: BTreeMap::from([((counts, [1, 0, 0]), 5)]),
            stores: BTreeMap::new(),
        };
        let resources = Resources::default();
        let bound = exe.bind_resources(&resources).unwrap();
        exe.dispatch(&bound, &mut backend, &mut BTreeMap::new(), [1, 1, 1], 32)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let want = [[0, 1, 3, 6], [5, 10, 11, 12], [0, 0, 3, 6]];
        for (request, row) in want.iter().enumerate() {
            for (x, &word) in row.iter().enumerate() {
                assert_eq!(
                    backend.stores[&(olds, [x as u32, request as u32, 0])],
                    word,
                    "{name} request {request} thread {x}"
                );
            }
        }
        assert_eq!(backend.words[&(counts, [0, 0, 0])], 10, "{name}");
        assert_eq!(backend.words[&(counts, [1, 0, 0])], 13, "{name}");
        assert_eq!(backend.words[&(counts, [2, 0, 0])], 9, "{name}");
    }
}
