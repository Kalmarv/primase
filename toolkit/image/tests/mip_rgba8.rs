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

/// Hand-worked: an odd side's last target pixel also covers the leftover source column, it is not clamped. 3x1 -> 1x1 averages all
/// three pixels, (0 + 1 + 4 + 1) / 3 = 2 (clamped to the first two it would be (0 + 1 + 1) / 2 = 1).
#[test]
fn an_odd_side_covers_its_leftover_column() {
    let rgba = [0, 0, 0, 0, 1, 1, 1, 1, 4, 4, 4, 4];
    let chain = mip_chain_rgba8(3, 1, &rgba).expect("chain");
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[1], (1, 1, vec![2, 2, 2, 2]));
}

/// Hand-worked: 5x3 -> 2x1 -> 1x1. Source column x covers target x * 5 / 2 .. (x + 1) * 5 / 2: target 0 columns 0..2, target 1 columns
/// 2..5, and the one row covers all three source rows. Columns hold 0, 0, 10, 20, 30 in every channel and row: target 0 is
/// (0 * 6 + 3) / 6 = 0, target 1 is (60 * 3 + 4) / 9 = 20; the last level is (0 + 20 + 1) / 2 = 10.
#[test]
fn odd_sides_in_both_axes_over_several_levels() {
    let columns = [0u8, 0, 10, 20, 30];
    let rgba: Vec<u8> = (0..3)
        .flat_map(|_| columns.iter().flat_map(|&v| [v; 4]))
        .collect();
    let chain = mip_chain_rgba8(5, 3, &rgba).expect("chain");
    assert_eq!(chain.len(), 3);
    assert_eq!(chain[1], (2, 1, vec![0, 0, 0, 0, 20, 20, 20, 20]));
    assert_eq!(chain[2], (1, 1, vec![10, 10, 10, 10]));
}
