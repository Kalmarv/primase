//! Read TAE parameter bytes using the credited template for the source game.
//! Smithbox cbd477a8; Vawser, Katalash, Meowmaritus and contributors (MIT;
//! assets/names/SMITHBOX-LICENSE). Unknown fields keep their bytes and a named refusal.
use crate::sekiro::character_data::DecodedEvent;
use fromsoft_formats::Game;
use primase_core::{EventValue, EventValueData, NameOrigin};
use std::collections::BTreeMap;
use std::sync::OnceLock;
pub const CREDIT: &str = "Smithbox cbd477a8; Vawser, Katalash, Meowmaritus and contributors (MIT)";
#[derive(Clone, Debug)]
struct Field {
    kind: String,
    name: String,
}
#[derive(Clone, Debug)]
struct Template {
    name: Option<String>,
    fields: Vec<Field>,
}

fn attrs(tag: &quick_xml::events::BytesStart<'_>) -> Result<BTreeMap<String, String>, String> {
    tag.attributes()
        .map(|attribute| {
            let attribute = attribute.map_err(|e| e.to_string())?;
            let key = attribute.key.as_ref().to_ascii_lowercase();
            // Bundled templates use XML 1.0; absent declarations have the same normalization.
            let value = attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(|e| e.to_string())?
                .into_owned();
            Ok((key, value))
        })
        .collect()
}

fn parse(xml: &str) -> Result<BTreeMap<i64, Template>, String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut templates = BTreeMap::new();
    let mut current: Option<(i64, Template)> = None;
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(tag) | Event::Empty(tag) => {
                let kind = tag.name().as_ref().to_owned();
                if kind == "event" {
                    if let Some((id, template)) = current.take() {
                        templates.entry(id).or_insert(template);
                    }
                    let attributes = attrs(&tag)?;
                    let id = attributes
                        .get("id")
                        .ok_or("template event has no id")?
                        .parse::<i64>()
                        .map_err(|e| e.to_string())?;
                    let name = attributes
                        .get("name")
                        .filter(|name| {
                            !name.trim().is_empty()
                                && !name
                                    .strip_prefix("Event ")
                                    .is_some_and(|id| id.trim().parse::<i64>().is_ok())
                        })
                        .cloned();
                    current = Some((
                        id,
                        Template {
                            name,
                            fields: Vec::new(),
                        },
                    ));
                } else if kind != "entry"
                    && kind != "event_template"
                    && let Some((_, template)) = &mut current
                {
                    let attributes = attrs(&tag)?;
                    let name = attributes
                        .get("name")
                        .filter(|name| !name.trim().is_empty())
                        .cloned()
                        .unwrap_or_else(|| format!("field_{}", template.fields.len()));
                    template.fields.push(Field { kind, name });
                }
            }
            Event::End(tag) if tag.name().as_ref() == "event" => {
                if let Some((id, template)) = current.take() {
                    templates.entry(id).or_insert(template);
                }
            }
            Event::Eof => break,
            Event::End(_)
            | Event::Text(_)
            | Event::Comment(_)
            | Event::Decl(_)
            | Event::CData(_)
            | Event::PI(_)
            | Event::DocType(_)
            | Event::GeneralRef(_) => {}
        }
    }
    if let Some((id, template)) = current {
        templates.entry(id).or_insert(template);
    }
    Ok(templates)
}

fn templates(game: Game) -> Result<&'static BTreeMap<i64, Template>, String> {
    static SDT: OnceLock<Result<BTreeMap<i64, Template>, String>> = OnceLock::new();
    static ER: OnceLock<Result<BTreeMap<i64, Template>, String>> = OnceLock::new();
    let result = match game {
        Game::Sekiro => SDT.get_or_init(|| parse(include_str!("../../../assets/tae/TAE.Template.SDT.xml"))),
        Game::EldenRing => ER.get_or_init(|| parse(include_str!("../../../assets/tae/TAE.Template.ER.xml"))),
    };
    result.as_ref().map_err(Clone::clone)
}

