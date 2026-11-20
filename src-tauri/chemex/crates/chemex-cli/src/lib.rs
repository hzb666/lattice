//! TeX protocol shared by the CLI and hosts embedding Chemex.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use chemex_cdxml::FieldValues;

mod structures;
pub use structures::{CompoundStructure, StructureReport, analyze_compounds};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub const PACKAGE: &str = include_str!("chemex.sty");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrawingRequest {
    pub counter: String,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct DrawingRegistration {
    pub counter: String,
    pub fields: Vec<String>,
    pub asset: String,
}

pub fn validate_drawings(
    expected: &[DrawingRequest],
    actual: &[DrawingRequest],
    pass: &str,
) -> Result<()> {
    if expected != actual {
        return Err(format!("CDXML references changed during the {pass} pass").into());
    }
    Ok(())
}

pub fn read_drawings(path: &Path) -> Result<Vec<DrawingRequest>> {
    let contents = fs::read_to_string(path).map_err(|error| {
        format!(
            "cannot read TeX drawing requests {}: {error}",
            path.display()
        )
    })?;
    let mut drawings = Vec::new();
    for line in contents.lines() {
        let Some(record) = line.strip_prefix(r"\chemex@drawrequest") else {
            return Err(format!("invalid TeX drawing request: {line}").into());
        };
        let (counter, rest) = group(record)?;
        let (path, rest) = group(rest)?;
        if !rest.trim().is_empty() {
            return Err(format!("unexpected data in TeX drawing request: {line}").into());
        }
        let counter = counter.trim().to_owned();
        validate_counter(&counter)?;
        let path = unescape_tex_path(path.trim())?;
        if path.is_empty() {
            return Err("CDXML path cannot be empty".into());
        }
        drawings.push(DrawingRequest { counter, path });
    }
    Ok(drawings)
}

pub fn read_numbers(path: &Path) -> Result<BTreeMap<String, String>> {
    let contents = fs::read_to_string(path).map_err(|error| {
        format!(
            "cannot read TeX compound numbers {}: {error}",
            path.display()
        )
    })?;
    let mut numbers = BTreeMap::new();
    for line in contents.lines() {
        let Some(record) = line.strip_prefix(r"\chemex@number") else {
            return Err(format!("invalid TeX compound number: {line}").into());
        };
        let (key, rest) = group(record)?;
        let (label, rest) = group(rest)?;
        if !rest.trim().is_empty() {
            return Err(format!("unexpected data in TeX compound number: {line}").into());
        }
        validate_label(&key, &label)?;
        if numbers.insert(key.clone(), label).is_some() {
            return Err(format!("TeX assigned compound key {key} more than once").into());
        }
    }
    Ok(numbers)
}

pub fn registrations(drawings: &[DrawingRegistration]) -> String {
    let mut output = String::new();
    for (index, drawing) in drawings.iter().enumerate() {
        let fields = drawing
            .fields
            .iter()
            .map(|key| format!(r"\chemex@assignfield{{{}}}{{{key}}}", drawing.counter))
            .collect::<String>();
        output.push_str(&format!(
            r"\chemex@register{{{}}}{{{}}}{{{fields}}}{{{}}}",
            index + 1,
            drawing.counter,
            drawing.asset
        ));
        output.push('\n');
    }
    output
}

pub fn prepared_registrations(
    drawings: &[DrawingRegistration],
    numbers: &BTreeMap<String, String>,
) -> Result<String> {
    let mut output = registrations(drawings);
    if !numbers.is_empty() {
        output.push_str(concat!(
            "\\begingroup\n",
            "\\catcode35=12 \\catcode36=12 \\catcode37=12 \\catcode38=12\n",
            "\\catcode94=12 \\catcode95=12 \\catcode126=12\n",
        ));
        for (key, label) in numbers {
            validate_label(key, label)?;
            output.push_str(&format!(r"\chemex@expected{{{key}}}{{{label}}}"));
            output.push('\n');
        }
        output.push_str("\\endgroup\n");
    }
    Ok(output)
}

pub fn field_values(fields: &[String], numbers: &BTreeMap<String, String>) -> Result<FieldValues> {
    let mut values = FieldValues::new();
    let mut inserted = BTreeSet::new();
    for key in fields {
        if !inserted.insert(key) {
            continue;
        }
        let label = numbers
            .get(key)
            .ok_or_else(|| format!("TeX did not assign a number to ChemDraw field {key}"))?;
        values.insert(key.clone(), label.clone())?;
    }
    Ok(values)
}

fn validate_counter(counter: &str) -> Result<()> {
    if counter.is_empty() || !counter.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Err(format!("invalid LaTeX counter name {counter:?}").into());
    }
    Ok(())
}

