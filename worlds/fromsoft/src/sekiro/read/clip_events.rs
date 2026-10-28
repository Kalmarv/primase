//! Read TAE parameter bytes using the credited template for the source game.
//! Smithbox cbd477a8; Vawser, Katalash, Meowmaritus and contributors (MIT;
//! assets/names/SMITHBOX-LICENSE). The templates are read by `fromsoft_formats::tae::template`; this adds the credit,
//! the bundled XML and the primase value types. Unknown fields keep their bytes and a named refusal.
use crate::sekiro::character_data::DecodedEvent;
use fromsoft_formats::Game;
use fromsoft_formats::tae::template::{EventTemplate, Templates, Value};
use primase_core::{EventValue, EventValueData, NameOrigin};
use std::sync::OnceLock;
pub const CREDIT: &str = "Smithbox cbd477a8; Vawser, Katalash, Meowmaritus and contributors (MIT)";

fn templates(game: Game) -> Result<&'static Templates, String> {
    static SDT: OnceLock<Result<Templates, String>> = OnceLock::new();
    static ER: OnceLock<Result<Templates, String>> = OnceLock::new();
    let result = match game {
        Game::Sekiro => SDT.get_or_init(|| {
            Templates::parse(include_str!("../../../assets/tae/TAE.Template.SDT.xml"))
                .map_err(|e| e.to_string())
        }),
        Game::EldenRing => ER.get_or_init(|| {
            Templates::parse(include_str!("../../../assets/tae/TAE.Template.ER.xml"))
                .map_err(|e| e.to_string())
        }),
    };
    result.as_ref().map_err(Clone::clone)
}

/// What the template made of an event's stored bytes.
#[derive(Debug)]
struct Decoded {
    values: Vec<EventValue>,
    /// Set where the record stores fewer bytes than the template's fields need and the fields it lacks are not all
    /// padding: the fields read stay in `values`, this names the ones not read.
    shorter: Option<String>,
    /// The float fields that store a word that is not a finite float (the template types them as floats; the installs
    /// store -1 and other negative integers there): kept as their four bytes, named here.
    not_floats: Vec<String>,
}

/// Sequential little-endian scalar fields exactly as the template states them. Anonymous fields get a structural
/// field_N label; duplicate source labels remain duplicates. A record that stores fewer bytes than the template's fields
/// need (1.5% of the stock events: 4,627 of Sekiro's, 268 of Elden Ring's) keeps the fields it stores, and says which it
/// lacks unless they are zero-asserted or padding. A float field whose word is not a finite float (events 708, 730 and 156
/// of Sekiro: 5,405 fields) keeps the four bytes and says so. An asserted-against field, a boolean that is neither 0 nor
/// 1 or non-zero bytes after the template's fields refuse all the typed values together; bytes survive.
fn values(template: &EventTemplate, bytes: &[u8]) -> Result<Decoded, String> {
    let decoded = template.decode(bytes).map_err(|e| e.to_string())?;
    if decoded.trailing_nonzero {
        return Err(format!(
            "{} trailing bytes after template fields contain nonzero data",
            bytes.len().saturating_sub(template.size())
        ));
    }
    let label =
        |index: usize, name: &Option<String>| name.clone().unwrap_or_else(|| format!("field_{index}"));
    let mut values = Vec::with_capacity(decoded.arguments.len());
    let mut not_floats = Vec::new();
    for a in &decoded.arguments {
        let name = label(a.index, &a.field.name);
        let value = match &a.value {
            Value::Bool(b) => EventValueData::Bool(*b),
            Value::Int(v) => EventValueData::I64(*v),
            Value::Float(f) => EventValueData::F64(*f),
            Value::Unsigned(u) => EventValueData::Bytes(u.to_le_bytes().to_vec()),
            Value::Bytes(b) => EventValueData::Bytes(b.clone()),
            Value::Word(w) => {
                not_floats.push(format!("{name} ({w:#010x})"));
                EventValueData::Bytes(w.to_le_bytes().to_vec())
            }
        };
        values.push(EventValue { name, value });
    }
    let first = decoded.arguments.len();
    let unread: Vec<String> = decoded
        .missing
        .iter()
        .enumerate()
        .filter(|(_, f)| !f.is_padding())
        .map(|(k, f)| label(first + k, &f.name))
        .collect();
    let shorter = (!unread.is_empty()).then(|| {
        format!(
            "stored shorter than the community template ({} bytes); fields not read: {}",
            bytes.len(),
            unread.join(", ")
        )
    });
    Ok(Decoded {
        values,
        shorter,
        not_floats,
    })
}

