//! TeX protocol shared by the CLI and hosts embedding Chemex.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::num::NonZeroUsize;
use std::path::Path;

use chemex_cdxml::{DrawingDocument, FieldValues};

mod mnova;
#[allow(dead_code)]
mod output;
mod structures;
#[allow(dead_code)]
mod tools;
#[cfg(windows)]
pub use mnova::mestrenova_status;
pub use mnova::{export_spectrum_pdf, is_valid_spectrum_pdf, spectrum_renderer_fingerprint};
pub use structures::{CompoundStructure, StructureReport, analyze_compounds};

pub fn analyze_compounds_with_rdkit(source: &str) -> std::result::Result<StructureReport, String> {
    analyze_compounds(source, |script, request| {
        tools::run_rdkit(script, request).map_err(|error| error.to_string())
    })
}

pub fn analyze_compounds_batch_with_rdkit(
    sources: &[&str],
) -> Vec<std::result::Result<StructureReport, String>> {
    structures::analyze_compounds_batch(sources, |script, request| {
        tools::run_rdkit(script, request).map_err(|error| error.to_string())
    })
}

pub fn formulas_for_keys(
    sources: &[String],
    keys: &BTreeSet<String>,
) -> std::result::Result<BTreeMap<String, String>, String> {
    let mut formulas = BTreeMap::new();
    let mut seen_sources = BTreeSet::new();
    for source in sources {
        if !seen_sources.insert(source) {
            continue;
        }
        let document = DrawingDocument::parse(source.clone()).map_err(|error| error.to_string())?;
        let relevant = document
            .fields()
            .iter()
            .any(|field| keys.contains(&field.compound_id));
        if !relevant {
            continue;
        }
        let report = analyze_compounds_with_rdkit(source)?;
        for compound in report.compounds {
            if !keys.contains(&compound.compound_id) {
                continue;
            }
            let formula = compound.formula.ok_or_else(|| {
                format!(
                    "ChemDraw key {} has no molecular formula: {}",
                    compound.compound_id,
                    compound
                        .status
                        .unwrap_or_else(|| "structure unavailable".into())
                )
            })?;
            if formulas
                .insert(compound.compound_id.clone(), formula.clone())
                .is_some_and(|existing| existing != formula)
            {
                return Err(format!(
                    "ChemDraw key {} maps to different molecular formulas",
                    compound.compound_id
                ));
            }
        }
    }
    for key in keys {
        if !formulas.contains_key(key) {
            return Err(format!(
                "ChemDraw key {key} has no molecular formula source"
            ));
        }
    }
    Ok(formulas)
}

pub fn attach_mnova_formulas(
    requests: &[FigureRequest],
    registrations: &mut [FigureRegistration],
    sources: &[String],
    numbers: &BTreeMap<String, String>,
) -> std::result::Result<(), String> {
    if requests.len() != registrations.len() {
        return Err("Mnova formula registrations do not match figure requests".into());
    }
    if requests
        .iter()
        .zip(registrations.iter())
        .any(|(request, registration)| {
            !matches!(
                (request, registration),
                (
                    FigureRequest::ChemDraw { .. },
                    FigureRegistration::ChemDraw(_)
                ) | (
                    FigureRequest::Mnova { .. },
                    FigureRegistration::Mnova { .. }
                )
            )
        })
    {
        return Err("Chemex registration types do not match figure requests".into());
    }
    let keys: BTreeSet<_> = requests
        .iter()
        .filter_map(|request| match request {
            FigureRequest::Mnova {
                compound: Some(key),
                ..
            } => Some(key.clone()),
            _ => None,
        })
        .collect();
    for key in &keys {
        if !numbers.contains_key(key) {
            return Err(format!(
                "Mnova formula compound {key} has no dynamic Chemex number; reference it with \\chemex or a ChemDraw field"
            ));
        }
    }
    let formulas = formulas_for_keys(sources, &keys)?;
    for (request, registration) in requests.iter().zip(registrations) {
        let FigureRequest::Mnova {
            compound: Some(key),
            ..
        } = request
        else {
            continue;
        };
        let formula = formulas.get(key).ok_or_else(|| {
            format!("Mnova formula compound {key} has no usable molecular formula")
        })?;
        let formula = formula_tex(formula).map_err(|error| format!("{key}: {error}"))?;
        if let FigureRegistration::Mnova {
            formula: target, ..
        } = registration
        {
            *target = Some(formula);
        }
    }
    Ok(())
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub const PACKAGE: &str = include_str!("chemex.sty");

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum FigureRequest {
    ChemDraw {
        counter: String,
        path: String,
    },
    Mnova {
        path: String,
        spectrum: NonZeroUsize,
        compound: Option<String>,
        x: u8,
        y: u8,
        size: String,
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
        compound: Option<String>,
        x: u8,
        y: u8,
        size: String,
        formula: Option<String>,
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
        let (compound, rest) = group(rest)?;
        let (x, rest) = group(rest)?;
        let (y, rest) = group(rest)?;
        let (size, rest) = group(rest)?;
        let (path, rest) = group(rest)?;
        if !rest.trim().is_empty() {
            return Err(format!("unexpected data in TeX spectrum request: {line}").into());
        }
        let spectrum = spectrum
            .trim()
            .parse::<NonZeroUsize>()
            .map_err(|_| format!("invalid Mnova spectrum index in TeX request: {line}"))?;
        let compound = optional_key(compound.trim())?;
        let x = parse_percent(&x, "x", &line)?;
        let y = parse_percent(&y, "y", &line)?;
        let size = validate_size(size.trim())?;
        let path = unescape_tex_path(path.trim())?;
        validate_extension(&path, "mnova")?;
        figures.push(FigureRequest::Mnova {
            path,
            spectrum,
            compound,
            x,
            y,
            size,
        });
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
            FigureRegistration::Mnova {
                spectrum,
                compound,
                x,
                y,
                size,
                formula,
                asset,
            } => {
                format!(
                    r"\chemex@registerspectrum{{{index}}}{{{spectrum}}}{{{asset}}}{{{}}}{{{x}}}{{{y}}}{{{size}}}{{{}}}",
                    compound.as_deref().unwrap_or_default(),
                    formula.as_deref().unwrap_or_default(),
                )
            }
        };
        output.push_str(&registration);
        output.push('\n');
    }
    output
}

