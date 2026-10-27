//! The repair of the appearance presets Studio writes for a costume.
//!
//! Studio's preset lists the slot of its last boxed parameter value, a pointer to an import, in the document's
//! type-info fixups as well as its import fixups (ReSkate Studio 2026-10-12, every preset of every costume seen). A
//! slot is one or the other: the game's own presets never list a slot in both, and a boxed value of pointer type is
//! listed among the imports only. The value stored there is a valid import pointer (odd, its index an import of the
//! document), so the type-info listing is the error: [`repair_slots`] removes it, nothing else.

use frostbite_formats::ebx::{Document, FieldType};
use frostbite_formats::fbmod::Fbmod;
use primase_foundations::compress::oodle::Oodle;

use super::{Result, costume_presets, set_document};

/// One preset repaired: its name and the slots taken out of its type-info fixups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repaired {
    pub preset: String,
    pub slots: Vec<u32>,
}

/// The type-info fixups of `doc` that are boxed values of pointer type also listed as imports, holding a valid import
/// pointer: removed from the type-info fixups and returned.
pub fn repair_slots(doc: &mut Document) -> Vec<u32> {
    let pointers: Vec<u32> = doc
        .boxed_values
        .iter()
        .filter(|b| FieldType::from_bits((b.encoded_type >> 5) & 0x1F) == FieldType::Pointer)
        .map(|b| b.offset)
        .filter(|o| doc.import_offsets.contains(o) && doc.type_info_offsets.contains(o))
        .filter(|&o| {
            let at = o as usize;
            doc.payload
                .get(at..at + 4)
                .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .is_some_and(|v| v & 1 == 1 && usize::try_from(v >> 1).is_ok_and(|i| i < doc.imports.len()))
        })
        .collect();
    doc.type_info_offsets.retain(|o| !pointers.contains(o));
    pointers
}

/// Repair every preset of the costume (see the module documentation); the presets that needed it, with their slots.
pub fn repair(m: &mut Fbmod, oodle: Option<&Oodle>) -> Result<Vec<Repaired>> {
    let mut out = Vec::new();
    for (i, mut doc) in costume_presets(m, oodle)? {
        let slots = repair_slots(&mut doc);
        if slots.is_empty() {
            continue;
        }
        set_document(m, i, &doc)?;
        out.push(Repaired {
            preset: m.entries[i].name.clone(),
            slots,
        });
    }
    m.drop_unused_payloads();
    Ok(out)
}
