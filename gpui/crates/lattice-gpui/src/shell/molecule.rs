use std::{
    any::Any,
    path::{Path, PathBuf},
};

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Task, Window, actions, div, prelude::FluentBuilder as _,
};
use gpui_kit::component::{
    dock::{BasePanel, Panel, PanelEvent, panel_handle},
    v_flex,
};
use lattice_core::{
    chemistry::{self, MoleculeLocation, ProjectMolecule, ProjectMoleculeReport},
    models::FileNode,
};
use lattice_i18n::t;
use lattice_ui::{ActiveLattice as _, Palette, components::Button};

use super::{
    Shell,
    commands::{Command, CommandContext, Section},
    parts::{Part, View},
};

actions!(lattice, [OpenMolecules, ToggleMoleculePanel]);

pub(crate) struct MoleculePanel {
    focus_handle: FocusHandle,
    root: Option<PathBuf>,
    files: Vec<String>,
    report: Option<Result<ProjectMoleculeReport, String>>,
    visible: bool,
    generation: u64,
    analysis: Option<Task<()>>,
}

impl gpui::EventEmitter<PanelEvent> for MoleculePanel {}

impl Focusable for MoleculePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for MoleculePanel {
    fn panel_name(&self) -> &'static str {
        "MoleculePanel"
    }

    fn closable(&self, _: &App) -> bool {
        true
    }
}

impl Panel for MoleculePanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(t!("Molecule").into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        t!("Molecule")
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for MoleculePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.palette().clone();
        let mut content = v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
            .p_3()
            .track_focus(&self.focus_handle);
        if self.root.is_none() {
            content = content.child(t!("Open a project to identify molecules"));
        } else if self.analysis.is_some() {
            content = content.child(t!("Analyzing ChemDraw structures…"));
        } else if let Some(Err(error)) = &self.report {
            content = content.child(format!("{}: {error}", t!("Could not scan the project")));
        } else if let Some(Ok(report)) = &self.report {
            let duplicates = report
                .molecules
                .iter()
                .filter(|molecule| molecule.duplicate_of.is_some())
                .count();
            content = content
                .child(div().text_color(palette.text_secondary).child(format!(
                    "{} · {}",
                    t!("{count} molecules", count = report.molecules.len()),
                    t!("{count} duplicate structures", count = duplicates)
                )))
                .child(
                    v_flex()
                        .id("molecule-results")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(report.errors.iter().map(|error| {
                            div()
                                .border_b_1()
                                .border_color(palette.line)
                                .p_2()
                                .text_color(palette.text_secondary)
                                .child(format!(
                                    "{} · {}",
                                    error.file,
                                    t!(
                                        "Could not analyze this drawing: {reason}",
                                        reason = &error.message
                                    )
                                ))
                        }))
                        .children(
                            report
                                .molecules
                                .iter()
                                .map(|molecule| molecule_row(molecule, &palette)),
                        ),
                );
            if report.molecules.is_empty() && report.errors.is_empty() {
                content = content.child(t!("No numbered molecules found"));
            }
        } else {
            content = content.child(t!("No molecule analysis available"));
        }
        content.when(self.root.is_some() && self.analysis.is_none(), |content| {
            content.child(
                Button::new("molecule-refresh", t!("Refresh"))
                    .compact()
                    .ghost()
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.invalidate();
                        panel.analyze(cx);
                    })),
            )
        })
    }
}

fn molecule_row(molecule: &ProjectMolecule, palette: &Palette) -> impl IntoElement {
    let location = &molecule.location;
    let formula = molecule
        .structure
        .formula
        .as_deref()
        .map(str::to_owned)
        .unwrap_or_else(|| t!("Formula unavailable").to_string());
    let status = molecule
        .duplicate_of
        .as_ref()
        .map(|duplicate| {
            t!(
                "Same structure as {target}",
                target = location_label(duplicate)
            )
        })
        .or_else(|| {
            molecule.structure.status.as_deref().map(|reason| {
                if molecule.structure.canonical_smiles.is_some() {
                    t!("Not compared: {reason}", reason = reason)
                } else {
                    t!("Could not identify structure: {reason}", reason = reason)
                }
            })
        });
    v_flex()
        .gap_1()
        .border_b_1()
        .border_color(palette.line)
        .p_2()
        .child(
            div()
                .text_color(palette.text)
                .child(format!("{} · {}", location.key, formula)),
        )
        .child(
            div()
                .text_color(palette.text_secondary)
                .child(location.file.clone()),
        )
        .when_some(status, |row, status| {
            row.child(div().text_color(palette.text_secondary).child(status))
        })
}

