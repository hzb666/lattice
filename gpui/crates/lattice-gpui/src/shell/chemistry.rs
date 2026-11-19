//! ChemDraw file browsing and numbered chemistry previews.

use std::{any::Any, fs, io::Read, path::Path, path::PathBuf, sync::Arc, time::Duration};

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
use lattice_core::models::FileNode;
use lattice_i18n::t;
use lattice_ui::{ActiveLattice as _, components::Button};

use super::{
    Shell,
    commands::{Command, CommandContext, Section},
    parts::{Part, View},
};

actions!(
    lattice,
    [OpenChemistry, OpenChemDraw, ToggleChemistry, ToggleChemDraw]
);

#[derive(Default)]
struct Chemistry {
    chemistry: Option<Entity<ChemistryPanel>>,
    chemdraw: Option<Entity<ChemDrawPanel>>,
}

struct ChemistryPanel {
    focus_handle: FocusHandle,
    root: Option<PathBuf>,
    files: Vec<String>,
    selected_file: Option<String>,
    groups: Vec<String>,
    compound_id: Option<String>,
    preview: Option<Arc<RenderImage>>,
    error: Option<String>,
    loading: bool,
    generation: u64,
    files_scroll: UniformListScrollHandle,
    preview_scroll: ScrollHandle,
    _load: Option<Task<()>>,
}

struct ChemDrawPanel {
    focus_handle: FocusHandle,
    root: Option<PathBuf>,
    files: Vec<String>,
    selected_file: Option<String>,
    preview: Option<Arc<RenderImage>>,
    error: Option<String>,
    loading: bool,
    generation: u64,
    files_scroll: UniformListScrollHandle,
    preview_scroll: ScrollHandle,
    _load: Option<Task<()>>,
}

macro_rules! impl_panel {
    ($type:ty, $name:literal, $title:literal) => {
        impl gpui::EventEmitter<PanelEvent> for $type {}

        impl Focusable for $type {
            fn focus_handle(&self, _: &App) -> FocusHandle {
                self.focus_handle.clone()
            }
        }

        impl BasePanel for $type {
            fn panel_name(&self) -> &'static str {
                $name
            }

            fn closable(&self, _: &App) -> bool {
                true
            }
        }

        impl Panel for $type {
            fn tab_name(&self, _: &App) -> Option<SharedString> {
                Some(t!($title).into())
            }

            fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                t!($title)
            }

            fn inner_padding(&self, _: &App) -> bool {
                false
            }
        }
    };
}

impl_panel!(ChemistryPanel, "ChemistryPanel", "Chemistry");
impl_panel!(ChemDrawPanel, "ChemDrawPanel", "ChemDraw");

impl Render for ChemDrawPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.palette().clone();
        let files = render_file_list(
            "chemdraw-files",
            &self.files,
            self.selected_file.as_deref(),
            &self.files_scroll,
            cx,
            |this, path, cx| this.select_file(path, cx),
        );
        let stage = preview_stage(
            "chemdraw-preview-stage",
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
        workspace(
            &self.focus_handle,
            palette.clone(),
            "chemdraw-file-browser",
            files,
            stage,
        )
    }
}

