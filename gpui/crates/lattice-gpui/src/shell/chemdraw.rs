//! CDXML browsing and numbered chemistry previews.

use std::{
    any::Any,
    cell::Cell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use chemex_cdxml::DrawingDocument;
use gpui::{
    App, AppContext as _, Bounds, Context, CursorStyle, Entity, FocusHandle, Focusable,
    ImageSource, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels,
    Point, RenderImage, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _,
    Task, UniformListScrollHandle, Window, actions, canvas, div, img, prelude::FluentBuilder as _,
    px, size,
};
use gpui_kit::component::{
    dock::{BasePanel, Panel, PanelEvent, panel_handle},
    h_flex, v_flex,
};
use lattice_core::chemistry::cdxml_files;
use lattice_core::{chemistry, models::FileNode, project};
use lattice_i18n::t;
use lattice_ui::{
    ActiveLattice as _, Icon, TextRole, TextRoleStyle as _,
    components::{Button, EmptyState, IconButton},
    scrollbar::FluidScrollbar,
    tokens::{SPACING_2, SPACING_4},
};

use super::preview_viewport::{
    self, MAX_SCALE, MIN_SCALE, PREVIEW_PADDING, PreviewFit, RENDER_SCALE, fit_scale, pan_offset,
    preview_size, zoom_step,
};
use super::{
    Shell,
    commands::{Command, CommandContext, Section},
    parts::{Part, View},
};

mod preview_cache;
mod view;
mod warmup;
use preview_cache::PreviewCache;
use warmup::Warmup;

const GROUPS_WIDTH: f32 = 152.;
const NARROW_WIDTH: f32 = 520.;
const WIDE_ASPECT: f32 = 1.5;

actions!(lattice, [OpenChemDraw, ToggleChemDraw]);

#[derive(Clone, Copy, PartialEq, Eq)]
enum AllPreview {
    Source,
    Rendered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChemistryLayout {
    Stacked,
    Standard,
    Wide,
}

pub(crate) struct ChemDrawPanel {
    focus_handle: FocusHandle,
    root: Option<PathBuf>,
    paper: Option<String>,
    files: Vec<String>,
    selected_file: Option<String>,
    show_details: bool,
    document: Option<Arc<DrawingDocument>>,
    groups: Vec<String>,
    numbers: Option<BTreeMap<String, String>>,
    warmup: Warmup,
    compound_id: Option<String>,
    all_preview: AllPreview,
    preview: Option<Arc<RenderImage>>,
    scale: f32,
    fit: PreviewFit,
    cache: Arc<PreviewCache>,
    error: Option<String>,
    loading: bool,
    generation: u64,
    files_scroll: UniformListScrollHandle,
    preview_scroll: ScrollHandle,
    preview_drag: Rc<Cell<Option<Point<Pixels>>>>,
    preview_stage: Rc<Cell<Bounds<gpui::Pixels>>>,
    panel_bounds: Rc<Cell<Bounds<gpui::Pixels>>>,
    _load: Option<Task<()>>,
}

impl gpui::EventEmitter<PanelEvent> for ChemDrawPanel {}

impl Focusable for ChemDrawPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for ChemDrawPanel {
    fn panel_name(&self) -> &'static str {
        "ChemDrawPanel"
    }

    fn closable(&self, _: &App) -> bool {
        true
    }
}

impl Panel for ChemDrawPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(t!("ChemDraw").into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        t!("ChemDraw")
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl ChemDrawPanel {
    fn preview_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = self.display_scale();
        let no_preview = self.preview.is_none();
        let zoom = h_flex()
            .flex_none()
            .child(
                IconButton::new("chemistry-zoom-out", Icon::ZOOM_OUT)
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
                IconButton::new("chemistry-zoom-in", Icon::ZOOM_IN)
                    .tooltip(t!("Zoom in"))
                    .disabled(no_preview || scale >= MAX_SCALE)
                    .on_click(cx.listener(|this, _, _, cx| this.step_zoom(1., cx))),
            );
        let fit = h_flex()
            .flex_none()
            .child(
                IconButton::new("chemistry-fit-width", Icon::RECTANGLE_HORIZONTAL)
                    .tooltip(t!("Fit page to width"))
                    .on(matches!(self.fit, PreviewFit::Width | PreviewFit::Both))
                    .disabled(no_preview)
                    .on_click(cx.listener(|this, _, _, cx| this.set_fit(PreviewFit::Width, cx))),
            )
            .child(
                IconButton::new("chemistry-fit-height", Icon::RECTANGLE_VERTICAL)
                    .tooltip(t!("Fit page to height"))
                    .on(self.fit == PreviewFit::Height)
                    .disabled(no_preview)
                    .on_click(cx.listener(|this, _, _, cx| this.set_fit(PreviewFit::Height, cx))),
            );
        h_flex()
            .id("chemistry-preview-toolbar")
            .w_full()
            .min_w_0()
            .flex_none()
            .flex_wrap()
            .gap(SPACING_2)
            .px(SPACING_4)
            .py(SPACING_2)
            .border_b_1()
            .border_color(cx.palette().line)
            .bg(cx.palette().editor)
            .child(zoom)
            .child(fit)
            .child(
                Button::new("chemistry-open-chemdraw", t!("Open in ChemDraw"))
                    .compact()
                    .ghost()
                    .trailing_icon(Icon::EXTERNAL_LINK)
                    .max_w_full()
                    .truncate()
                    .tooltip(t!("Open in ChemDraw"))
                    .disabled(self.selected_file.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.open_selected(cx))),
            )
    }

    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            root: None,
            paper: None,
            files: Vec::new(),
            selected_file: None,
            show_details: false,
            document: None,
            groups: Vec::new(),
            numbers: None,
            warmup: Warmup::default(),
            compound_id: None,
            all_preview: AllPreview::Source,
            preview: None,
            scale: 1.,
            fit: PreviewFit::Both,
            cache: Arc::default(),
            error: None,
            loading: false,
            generation: 0,
            files_scroll: UniformListScrollHandle::new(),
            preview_scroll: ScrollHandle::new(),
            preview_drag: Rc::default(),
            preview_stage: Rc::default(),
            panel_bounds: Rc::default(),
            _load: None,
        }
    }

    fn set_project(&mut self, root: &Path, files: &[String], cx: &mut Context<Self>) {
        if self.root.as_deref() == Some(root) {
            if self.files == files {
                return;
            }
            self.files = files.to_vec();
            if self
                .selected_file
                .as_ref()
                .is_some_and(|file| self.files.contains(file))
            {
                cx.notify();
                return;
            }
        } else {
            self.root = Some(root.to_path_buf());
            self.paper = None;
            self.numbers = None;
            self.files = files.to_vec();
        }
        self.selected_file = self.files.first().cloned();
        self.show_details = false;
        self.reset_drawing();
        cx.notify();
    }

    fn reset_drawing(&mut self) {
        self.document = None;
        self.groups.clear();
        self.compound_id = None;
        self.all_preview = AllPreview::Source;
        self.warmup.reset();
        self.invalidate();
        self.preview = None;
    }

    fn invalidate(&mut self) {
        self.cancel_load();
        self.error = None;
    }

    fn select_file(&mut self, relative: &str, cx: &mut Context<Self>) {
        if !self.files.iter().any(|file| file == relative) {
            return;
        }
        let size = self.panel_bounds.get().size;
        let opening_details = !self.show_details
            && chemistry_layout(size.width, size.height) == ChemistryLayout::Stacked;
        self.show_details = true;
        if self.selected_file.as_deref() == Some(relative) {
            if self.compound_id.is_some() || (!self.loading && self.preview.is_none()) {
                self.select_compound(None, cx);
            } else if opening_details {
                cx.notify();
            }
            return;
        }
        self.selected_file = Some(relative.to_owned());
        self.reset_drawing();
        self.load_preview(None, cx);
    }

    fn select_compound(&mut self, compound_id: Option<&str>, cx: &mut Context<Self>) {
        if compound_id.is_some_and(|id| !self.groups.iter().any(|group| group == id)) {
            return;
        }
        if self.compound_id.as_deref() == compound_id && (self.loading || self.preview.is_some()) {
            return;
        }
        self.load_preview(compound_id.map(str::to_owned), cx);
    }

    fn select_all_preview(&mut self, mode: AllPreview, cx: &mut Context<Self>) {
        if self.compound_id.is_some() || self.all_preview == mode {
            return;
        }
        self.all_preview = mode;
        self.load_preview(None, cx);
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

    fn open_selected(&self, cx: &mut Context<Self>) {
        if let (Some(root), Some(file)) = (&self.root, &self.selected_file) {
            crate::opener::open_project_file(root, file, cx);
        }
    }

    fn load_preview(&mut self, compound_id: Option<String>, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(file) = self.selected_file.clone() else {
            return;
        };
        let paper = self.paper.clone();
        let all_preview = self.all_preview;
        let cache = self.cache.clone();
        let document = self.document.clone();
        let cached_numbers = self.numbers.clone();
        self.cancel_load();
        self.warmup.pause();
        let generation = self.generation;
        let renderer = cx.svg_renderer();
        self.compound_id = compound_id.clone();
        self.preview = None;
        self.error = None;
        self.loading = true;
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let numbers = cached_numbers.unwrap_or_else(|| {
                        paper
                            .as_deref()
                            .and_then(|paper| chemistry::built_compound_numbers(&root, paper))
                            .unwrap_or_default()
                    });
                    let (document, parsed) = match document {
                        Some(document) => (document, None),
                        None => {
                            let path = project::safe_path(&root, &file)?;
                            let document = Arc::new(chemistry::read_document(&path)?);
                            let groups = chemistry::compound_groups(&document);
                            (document.clone(), Some((document, groups, numbers.clone())))
                        }
                    };
                    let svg = match compound_id.as_deref() {
                        Some(id) => chemistry::preview_group_svg(&root, &document, id),
                        None => match all_preview {
                            AllPreview::Source => chemistry::preview_svg(&root, document.source()),
                            AllPreview::Rendered => paper
                                .as_deref()
                                .and_then(|paper| {
                                    chemistry::built_preview_svg(&root, paper, document.source())
                                })
                                .map_or_else(
                                    || {
                                        chemistry::render_compound_numbers(&document, &numbers)
                                            .and_then(|source| {
                                                chemistry::preview_svg(&root, &source)
                                            })
                                    },
                                    Ok,
                                ),
                        },
                    };
                    let preview = svg.and_then(|svg| cache.render(&svg, &renderer));
                    Ok::<_, String>((parsed, preview, numbers))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                let result = result.and_then(|(parsed, preview, numbers)| {
                    if let Some((document, groups, numbers)) = parsed {
                        this.document = Some(document);
                        this.groups = groups;
                        this.numbers = Some(numbers);
                    } else {
                        this.numbers = Some(numbers);
                    }
                    preview
                });
                match result {
                    Ok(preview) => this.preview = Some(preview),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
                this.warm_groups(cx);
            })
            .ok();
        }));
        cx.notify();
    }

    fn warm_groups(&mut self, cx: &mut Context<Self>) {
        if self.loading || !self.warmup.allowed {
            return;
        }
        if let Some(root) = self.root.clone()
            && let Some(document) = self.document.clone()
        {
            self.warmup.start((root, document, self.groups.clone()), cx);
        }
    }

    fn cancel_load(&mut self) {
        self.preview_drag.set(None);
        self._load = None;
        self.loading = false;
        self.generation = self.generation.wrapping_add(1);
    }
}

