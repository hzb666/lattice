use std::{
    any::Any,
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, CursorStyle, Entity, FocusHandle, Focusable,
    ImageSource, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels,
    Point, RenderImage, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _,
    Task, Window, actions, canvas, div, img, prelude::FluentBuilder as _, px, size,
};
use gpui_kit::component::{
    dock::{BasePanel, Panel, PanelEvent, panel_handle},
    h_flex, v_flex,
};
use lattice_core::{
    chemistry::{self, MoleculeLocation, ProjectMolecule, ProjectMoleculeReport},
    models::FileNode,
    project,
};
use lattice_i18n::t;
use lattice_ui::{
    ActiveLattice as _, Icon, TextRole, TextRoleStyle as _,
    components::{EmptyState, IconButton},
    scrollbar::FluidScrollbar,
    tokens::SPACING_2,
};

use super::preview_viewport::{
    self, MAX_SCALE, MIN_SCALE, PREVIEW_PADDING, PreviewFit, RENDER_SCALE, fit_scale, preview_size,
    zoom_step,
};
use super::{
    Shell,
    commands::{Command, CommandContext, Section},
    parts::{Part, View},
};

mod view;

const NARROW_WIDTH: f32 = 560.;

actions!(lattice, [OpenMolecules, ToggleMoleculePanel]);