impl ChemDrawPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            root: None,
            files: Vec::new(),
            selected_file: None,
            preview: None,
            error: None,
            loading: false,
            generation: 0,
            files_scroll: UniformListScrollHandle::new(),
            preview_scroll: ScrollHandle::new(),
            _load: None,
        }
    }

    fn set_project(&mut self, root: &Path, files: &[String], cx: &mut Context<Self>) {
        if self.root.as_deref() == Some(root) && self.files == files {
            return;
        }
        self.root = Some(root.to_path_buf());
        self.files.clear();
        self.files.extend_from_slice(files);
        self.selected_file = None;
        self.preview = None;
        self.error = None;
        self.cancel_load();
        cx.notify();
    }

    fn select_file(&mut self, relative: &str, cx: &mut Context<Self>) {
        if !self.files.iter().any(|file| file == relative)
            || self.selected_file.as_deref() == Some(relative)
            || self.loading
        {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        self.selected_file = Some(relative.to_owned());
        self.preview = None;
        self.error = None;
        self.load_file(root.join(relative), relative.to_owned(), cx);
    }

    fn load_file(&mut self, path: PathBuf, relative: String, cx: &mut Context<Self>) {
        self.cancel_load();
        let generation = self.generation;
        let renderer = cx.svg_renderer();
        self.loading = true;
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { render_file(&path, &renderer) })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation
                    || this.selected_file.as_deref() != Some(relative.as_str())
                {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(preview) => this.preview = Some(preview),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn cancel_load(&mut self) {
        self._load = None;
        self.loading = false;
        self.generation = self.generation.wrapping_add(1);
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
        let compounds = self.groups.iter().enumerate().map(|(index, compound_id)| {
            let id = compound_id.clone();
            Button::new(("chemistry-compound", index), id.clone())
                .compact()
                .selected(self.compound_id.as_deref() == Some(id.as_str()))
                .on_click(cx.listener(move |this, _, _, cx| this.select_compound(&id, cx)))
        });
        let mut stage = preview_stage(
            "chemistry-preview-stage",
            self.preview.as_ref(),
            self.error.as_deref(),
            self.loading,
            if self.files.is_empty() {
                t!("No CDXML files in this project")
            } else if self.selected_file.is_some() {
                t!("No numbered ChemDraw group is available")
            } else {
                t!("Choose a CDXML file to inspect compound numbers")
            },
            &self.preview_scroll,
            palette.clone(),
        );
        if !self.groups.is_empty() {
            stage = stage.child(
                div()
                    .id("chemistry-compounds")
                    .absolute()
                    .top(px(8.))
                    .left(px(8.))
                    .right(px(8.))
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .children(compounds),
            );
        }
        workspace(
            &self.focus_handle,
            palette,
            "chemistry-file-browser",
            files,
            stage,
        )
    }
}

impl ChemistryPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            root: None,
            files: Vec::new(),
            selected_file: None,
            groups: Vec::new(),
            compound_id: None,
            preview: None,
            error: None,
            loading: false,
            generation: 0,
            files_scroll: UniformListScrollHandle::new(),
            preview_scroll: ScrollHandle::new(),
            _load: None,
        }
    }

    fn set_project(&mut self, root: &Path, files: &[String], cx: &mut Context<Self>) {
        if self.root.as_deref() == Some(root) && self.files == files {
            return;
        }
        self.root = Some(root.to_path_buf());
        self.files.clear();
        self.files.extend_from_slice(files);
        self.selected_file = None;
        self.groups.clear();
        self.compound_id = None;
        self.preview = None;
        self.error = None;
        self.cancel_load();
        if let Some(first) = self.files.first().cloned() {
            self.select_file(&first, cx);
        }
        cx.notify();
    }

    fn select_file(&mut self, relative: &str, cx: &mut Context<Self>) {
        if !self.files.iter().any(|file| file == relative)
            || self.selected_file.as_deref() == Some(relative)
        {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        self.selected_file = Some(relative.to_owned());
        self.groups.clear();
        self.compound_id = None;
        self.preview = None;
        self.error = None;
        self.cancel_load();
        let generation = self.generation;
        let path = relative.to_owned();
        let source = root.join(relative);
        let renderer = cx.svg_renderer();
        self.loading = true;
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let document = read_document(&source)?;
                    let groups = compound_groups(&document);
                    let preview = groups
                        .first()
                        .map(|compound_id| {
                            let source = document
                                .compound_group_source(compound_id)
                                .map_err(|error| error.to_string())?;
                            render_cdxml(&source, &renderer)
                        })
                        .transpose()?;
                    Ok::<_, String>((groups, preview))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation
                    || this.selected_file.as_deref() != Some(path.as_str())
                {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((groups, preview)) => {
                        this.compound_id = groups.first().cloned();
                        this.groups = groups;
                        this.preview = preview;
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn select_compound(&mut self, compound_id: &str, cx: &mut Context<Self>) {
        let Some(id) = self
            .groups
            .iter()
            .find(|id| id.as_str() == compound_id)
            .cloned()
        else {
            return;
        };
        if self.compound_id.as_deref() == Some(compound_id) || self.loading {
            return;
        }
        let generation = self.next_generation();
        let task_id = id.clone();
        let expected_id = id.clone();
        let selected_file = self.selected_file.clone();
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(file) = selected_file.as_ref() else {
            return;
        };
        let path = root.join(file);
        let renderer = cx.svg_renderer();
        self.compound_id = Some(id.clone());
        self.preview = None;
        self.error = None;
        self.loading = true;
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let document = read_document(&path)?;
                    let source = document
                        .compound_group_source(&task_id)
                        .map_err(|error| error.to_string())?;
                    render_cdxml(&source, &renderer)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation
                    || this.selected_file != selected_file
                    || this.compound_id.as_deref() != Some(expected_id.as_str())
                {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(preview) => this.preview = Some(preview),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    fn cancel_load(&mut self) {
        self._load = None;
        self.loading = false;
        self.next_generation();
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

fn read_document(source: &Path) -> Result<chemex_cdxml::DrawingDocument, String> {
    let mut content = String::new();
    fs::File::open(source)
        .and_then(|file| {
            file.take((chemex_cdxml::MAX_SOURCE_BYTES + 1) as u64)
                .read_to_string(&mut content)
        })
        .map_err(|error| error.to_string())?;
    chemex_cdxml::DrawingDocument::parse(content).map_err(|error| error.to_string())
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
        if document.compound_group_source(compound_id).is_ok() {
            groups.push(compound_id.to_owned());
        }
    }
    groups
}

fn render_cdxml(source: &str, renderer: &gpui::SvgRenderer) -> Result<Arc<RenderImage>, String> {
    if !cfg!(windows) {
        return Err(t!("ChemDraw preview export is supported on Windows").to_string());
    }
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    fs::write(directory.path().join("drawing.cdxml"), source).map_err(|error| error.to_string())?;
    chemex_chemdraw::export_svg(directory.path(), Duration::from_secs(120))
        .map_err(|error| error.to_string())?;
    let bytes =
        fs::read(directory.path().join("drawing.svg")).map_err(|error| error.to_string())?;
    renderer
        .render_single_frame(&bytes, 2.)
        .map_err(|error| error.to_string())
}

fn render_document(
    source: &Path,
    renderer: &gpui::SvgRenderer,
) -> Result<Arc<RenderImage>, String> {
    let document = read_document(source)?;
    render_cdxml(document.source(), renderer)
}

fn render_file(source: &Path, renderer: &gpui::SvgRenderer) -> Result<Arc<RenderImage>, String> {
    render_document(source, renderer)
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
    Box::new(Chemistry {
        chemistry: Some(cx.new(ChemistryPanel::new)),
        chemdraw: Some(cx.new(ChemDrawPanel::new)),
    })
}

pub(super) const PART: Part = Part {
    install,
    views: &[
        View {
            kind: "chemistry",
            min: (360., 220.),
            panel: |shell| panel_handle(shell.parts.get::<Chemistry>().chemistry.clone().unwrap()),
        },
        View {
            kind: "chemdraw",
            min: (360., 220.),
            panel: |shell| panel_handle(shell.parts.get::<Chemistry>().chemdraw.clone().unwrap()),
        },
    ],
    actions: |element, cx| {
        element
            .on_action(cx.listener(|this, _: &OpenChemistry, window, cx| {
                this.open_chemistry(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenChemDraw, window, cx| {
                this.open_chemdraw(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleChemistry, window, cx| {
                this.toggle_view("chemistry", window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleChemDraw, window, cx| {
                this.toggle_chemdraw(window, cx);
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
            .get_mut::<Chemistry>()
            .chemistry
            .as_ref()
            .unwrap()
            .update(cx, |panel, cx| {
                panel.set_project(root, &files, cx);
            });
        self.parts
            .get::<Chemistry>()
            .chemdraw
            .as_ref()
            .unwrap()
            .update(cx, |panel, cx| {
                panel.set_project(root, &files, cx);
            });
    }

    fn open_chemistry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root(cx).is_none() {
            return;
        }
        let panel = self.parts.get::<Chemistry>().chemistry.clone().unwrap();
        panel.update(cx, |panel, cx| {
            if panel.selected_file.is_none()
                && let Some(first) = panel.files.first().cloned()
            {
                panel.select_file(&first, cx);
            }
        });
        self.show_view("chemistry", true, window, cx);
    }

    fn open_chemdraw(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.parts.get::<Chemistry>().chemdraw.clone().unwrap();
        panel.update(cx, |panel, cx| {
            if panel.selected_file.is_none()
                && let Some(first) = panel.files.first().cloned()
            {
                panel.select_file(&first, cx);
            }
        });
        self.show_view("chemdraw", true, window, cx);
    }

    fn toggle_chemdraw(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.view_on_screen("chemdraw", cx) {
            self.toggle_view("chemdraw", window, cx);
        } else {
            self.open_chemdraw(window, cx);
        }
    }
}

fn commands(context: &CommandContext, out: &mut Vec<Command>) {
    if !context.in_project {
        return;
    }
    out.push(
        Command::new(
            "panel-chemistry",
            t!("Show Chemistry panel"),
            Section::Layout,
            OpenChemistry,
        )
        .keywords("chemistry compound number"),
    );
    out.push(
        Command::new(
            "panel-chemdraw",
            t!("Show ChemDraw panel"),
            Section::Layout,
            OpenChemDraw,
        )
        .keywords("chemdraw cdxml drawing"),
    );
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