impl ChemDrawPanel {
    fn preview_stage(&self, cx: &App) -> impl IntoElement + use<> {
        let bounds = self.preview_stage.clone();
        let content = match (&self.preview, &self.error, self.loading) {
            (Some(preview), _, _) => self.preview_image(preview).into_any_element(),
            (None, Some(error), _) => div()
                .text_color(cx.palette().text_secondary)
                .child(error.clone())
                .into_any_element(),
            (None, None, true) => div()
                .text_color(cx.palette().text_secondary)
                .child(t!("Exporting with ChemDraw…"))
                .into_any_element(),
            (None, None, false) => EmptyState::new(if self.files.is_empty() {
                t!("No CDXML files in this project")
            } else {
                t!("Choose a CDXML file to preview")
            })
            .into_any_element(),
        };
        div()
            .id("chemistry-preview-stage")
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
    }

    fn preview_image(&self, preview: &Arc<RenderImage>) -> impl IntoElement {
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
            .id("chemistry-preview-image")
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("chemistry-preview-scroll")
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
                "chemistry-preview-scrollbar",
                &self.preview_scroll,
            ))
            .child(FluidScrollbar::horizontal(
                "chemistry-preview-scrollbar-x",
                &self.preview_scroll,
            ))
    }
}

fn chemistry_layout(width: gpui::Pixels, height: gpui::Pixels) -> ChemistryLayout {
    let width = f32::from(width);
    let height = f32::from(height);
    if width < NARROW_WIDTH || height / width.max(1.) >= WIDE_ASPECT {
        ChemistryLayout::Stacked
    } else if width / height.max(1.) >= WIDE_ASPECT {
        ChemistryLayout::Wide
    } else {
        ChemistryLayout::Standard
    }
}

