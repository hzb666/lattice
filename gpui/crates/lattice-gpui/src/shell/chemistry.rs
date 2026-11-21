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
    App, AppContext as _, Bounds, Context, CursorStyle, DispatchPhase, Entity, FocusHandle,
    Focusable, ImageSource, InteractiveElement as _, IntoElement, MouseButton, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, Point, Render, RenderImage, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Task, UniformListScrollHandle,
    Window, actions, canvas, div, img, prelude::FluentBuilder as _, px, relative, size,
    uniform_list,
};
use gpui_kit::component::{
    Selectable as _,
    dock::{BasePanel, Panel, PanelEvent, panel_handle},
    h_flex,
    menu::{ContextMenuExt as _, PopupMenuItem},
    v_flex,
};
use lattice_core::{chemistry, models::FileNode, project};
use lattice_i18n::t;
use lattice_ui::{
    ActiveLattice as _, Icon, TextRole, TextRoleStyle as _,
    components::{Button, EmptyState, IconButton, LatticeTooltip, TreeRow},
    scrollbar::FluidScrollbar,
    tokens::{SPACING_2, SPACING_4},
};

use super::{
    Shell,
    commands::{Command, CommandContext, Section},
    parts::{Part, View},
};

mod preview_cache;
mod warmup;
use preview_cache::PreviewCache;
use warmup::Warmup;

const MIN_SCALE: f32 = 0.1;
const MAX_SCALE: f32 = 3.;
const ZOOM_STEP: f32 = 0.1;
const RENDER_SCALE: f32 = 2.;
const PREVIEW_PADDING: f32 = 20.;
const GROUPS_WIDTH: f32 = 152.;
const NARROW_WIDTH: f32 = 520.;
const WIDE_ASPECT: f32 = 1.5;

actions!(lattice, [OpenChemistry, ToggleChemistry]);

#[derive(Clone, Copy, PartialEq, Eq)]
enum AllPreview {
    Source,
    Rendered,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PreviewFit {
    Both,
    Width,
    Height,
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChemistryLayout {
    Stacked,
    Standard,
    Wide,
}

pub(crate) struct ChemistryPanel {
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

impl gpui::EventEmitter<PanelEvent> for ChemistryPanel {}

impl Focusable for ChemistryPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for ChemistryPanel {
    fn panel_name(&self) -> &'static str {
        "ChemistryPanel"
    }

    fn closable(&self, _: &App) -> bool {
        true
    }
}

impl Panel for ChemistryPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(t!("Chemistry").into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        t!("Chemistry")
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for ChemistryPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.palette().clone();
        let root = self.root.clone();
        let layout = chemistry_layout(
            self.panel_bounds.get().size.width,
            self.panel_bounds.get().size.height,
        );
        let files = render_file_list(
            "chemistry-files",
            &self.files,
            if layout != ChemistryLayout::Stacked || self.show_details {
                self.selected_file.as_deref()
            } else {
                None
            },
            root.clone(),
            &self.files_scroll,
            cx,
            |this, path, cx| this.select_file(path, cx),
        );
        let stage = self.preview_stage(cx);
        let content = div()
            .id("chemistry-preview-content")
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when(layout != ChemistryLayout::Wide, |content| {
                content.flex_col()
            })
            .when(self.document.is_some(), |content| {
                content.child(self.group_list(layout, cx))
            })
            .child(stage);
        let preview = v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(self.preview_toolbar(cx))
            .child(content);
        workspace(
            &self.focus_handle,
            palette,
            "chemistry-file-browser",
            files,
            preview,
            layout,
            layout != ChemistryLayout::Stacked || self.show_details,
            self.panel_bounds.clone(),
        )
    }
}

impl ChemistryPanel {
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

