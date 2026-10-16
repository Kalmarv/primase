//! Private corpus paths are caller-supplied; no game data is bundled.
use primase_core::gate::{GateReport, Tolerance};
use primase_fromsoft::{
    NameDictionary,
    archive::{Archive, public_key},
    bhd::{Header, unwrap_rsa},
};
use primase_testdata::real_data;
#[test]
fn headers() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(oracle) = real_data("PRIMASE_FROMSOFT_BHD_ORACLE") else {
        return Ok(());
    };
    let mut report = GateReport::new("RSA BHD C# oracle", Tolerance::Exact);
    for shard in 1..=5 {
        let (n, e) = public_key(shard)?;
        let enc = std::fs::read(root.join(format!("Data{shard}.bhd")))?;
        let plain = unwrap_rsa(&enc, n, e)?;
        let want = std::fs::read(oracle.join(format!("Data{shard}.plain")))?;
        report.compare_bytes(format!("Data{shard}"), &plain, &want);
        let header = Header::read(&plain)?;
        println!("Data{shard}: {} entries", header.entries.len());
    }
    println!("{report}");
    report.assert_passed();
    let archive = Archive::open(root)?;
    assert_eq!(archive.entries.len(), 8765);
    assert_eq!(archive.headers[0].entries.len(), 1183);
    assert_eq!(archive.headers[4].entries.len(), 5449);
    if let Some(names) = real_data("PRIMASE_FROMSOFT_NAMES") {
        let dict = NameDictionary::read(names)?;
        let named = archive
            .entries
            .keys()
            .filter(|h| dict.names.contains_key(h))
            .count();
        println!("named {named}");
        assert_eq!(named, 8752);
    }
    Ok(())
}

