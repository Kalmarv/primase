//! STUNTBOOST's node names, parsed by the rules the game itself applies (its pipeline's `ParsedName`, which the
//! exporter's grammar, StuntboostTools `bake_v3/README.md`, describes):
//!
//! - With an `=`, the name is what precedes the first `=` and the code is the rest from the `=` on, kept whole (a
//!   Blender `.NNN` suffix after the code stays part of it). Without one, the name ends at its first `.` (what follows,
//!   normally Blender's `.NNN`, is dropped).
//! - The type characters are read backwards from the end of the node name (the name up to its first `.`), any order,
//!   until another character: `_` invisible (in game and in the bake), `*` visible in the bake but not baked to, `°`
//!   invisible in the bake, `<` solid collision, `#` a trigger; as many characters as were read come off the end of the
//!   name. When the same kind appears twice, the one nearer the start wins. An invisible node takes no part in the bake.
//! - The code is C#-like calls without spaces: a class then property setters or methods, each `Name` or `Name(args)`
//!   with arguments split at top-level commas; no class (code starting with `=.`, or none) means `Entity()`. For the
//!   calls, a trailing `.NNN` (three digits) is Blender's duplicate suffix and is not a call.
//!
//! Parsing never fails: a name is what it says; what a game makes of an unknown class is the program's.

use super::{StuntboostBake, StuntboostCall, StuntboostCollision, StuntboostName};

/// The text up to the first `c`, or all of it.
fn before(s: &str, c: char) -> &str {
    s.find(c).map_or(s, |i| &s[..i])
}

/// A trailing `.NNN` (three digits): the text before it and the number.
fn duplicate_suffix(s: &str) -> Option<(&str, u32)> {
    let (head, digits) = s.rsplit_once('.')?;
    (digits.len() == 3 && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| (head, digits.parse().unwrap_or(0)))
}

/// `name` parsed (see the module documentation).
pub fn parse(raw: &str) -> StuntboostName {
    let (mut inner, code, dropped) = match raw.find('=') {
        Some(i) => (&raw[..i], &raw[i..], None),
        None => match raw.find('.') {
            Some(i) => (&raw[..i], "", Some(&raw[i..])),
            None => (raw, "", None),
        },
    };
    // the type characters, backwards from the end of the node name (the name up to its first '.')
    let (mut invisible, mut bake, mut collision) = (false, StuntboostBake::Baked, StuntboostCollision::None);
    let mut count = 0usize;
    for ch in before(inner, '.').chars().rev() {
        match ch {
            '_' => invisible = true,
            '°' => bake = StuntboostBake::HiddenInBake,
            '*' => bake = StuntboostBake::NotBakedTo,
            '<' => collision = StuntboostCollision::Solid,
            '#' => collision = StuntboostCollision::Trigger,
            _ => break,
        }
        count += 1;
    }
    // as many characters come off the end of the name (not bytes: a cut inside a character would drop the name)
    if let Some(last) = count.checked_sub(1)
        && let Some((cut, _)) = inner.char_indices().rev().nth(last)
    {
        inner = &inner[..cut];
    }
    if invisible {
        bake = StuntboostBake::HiddenInBake;
    }
    // the calls: the code without its '=' and without a duplicate suffix
    let (body, duplicate) = match code.strip_prefix('=') {
        Some(body) => match duplicate_suffix(body) {
            Some((head, n)) if !head.is_empty() => (head, Some(n)),
            _ => (body, None),
        },
        None => ("", dropped.and_then(|d| duplicate_suffix(d).map(|(_, n)| n))),
    };
    let calls = calls(body);
    let (class, methods) = if !body.is_empty() && !body.starts_with('.') && !calls.is_empty() {
        let mut it = calls.into_iter();
        (it.next(), it.collect())
    } else {
        (None, calls)
    };
    StuntboostName {
        raw: raw.to_owned(),
        name: inner.to_owned(),
        invisible,
        bake,
        collision,
        code: code.to_owned(),
        class,
        methods,
        duplicate,
    }
}

/// The calls of a code part: `A(x,y).B.C(z)` -> A [x, y], B (no parentheses), C [z]; a dot inside parentheses does not
/// split.
fn calls(code: &str) -> Vec<StuntboostCall> {
    let mut depth = 0usize;
    let mut start = 0;
    let mut pieces = Vec::new();
    for (i, ch) in code.char_indices() {
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
    (pieces.into_iter().filter(|p| !p.is_empty()))
        .map(|piece| match piece.split_once('(') {
            Some((name, rest)) => StuntboostCall {
                name: name.to_owned(),
                args: Some(arguments(rest.strip_suffix(')').unwrap_or(rest))),
            },
            None => StuntboostCall {
                name: piece.to_owned(),
                args: None,
            },
        })
        .collect()
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

    /// The names the game's levels use, parsed as the game's `ParsedName` reads them.
    #[test]
    fn the_levels_names_parse_by_the_games_rules() {
        // no '=': the name ends at its first '.'
        let n = parse("plant_pot_high_<.001");
        assert_eq!(
            (
                n.name.as_str(),
                n.invisible,
                n.collision,
                n.code.as_str(),
                n.duplicate
            ),
            ("plant_pot_high", true, StuntboostCollision::Solid, "", Some(1))
        );
        assert_eq!(
            n.bake,
            StuntboostBake::HiddenInBake,
            "an invisible node takes no part in the bake"
        );

        // the type characters are counted in characters: a multi-byte character after the first '.' is not cut through
        let n = parse("a<.é=X()");
        assert_eq!(
            (n.name.as_str(), n.collision),
            ("a<.", StuntboostCollision::Solid)
        );
        let n = parse("ab_<é=X()");
        assert_eq!(
            (n.name.as_str(), n.collision),
            ("ab_<é", StuntboostCollision::None)
        );

        // '=': the code keeps its suffix; the calls do not
        let n = parse("_#=Checkpoint(Mandatory).003");
        assert_eq!((n.name.as_str(), n.invisible), ("", true));
        assert_eq!(
            (n.collision, n.code.as_str()),
            (StuntboostCollision::Trigger, "=Checkpoint(Mandatory).003")
        );
        assert_eq!(n.class, Some(call("Checkpoint", &["Mandatory"])));
        assert!(n.methods.is_empty());
        assert_eq!(n.duplicate, Some(3));

        // type characters in any order
        let n = parse("wall<_");
        assert_eq!(
            (n.name.as_str(), n.invisible, n.collision),
            ("wall", true, StuntboostCollision::Solid)
        );

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

        let n = parse("x=Thing().Move(Vector3(1,0,0)).Disable()");
        assert_eq!(
            n.methods,
            [call("Move", &["Vector3(1,0,0)"]), call("Disable", &[""])]
        );

        // a code part starting with a method: no class (Entity())
        let n = parse("box=.Disable()");
        assert!(n.class.is_none());
        assert_eq!(n.methods, [call("Disable", &[""])]);

        // no '=', a dot in the name: the name ends there, as the game reads it
        assert_eq!(parse("v1.5_ramp").name, "v1");
        // '=' keeps the dot in the name; the type characters come from before it
        let n = parse("v1.5_ramp<=Thing()");
        assert_eq!(
            (n.name.as_str(), n.collision),
            ("v1.5_ramp<", StuntboostCollision::None)
        );
        assert_eq!(parse("=Skybox().001").class, Some(call("Skybox", &[""])));
    }
}