    fn group_list(&self, layout: ChemistryLayout, cx: &mut Context<Self>) -> gpui::AnyElement {
        let all = Button::new("chemistry-compound-all", t!("All"))
            .compact()
            .ghost()
            .selected(self.compound_id.is_none())
            .on_click(cx.listener(|this, _, _, cx| this.select_compound(None, cx)));
        let source = Button::new("chemistry-all-source", t!("Source"))
            .compact()
            .ghost()
            .selected(self.compound_id.is_none() && self.all_preview == AllPreview::Source)
            .on_click(
                cx.listener(|this, _, _, cx| this.select_all_preview(AllPreview::Source, cx)),
            );
        let rendered = Button::new("chemistry-all-rendered", t!("Rendered"))
            .compact()
            .ghost()
            .selected(self.compound_id.is_none() && self.all_preview == AllPreview::Rendered)
            .disabled(self.numbers.as_ref().is_none_or(BTreeMap::is_empty))
            .on_click(
                cx.listener(|this, _, _, cx| this.select_all_preview(AllPreview::Rendered, cx)),
            );
        let compounds = self.groups.iter().map(|compound_id| {
            let id = compound_id.clone();
            let label = self
                .numbers
                .as_ref()
                .and_then(|numbers| numbers.get(&id))
                .map_or_else(|| id.clone(), |number| format!("{id} · {number}"));
            Button::new(format!("chemistry-compound:{id}"), label.clone())
                .compact()
                .ghost()
                .max_w_full()
                .truncate()
                .tooltip(label)
                .selected(self.compound_id.as_deref() == Some(id.as_str()))
                .on_click(cx.listener(move |this, _, _, cx| this.select_compound(Some(&id), cx)))
        });
        let buttons = div()
            .flex()
            .w_full()
            .min_w_0()
            .flex_none()
            .gap(SPACING_2)
            .p(SPACING_4)
            .when(layout == ChemistryLayout::Wide, |buttons| {
                buttons.flex_col().items_start()
            })
            .when(layout != ChemistryLayout::Wide, |buttons| {
                buttons.flex_wrap().items_center().content_start()
            })
            .child(all)
            .when(self.compound_id.is_none(), |bar| {
                bar.child(source).child(rendered)
            })
            .children(compounds);
        v_flex()
            .id("chemistry-compounds")
            .flex_none()
            .min_w_0()
            .min_h_0()
            .border_color(cx.palette().line)
            .when(layout == ChemistryLayout::Wide, |list| {
                list.w(px(GROUPS_WIDTH)).border_r_1()
            })
            .when(layout != ChemistryLayout::Wide, |list| {
                list.w_full().max_h(relative(0.4)).border_b_1()
            })
            .overflow_y_scroll()
            .child(buttons)
            .into_any_element()
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
            crate::opener::open_cdxml(root, file, cx);
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
                            let groups = compound_groups(&document);
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

fn render_file_list<T, F>(
    id: &'static str,
    files: &[String],
    selected: Option<&str>,
    root: Option<PathBuf>,
    scroll: &UniformListScrollHandle,
    cx: &mut Context<T>,
    select: F,
) -> gpui::AnyElement
where
    T: 'static,
    F: Fn(&mut T, &str, &mut Context<T>) + Clone + 'static,
{
    let files = files.to_vec();
    let selected = selected.map(str::to_owned);
    uniform_list(
        id,
        files.len(),
        cx.processor(move |_this, range: std::ops::Range<usize>, _, cx| {
            range
                .map(|index| {
                    let path = files[index].clone();
                    let selected = selected.as_deref() == Some(path.as_str());
                    let select = select.clone();
                    let selected_path = path.clone();
                    let menu_root = root.clone();
                    TreeRow::file(format!("{id}:{path}"), path.clone(), 0)
                        .selected(selected)
                        .into_list_item(cx)
                        .w_full()
                        .tooltip({
                            let path = path.clone();
                            move |_, cx| LatticeTooltip::view(path.clone(), None, cx)
                        })
                        .on_click(
                            cx.listener(move |this, _, _, cx| select(this, &selected_path, cx)),
                        )
                        .context_menu(move |menu, _, _| {
                            if !is_cdxml(Path::new(&path)) {
                                return menu;
                            }
                            let menu_path = path.clone();
                            let menu_root = menu_root.clone();
                            menu.item(
                                PopupMenuItem::new(t!("Open in ChemDraw"))
                                    .icon(Icon::EXTERNAL_LINK.kit())
                                    .on_click(move |_, _, cx| {
                                        if let Some(root) = &menu_root {
                                            crate::opener::open_cdxml(root, &menu_path, cx);
                                        }
                                    }),
                            )
                        })
                })
                .collect()
        }),
    )
    .track_scroll(scroll)
    .size_full()
    .into_any_element()
}

fn workspace(
    focus: &FocusHandle,
    palette: lattice_ui::Palette,
    browser_id: &'static str,
    files: impl IntoElement,
    preview: impl IntoElement,
    layout: ChemistryLayout,
    selected: bool,
    bounds: Rc<Cell<Bounds<gpui::Pixels>>>,
) -> impl IntoElement {
    let content = if layout == ChemistryLayout::Stacked {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(file_browser(browser_id, files, layout, selected))
            .when(selected, |this| this.child(preview))
            .into_any_element()
    } else {
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(file_browser(browser_id, files, layout, true))
            .child(preview)
            .into_any_element()
    };
    v_flex()
        .track_focus(focus)
        .size_full()
        .bg(palette.panel)
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

fn file_browser(
    id: &'static str,
    rows: impl IntoElement,
    layout: ChemistryLayout,
    has_preview: bool,
) -> impl IntoElement {
    div()
        .id(id)
        .when(layout == ChemistryLayout::Stacked, |browser| {
            browser.w_full().flex_1()
        })
        .when(layout != ChemistryLayout::Stacked, |browser| {
            browser.w(px(176.)).flex_none()
        })
        .min_h_0()
        .min_w_0()
        .flex()
        .flex_col()
        .when(layout == ChemistryLayout::Stacked, |browser| {
            browser.when(has_preview, |browser| browser.border_b_1())
        })
        .when(layout != ChemistryLayout::Stacked, |browser| {
            browser.border_r_1()
        })
        .border_color(gpui::transparent_black())
        .child(div().flex_1().min_h_0().child(rows))
}

impl ChemistryPanel {
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
            .child(self.preview_pan_events())
            .child(FluidScrollbar::vertical(
                "chemistry-preview-scrollbar",
                &self.preview_scroll,
            ))
            .child(FluidScrollbar::horizontal(
                "chemistry-preview-scrollbar-x",
                &self.preview_scroll,
            ))
    }

    fn preview_pan_events(&self) -> impl IntoElement {
        let drag = self.preview_drag.clone();
        let scroll = self.preview_scroll.clone();
        canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                let move_drag = drag.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
                    if phase != DispatchPhase::Capture || move_drag.get().is_none() {
                        return;
                    }
                    if event.pressed_button != Some(MouseButton::Left) {
                        move_drag.set(None);
                        window.refresh();
                    } else if let Some(last) = move_drag.replace(Some(event.position)) {
                        scroll.set_offset(pan_offset(
                            scroll.offset(),
                            event.position - last,
                            scroll.max_offset(),
                        ));
                        window.refresh();
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, _| {
                    if phase == DispatchPhase::Capture
                        && event.button == MouseButton::Left
                        && drag.take().is_some()
                    {
                        window.refresh();
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}

fn preview_size(preview: &RenderImage) -> gpui::Size<f32> {
    let size = preview.size(0);
    size.map(|length| length.0 as f32 / RENDER_SCALE)
}

fn pan_offset(
    offset: Point<Pixels>,
    delta: Point<Pixels>,
    overflow: Point<Pixels>,
) -> Point<Pixels> {
    (offset + delta).clamp(&(-overflow), &Point::default())
}

fn zoom_step(scale: f32, direction: f32) -> f32 {
    (((scale + direction * ZOOM_STEP) / ZOOM_STEP).round() * ZOOM_STEP).clamp(MIN_SCALE, MAX_SCALE)
}

fn fit_scale(natural: gpui::Size<f32>, room: gpui::Size<f32>, fit: PreviewFit) -> f32 {
    let width = natural.width.max(1.);
    let height = natural.height.max(1.);
    match fit {
        PreviewFit::Both => (room.width / width).min(room.height / height).min(1.),
        PreviewFit::Width => room.width / width,
        PreviewFit::Height => room.height / height,
        PreviewFit::Manual => 1.,
    }
    .clamp(MIN_SCALE, MAX_SCALE)
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

fn compound_groups(document: &chemex_cdxml::DrawingDocument) -> Vec<String> {
    let mut groups = Vec::new();
    for compound_id in document
        .fields()
        .iter()
        .map(|field| field.compound_id.as_str())
    {
        if groups.iter().any(|key| key == compound_id) {
            continue;
        }
        if document.compound_group(compound_id).is_ok() {
            groups.push(compound_id.to_owned());
        }
    }
    groups
}

fn cdxml_files(files: &[FileNode]) -> Vec<String> {
    let mut result = Vec::new();
    collect_cdxml_files(files, &mut result);
    result
}

fn collect_cdxml_files(files: &[FileNode], result: &mut Vec<String>) {
    for file in files {
        if file.kind == "directory" {
            collect_cdxml_files(&file.children, result);
        } else if is_cdxml(Path::new(&file.path)) {
            result.push(file.path.clone());
        }
    }
}

fn is_cdxml(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cdxml"))
}

fn install(_: &mut Window, cx: &mut Context<Shell>) -> Box<dyn Any> {
    Box::new(cx.new(ChemistryPanel::new))
}

pub(super) const PART: Part = Part {
    install,
    views: &[View {
        kind: "chemistry",
        min: (360., 220.),
        panel: |shell| panel_handle(shell.parts.get::<Entity<ChemistryPanel>>().clone()),
    }],
    actions: |element, cx| {
        element
            .on_action(cx.listener(|this, _: &OpenChemistry, window, cx| {
                this.open_chemistry(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleChemistry, window, cx| {
                this.toggle_view("chemistry", window, cx);
            }))
    },
    commands,
};

impl Shell {
    pub(super) fn chemistry_project_opened(
        &mut self,
        root: &Path,
        files: &[FileNode],
        cx: &mut Context<Self>,
    ) {
        let files = cdxml_files(files);
        self.parts
            .get::<Entity<ChemistryPanel>>()
            .update(cx, |panel, cx| panel.set_project(root, &files, cx));
    }

    pub(super) fn sync_chemistry_preview(&mut self, cx: &mut Context<Self>) {
        if !self.view_on_screen("chemistry", cx) || self.build_task.is_some() {
            self.pause_chemistry_warmup(cx);
            return;
        }
        let paper = self
            .project
            .as_ref()
            .and_then(|project| project.read(cx).main_document().map(str::to_owned));
        self.parts
            .get::<Entity<ChemistryPanel>>()
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
            .get::<Entity<ChemistryPanel>>()
            .update(cx, |panel, _| {
                panel.warmup.allowed = false;
                panel.warmup.pause();
            });
    }

    pub(super) fn chemistry_files_changed(
        &mut self,
        paths: Option<&[String]>,
        cx: &mut Context<Self>,
    ) {
        let changed = self
            .parts
            .get::<Entity<ChemistryPanel>>()
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
            self.sync_chemistry_preview(cx);
        }
    }

    pub(super) fn chemistry_built(&mut self, document: &str, cx: &mut Context<Self>) {
        if self
            .project
            .as_ref()
            .and_then(|project| project.read(cx).main_document())
            == Some(document)
        {
            self.parts
                .get::<Entity<ChemistryPanel>>()
                .update(cx, |panel, cx| {
                    panel.paper = None;
                    panel.numbers = None;
                    panel.invalidate();
                    panel.paper = Some(document.to_owned());
                    panel.load_preview(panel.compound_id.clone(), cx);
                });
        }
    }

    fn open_chemistry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root(cx).is_none() {
            return;
        }
        self.show_view("chemistry", true, window, cx);
        self.sync_chemistry_preview(cx);
    }
}

fn commands(context: &CommandContext, out: &mut Vec<Command>) {
    if context.in_project {
        out.push(
            Command::new(
                "panel-chemistry",
                t!("Show Chemistry panel"),
                Section::Layout,
                OpenChemistry,
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
            cdxml_files(&files)
        );
    }

    #[test]
    fn includes_only_numbered_groups_that_chemex_can_extract() {
        let source = "<CDXML><page id='1'><group id='10'><fragment id='11'><n id='12'/></fragment><t id='13'><s>[[@x]]</s></t></group></page></CDXML>";
        let document = chemex_cdxml::DrawingDocument::parse(source.to_owned()).unwrap();
        let groups = compound_groups(&document);
        assert!(!groups.is_empty());
        assert!(
            groups
                .iter()
                .all(|id| { !id.is_empty() && document.compound_group_source(id).is_ok() })
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