pub fn formula_tex(formula: &str) -> Result<String> {
    let mut output = String::new();
    let mut chars = formula.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            'A'..='Z' => {
                output.push_str(r"\mathrm{");
                output.push(character);
                if chars.peek().is_some_and(|next| next.is_ascii_lowercase()) {
                    output.push(chars.next().unwrap_or_default());
                }
                output.push('}');
            }
            '0'..='9' => {
                output.push_str("_{");
                output.push(character);
                while chars.peek().is_some_and(|next| next.is_ascii_digit()) {
                    output.push(chars.next().unwrap_or_default());
                }
                output.push('}');
            }
            '(' | ')' | '[' | ']' | '.' | '+' | '-' => output.push(character),
            _ => {
                return Err(
                    format!("unsupported molecular formula character: {character:?}").into(),
                );
            }
        }
    }
    if output.is_empty() {
        return Err("molecular formula cannot be empty".into());
    }
    Ok(output)
}

pub fn formula_display(formula: &str) -> Result<String> {
    let mut output = String::new();
    for character in formula.chars() {
        match character {
            '0' => output.push('₀'),
            '1' => output.push('₁'),
            '2' => output.push('₂'),
            '3' => output.push('₃'),
            '4' => output.push('₄'),
            '5' => output.push('₅'),
            '6' => output.push('₆'),
            '7' => output.push('₇'),
            '8' => output.push('₈'),
            '9' => output.push('₉'),
            '.' => output.push('·'),
            'A'..='Z' | 'a'..='z' | '(' | ')' | '[' | ']' | '+' | '-' => output.push(character),
            _ => {
                return Err(
                    format!("unsupported molecular formula character: {character:?}").into(),
                );
            }
        }
    }
    if output.is_empty() {
        return Err("molecular formula cannot be empty".into());
    }
    Ok(output)
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

fn optional_key(value: &str) -> Result<Option<String>> {
    if value.is_empty() {
        return Ok(None);
    }
    validate_key(value)?;
    Ok(Some(value.to_owned()))
}

fn validate_key(key: &str) -> Result<()> {
    let mut parts = key.split('|');
    let valid_part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    };
    if !valid_part(parts.next().unwrap_or_default())
        || parts.next().is_some_and(|part| !valid_part(part))
        || parts.next().is_some()
    {
        return Err(format!("invalid TeX compound key {key:?}").into());
    }
    Ok(())
}

fn parse_percent(value: &str, name: &str, line: &str) -> Result<u8> {
    let value = value
        .parse::<u8>()
        .map_err(|_| format!("invalid Mnova {name} percentage in TeX request: {line}"))?;
    if value > 100 {
        return Err(format!("Mnova {name} percentage must be between 0 and 100").into());
    }
    Ok(value)
}

fn validate_size(size: &str) -> Result<String> {
    let units = [
        ("pt", 1.0),
        ("bp", 72.27 / 72.0),
        ("mm", 72.27 / 25.4),
        ("cm", 72.27 / 2.54),
        ("in", 72.27),
    ];
    let Some((number, scale)) = units
        .iter()
        .find_map(|(unit, scale)| size.strip_suffix(unit).map(|number| (number, scale)))
    else {
        return Err(format!("invalid Mnova formula size {size:?}").into());
    };
    let value = number
        .parse::<f64>()
        .map_err(|_| format!("invalid Mnova formula size {size:?}"))?;
    if !value.is_finite() || value <= 0.0 || value * scale > 16_000.0 {
        return Err(format!(
            "Mnova formula size must be positive and no larger than 16000 TeX points: {size:?}"
        )
        .into());
    }
    Ok(size.to_owned())
}

