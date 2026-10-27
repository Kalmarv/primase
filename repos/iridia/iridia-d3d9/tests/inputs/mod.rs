//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        General = "general",
    }
    installs {}
    keys {}
    tools {}
    oracle_sets {
        D3dDisassembler = "d3d-disassembler" {
            about: "Microsoft's D3D shader disassembly of a game's shader containers (`NNNN.dxbc` with its listing)",
            source: "independent: Microsoft's D3D Shader Disassembler, run over the user's own install",
        },
    }
    declared {
        Query::Oracle(OracleSet::D3dDisassembler, Game::General, "sm3") => {
            status: Live,
            gate: true,
            about: r#"privately extracted raw ps_3_0 (`pixel.bin`) and two vs_3_0 programs (`vertex.bin`, `vertex-layered.bin`), each with Microsoft's `.asm` disassembly; repos/iridia/iridia-d3d9/tests/disassembly.rs checks all executable operands, declarations, definitions and CTAB allocations"#,
            source: r#"independent: Microsoft d3dcompiler_47.dll D3DDisassemble via repos/iridia/iridia-d3d9/tests/oracles/disassemble.py, on a shipped Shaun White Skateboarding SDB2 v39 pixel shader (record 5) and vertex shaders (records 5 and 150, ekshadersPChigh.bin); raw bytes and disassembly stay outside the repository"#,
        },
    }
    debug {}
}