/// One typed descriptor per raw event, in identical order. Event 198's observed structural
/// fields are appended without guessing its transition envelope.
pub fn decode(game: Game, events: &[fromsoft_formats::tae::Event]) -> Vec<DecodedEvent> {
    events
        .iter()
        .map(|event| {
            let mut out = DecodedEvent::default();
            match templates(game) {
                Ok(templates) => match templates.get(event.kind) {
                    Some(template) => {
                        out.name = template.name.clone();
                        out.naming = out.name.as_ref().map(|_| NameOrigin::Community {
                            credit: CREDIT.into(),
                        });
                        match values(template, &event.parameters) {
                            Ok(decoded) => {
                                out.values = decoded.values;
                                out.refusals.extend(decoded.shorter);
                                if !decoded.not_floats.is_empty() {
                                    out.refusals.push(format!(
                                        "float fields that store a word that is not a float, kept as bytes: {}",
                                        decoded.not_floats.join(", ")
                                    ));
                                }
                            }
                            Err(reason) => out.refusals.push(reason),
                        }
                    }
                    None => out.refusals.push("no community parameter template".into()),
                },
                Err(reason) => out
                    .refusals
                    .push(format!("community template not read: {reason}")),
            }
            if event.kind == 198 {
                let (fields, refusal) = super::material_events::event_198(game, &event.parameters);
                out.values.extend(fields);
                out.refusals.extend(refusal);
            }
            out
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn parse(xml: &str) -> Templates {
        Templates::parse(xml).unwrap()
    }

    #[test]
    fn template_fields_keep_order_duplicate_names_and_scalar_widths() {
        let parsed = parse(
            r#"<event_template><event id="7" Name="A &amp; B"><s32 name="v"><entry value="0" name="zero"/></s32><f32 name="v"/><b/><s64 name="wide"/></event></event_template>"#,
        );
        let t = parsed.get(7).unwrap();
        assert_eq!(t.name.as_deref(), Some("A & B"));
        let bytes = [
            (-2i32).to_le_bytes().to_vec(),
            0.5f32.to_le_bytes().to_vec(),
            vec![1],
            (-9i64).to_le_bytes().to_vec(),
            vec![0; 3],
        ]
        .concat();
        let fields = values(t, &bytes).unwrap().values;
        assert_eq!(
            fields.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
            ["v", "v", "field_2", "wide"]
        );
        assert_eq!(fields[0].value, EventValueData::I64(-2));
        assert_eq!(fields[1].value, EventValueData::F64(0.5));
        assert_eq!(fields[2].value, EventValueData::Bool(true));
        assert_eq!(fields[3].value, EventValueData::I64(-9));
        // a record that stores only the first field keeps it and names the ones it lacks
        let short = values(t, &bytes[..4]).unwrap();
        assert_eq!(short.values.len(), 1);
        assert_eq!(short.values[0].value, EventValueData::I64(-2));
        let note = short.shorter.unwrap();
        assert!(
            note.contains("stored shorter than the community template"),
            "{note}"
        );
        assert!(note.ends_with("fields not read: v, field_2, wide"), "{note}");
        // an unknown field type is refused when the template is read
        assert!(
            Templates::parse(
                r#"<event_template><event id="8"><s32 name="a"/><dummy/></event></event_template>"#
            )
            .is_err()
        );
    }

    /// The fields a record lacks are not reported when they are asserted zero or named padding; a stored value that
    /// differs from an asserted one is a named refusal.
    #[test]
    fn padding_and_asserted_zero_fields_may_be_missing_and_assertions_are_enforced() {
        let parsed = parse(
            r#"<event_template><event id="9"><s32 name="a"/><s32 assert="0"/><u8 name="Padding1"/><s32 name="b"/></event><event id="10"><s32 name="a"/><s16 assert="0"/><s32 assert="0"/></event></event_template>"#,
        );
        // 9: a, then the asserted-zero field, the padding and `b` missing: b is reported
        let d = values(parsed.get(9).unwrap(), &[1, 0, 0, 0]).unwrap();
        assert_eq!(d.values.len(), 1);
        assert!(d.shorter.unwrap().ends_with("fields not read: b"));
        // 10: everything missing is asserted zero: nothing to report
        let d = values(parsed.get(10).unwrap(), &[7, 0, 0, 0]).unwrap();
        assert_eq!((d.values.len(), d.shorter), (1, None));
        // the whole record stored: the asserted value is read and held to the assertion
        let d = values(parsed.get(10).unwrap(), &[7, 0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!((d.values.len(), d.shorter), (3, None));
        let e = values(parsed.get(10).unwrap(), &[7, 0, 0, 0, 1, 0, 0, 0, 0, 0]).unwrap_err();
        assert!(e.contains("asserts 0") && e.contains("Int(1)"), "{e}");
        // a partial field counts as missing
        let d = values(parsed.get(10).unwrap(), &[7, 0, 0, 0, 0]).unwrap();
        assert_eq!((d.values.len(), d.shorter), (1, None));
    }

    /// A float field that stores a word that is not a finite float (-1 in Sekiro's event 708) keeps its four bytes and is
    /// named; the fields around it are still read.
    #[test]
    fn a_float_field_that_stores_an_integer_is_kept_as_bytes() {
        let parsed = parse(
            r#"<event_template><event id="708"><s32 name="a"/><f32 name="b"/><f32 name="c"/></event></event_template>"#,
        );
        let bytes = [7i32.to_le_bytes(), u32::MAX.to_le_bytes(), 0.25f32.to_le_bytes()].concat();
        let d = values(parsed.get(708).unwrap(), &bytes).unwrap();
        assert_eq!(d.values[0].value, EventValueData::I64(7));
        assert_eq!(d.values[1].value, EventValueData::Bytes(vec![255; 4]));
        assert_eq!(d.values[2].value, EventValueData::F64(0.25));
        assert_eq!(d.not_floats, ["b (0xffffffff)"]);
    }

    #[test]
    fn bundled_templates_cover_every_authored_event_without_inventing_a_sekiro_198() {
        for (game, xml, count, has_198) in [
            (
                Game::Sekiro,
                include_str!("../../../assets/tae/TAE.Template.SDT.xml"),
                190,
                false,
            ),
            (
                Game::EldenRing,
                include_str!("../../../assets/tae/TAE.Template.ER.xml"),
                156,
                true,
            ),
        ] {
            let authored = xml
                .lines()
                .filter(|line| line.trim_start().starts_with("<event "))
                .count();
            assert_eq!(authored, count, "{game:?}: bundled XML event count changed");
            let parsed = templates(game).unwrap();
            assert_eq!(
                parsed.len(),
                authored,
                "{game:?}: every XML event must have a template"
            );
            assert_eq!(
                parsed.get(198).is_some(),
                has_198,
                "{game:?}: event 198 exists only where authored"
            );
        }
    }

    #[test]
    fn reading_keeps_unknown_rows_and_material_structural_facts() {
        let events = vec![
            fromsoft_formats::tae::Event {
                kind: 987654,
                unk04: 0,
                start: 0.,
                end: 1.,
                parameters: vec![4, 5],
            },
            fromsoft_formats::tae::Event {
                kind: 198,
                unk04: 0,
                start: 0.,
                end: 1.,
                parameters: vec![0; 64],
            },
        ];
        let typed = decode(Game::EldenRing, &events);
        assert_eq!(typed.len(), 2);
        assert!(!typed[0].refusals.is_empty());
        assert!(
            typed[1]
                .values
                .iter()
                .any(|value| value.name == "material_ex.row_id")
        );
        assert_eq!(events[0].parameters, [4, 5]);
    }
}