fn validate_label(key: &str, label: &str) -> Result<()> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    {
        return Err(format!("invalid TeX compound key {key:?}").into());
    }
    if label.trim().is_empty()
        || label.chars().any(|character| {
            character.is_control()
                || matches!(character, '\\' | '{' | '}' | '\u{fffe}' | '\u{ffff}')
        })
    {
        return Err(format!(
            "LaTeX counter for {key} must expand to plain text usable in ChemDraw"
        )
        .into());
    }
    Ok(())
}

fn group(input: &str) -> Result<(String, &str)> {
    let input = input.trim_start();
    let Some(rest) = input.strip_prefix('{') else {
        return Err(format!("expected a braced TeX field in {input:?}").into());
    };
    let mut depth = 1;
    let mut escaped = false;
    for (index, character) in rest.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((rest[..index].to_owned(), &rest[index + 1..]));
                }
            }
            _ => {}
        }
    }
    Err("unclosed braced field in TeX output".into())
}

fn unescape_tex_path(path: &str) -> Result<String> {
    let path = path
        .strip_prefix('"')
        .and_then(|path| path.strip_suffix('"'))
        .unwrap_or(path);
    let mut output = String::new();
    let mut characters = path.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        let Some(escaped) = characters.next() else {
            return Err("trailing escape in CDXML path".into());
        };
        if !matches!(escaped, '#' | '$' | '%' | '&' | '_' | '{' | '}') {
            return Err(format!("unsupported TeX escape \\{escaped} in drawing path").into());
        }
        output.push(escaped);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawing_declarations_must_match_across_tex_passes() {
        let declaration = DrawingRequest {
            counter: "compound".into(),
            path: "drawing.cdxml".into(),
        };
        assert!(
            validate_drawings(
                std::slice::from_ref(&declaration),
                std::slice::from_ref(&declaration),
                "final"
            )
            .is_ok()
        );

        let changed = DrawingRequest {
            counter: "scheme".into(),
            path: "appendix.cdxml".into(),
        };
        assert!(validate_drawings(&[declaration], &[changed], "final").is_err());
    }

    #[test]
    fn reads_tex_drawing_requests_with_unicode_and_spaces() {
        let path = std::env::temp_dir().join(format!("chemex-drawings-{}.dat", std::process::id()));
        fs::write(
            &path,
            "\\chemex@drawrequest{scheme}{\"figures/有机 1.cdxml\"}\n\\chemex@drawrequest{appendix}{figures/a\\_b.cdxml}\n",
        )
        .unwrap();
        assert_eq!(
            vec![
                DrawingRequest {
                    counter: "scheme".into(),
                    path: "figures/有机 1.cdxml".into()
                },
                DrawingRequest {
                    counter: "appendix".into(),
                    path: "figures/a_b.cdxml".into()
                }
            ],
            read_drawings(&path).unwrap()
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reads_plain_text_labels_and_rejects_tex_commands() {
        let path = std::env::temp_dir().join(format!("chemex-numbers-{}.dat", std::process::id()));
        fs::write(&path, "\\chemex@number{compound-a}{A 1}\n").unwrap();
        let numbers = read_numbers(&path).unwrap();
        assert_eq!("A 1", numbers["compound-a"]);
        fs::write(&path, "\\chemex@number{compound-c}{\\textbf {I}}\n").unwrap();
        assert!(read_numbers(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn registration_runs_keys_at_each_drawing_position() {
        let output = registrations(&[DrawingRegistration {
            counter: "scheme".into(),
            fields: vec!["compound-a".into(), "compound-b".into()],
            asset: "drawings/drawing-001.pdf".into(),
        }]);
        assert!(output.contains(
            r"\chemex@assignfield{scheme}{compound-a}\chemex@assignfield{scheme}{compound-b}"
        ));
        assert!(output.contains("drawings/drawing-001.pdf"));
    }

    #[test]
    fn prepared_registrations_embed_plain_text_expectations_as_safe_tex() {
        let output = prepared_registrations(
            &[DrawingRegistration {
                counter: "compound".into(),
                fields: vec!["compound-a".into()],
                asset: "drawings/drawing-001.pdf".into(),
            }],
            &BTreeMap::from([("compound-a".into(), "1 # % & _".into())]),
        )
        .unwrap();

        assert!(output.contains(r"\chemex@expected{compound-a}{1 # % & _}"));
        assert!(output.contains(r"\catcode35=12 \catcode36=12 \catcode37=12 \catcode38=12"));
        assert!(output.contains(r"\catcode94=12 \catcode95=12 \catcode126=12"));
        assert!(
            prepared_registrations(&[], &BTreeMap::from([("invalid key".into(), "1".into())]))
                .is_err()
        );
    }
}
