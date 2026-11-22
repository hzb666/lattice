use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const SOURCE: &str = include_str!("../../chemex-cdxml/tests/fixtures/fields.cdxml");
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Project(PathBuf);

impl Project {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "chemex-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("drawing.cdxml"), SOURCE).unwrap();
        fs::write(
            path.join("appendix.cdxml"),
            SOURCE
                .replace("compound-001", "appendix-a")
                .replace("002]]", "appendix-b]]"),
        )
        .unwrap();
        fs::write(
            path.join("manuscript.tex"),
            r"\documentclass{article}
\usepackage{chemex}
\newcounter{compound}
\renewcommand{\thecompound}{\Roman{compound}}
\begin{document}
References: \chemex{compound-001}, \chemex{compound-002}.
\chemd[compound]{drawing.cdxml}
\setcounter{compound}{0}
\renewcommand{\thecompound}{\alph{compound}}
\chemd[compound]{appendix.cdxml}
\end{document}
",
        )
        .unwrap();
        Self(path)
    }

    fn run(&self, command: &str, destination: &str) -> Output {
        self.command(command, destination).output().unwrap()
    }

    fn command(&self, command: &str, destination: &str) -> Command {
        let mut process = Command::new(env!("CARGO_BIN_EXE_chemex"));
        process
            .arg(command)
            .arg(self.0.join("manuscript.tex"))
            .arg(self.0.join(destination));
        process
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn inspect_reports_fields_hierarchy_and_chemistry_as_json() {
    let project = Project::new();
    let output = Command::new(env!("CARGO_BIN_EXE_chemex"))
        .arg("inspect")
        .arg(project.0.join("drawing.cdxml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(3, report["fields"].as_array().unwrap().len());
    assert_eq!("compound-001", report["fields"][0]["compound_id"]);
    assert_eq!(2, report["fragments"][0]["atoms"].as_array().unwrap().len());
    assert_eq!(
        SOURCE,
        fs::read_to_string(project.0.join("drawing.cdxml")).unwrap()
    );
}

#[test]
fn structures_command_reports_clear_missing_uv_error_without_touching_cdxml() {
    let project = Project::new();
    let source = project.0.join("drawing.cdxml");
    let cdxml = "<CDXML><page><group id='1'><fragment id='2'><n id='3' Element='6'/></fragment><t id='4'><s>[[@carbon]]</s></t></group></page></CDXML>";
    fs::write(&source, cdxml).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chemex"))
        .env("CHEMEX_UV", project.0.join("missing-uv.exe"))
        .arg("structures")
        .arg(&source)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("CHEMEX_UV"));
    assert_eq!(cdxml, fs::read_to_string(source).unwrap());
}

#[test]
fn existing_output_is_never_overwritten() {
    let project = Project::new();
    let destination = project.0.join("existing");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep.txt"), "previous output").unwrap();
    let output = project.run("render", "existing");
    assert!(!output.status.success());
    assert_eq!(
        "previous output",
        fs::read_to_string(destination.join("keep.txt")).unwrap()
    );
}

#[test]
fn render_uses_tex_numbering_and_keeps_sources_unchanged() {
    let Ok(latexmk) = std::env::var("CHEMEX_LATEXMK") else {
        return;
    };
    if !PathBuf::from(latexmk).is_file() {
        return;
    }
    let project = Project::new();
    let output = project.run("render", "rendered");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rendered = project.0.join("rendered");
    let first = fs::read_to_string(rendered.join("drawings/drawing-001/drawing.cdxml")).unwrap();
    let second = fs::read_to_string(rendered.join("drawings/drawing-002/drawing.cdxml")).unwrap();
    let first = chemex_cdxml::DrawingDocument::parse(first).unwrap();
    let second = chemex_cdxml::DrawingDocument::parse(second).unwrap();
    assert!(first.texts().iter().any(|text| text.text == "产物 & II"));
    assert!(second.texts().iter().any(|text| text.text == "产物 & b"));
    assert_eq!(
        SOURCE,
        fs::read_to_string(project.0.join("drawing.cdxml")).unwrap()
    );
    assert!(fs::read_to_string(project.0.join("manuscript.tex")).is_ok());
}

#[test]
fn changing_tex_counter_style_changes_drawing_labels_without_json() {
    let Ok(latexmk) = std::env::var("CHEMEX_LATEXMK") else {
        return;
    };
    if !PathBuf::from(latexmk).is_file() {
        return;
    }
    let project = Project::new();
    let path = project.0.join("manuscript.tex");
    let source = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        source.replace("\\Roman{compound}", "\\Alph{compound}"),
    )
    .unwrap();
    let output = project.run("render", "rendered");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let drawing = fs::read_to_string(
        project
            .0
            .join("rendered/drawings/drawing-001/drawing.cdxml"),
    )
    .unwrap();
    let drawing = chemex_cdxml::DrawingDocument::parse(drawing).unwrap();
    assert!(drawing.texts().iter().any(|text| text.text == "产物 & B"));
}

#[test]
fn reusing_a_key_with_another_counter_fails() {
    let Ok(latexmk) = std::env::var("CHEMEX_LATEXMK") else {
        return;
    };
    if !PathBuf::from(latexmk).is_file() {
        return;
    }
    let project = Project::new();
    let path = project.0.join("manuscript.tex");
    let source = fs::read_to_string(&path).unwrap();
    let source = source.replace(
        "\\end{document}",
        "\\chemex[scheme]{compound-001}\n\\end{document}",
    );
    fs::write(
        &path,
        source.replace(
            "\\newcounter{compound}",
            "\\newcounter{compound}\n\\newcounter{scheme}",
        ),
    )
    .unwrap();
    let output = project.run("render", "reused-key");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("diagnostics retained"));
}