#[allow(unsafe_code)]
mod native {
    use primase_compress::oodle::{Oodle, Version};
    pub fn load(path: std::path::PathBuf) -> primase_compress::Result<Oodle> {
        // SAFETY: the gate requires an explicitly supplied, trusted version-6 DLL.
        unsafe { Oodle::load(path, Version::V6) }
    }
}
/// Whole-install differential against the reference C# archive/binder decoder.
/// Every indexed entry is attempted; corrupt entries remain explicit failures.
#[test]
fn whole_install() -> Result<(), Box<dyn std::error::Error>> {
    use primase_fromsoft::{Binder, dcx::Decoder};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(oracle) = real_data("PRIMASE_FROMSOFT_BHD_ORACLE") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let oodle = native::load(dll)?;
    let decoder = Decoder {
        oodle: Some(&oodle),
        max_output: 512 * 1024 * 1024,
    };
    let archive = Archive::open(root)?;
    let entry_text = std::fs::read_to_string(oracle.join("entries.tsv"))?;
    let member_text = std::fs::read_to_string(oracle.join("binders.tsv"))?;
    let mut expected = BTreeMap::new();
    for line in entry_text.lines() {
        let f: Vec<_> = line.split('\t').collect();
        expected.insert(
            (f[0].parse::<u8>()?, u32::from_str_radix(f[1], 16)?),
            (f[3].parse::<usize>()?, f[4].to_owned()),
        );
    }
    let mut members = BTreeMap::new();
    for line in member_text.lines() {
        let f: Vec<_> = line.split('\t').collect();
        members.insert(
            (
                f[0].parse::<u8>()?,
                u32::from_str_radix(f[1], 16)?,
                f[2].parse::<usize>()?,
            ),
            (
                f[3].parse::<i32>()?,
                f[4].to_owned(),
                f[5].parse::<usize>()?,
                f[6].to_owned(),
            ),
        );
    }
    let error_text = std::fs::read_to_string(oracle.join("errors.tsv"))?;
    let mut rejected = BTreeMap::new();
    for line in error_text.lines() {
        let f: Vec<_> = line.split('\t').collect();
        rejected.insert(
            (f[0].parse::<u8>()?, u32::from_str_radix(f[1], 16)?),
            (f[2].to_owned(), f[3].parse::<usize>()?, f[4].to_owned()),
        );
    }
    let mut agreed_rejections = 0;
    let mut counts = BTreeMap::<(u8, String), usize>::new();
    let mut failures = Vec::new();
    let mut checked = 0;
    let mut checked_members = 0;
    for (&hash, entry) in &archive.entries {
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let raw = archive.read_stored(hash)?;
            let kind = if raw.starts_with(b"DCX\0") {
                String::from_utf8_lossy(raw.get(40..44).unwrap_or_default()).into_owned()
            } else {
                String::from_utf8_lossy(raw.get(..4).unwrap_or_default()).into_owned()
            };
            *counts.entry((entry.shard, kind)).or_default() += 1;
            let bytes = decoder.decode_if_dcx(&raw)?;
            let want = expected.get(&(entry.shard, hash)).ok_or("missing oracle entry")?;
            assert_eq!(
                (
                    bytes.len(),
                    Sha256::digest(&bytes)
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<String>()
                ),
                *want,
                "{hash:08x}"
            );
            checked += 1;
            if bytes.starts_with(b"BND4") {
                let binder = Binder::read(&bytes, &decoder)?;
                for (i, file) in binder.files.iter().enumerate() {
                    let want = members
                        .get(&(entry.shard, hash, i))
                        .ok_or("missing oracle member")?;
                    assert_eq!(
                        (
                            file.id.unwrap_or(-1),
                            file.name.clone().unwrap_or_default(),
                            file.bytes.len(),
                            Sha256::digest(&file.bytes)
                                .iter()
                                .map(|b| format!("{b:02X}"))
                                .collect::<String>()
                        ),
                        *want,
                        "{hash:08x} member {i}"
                    );
                    checked_members += 1;
                }
            }
            Ok(())
        })();
        match result {
            Err(e) => {
                let equivalent = match (
                    rejected.get(&(entry.shard, hash)),
                    e.downcast_ref::<primase_fromsoft::Error>(),
                ) {
                    (
                        Some((kind, offset, message)),
                        Some(primase_fromsoft::Error::Invalid {
                            format,
                            offset: actual,
                            detail,
                        }),
                    ) => kind == format && offset == actual && message == detail,
                    _ => false,
                };
                if equivalent {
                    agreed_rejections += 1;
                    println!("AGREED REJECTION Data{} {hash:08x}: {e}", entry.shard);
                } else {
                    failures.push(format!("Data{} {hash:08x}: {e}", entry.shard));
                }
            }
            Ok(()) => {
                if rejected.contains_key(&(entry.shard, hash)) {
                    failures.push(format!("{hash:08x}: unexpectedly accepted rejected oracle input"));
                }
            }
        }
    }
    let mut samples = GateReport::new("C# decoded entry bytes", Tolerance::Exact);
    for line in std::fs::read_to_string(oracle.join("samples.tsv"))?.lines() {
        let (hash, path) = line.split_once('\t').ok_or("sample manifest row")?;
        let bytes = archive.read(path, &decoder)?;
        let want = std::fs::read(oracle.join("samples").join(format!("{hash}.bin")))?;
        samples.compare_bytes(path, &bytes, &want);
    }
    println!("{samples}");
    samples.assert_passed();
    println!(
        "Equivalent rejected entries {agreed_rejections}/{}",
        rejected.len()
    );
    assert_eq!(agreed_rejections, rejected.len());
    assert_eq!(checked + agreed_rejections, archive.entries.len());
    for ((shard, kind), count) in counts {
        println!("Data{shard} {kind:?}: {count}");
    }
    println!(
        "entries matched {checked}/{}; members matched {checked_members}/{}; failures {}",
        expected.len(),
        members.len(),
        failures.len()
    );
    for failure in &failures {
        println!("FAIL {failure}");
    }
    assert_eq!(checked, expected.len());
    assert_eq!(checked_members, members.len());
    assert!(failures.is_empty(), "{} archive failures", failures.len());
    Ok(())
}
