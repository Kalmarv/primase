//! The state axes the `gta.*` dialect defines: `gta.hour` ([`hour_axis`]) and GTA's script-managed maps (IPLs).
//!
//! IPLs are the maps the game streams only when a script requests them (`CMapData` flags bit 0). The reader makes each
//! a `gta.ipl.<map name>` state axis without a default: whether the game's start scripts load it is in those scripts
//! (the building controller's state table, `REQUEST_IPL` calls), which are not read yet, so the start state is
//! unknown and a program that wants one assigns it. [`ipl_base`]: a map's group name, from the streaming suffixes
//! GTA's map names carry.
use primase_core::World;
use primase_core::state::{Domain, Scope, StateAxis};

/// The global `gta.hour` axis of `w` ({0..23}, value h is hour h, no default, class `time`): the one there, or a new
/// one. The reader makes it for time archetypes; a pass that keeps the hour as state (lights by their hours) makes it
/// when the region has none.
pub fn hour_axis(w: &mut World) -> primase_core::Entity {
    if let Some((e, _)) = w.iter::<StateAxis>().find(|(_, a)| a.key == "gta.hour") {
        return e;
    }
    let e = w.spawn();
    w.insert(
        e,
        StateAxis {
            key: "gta.hour".into(),
            domain: Domain::Enum((0..24).map(|h| h.to_string()).collect()),
            default: None,
            scope: Scope::Global,
            class: "time".into(),
            source: "the game's hours: time archetypes' timeFlags (CTimeArchetypeDef), lights' time_flags"
                .into(),
        },
    );
    e
}

/// The group name of map `name`: the streaming suffixes GTA's map names carry after their group's (`_strm_3`, `_long_0`,
/// `_lod`) stripped, a numeric part then a known suffix, up to four times.
pub fn ipl_base(name: &str) -> &str {
    const SUFFIXES: [&str; 9] = [
        "_lod",
        "_slod",
        "_slod2",
        "_strm",
        "_long",
        "_critical",
        "_occl",
        "_lodlights",
        "_distantlights",
    ];
    let mut n = name;
    for _ in 0..4 {
        if let Some((head, tail)) = n.rsplit_once('_')
            && !tail.is_empty()
            && tail.bytes().all(|b| b.is_ascii_digit())
        {
            n = head;
        }
        match SUFFIXES.iter().find(|s| n.ends_with(*s)) {
            Some(s) => n = &n[..n.len() - s.len()],
            None => break,
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_names_lose_their_streaming_suffixes() {
        assert_eq!(ipl_base("bh1_16_original_strm_3"), "bh1_16_original");
        assert_eq!(ipl_base("apa_stilt_ch2_04_ext1"), "apa_stilt_ch2_04_ext1");
        assert_eq!(ipl_base("prologue01_lod"), "prologue01");
        assert_eq!(ipl_base("bh1_16_refurb_lod"), "bh1_16_refurb");
    }
}