#[derive(Clone, Copy, PartialEq, Eq)]
enum MoleculeView {
    List,
    Gallery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MoleculeLayout {
    Stacked,
    Wide,
}

pub(crate) struct MoleculePanel {
    focus_handle: FocusHandle,
    root: Option<PathBuf>,
    files: Vec<String>,
    report: Option<Result<ProjectMoleculeReport, String>>,
    visible: bool,
    generation: u64,
    analysis: Option<Task<()>>,
    view: MoleculeView,
    selected: Option<MoleculeLocation>,
    preview: Option<Arc<RenderImage>>,
    preview_error: Option<String>,
    preview_loading: bool,
    preview_generation: u64,
    preview_task: Option<Task<()>>,
    scale: f32,
    fit: PreviewFit,
    panel_bounds: Rc<Cell<Bounds<Pixels>>>,
    preview_stage: Rc<Cell<Bounds<Pixels>>>,
    preview_scroll: ScrollHandle,
    preview_drag: Rc<Cell<Option<Point<Pixels>>>>,
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
            view: MoleculeView::List,
            selected: None,
            preview: None,
            preview_error: None,
            preview_loading: false,
            preview_generation: 0,
            preview_task: None,
            scale: 1.,
            fit: PreviewFit::Both,
            panel_bounds: Rc::default(),
            preview_stage: Rc::default(),
            preview_scroll: ScrollHandle::new(),
            preview_drag: Rc::default(),
        }
    }

    fn preview_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(self.preview_toolbar(cx))
            .child(self.preview_stage(cx))
            .into_any_element()
    }

    fn preview_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = self.display_scale();
        let no_preview = self.preview.is_none();
        let zoom = h_flex()
            .flex_none()
            .child(
                IconButton::new("molecule-zoom-out", Icon::ZOOM_OUT)
                    .tooltip(t!("Zoom out"))
                    .disabled(no_preview || scale <= MIN_SCALE)
                    .on_click(cx.listener(|this, _, _, cx| this.step_zoom(-1., cx))),
            )
            .child(
                div()
                    .min_w(px(42.))
                    .text_center()
                    .text_role(TextRole::Caption, cx)
                    .text_color(cx.palette().text_secondary)
                    .child(format!("{}%", (scale * 100.).round() as i64)),
            )
            .child(
                IconButton::new("molecule-zoom-in", Icon::ZOOM_IN)
                    .tooltip(t!("Zoom in"))
                    .disabled(no_preview || scale >= MAX_SCALE)
                    .on_click(cx.listener(|this, _, _, cx| this.step_zoom(1., cx))),
            );
        let fit = h_flex()
            .flex_none()
            .child(
                IconButton::new("molecule-fit-width", Icon::RECTANGLE_HORIZONTAL)
                    .tooltip(t!("Fit page to width"))
                    .on(matches!(self.fit, PreviewFit::Width | PreviewFit::Both))
                    .disabled(no_preview)
                    .on_click(cx.listener(|this, _, _, cx| this.set_fit(PreviewFit::Width, cx))),
            )
            .child(
                IconButton::new("molecule-fit-height", Icon::RECTANGLE_VERTICAL)
                    .tooltip(t!("Fit page to height"))
                    .on(self.fit == PreviewFit::Height)
                    .disabled(no_preview)
                    .on_click(cx.listener(|this, _, _, cx| this.set_fit(PreviewFit::Height, cx))),
            );
        h_flex()
            .id("molecule-preview-toolbar")
            .w_full()
            .min_w_0()
            .flex_none()
            .flex_wrap()
            .gap(SPACING_2)
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(cx.palette().line)
            .bg(cx.palette().editor)
            .child(zoom)
            .child(fit)
    }

    fn preview_stage(&self, cx: &mut Context<Self>) -> AnyElement {
        let bounds = self.preview_stage.clone();
        let content = match (&self.preview, &self.preview_error, self.preview_loading) {
            (Some(preview), _, _) => self.preview_image(preview),
            (None, Some(error), _) => div()
                .text_color(cx.palette().text_secondary)
                .child(error.clone())
                .into_any_element(),
            (None, None, true) => div()
                .text_color(cx.palette().text_secondary)
                .child(t!("Rendering molecule preview…"))
                .into_any_element(),
            (None, None, false) => {
                EmptyState::new(t!("Select a molecule to preview")).into_any_element()
            }
        };
        div()
            .id("molecule-preview-stage")
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .p(px(PREVIEW_PADDING))
            .child(content)
            .child(
                canvas(
                    move |next, window, _| {
                        if bounds.get().size != next.size {
                            window.request_animation_frame();
                        }
                        bounds.set(next);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }

    fn preview_image(&self, preview: &Arc<RenderImage>) -> AnyElement {
        let natural = preview_size(preview);
        let scale = self.display_scale();
        let drawn = size(px(natural.width * scale), px(natural.height * scale));
        let viewport = self.preview_stage.get().size;
        let padding = px(2. * PREVIEW_PADDING);
        let content = viewport.max(&size(drawn.width + padding, drawn.height + padding));
        let drag = self.preview_drag.clone();
        let scroll = self.preview_scroll.clone();
        let pan_events = preview_viewport::pan_events(drag.clone(), scroll.clone());
        div()
            .id("molecule-preview-image")
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("molecule-preview-scroll")
                    .absolute()
                    .inset_0()
                    .overflow_scroll()
                    .track_scroll(&scroll)
                    .when(content != viewport, |surface| {
                        surface.cursor(if drag.get().is_some() {
                            CursorStyle::ClosedHand
                        } else {
                            CursorStyle::OpenHand
                        })
                    })
                    .on_mouse_down(MouseButton::Left, move |event, window, _| {
                        if scroll.max_offset() != Point::default() {
                            drag.set(Some(event.position));
                            window.refresh();
                        }
                    })
                    .child(
                        h_flex()
                            .w(content.width)
                            .h(content.height)
                            .justify_center()
                            .child(
                                img(ImageSource::Render(preview.clone()))
                                    .w(drawn.width)
                                    .h(drawn.height),
                            ),
                    ),
            )
            .child(pan_events)
            .child(FluidScrollbar::vertical(
                "molecule-preview-scrollbar",
                &self.preview_scroll,
            ))
            .child(FluidScrollbar::horizontal(
                "molecule-preview-scrollbar-x",
                &self.preview_scroll,
            ))
            .into_any_element()
    }

    fn display_scale(&self) -> f32 {
        let Some(preview) = &self.preview else {
            return self.scale;
        };
        if self.fit == PreviewFit::Manual {
            return self.scale;
        }
        let bounds = self.preview_stage.get().size;
        let room = size(
            (f32::from(bounds.width) - 2. * PREVIEW_PADDING).max(1.),
            (f32::from(bounds.height) - 2. * PREVIEW_PADDING).max(1.),
        );
        fit_scale(preview_size(preview), room, self.fit)
    }

    fn step_zoom(&mut self, direction: f32, cx: &mut Context<Self>) {
        self.scale = zoom_step(self.display_scale(), direction);
        self.fit = PreviewFit::Manual;
        cx.notify();
    }

    fn set_fit(&mut self, fit: PreviewFit, cx: &mut Context<Self>) {
        self.fit = fit;
        self.scale = 1.;
        cx.notify();
    }

    fn select_molecule(&mut self, location: MoleculeLocation, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&location) {
            return;
        }
        self.selected = Some(location);
        self.load_preview(cx);
        cx.notify();
    }

    fn set_project(&mut self, root: &Path, files: &[FileNode], cx: &mut Context<Self>) {
        let files: Vec<_> = chemistry::cdxml_files(files).map(str::to_owned).collect();
        if self.root.as_deref() == Some(root) && self.files == files {
            return;
        }
        self.root = Some(root.to_path_buf());
        self.files = files;
        self.selected = None;
        self.clear_preview();
        self.invalidate();
        cx.notify();
    }

    fn files_changed(&mut self, paths: Option<&[String]>, cx: &mut Context<Self>) {
        if paths.is_none_or(|paths| {
            paths
                .iter()
                .any(|path| chemistry::is_cdxml(Path::new(path)))
        }) {
            self.clear_preview();
            self.invalidate();
            self.analyze(cx);
            cx.notify();
        }
    }

    fn clear_preview(&mut self) {
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.preview_task = None;
        self.preview = None;
        self.preview_error = None;
        self.preview_loading = false;
        self.preview_drag.set(None);
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
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
                        let selected_exists = panel.selected.as_ref().is_some_and(|selected| {
                            result.as_ref().ok().is_some_and(|report| {
                                report
                                    .molecules
                                    .iter()
                                    .any(|molecule| molecule.location == *selected)
                            })
                        });
                        if !selected_exists {
                            panel.selected = None;
                            panel.clear_preview();
                        }
                        panel.report = Some(result);
                        if selected_exists {
                            panel.load_preview(cx);
                        }
                    } else {
                        panel.analyze(cx);
                    }
                    cx.notify();
                })
                .ok();
        }));
        cx.notify();
    }

    fn load_preview(&mut self, cx: &mut Context<Self>) {
        let (Some(root), Some(location)) = (self.root.clone(), self.selected.clone()) else {
            self.clear_preview();
            return;
        };
        self.preview_generation = self.preview_generation.wrapping_add(1);
        let generation = self.preview_generation;
        let renderer = cx.svg_renderer();
        self.preview_task = None;
        self.preview = None;
        self.preview_error = None;
        self.preview_loading = true;
        self.preview_task = Some(cx.spawn(async move |panel, cx| {
            let result = cx
                .background_spawn(async move {
                    let path = project::safe_path(&root, &location.file)?;
                    let document = chemistry::read_document(&path)?;
                    let svg = chemistry::preview_group_svg(&root, &document, &location.key)?;
                    renderer
                        .render_single_frame(&svg, RENDER_SCALE)
                        .map_err(|error| error.to_string())
                })
                .await;
            panel
                .update(cx, |panel, cx| {
                    if panel.preview_generation != generation {
                        return;
                    }
                    panel.preview_task = None;
                    panel.preview_loading = false;
                    match result {
                        Ok(preview) => panel.preview = Some(preview),
                        Err(error) => {
                            panel.preview_error = Some(
                                t!("Could not render molecule preview").to_string()
                                    + &format!(": {error}"),
                            );
                        }
                    }
                    cx.notify();
                })
                .ok();
        }));
        cx.notify();
    }
}