fn install(_: &mut Window, cx: &mut Context<Shell>) -> Box<dyn Any> {
    Box::new(cx.new(ChemDrawPanel::new))
}

pub(super) const PART: Part = Part {
    install,
    views: &[View {
        kind: "chemdraw",
        min: (360., 220.),
        panel: |shell| panel_handle(shell.parts.get::<Entity<ChemDrawPanel>>().clone()),
    }],
    actions: |element, cx| {
        element
            .on_action(cx.listener(|this, _: &OpenChemDraw, window, cx| {
                this.open_chemdraw(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleChemDraw, window, cx| {
                this.toggle_view("chemdraw", window, cx);
            }))
    },
    commands,
};

impl Shell {
    pub(super) fn chemdraw_project_opened(
        &mut self,
        root: &Path,
        files: &[FileNode],
        cx: &mut Context<Self>,
    ) {
        let files: Vec<_> = cdxml_files(files).map(str::to_owned).collect();
        self.parts
            .get::<Entity<ChemDrawPanel>>()
            .update(cx, |panel, cx| panel.set_project(root, &files, cx));
    }

    pub(super) fn sync_chemdraw_preview(&mut self, cx: &mut Context<Self>) {
        if !self.view_on_screen("chemdraw", cx) || self.build_task.is_some() {
            self.pause_chemistry_warmup(cx);
            return;
        }
        let paper = self
            .project
            .as_ref()
            .and_then(|project| project.read(cx).main_document().map(str::to_owned));
        self.parts
            .get::<Entity<ChemDrawPanel>>()
            .update(cx, |panel, cx| {
                let paper_changed = panel.paper != paper;
                if paper_changed {
                    panel.invalidate();
                    panel.numbers = None;
                }
                panel.paper = paper;
                panel.warmup.allowed = true;
                if paper_changed
                    || (!panel.loading && (panel.preview.is_none() || panel.error.is_some()))
                {
                    panel.load_preview(panel.compound_id.clone(), cx);
                } else {
                    panel.warm_groups(cx);
                }
            });
    }

    pub(super) fn pause_chemistry_warmup(&mut self, cx: &mut Context<Self>) {
        self.parts
            .get::<Entity<ChemDrawPanel>>()
            .update(cx, |panel, _| {
                panel.warmup.allowed = false;
                panel.warmup.pause();
            });
    }

    pub(super) fn chemdraw_files_changed(
        &mut self,
        paths: Option<&[String]>,
        cx: &mut Context<Self>,
    ) {
        let changed = self
            .parts
            .get::<Entity<ChemDrawPanel>>()
            .update(cx, |panel, cx| {
                let changed = panel.selected_file.as_ref().is_some_and(|file| {
                    paths.is_none_or(|paths| paths.iter().any(|path| path == file))
                });
                if changed {
                    panel.reset_drawing();
                    cx.notify();
                }
                changed
            });
        if changed {
            self.sync_chemdraw_preview(cx);
        }
    }

    pub(super) fn chemdraw_built(&mut self, document: &str, cx: &mut Context<Self>) {
        if self
            .project
            .as_ref()
            .and_then(|project| project.read(cx).main_document())
            == Some(document)
        {
            self.parts
                .get::<Entity<ChemDrawPanel>>()
                .update(cx, |panel, cx| {
                    panel.paper = None;
                    panel.numbers = None;
                    panel.invalidate();
                    panel.paper = Some(document.to_owned());
                    panel.load_preview(panel.compound_id.clone(), cx);
                });
        }
    }

    fn open_chemdraw(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root(cx).is_none() {
            return;
        }
        self.show_view("chemdraw", true, window, cx);
        self.sync_chemdraw_preview(cx);
    }
}

fn commands(context: &CommandContext, out: &mut Vec<Command>) {
    if context.in_project {
        out.push(
            Command::new(
                "panel-chemdraw",
                t!("Show ChemDraw panel"),
                Section::Layout,
                OpenChemDraw,
            )
            .keywords("chemistry chemdraw cdxml compound number drawing"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, kind: &str, children: Vec<FileNode>) -> FileNode {
        FileNode {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            path: path.to_string(),
            kind: kind.to_string(),
            content_kind: "binary".into(),
            size: 0,
            children,
        }
    }

    #[test]
    fn lists_nested_cdxml_files_and_ignores_binary_cdx() {
        let files = vec![
            file("drawing.cdx", "file", Vec::new()),
            file(
                "figures",
                "directory",
                vec![
                    file("figures/first.CDXML", "file", Vec::new()),
                    file("figures/notes.txt", "file", Vec::new()),
                    file("figures/second.cdxml", "file", Vec::new()),
                ],
            ),
        ];
        assert_eq!(
            vec!["figures/first.CDXML", "figures/second.cdxml"],
            cdxml_files(&files).collect::<Vec<_>>()
        );
    }

    #[test]
    fn panel_layout_adapts_to_portrait_standard_and_wide_sizes() {
        assert_eq!(
            ChemistryLayout::Stacked,
            chemistry_layout(px(360.), px(640.))
        );
        assert_eq!(ChemistryLayout::Wide, chemistry_layout(px(960.), px(520.)));
        assert_eq!(
            ChemistryLayout::Standard,
            chemistry_layout(px(760.), px(640.))
        );
        assert_eq!(
            ChemistryLayout::Stacked,
            chemistry_layout(px(480.), px(300.))
        );
        assert_eq!(
            ChemistryLayout::Standard,
            chemistry_layout(px(520.), px(520.))
        );
    }

    #[test]
    fn preview_fit_modes_respect_the_requested_axis_and_manual_mode() {
        let natural = size(400., 200.);
        let room = size(100., 300.);
        assert_eq!(0.25, fit_scale(natural, room, PreviewFit::Both));
        assert_eq!(0.25, fit_scale(natural, room, PreviewFit::Width));
        assert_eq!(fit_scale(natural, room, PreviewFit::Height), 1.5);
        assert_eq!(
            fit_scale(natural, size(800., 100.), PreviewFit::Height),
            0.5
        );
        assert_eq!(fit_scale(natural, room, PreviewFit::Manual), 1.);
    }

    #[test]
    fn preview_zoom_stops_at_ten_and_three_hundred_percent() {
        assert_eq!(0.1, zoom_step(0.12, -1.));
        assert_eq!(3., zoom_step(2.98, 1.));
        assert_eq!(
            0.1,
            fit_scale(size(500., 500.), size(10., 10.), PreviewFit::Both)
        );
        assert_eq!(
            3.,
            fit_scale(size(10., 10.), size(500., 500.), PreviewFit::Width)
        );
    }

    #[test]
    fn preview_pan_stays_inside_the_scrollable_area() {
        use gpui::point;

        assert_eq!(
            point(px(0.), px(-80.)),
            pan_offset(
                point(px(-5.), px(-40.)),
                point(px(30.), px(-50.)),
                point(px(100.), px(80.))
            ),
        );
        assert_eq!(
            point(px(-35.), px(0.)),
            pan_offset(
                point(px(-5.), px(0.)),
                point(px(-30.), px(-50.)),
                point(px(100.), px(0.))
            ),
        );
    }
}