fn validate_label(key: &str, label: &str) -> Result<()> {
    validate_key(key)?;
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
    fn compound_keys_allow_one_parent_child_separator() {
        assert!(validate_key("parent").is_ok());
        assert!(validate_key("parent|child").is_ok());
        for key in ["|child", "parent|", "parent|child|nested"] {
            assert!(validate_key(key).is_err(), "accepted malformed key {key}");
        }
    }

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
            compound: None,
            x: 50,
            y: 15,
            size: "8pt".into(),
        };
        assert!(validate_figures(&[declaration], &[changed], "final").is_err());
    }

    #[test]
    fn reads_tex_figure_requests_with_unicode_spaces_and_spectrum_indices() {
        let path = std::env::temp_dir().join(format!("chemex-figures-{}.dat", std::process::id()));
        fs::write(
            &path,
            "\\chemex@drawrequest{scheme}{\"figures/有机 1.cdxml\"}\n\\chemex@spectrumrequest{2}{sample}{78}{12}{8pt}{figures/a\\_b.mnova}\n",
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
                    compound: Some("sample".into()),
                    x: 78,
                    y: 12,
                    size: "8pt".into(),
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
        fs::write(
            &path,
            r"\chemex@spectrumrequest{1}{}{50}{15}{8pt}{sample.mnova}",
        )
        .unwrap();
        assert_eq!(
            vec![FigureRequest::Mnova {
                path: "sample.mnova".into(),
                spectrum: NonZeroUsize::new(1).unwrap(),
                compound: None,
                x: 50,
                y: 15,
                size: "8pt".into(),
            }],
            read_figures(&path).unwrap()
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_zero_and_wrong_extension_spectrum_requests() {
        let path =
            std::env::temp_dir().join(format!("chemex-figures-invalid-{}.dat", std::process::id()));
        fs::write(
            &path,
            "\\chemex@spectrumrequest{0}{}{50}{15}{8pt}{sample.mnova}\n",
        )
        .unwrap();
        assert!(read_figures(&path).is_err());
        fs::write(
            &path,
            "\\chemex@spectrumrequest{1}{}{50}{15}{8pt}{sample.cdxml}\n",
        )
        .unwrap();
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
                compound: Some("compound-a".into()),
                x: 78,
                y: 12,
                size: "8pt".into(),
                formula: Some(r"\mathrm{C}_{6}\mathrm{H}_{6}".into()),
                asset: "mnova-asset.pdf".into(),
            },
        ]);
        assert!(output.contains(
            r"\chemex@assignfield{scheme}{compound-a}\chemex@assignfield{scheme}{compound-b}"
        ));
        assert!(output.contains("drawings/drawing-001.pdf"));
        assert!(output.contains(
            r"\chemex@registerspectrum{2}{2}{mnova-asset.pdf}{compound-a}{78}{12}{8pt}{\mathrm{C}_{6}\mathrm{H}_{6}}"
        ));
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

    #[test]
    fn molecular_formula_becomes_safe_subscript_tex() {
        assert_eq!(
            r"\mathrm{C}_{6}\mathrm{H}_{6}\mathrm{Cl}",
            formula_tex("C6H6Cl").unwrap()
        );
        assert_eq!("C₆H₆Cl", formula_display("C6H6Cl").unwrap());
        assert!(formula_tex("C6{bad}").is_err());
    }

    #[test]
    fn spectrum_size_accepts_tex_lengths_but_not_math_only_units() {
        assert_eq!("8pt", validate_size("8pt").unwrap());
        assert_eq!("2mm", validate_size("2mm").unwrap());
        assert!(validate_size("8em").is_err());
        assert!(validate_size("300in").is_err());
    }

    #[test]
    fn mnova_formula_binding_requires_a_registered_dynamic_compound() {
        let request = FigureRequest::Mnova {
            path: "spectrum.mnova".into(),
            spectrum: NonZeroUsize::MIN,
            compound: Some("product-17".into()),
            x: 78,
            y: 12,
            size: "8pt".into(),
        };
        let mut registration = FigureRegistration::Mnova {
            spectrum: NonZeroUsize::MIN,
            compound: Some("product-17".into()),
            x: 78,
            y: 12,
            size: "8pt".into(),
            formula: None,
            asset: "spectrum.pdf".into(),
        };

        assert!(
            attach_mnova_formulas(
                &[request],
                std::slice::from_mut(&mut registration),
                &[],
                &BTreeMap::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn mnova_formula_binding_rejects_misaligned_registrations() {
        let drawing = FigureRegistration::ChemDraw(DrawingRegistration {
            counter: "compound".into(),
            fields: Vec::new(),
            asset: "drawing.pdf".into(),
        });
        let mut unmatched = vec![drawing];
        assert!(attach_mnova_formulas(&[], &mut unmatched, &[], &BTreeMap::new()).is_err());

        let request = FigureRequest::ChemDraw {
            counter: "compound".into(),
            path: "drawing.cdxml".into(),
        };
        let mut wrong_type = vec![FigureRegistration::Mnova {
            spectrum: NonZeroUsize::MIN,
            compound: None,
            x: 50,
            y: 15,
            size: "8pt".into(),
            formula: None,
            asset: "spectrum.pdf".into(),
        }];
        assert!(
            attach_mnova_formulas(&[request], &mut wrong_type, &[], &BTreeMap::new(),).is_err()
        );
    }
}