fn formula(molecule: &ProjectMolecule) -> String {
    molecule
        .structure
        .formula
        .as_deref()
        .map(chemistry::display_formula)
        .unwrap_or_else(|| t!("Formula unavailable").to_string())
}

fn molecule_status(molecule: &ProjectMolecule) -> Option<String> {
    molecule
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
        })
}

fn location_label(location: &MoleculeLocation) -> String {
    format!("{} · {}", location.file, location.key)
}

fn molecule_layout(width: Pixels) -> MoleculeLayout {
    if f32::from(width) < NARROW_WIDTH {
        MoleculeLayout::Stacked
    } else {
        MoleculeLayout::Wide
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn molecule_panel_switches_between_stacked_and_wide_layouts() {
        assert_eq!(MoleculeLayout::Stacked, molecule_layout(px(480.)));
        assert_eq!(MoleculeLayout::Wide, molecule_layout(px(560.)));
    }

    #[test]
    fn preview_fit_modes_and_zoom_are_bounded() {
        assert_eq!(
            0.25,
            fit_scale(size(400., 200.), size(100., 300.), PreviewFit::Both)
        );
        assert_eq!(
            3.,
            fit_scale(size(10., 10.), size(500., 500.), PreviewFit::Width)
        );
        assert_eq!(0.1, zoom_step(0.12, -1.));
        assert_eq!(3., zoom_step(2.98, 1.));
    }
}
