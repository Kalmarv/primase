use primase_image::mip_chain_rgba8;

fn sizes(w: u32, h: u32) -> Vec<(u32, u32)> {
    let rgba = vec![7u8; (w * h * 4) as usize];
    mip_chain_rgba8(w, h, &rgba)
        .expect("chain")
        .iter()
        .map(|l| (l.0, l.1))
        .collect()
}
#[test]
fn non_square_and_odd_chains() {
    assert_eq!(sizes(8, 2), [(8, 2), (4, 1), (2, 1), (1, 1)]);
    assert_eq!(sizes(1, 1), [(1, 1)]);
    assert_eq!(sizes(5, 3), [(5, 3), (2, 1), (1, 1)]);
    assert_eq!(sizes(1, 7), [(1, 7), (1, 3), (1, 1)]);
}
#[test]
fn averages_round_half_up() {
    // 2x2 -> 1x1: (sum + 2) / 4 per channel.
    let rgba = [0, 1, 2, 255, 0, 1, 2, 255, 0, 0, 2, 255, 1, 1, 1, 255];
    let chain = mip_chain_rgba8(2, 2, &rgba).expect("chain");
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].2, rgba);
    assert_eq!(chain[1], (1, 1, vec![0, 1, 2, 255]));
    let rgba = [0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0];
    assert_eq!(mip_chain_rgba8(2, 2, &rgba).expect("chain")[1].2, [1, 1, 1, 1]);
}
#[test]
fn rejects_wrong_length() {
    assert!(mip_chain_rgba8(2, 2, &[0; 15]).is_err());
    assert!(mip_chain_rgba8(0, 2, &[]).is_err());
}
