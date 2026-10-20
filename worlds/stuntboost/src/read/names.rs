//! STUNTBOOST's node names, parsed by the grammar its Blender exporter documents (StuntboostTools,
//! `bake_v3/README.md`, "Current Object Naming Syntax"):
//!
//! `{hide}{name}{type}{=Class(args){.Method(args)...}}{.NNN}`, every part optional:
//!
//! - hide: `/` hidden before export, `//` excluded from bake and export (neither normally reaches the game);
//! - type, the last characters of the part before `=`, in this order: `_` invisible in game and bake; `*` visible in
//!   the bake but not baked to, or `°` invisible in the bake; `<` solid collision, or `#` a trigger;
//! - code after the first `=`: C#-like calls without spaces, a class then property setters or methods, each `Name`
//!   or `Name(args)` with arguments split at top-level commas; no class (code starting with `.`, or none) means
//!   `Entity()`;
//! - `.NNN` (three digits) at the very end: Blender's duplicate suffix, ignored.
//!
//! Parsing never fails: a name is what it says; what a game makes of an unknown class is the program's.

use crate::components::{
    StuntboostBake, StuntboostCall, StuntboostCollision, StuntboostHide, StuntboostName,
};

/// `name` parsed (see the module documentation).
pub fn parse(raw: &str) -> StuntboostName {
    // the duplicate suffix: a dot and three digits at the very end
    let (body, duplicate) = match raw.char_indices().rev().nth(3) {
        Some((i, '.')) if raw.len() - i == 4 && raw[i + 1..].bytes().all(|b| b.is_ascii_digit()) => {
            (&raw[..i], raw[i + 1..].parse::<u32>().ok())
        }
        _ => (raw, None),
    };
    let (hide, body) = if let Some(rest) = body.strip_prefix("//") {
        (StuntboostHide::Excluded, rest)
    } else if let Some(rest) = body.strip_prefix('/') {
        (StuntboostHide::BeforeExport, rest)
    } else {
        (StuntboostHide::None, body)
    };
    let (head, code) = match body.split_once('=') {
        Some((h, c)) => (h, Some(c)),
        None => (body, None),
    };
    // the type characters, from the end: collision, then bake, then visibility
    let mut name = head;
    let collision = if let Some(n) = name.strip_suffix('<') {
        name = n;
        StuntboostCollision::Solid
    } else if let Some(n) = name.strip_suffix('#') {
        name = n;
        StuntboostCollision::Trigger
    } else {
        StuntboostCollision::None
    };
    let bake = if let Some(n) = name.strip_suffix('*') {
        name = n;
        StuntboostBake::NotBakedTo
    } else if let Some(n) = name.strip_suffix('°') {
        name = n;
        StuntboostBake::HiddenInBake
    } else {
        StuntboostBake::Baked
    };
    let invisible = match name.strip_suffix('_') {
        Some(n) => {
            name = n;
            true
        }
        None => false,
    };
    let calls = code.map(calls).unwrap_or_default();
    let (class, methods) = match code {
        Some(c) if !c.starts_with('.') && !calls.is_empty() => {
            let mut it = calls.into_iter();
            (it.next(), it.collect())
        }
        _ => (None, calls),
    };
    StuntboostName {
        raw: raw.to_owned(),
        hide,
        name: name.to_owned(),
        invisible,
        bake,
        collision,
        class,
        methods,
        duplicate,
    }
}

/// The calls of a code part: `A(x,y).B.C(z)` -> A [x, y], B (no parentheses), C [z]; a dot inside parentheses does not
/// split.
fn calls(code: &str) -> Vec<StuntboostCall> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let bytes: Vec<(usize, char)> = code.char_indices().collect();
    let mut pieces = Vec::new();
    for &(i, ch) in &bytes {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '.' if depth == 0 => {
                pieces.push(&code[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    pieces.push(&code[start..]);
    for piece in pieces.into_iter().filter(|p| !p.is_empty()) {
        out.push(match piece.split_once('(') {
            Some((name, rest)) => StuntboostCall {
                name: name.to_owned(),
                args: Some(arguments(rest.strip_suffix(')').unwrap_or(rest))),
            },
            None => StuntboostCall {
                name: piece.to_owned(),
                args: None,
            },
        });
    }
    out
}

/// Arguments split at top-level commas, each trimmed: `Vector3(1,0,0),x` -> [`Vector3(1,0,0)`, `x`]; `( )` -> [``].
fn arguments(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(text[start..i].trim().to_owned());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(text[start..].trim().to_owned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: &[&str]) -> StuntboostCall {
        StuntboostCall {
            name: name.into(),
            args: Some(args.iter().map(|a| a.to_string()).collect()),
        }
    }

    /// The names the game's levels use, parsed part by part.
    #[test]
    fn the_levels_names_parse_by_the_exporters_grammar() {
        let n = parse("plant_pot_high_<.001");
        assert_eq!(
            (n.name.as_str(), n.invisible, n.collision, n.duplicate),
            ("plant_pot_high", true, StuntboostCollision::Solid, Some(1))
        );
        assert!(n.class.is_none() && n.methods.is_empty());

        let n = parse("_#=Checkpoint(Mandatory).003");
        assert_eq!((n.name.as_str(), n.invisible), ("", true));
        assert_eq!(n.collision, StuntboostCollision::Trigger);
        assert_eq!(n.class, Some(call("Checkpoint", &["Mandatory"])));
        assert_eq!(n.duplicate, Some(3));

        let n = parse("=Spawn(,22).Grind()");
        assert_eq!(n.class, Some(call("Spawn", &["", "22"])));
        assert_eq!(n.methods, [call("Grind", &[""])]);

        let n = parse("walls°=BackfaceShadowCaster().DynamicShadow( )");
        assert_eq!((n.name.as_str(), n.bake), ("walls", StuntboostBake::HiddenInBake));
        assert_eq!(n.methods, [call("DynamicShadow", &[""])]);

        let n = parse("_<=BoomboxButton");
        assert_eq!(
            n.class,
            Some(StuntboostCall {
                name: "BoomboxButton".into(),
                args: None
            })
        );

        let n = parse("=GoalCameraPoints().Levels(Castle,TrailerCastle)");
        assert_eq!(n.methods, [call("Levels", &["Castle", "TrailerCastle"])]);

        let n = parse("x=Thing().Move(Vector3(1,0,0)).Disable()");
        assert_eq!(
            n.methods,
            [call("Move", &["Vector3(1,0,0)"]), call("Disable", &[""])]
        );

        // a code part starting with a method: no class (Entity())
        let n = parse("box=.Disable()");
        assert!(n.class.is_none());
        assert_eq!(n.methods, [call("Disable", &[""])]);

        // hide prefixes; a name without code keeps its dots when they are not a three-digit suffix
        assert_eq!(parse("//old_wall").hide, StuntboostHide::Excluded);
        assert_eq!(parse("/helper*").bake, StuntboostBake::NotBakedTo);
        assert_eq!(parse("v1.5_ramp").name, "v1.5_ramp");
        assert_eq!(parse("=Skybox().001").class, Some(call("Skybox", &[""])));
    }
}
