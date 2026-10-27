#![allow(clippy::unwrap_used)]
//! Hand-worked token boundaries: END inside a comment or literal is data, not the program's END.
use iridia::Stage;

#[test]
fn frame_keeps_both_stage_versions_and_leaves_container_bytes() {
    for (version, stage) in [(0xfffe_0300, Stage::Vertex), (0xffff_0300, Stage::Pixel)] {
        // COMMENT with an END-shaped word, DEF with another, real END, then unaligned metadata.
        let words: [u32; 11] = [
            version,
            0x0002_fffe,
            0xffff,
            0x1234,
            0x0500_0051,
            0xa00f_0000,
            0xffff,
            0,
            0,
            0,
            0xffff,
        ];
        let mut bytes: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        bytes.extend([0xbe, 0xef, 1]);
        let (got, program) = iridia_d3d9::frame(&bytes).unwrap();
        assert_eq!(got, stage);
        assert_eq!(program.len(), 44);
        assert_eq!(program, &bytes[..44]);
        for n in 0..44 {
            assert!(
                iridia_d3d9::frame(&bytes[..n]).is_err(),
                "accepted truncated prefix {n}"
            );
        }
    }
    assert!(
        iridia_d3d9::frame(&[0, 2, 255, 255])
            .unwrap_err()
            .detail
            .contains("version")
    );
    let bad: Vec<_> = [0xffff_0300u32, 0x7fff_fffe]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    assert!(iridia_d3d9::frame(&bad).unwrap_err().detail.contains("span"));
}