#[test]
fn direct_graphics_support_numbered_and_plain_cdxml_and_reuse_cached_assets() {
    use sha2::{Digest, Sha256};

    let Some(latexmk) = std::env::var_os("CHEMEX_LATEXMK") else {
        return;
    };
    assert!(PathBuf::from(latexmk).is_file());
    let project = Project::new();
    let plain = "<CDXML><page><t><s>Ordinary drawing</s></t></page></CDXML>";
    fs::write(project.0.join("plain drawing.CDXML"), plain).unwrap();
    let manuscript = r#"\documentclass{article}
\usepackage{chemex}
\newcounter{compound}
\begin{document}
\includegraphics{"plain drawing.CDXML"}
\sbox{0}{\includegraphics*[width=72bp,trim=1 1 1 1]{drawing.cdxml}}
\ifdim\wd0=0pt\else
  \ifdim\wd0<71bp\errmessage{Chemex ignored graphicx width}\fi
  \ifdim\wd0>73bp\errmessage{Chemex ignored graphicx width}\fi
\fi
\usebox{0}
References: \chemex{compound-001}.
\IfFileExists{ordinary.pdf}{\includegraphics[width=20bp]{ordinary.pdf}}{}
\end{document}
"#;
    fs::write(project.0.join("manuscript.tex"), manuscript).unwrap();
    assert_success(project.run("render", "numbered"));

    let rendered = project.0.join("numbered");
    let first = fs::read(rendered.join("drawings/drawing-001/drawing.cdxml")).unwrap();
    assert_eq!(plain.as_bytes(), first);
    let second = fs::read(rendered.join("drawings/drawing-002/drawing.cdxml")).unwrap();
    let document =
        chemex_cdxml::DrawingDocument::parse(String::from_utf8(second.clone()).unwrap()).unwrap();
    assert!(document.texts().iter().any(|text| text.text == "产物 & 2"));
    let pdf = fs::read(rendered.join("numbering.pdf")).unwrap();
    fs::write(project.0.join("ordinary.pdf"), &pdf).unwrap();
    let cache = project.0.join(".chem/cache/chemdraw");
    fs::create_dir_all(&cache).unwrap();
    for source in [first, second] {
        let key = format!("{:x}", Sha256::digest(source));
        fs::write(cache.join(format!("{key}.pdf")), &pdf).unwrap();
        fs::write(cache.join(format!("{key}.svg")), "<svg/>").unwrap();
    }
    assert_success(project.run("export", "exported"));
    assert_eq!(
        "\\ChemexPreparedMode\n",
        fs::read_to_string(project.0.join("exported/chemex-mode.tex")).unwrap()
    );
    for destination in ["first-build", "second-build"] {
        let output = project
            .command("build", destination)
            .env("CHEMEX_INKSCAPE", project.0.join("missing-inkscape.exe"))
            .output()
            .unwrap();
        assert_eq!(
            2,
            String::from_utf8_lossy(&output.stdout)
                .matches("Reused cached CDXML export")
                .count()
        );
        assert_success(output);
        let build = project.0.join(destination);
        assert!(build.join("manuscript.pdf").is_file());
        assert_eq!(1, fs::read_dir(build).unwrap().count());
    }
    assert_eq!(
        manuscript,
        fs::read_to_string(project.0.join("manuscript.tex")).unwrap()
    );
}

