//! CDXML browsing and numbered chemistry previews.

use std::{any::Any, path::Path, path::PathBuf, sync::Arc};

use chemex_cdxml::DrawingDocument;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, ImageSource,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, RenderImage, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Task, UniformListScrollHandle,
    Window, actions, div, img, prelude::FluentBuilder as _, px, uniform_list,
};
use gpui_kit::component::{
    Selectable as _,
    dock::{BasePanel, Panel, PanelEvent, panel_handle},
    v_flex,
};
use lattice_core::{chemistry, models::FileNode, project};
use lattice_i18n::t;
use lattice_ui::{ActiveLattice as _, components::Button};

use super::{
    Shell,
    commands::{Command, CommandContext, Section},
    parts::{Part, View},
};

mod preview_cache;
mod warmup;
use preview_cache::PreviewCache;
use warmup::Warmup;

actions!(lattice, [OpenChemistry, ToggleChemistry]);

pub(crate) struct ChemistryPanel {
    focus_handle: FocusHandle,
    root: Option<PathBuf>,
    paper: Option<String>,
    files: Vec<String>,
    selected_file: Option<String>,
    document: Option<Arc<DrawingDocument>>,
    groups: Vec<String>,
    warmup: Warmup,
    compound_id: Option<String>,
    preview: Option<Arc<RenderImage>>,
    cache: Arc<PreviewCache>,
    error: Option<String>,
    loading: bool,
    generation: u64,
    files_scroll: UniformListScrollHandle,
    preview_scroll: ScrollHandle,
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
        let files = render_file_list(
            "chemistry-files",
            &self.files,
            self.selected_file.as_deref(),
            &self.files_scroll,
            cx,
            |this, path, cx| this.select_file(path, cx),
        );
        let all = Button::new("chemistry-compound-all", t!("All"))
            .compact()
            .selected(self.compound_id.is_none())
            .on_click(cx.listener(|this, _, _, cx| this.select_compound(None, cx)));
        let compounds = self.groups.iter().map(|compound_id| {
            let id = compound_id.clone();
            Button::new(format!("chemistry-compound:{id}"), id.clone())
                .compact()
                .selected(self.compound_id.as_deref() == Some(id.as_str()))
                .on_click(cx.listener(move |this, _, _, cx| this.select_compound(Some(&id), cx)))
        });
        let stage = preview_stage(
            "chemistry-preview-stage",
            self.preview.as_ref(),
            self.error.as_deref(),
            self.loading,
            if self.files.is_empty() {
                t!("No CDXML files in this project")
            } else {
                t!("Choose a CDXML file to preview")
            },
            &self.preview_scroll,
            palette.clone(),
        );
        let preview = v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when(!self.groups.is_empty(), |preview| {
                preview.child(
                    div()
                        .id("chemistry-compounds")
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .p(px(8.))
                        .overflow_x_scroll()
                        .child(all)
                        .children(compounds),
                )
            })
            .child(stage);
        workspace(
            &self.focus_handle,
            palette,
            "chemistry-file-browser",
            files,
            preview,
        )
    }
}

