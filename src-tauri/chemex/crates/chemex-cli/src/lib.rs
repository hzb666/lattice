//! TeX protocol shared by the CLI and hosts embedding Chemex.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::num::NonZeroUsize;
use std::path::Path;

use chemex_cdxml::FieldValues;

mod mnova;
#[allow(dead_code)]
mod output;
mod structures;
#[allow(dead_code)]
mod tools;
pub use mnova::{export_spectrum_pdf, is_valid_spectrum_pdf, spectrum_renderer_fingerprint};
pub use structures::{CompoundStructure, StructureReport, analyze_compounds};

pub fn analyze_compounds_with_rdkit(source: &str) -> std::result::Result<StructureReport, String> {
    analyze_compounds(source, |script, request| {
        tools::run_rdkit(script, request).map_err(|error| error.to_string())
    })
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub const PACKAGE: &str = include_str!("chemex.sty");

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FigureRequest {
    ChemDraw {
        counter: String,
        path: String,
    },
    Mnova {
        path: String,
        spectrum: NonZeroUsize,
    },
}

#[derive(Clone, Debug)]
pub struct DrawingRegistration {
    pub counter: String,
    pub fields: Vec<String>,
    pub asset: String,
}

#[derive(Clone, Debug)]
pub enum FigureRegistration {
    ChemDraw(DrawingRegistration),
    Mnova {
        spectrum: NonZeroUsize,
        asset: String,
    },
}

impl FigureRequest {
    pub fn path(&self) -> &str {
        match self {
            Self::ChemDraw { path, .. } | Self::Mnova { path, .. } => path,
        }
    }
}

pub fn validate_figures(
    expected: &[FigureRequest],
    actual: &[FigureRequest],
    pass: &str,
) -> Result<()> {
    if expected != actual {
        return Err(format!("Chemex figure references changed during the {pass} pass").into());
    }
    Ok(())
}

pub fn read_figures(path: &Path) -> Result<Vec<FigureRequest>> {
    let contents = fs::read_to_string(path).map_err(|error| {
        format!(
            "cannot read TeX figure requests {}: {error}",
            path.display()
        )
    })?;
    let mut figures = Vec::new();
    for line in contents.lines() {
        if let Some(record) = line.strip_prefix(r"\chemex@drawrequest") {
            let (counter, rest) = group(record)?;
            let (path, rest) = group(rest)?;
            if !rest.trim().is_empty() {
                return Err(format!("unexpected data in TeX drawing request: {line}").into());
            }
            let counter = counter.trim().to_owned();
            validate_counter(&counter)?;
            let path = unescape_tex_path(path.trim())?;
            validate_extension(&path, "cdxml")?;
            figures.push(FigureRequest::ChemDraw { counter, path });
            continue;
        }

        let Some(record) = line.strip_prefix(r"\chemex@spectrumrequest") else {
            return Err(format!("invalid TeX figure request: {line}").into());
        };
        let (spectrum, rest) = group(record)?;
        let (path, rest) = group(rest)?;
        if !rest.trim().is_empty() {
            return Err(format!("unexpected data in TeX spectrum request: {line}").into());
        }
        let spectrum = spectrum
            .trim()
            .parse::<NonZeroUsize>()
            .map_err(|_| format!("invalid Mnova spectrum index in TeX request: {line}"))?;
        let path = unescape_tex_path(path.trim())?;
        validate_extension(&path, "mnova")?;
        figures.push(FigureRequest::Mnova { path, spectrum });
    }
    Ok(figures)
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

pub fn registrations(figures: &[FigureRegistration]) -> String {
    let mut output = String::new();
    for (index, figure) in figures.iter().enumerate() {
        let index = index + 1;
        let registration = match figure {
            FigureRegistration::ChemDraw(drawing) => {
                let fields = drawing
                    .fields
                    .iter()
                    .map(|key| format!(r"\chemex@assignfield{{{}}}{{{key}}}", drawing.counter))
                    .collect::<String>();
                format!(
                    r"\chemex@register{{{index}}}{{{}}}{{{fields}}}{{{}}}",
                    drawing.counter, drawing.asset
                )
            }
            FigureRegistration::Mnova { spectrum, asset } => {
                format!(r"\chemex@registerspectrum{{{index}}}{{{spectrum}}}{{{asset}}}")
            }
        };
        output.push_str(&registration);
        output.push('\n');
    }
    output
}

pub fn prepared_registrations(
    figures: &[FigureRegistration],
    numbers: &BTreeMap<String, String>,
) -> Result<String> {
    let mut output = registrations(figures);
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

fn validate_extension(path: &str, expected: &str) -> Result<()> {
    if path.is_empty() {
        return Err(format!("{expected} path cannot be empty").into());
    }
    if !Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
    {
        return Err(format!("expected a .{expected} file: {path}").into());
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
            return Err("trailing escape in figure path".into());
        };
        if !matches!(escaped, '#' | '$' | '%' | '&' | '_' | '{' | '}') {
            return Err(format!("unsupported TeX escape \\{escaped} in figure path").into());
        }
        output.push(escaped);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn figure_declarations_must_match_across_tex_passes() {
        let declaration = FigureRequest::ChemDraw {
            counter: "compound".into(),
            path: "drawing.cdxml".into(),
        };
        assert!(
            validate_figures(
                std::slice::from_ref(&declaration),
                std::slice::from_ref(&declaration),
                "final"
            )
            .is_ok()
        );

        let changed = FigureRequest::Mnova {
            path: "appendix.mnova".into(),
            spectrum: NonZeroUsize::new(2).unwrap(),
        };
        assert!(validate_figures(&[declaration], &[changed], "final").is_err());
    }

    #[test]
    fn reads_tex_figure_requests_with_unicode_spaces_and_spectrum_indices() {
        let path = std::env::temp_dir().join(format!("chemex-figures-{}.dat", std::process::id()));
        fs::write(
            &path,
            "\\chemex@drawrequest{scheme}{\"figures/有机 1.cdxml\"}\n\\chemex@spectrumrequest{2}{figures/a\\_b.mnova}\n",
        )
        .unwrap();
        assert_eq!(
            vec![
                FigureRequest::ChemDraw {
                    counter: "scheme".into(),
                    path: "figures/有机 1.cdxml".into()
                },
                FigureRequest::Mnova {
                    path: "figures/a_b.mnova".into(),
                    spectrum: NonZeroUsize::new(2).unwrap(),
                }
            ],
            read_figures(&path).unwrap()
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn spectrum_requests_default_to_the_first_spectrum() {
        let path =
            std::env::temp_dir().join(format!("chemex-figures-default-{}.dat", std::process::id()));
        fs::write(&path, r"\chemex@spectrumrequest{1}{sample.mnova}").unwrap();
        assert_eq!(
            vec![FigureRequest::Mnova {
                path: "sample.mnova".into(),
                spectrum: NonZeroUsize::new(1).unwrap(),
            }],
            read_figures(&path).unwrap()
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_zero_and_wrong_extension_spectrum_requests() {
        let path =
            std::env::temp_dir().join(format!("chemex-figures-invalid-{}.dat", std::process::id()));
        fs::write(&path, "\\chemex@spectrumrequest{0}{sample.mnova}\n").unwrap();
        assert!(read_figures(&path).is_err());
        fs::write(&path, "\\chemex@spectrumrequest{1}{sample.cdxml}\n").unwrap();
        assert!(read_figures(&path).is_err());
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
        let output = registrations(&[
            FigureRegistration::ChemDraw(DrawingRegistration {
                counter: "scheme".into(),
                fields: vec!["compound-a".into(), "compound-b".into()],
                asset: "drawings/drawing-001.pdf".into(),
            }),
            FigureRegistration::Mnova {
                spectrum: NonZeroUsize::new(2).unwrap(),
                asset: "mnova-asset.pdf".into(),
            },
        ]);
        assert!(output.contains(
            r"\chemex@assignfield{scheme}{compound-a}\chemex@assignfield{scheme}{compound-b}"
        ));
        assert!(output.contains("drawings/drawing-001.pdf"));
        assert!(output.contains(r"\chemex@registerspectrum{2}{2}{mnova-asset.pdf}"));
    }

    #[test]
    fn prepared_registrations_embed_plain_text_expectations_as_safe_tex() {
        let output = prepared_registrations(
            &[FigureRegistration::ChemDraw(DrawingRegistration {
                counter: "compound".into(),
                fields: vec!["compound-a".into()],
                asset: "drawings/drawing-001.pdf".into(),
            })],
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