/// Sequential little-endian scalar fields exactly as the template states them. Anonymous
/// fields get a structural field_N label; duplicate source labels remain duplicates. All
/// typed values are refused together on a truncated/unknown/non-finite field; bytes survive.
fn values(template: &Template, bytes: &[u8]) -> Result<Vec<EventValue>, String> {
    let mut offset = 0usize;
    let mut out = Vec::new();
    for field in &template.fields {
        let width = match field.kind.as_str() {
            "b" | "u8" | "s8" => 1,
            "u16" | "s16" => 2,
            "u32" | "s32" | "f32" => 4,
            "s64" => 8,
            kind => {
                return Err(format!(
                    "field {:?}: template type {kind} is not decoded",
                    field.name
                ));
            }
        };
        let b = bytes.get(offset..offset + width).ok_or_else(|| {
            format!(
                "field {:?}: needs {width} bytes at {offset}, record has {}",
                field.name,
                bytes.len()
            )
        })?;
        let value = match field.kind.as_str() {
            "b" => match b[0] {
                0 => EventValueData::Bool(false),
                1 => EventValueData::Bool(true),
                v => return Err(format!("field {:?}: boolean byte {v} is not 0/1", field.name)),
            },
            "u8" => EventValueData::I64(i64::from(b[0])),
            "s8" => EventValueData::I64(i64::from(b[0] as i8)),
            "u16" => EventValueData::I64(i64::from(u16::from_le_bytes([b[0], b[1]]))),
            "s16" => EventValueData::I64(i64::from(i16::from_le_bytes([b[0], b[1]]))),
            "u32" => EventValueData::I64(i64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))),
            "s32" => EventValueData::I64(i64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))),
            "s64" => EventValueData::I64(i64::from_le_bytes(b.try_into().map_err(|_| "s64 width")?)),
            "f32" => {
                let v = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                if !v.is_finite() {
                    return Err(format!("field {:?}: non-finite float", field.name));
                }
                EventValueData::F64(f64::from(v))
            }
            kind => {
                return Err(format!(
                    "field {:?}: template type {kind} is not decoded",
                    field.name
                ));
            }
        };
        out.push(EventValue {
            name: field.name.clone(),
            value,
        });
        offset += width;
    }
    if bytes[offset..].iter().any(|b| *b != 0) {
        return Err(format!(
            "{} trailing bytes after template fields contain nonzero data",
            bytes.len() - offset
        ));
    }
    Ok(out)
}

/// One typed descriptor per raw event, in identical order. Event 198's observed structural
/// fields are appended without guessing its transition envelope.
pub fn decode(game: Game, events: &[fromsoft_formats::tae::Event]) -> Vec<DecodedEvent> {
    events
        .iter()
        .map(|event| {
            let mut out = DecodedEvent::default();
            match templates(game) {
                Ok(templates) => match templates.get(&i64::from(event.kind)) {
                    Some(template) => {
                        out.name = template.name.clone();
                        out.naming = out.name.as_ref().map(|_| NameOrigin::Community {
                            credit: CREDIT.into(),
                        });
                        match values(template, &event.parameters) {
                            Ok(values) => out.values = values,
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
    #[test]
    fn template_fields_keep_order_duplicate_names_and_scalar_widths() {
        let parsed=parse(r#"<event_template><event id="7" Name="A &amp; B"><s32 name="v"><entry value="0" name="zero"/></s32><f32 name="v"/><b/><s64 name="wide"/></event></event_template>"#).unwrap();
        let t = &parsed[&7];
        assert_eq!(t.name.as_deref(), Some("A & B"));
        let bytes = [
            (-2i32).to_le_bytes().to_vec(),
            0.5f32.to_le_bytes().to_vec(),
            vec![1],
            (-9i64).to_le_bytes().to_vec(),
            vec![0; 3],
        ]
        .concat();
        let fields = values(t, &bytes).unwrap();
        assert_eq!(
            fields.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
            ["v", "v", "field_2", "wide"]
        );
        assert_eq!(fields[0].value, EventValueData::I64(-2));
        assert_eq!(fields[1].value, EventValueData::F64(0.5));
        assert_eq!(fields[2].value, EventValueData::Bool(true));
        assert_eq!(fields[3].value, EventValueData::I64(-9));
        assert!(values(t, &bytes[..4]).is_err());
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
                parsed.contains_key(&198),
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
