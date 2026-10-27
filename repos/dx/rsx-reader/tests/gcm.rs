//! Hand-worked `CellGcmTexture` descriptors.
use rsx_reader::gcm::{CellGcmTexture, Error};

#[test]
fn descriptor_endian_and_metadata() {
    let bytes = [
        0xe5, 2, 2, 0, 0, 0, 0xaa, 0xe4, 0x12, 0x34, 1, 0x23, 0, 1, 1, 7, 0, 0, 0x50, 0, 0x12, 0x34, 0x56,
        0x78,
    ];
    let d = CellGcmTexture::parse(&bytes).expect("descriptor");
    assert_eq!((d.width, d.height, d.depth), (0x1234, 0x123, 1));
    assert_eq!((d.pitch, d.offset, d.location, d.pad), (0x5000, 0x12345678, 1, 7));
    assert!(d.linear() && d.unnormalized_coordinates());
    assert!(matches!(
        CellGcmTexture::parse(&bytes[..23]),
        Err(Error::DescriptorLength(23))
    ));
}