fn location_label(location: &MoleculeLocation) -> String {
    format!("{} · {}", location.file, location.key)
}

impl MoleculePanel {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            root: None,
            files: Vec::new(),
            report: None,
            visible: false,
            generation: 0,
            analysis: None,
        }
    }

    fn set_project(&mut self, root: &Path, files: &[FileNode], cx: &mut Context<Self>) {
        let files: Vec<_> = chemistry::cdxml_files(files).map(str::to_owned).collect();
        if self.root.as_deref() == Some(root) && self.files == files {
            return;
        }
        self.root = Some(root.to_path_buf());
        self.files = files;
        self.invalidate();
        cx.notify();
    }

    fn files_changed(&mut self, paths: Option<&[String]>, cx: &mut Context<Self>) {
        if paths.is_none_or(|paths| {
            paths
                .iter()
                .any(|path| chemistry::is_cdxml(Path::new(path)))
        }) {
            self.invalidate();
            self.analyze(cx);
            cx.notify();
        }
    }

    fn invalidate(&mut self) {
        self.generation += 1;
        self.report = None;
    }

    fn analyze(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        if !self.visible || self.report.is_some() || self.analysis.is_some() {
            return;
        }
        let generation = self.generation;
        let task = cx.background_spawn(async move { chemistry::analyze_project_molecules(&root) });
        self.analysis = Some(cx.spawn(async move |panel, cx| {
            let result = task.await;
            panel
                .update(cx, |panel, cx| {
                    panel.analysis = None;
                    if panel.generation == generation {
                        panel.report = Some(result);
                    } else {
                        panel.analyze(cx);
                    }
                    cx.notify();
                })
                .ok();
        }));
        cx.notify();
    }
}

fn install(_: &mut Window, cx: &mut Context<Shell>) -> Box<dyn Any> {
    Box::new(cx.new(MoleculePanel::new))
}

pub(super) const PART: Part = Part {
    install,
    views: &[View {
        kind: "molecule",
        min: (360., 220.),
        panel: |shell| panel_handle(shell.parts.get::<Entity<MoleculePanel>>().clone()),
    }],
    actions: |element, cx| {
        element
            .on_action(cx.listener(|this, _: &OpenMolecules, window, cx| {
                this.open_molecules(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleMoleculePanel, window, cx| {
                if this.root(cx).is_some() {
                    this.toggle_view("molecule", window, cx);
                }
            }))
    },
    commands,
};

impl Shell {
    pub(super) fn molecules_project_opened(
        &mut self,
        root: &Path,
        files: &[FileNode],
        cx: &mut Context<Self>,
    ) {
        self.parts
            .get::<Entity<MoleculePanel>>()
            .update(cx, |panel, cx| panel.set_project(root, files, cx));
    }

    pub(super) fn sync_molecules(&mut self, cx: &mut Context<Self>) {
        let visible = self.view_on_screen("molecule", cx);
        self.parts
            .get::<Entity<MoleculePanel>>()
            .update(cx, |panel, cx| {
                panel.visible = visible;
                panel.analyze(cx);
            });
    }

    pub(super) fn molecules_files_changed(
        &mut self,
        paths: Option<&[String]>,
        cx: &mut Context<Self>,
    ) {
        self.parts
            .get::<Entity<MoleculePanel>>()
            .update(cx, |panel, cx| panel.files_changed(paths, cx));
    }

    fn open_molecules(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root(cx).is_none() {
            return;
        }
        self.show_view("molecule", true, window, cx);
    }
}

fn commands(context: &CommandContext, out: &mut Vec<Command>) {
    if context.in_project {
        out.push(
            Command::new(
                "panel-molecules",
                t!("Show Molecule panel"),
                Section::Layout,
                OpenMolecules,
            )
            .keywords("chemistry molecule structure rdkit duplicate cdxml"),
        );
    }
}
