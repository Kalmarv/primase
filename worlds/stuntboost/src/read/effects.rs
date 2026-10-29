//! Compiled effects retained as source data; resolution happens in the shader pass.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the compiled effect and level shading table from the install"
)]
use crate::components::StuntboostEffect;
use primase_core::World;
use std::path::Path;

pub fn read_effects(install: &Path, scale: f64, level: &str, world: &mut World) -> crate::Result<()> {
    let path = install.join("Content/Effects/D3/Render.xnb");
    let (bytes, missing) = match std::fs::read(&path) {
        Ok(bytes) => {
            let xnb = monogame_formats::xnb::Xnb::parse(&bytes)?;
            (Some(xnb.effect()?.to_vec()), None)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (None, Some("D3/Render effect content is absent".into()))
        }
        Err(source) => return Err(crate::Error::Io { path, source }),
    };
    let table = install.join("Content/Other/Levels.xnb");
    let shading = match std::fs::read(&table) {
        Ok(bytes) => {
            let xnb = monogame_formats::xnb::Xnb::parse(&bytes)?;
            let (reader, mut cursor) = xnb.primary()?;
            if !reader.is_string() {
                return Err(crate::Error::Format("level shading table is not a string".into()));
            }
            let text = cursor.string()?;
            let rows = csv_rows(text);
            let headers = rows.first().cloned().unwrap_or_default();
            rows.iter()
                .skip(1)
                .find(|r| r.first().is_some_and(|n| n == level))
                .map(|row| headers.into_iter().zip(row.iter().cloned()).collect())
                .unwrap_or_default()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(source) => return Err(crate::Error::Io { path: table, source }),
    };
    let entity = world.spawn();
    world.insert(
        entity,
        StuntboostEffect {
            bytes,
            missing,
            metres_per_unit: scale,
            shading,
        },
    );
    Ok(())
}

/// CSV quoting (including escaped quotes) is part of the game's StringReader level table.
fn csv_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => row.push(std::mem::take(&mut field)),
            '\n' if !quoted => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            '\r' if !quoted => {}
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}
#[cfg(test)]
mod tests {
    #[test]
    fn quoted_colours_and_escaped_quotes() {
        assert_eq!(
            super::csv_rows("Name,FogColor\r\nPark,\"(0.8, 0.8, 0.6, 1.0)\"\n\"a\"\"b\",\n"),
            vec![
                vec!["Name", "FogColor"],
                vec!["Park", "(0.8, 0.8, 0.6, 1.0)"],
                vec!["a\"b", ""]
            ]
        );
    }
}
