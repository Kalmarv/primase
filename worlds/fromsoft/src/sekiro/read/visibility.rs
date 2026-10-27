//! Reading TAE 233/711/713 mask fields for Sekiro and Elden Ring. Both games' templates
//! agree. Semantics checked against DSAnimStudio f1bff06c TaeActionSimulationEnvironment.cs
//! (GPL-3.0, reference only; no code copied). No seek/playback evaluator lives in the reader.
use crate::components::visibility::{ClipVisibility, MaskChange};
use fromsoft_formats::tae::Event;

pub fn decode(character: &str, events: &[Event]) -> ClipVisibility {
    let mut out = ClipVisibility {
        character: character.into(),
        ..Default::default()
    };
    for event in events
        .iter()
        .filter(|event| matches!(event.kind, 233 | 711 | 713))
    {
        let Some(bytes) = event.parameters.get(..32) else {
            out.refusals.push(format!(
                "TAE {} at {}: {} bytes, needs 32 mask bytes",
                event.kind,
                event.start,
                event.parameters.len()
            ));
            continue;
        };
        // Keep the raw event, but refuse behavior if an unexplained tail carries information.
        if event.parameters[32..].iter().any(|byte| *byte != 0) {
            out.refusals.push(format!(
                "TAE {} at {}: nonzero unknown bytes after the 32 mask groups",
                event.kind, event.start
            ));
            continue;
        }
        let mut change = MaskChange {
            kind: event.kind,
            start: event.start,
            end: event.end,
            show: 0,
            hide: 0,
            latched: event.kind == 233,
        };
        let mut valid = true;
        for (group, &value) in bytes.iter().enumerate() {
            match (event.kind, value) {
                (233, 0) | (711, 1) => change.hide |= 1 << group,
                (233 | 713, 1) => change.show |= 1 << group,
                (233, 255) | (711 | 713, 0) => {}
                _ => {
                    valid = false;
                    out.refusals.push(format!(
                        "TAE {} at {} group {group}: unsupported value {value}",
                        event.kind, event.start
                    ));
                }
            }
        }
        if valid {
            out.changes.push(change);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(kind: i32, start: f32, end: f32, group: usize, value: u8) -> Event {
        let mut parameters = vec![if kind == 233 { 255 } else { 0 }; 32];
        parameters[group] = value;
        Event {
            kind,
            start,
            end,
            unk04: 0,
            parameters,
        }
    }
    /// Hand-worked masks: 233 has three states; 711/713 only modify bytes set to one.
    #[test]
    fn decoded_mask_facts_keep_source_priority_and_latched_flag() {
        let decoded = decode(
            "c1010",
            &[
                event(233, 0., 0., 3, 1),
                event(711, 1., 2., 3, 1),
                event(713, 1.5, 1.75, 3, 1),
            ],
        );
        assert!(decoded.refusals.is_empty());
        assert_eq!(decoded.changes.len(), 3);
        assert_eq!(
            (
                decoded.changes[0].show,
                decoded.changes[0].hide,
                decoded.changes[0].latched
            ),
            (1 << 3, 0, true)
        );
        assert_eq!(
            (
                decoded.changes[1].show,
                decoded.changes[1].hide,
                decoded.changes[1].latched
            ),
            (0, 1 << 3, false)
        );
        assert_eq!(
            (
                decoded.changes[2].show,
                decoded.changes[2].hide,
                decoded.changes[2].latched
            ),
            (1 << 3, 0, false)
        );
        assert_eq!(decoded.changes[2].end, 1.75);
    }
    #[test]
    fn unknown_short_and_nonzero_tail_records_are_retained_as_named_refusals() {
        assert!(decode("c1010", &[event(233, 0., 1., 2, 2)]).changes.is_empty());
        let mut short = event(233, 0., 1., 2, 1);
        short.parameters.truncate(31);
        assert!(decode("c1010", &[short]).refusals[0].contains("needs 32"));
        let mut tail = event(713, 0., 1., 2, 1);
        tail.parameters.push(1);
        assert!(decode("c1010", &[tail]).refusals[0].contains("unknown bytes"));
    }
    #[test]
    fn imported_records_keep_mask_facts_and_import_cycles_are_refused() {
        use fromsoft_formats::tae::{Animation, Header, Tae};
        let own = Animation {
            id: 1,
            header: Header::None,
            file_name: None,
            events: vec![event(233, 0., 1., 3, 1)],
        };
        let alias = Animation {
            id: 2,
            header: Header::ImportOther { from: 1, unknown: 0 },
            file_name: None,
            events: vec![],
        };
        let mut tae = Tae {
            id: 1,
            event_bank: 0,
            skeleton_name: None,
            sib_name: None,
            group: 0,
            animations: vec![own, alias],
        };
        let inherited = tae.resolved_animation(2).unwrap().unwrap();
        assert_eq!(decode("c1010", &inherited.events).changes[0].show, 1 << 3);
        tae.animations[0].header = Header::ImportOther { from: 2, unknown: 0 };
        assert!(tae.resolved_animation(2).is_err());
        tae.animations[0].header = Header::ImportOther { from: 3, unknown: 0 };
        assert!(tae.resolved_animation(2).is_err());
    }
}