impl ChemistryPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            root: None,
            paper: None,
            files: Vec::new(),
            selected_file: None,
            document: None,
            groups: Vec::new(),
            warmup: Warmup::default(),
            compound_id: None,
            preview: None,
            cache: Arc::default(),
            error: None,
            loading: false,
            generation: 0,
            files_scroll: UniformListScrollHandle::new(),
            preview_scroll: ScrollHandle::new(),
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
            self.files = files.to_vec();
        }
        self.selected_file = self.files.first().cloned();
        self.reset_drawing();
        cx.notify();
    }

    fn reset_drawing(&mut self) {
        self.document = None;
        self.groups.clear();
        self.compound_id = None;
        self.warmup.reset();
        self.invalidate();
    }

    fn invalidate(&mut self) {
        self.cancel_load();
        self.preview = None;
        self.error = None;
    }

    fn select_file(&mut self, relative: &str, cx: &mut Context<Self>) {
        if !self.files.iter().any(|file| file == relative) {
            return;
        }
        if self.selected_file.as_deref() == Some(relative) {
            if self.compound_id.is_some() || (!self.loading && self.preview.is_none()) {
                self.select_compound(None, cx);
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

    fn load_preview(&mut self, compound_id: Option<String>, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(file) = self.selected_file.clone() else {
            return;
        };
        let paper = self.paper.clone();
        let cache = self.cache.clone();
        let document = self.document.clone();
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
                    let (document, parsed) = match document {
                        Some(document) => (document, None),
                        None => {
                            let path = project::safe_path(&root, &file)?;
                            let document = Arc::new(chemistry::read_document(&path)?);
                            let groups = compound_groups(&document);
                            (document.clone(), Some((document, groups)))
                        }
                    };
                    let svg = match compound_id.as_deref() {
                        Some(id) => chemistry::preview_group_svg(&root, &document, id),
                        None => paper
                            .as_deref()
                            .and_then(|paper| {
                                chemistry::built_preview_svg(&root, paper, document.source())
                            })
                            .map_or_else(|| chemistry::preview_svg(&root, document.source()), Ok),
                    };
                    let preview = svg.and_then(|svg| cache.render(&svg, &renderer));
                    Ok::<_, String>((parsed, preview))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                let result = result.and_then(|(parsed, preview)| {
                    if let Some((document, groups)) = parsed {
                        this.document = Some(document);
                        this.groups = groups;
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
        self._load = None;
        self.loading = false;
        self.generation = self.generation.wrapping_add(1);
    }
}

fn render_file_list<T, F>(
    id: &'static str,
    files: &[String],
    selected: Option<&str>,
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
                    file_row((id, index), &path, selected, cx)
                        .on_click(cx.listener(move |this, _, _, cx| select(this, &path, cx)))
                })
                .collect()
        }),
    )
    .track_scroll(scroll)
    .size_full()
    .into_any_element()
}

fn file_row(
    id: impl Into<gpui::ElementId>,
    path: &str,
    selected: bool,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let palette = cx.palette();
    div()
        .id(id)
        .min_w_0()
        .mx(px(5.))
        .my(px(2.))
        .px(px(8.))
        .py(px(7.))
        .rounded(px(5.))
        .when(selected, |row| row.bg(palette.row_selected))
        .when(!selected, |row| row.hover(|style| style.bg(palette.hover)))
        .text_color(palette.text)
        .child(div().truncate().child(path.to_owned()))
}

fn workspace(
    focus: &FocusHandle,
    palette: lattice_ui::Palette,
    browser_id: &'static str,
    files: impl IntoElement,
    preview: impl IntoElement,
) -> impl IntoElement {
    v_flex()
        .track_focus(focus)
        .size_full()
        .bg(palette.panel)
        .flex_row()
        .child(file_browser(browser_id, files))
        .child(preview)
}

fn file_browser(id: &'static str, rows: impl IntoElement) -> impl IntoElement {
    div()
        .id(id)
        .w(px(208.))
        .flex_none()
        .min_h_0()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(gpui::transparent_black())
        .child(div().flex_1().min_h_0().child(rows))
}

fn preview_stage(
    id: &'static str,
    preview: Option<&Arc<RenderImage>>,
    error: Option<&str>,
    loading: bool,
    empty: impl IntoElement,
    scroll: &ScrollHandle,
    palette: lattice_ui::Palette,
) -> gpui::Stateful<gpui::Div> {
    let mut stage = div()
        .id(id)
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_scroll()
        .track_scroll(scroll)
        .flex()
        .items_center()
        .justify_center()
        .p(px(20.));
    if let Some(preview) = preview {
        stage = stage.child(
            img(ImageSource::Render(preview.clone()))
                .max_w_full()
                .max_h_full(),
        );
    } else if let Some(error) = error {
        stage = stage.child(
            div()
                .text_color(palette.text_secondary)
                .child(error.to_owned()),
        );
    } else if loading {
        stage = stage.child(
            div()
                .text_color(palette.text_secondary)
                .child(t!("Exporting with ChemDraw…")),
        );
    } else {
        stage = stage.child(div().text_color(palette.text_secondary).child(empty));
    }
    stage
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
                if panel.paper != paper && panel.compound_id.is_none() {
                    panel.invalidate();
                }
                panel.paper = paper;
                panel.warmup.allowed = true;
                if !panel.loading && panel.preview.is_none() && panel.error.is_none() {
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
                    if panel.compound_id.is_none() {
                        panel.invalidate();
                        cx.notify();
                    }
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
}