#[test]
fn inserting_a_drawing_renumbers_earlier_prose_references() {
    use sha2::Digest;

    let Some(latexmk) = std::env::var_os("CHEMEX_LATEXMK") else {
        return;
    };
    assert!(PathBuf::from(latexmk).is_file());
    let project = Project::new();
    let drawing = |key: &str| format!("<CDXML><page><t><s>[[@{key}]]</s></t></page></CDXML>");
    fs::write(project.0.join("first.cdxml"), drawing("first")).unwrap();
    fs::write(
        project.0.join("chapter.tex"),
        r"\includegraphics{first.cdxml}",
    )
    .unwrap();
    let paper = r"\documentclass{article}
\usepackage{chemex}
\newcounter{compound}
\begin{document}
Compound \chemex{first}.
\input{chapter}
\includegraphics{first.cdxml}
\end{document}
";
    let manuscript = project.0.join("manuscript.tex");
    fs::write(&manuscript, paper).unwrap();
    assert_success(project.run("render", "before"));
    fs::write(project.0.join("inserted.cdxml"), drawing("inserted")).unwrap();
    fs::write(
        &manuscript,
        paper.replace(
            "\\input{chapter}",
            "\\includegraphics{inserted.cdxml}\n\\input{chapter}",
        ),
    )
    .unwrap();
    assert_success(project.run("render", "after"));
    let cache = project.0.join(".chem/cache/chemdraw");
    fs::create_dir_all(&cache).unwrap();
    let pdf = fs::read(project.0.join("after/numbering.pdf")).unwrap();
    for drawing in fs::read_dir(project.0.join("after/drawings")).unwrap() {
        let drawing = drawing.unwrap().path().join("drawing.cdxml");
        let source = fs::read(drawing).unwrap();
        let key = format!("{:x}", sha2::Sha256::digest(source));
        fs::write(cache.join(format!("{key}.pdf")), &pdf).unwrap();
        fs::write(cache.join(format!("{key}.svg")), "<svg/>").unwrap();
    }
    for (directory, index, number) in [
        ("before", 1, "1"),
        ("after", 1, "1"),
        ("after", 2, "2"),
        ("after", 3, "2"),
    ] {
        let path = project.0.join(format!(
            "{directory}/drawings/drawing-{index:03}/drawing.cdxml"
        ));
        let rendered =
            chemex_cdxml::DrawingDocument::parse(fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(number, rendered.texts()[0].text);
    }
    let numbers = fs::read_to_string(project.0.join("after/chemex-numbers.dat")).unwrap();
    assert!(numbers.contains(r"\chemex@number{inserted}{1}"));
    assert!(numbers.contains(r"\chemex@number{first}{2}"));
    assert_eq!(
        drawing("first"),
        fs::read_to_string(project.0.join("first.cdxml")).unwrap()
    );
    assert_success(project.run("build", "built"));
    assert!(project.0.join("built/manuscript.pdf").is_file());
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
