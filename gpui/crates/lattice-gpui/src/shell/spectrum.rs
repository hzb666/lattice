use std::{
    any::Any,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, ImageSource,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, RenderImage, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, Window, actions, div, img, px,
};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    Selectable as _,
    dock::{BasePanel, Panel, PanelEvent, panel_handle},
    h_flex, v_flex,
};
use lattice_core::{
    chemistry::{self, is_mnova},
    models::FileNode,
};
use lattice_i18n::t;
use lattice_ui::{
    ActiveLattice as _, Icon,
    components::{Button, EmptyState},
    scrollbar::FluidScrollbar,
};

use super::{
    Shell,
    commands::{Command, CommandContext, Section},
    parts::{Part, View},
};

const PREVIEW_WIDTH: f32 = 720.;
const PREVIEW_HEIGHT: f32 = 520.;

actions!(lattice, [OpenSpectrum, ToggleSpectrumPanel]);

pub(crate) struct SpectrumPanel {
    focus_handle: FocusHandle,
    root: Option<PathBuf>,
    files: Vec<String>,
    selected_file: Option<String>,
    preview: Option<Arc<RenderImage>>,
    error: Option<String>,
    loading: bool,
    visible: bool,
    generation: u64,
    _load: Option<Task<()>>,
    preview_scroll: gpui::ScrollHandle,
}

impl gpui::EventEmitter<PanelEvent> for SpectrumPanel {}

impl Focusable for SpectrumPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for SpectrumPanel {
    fn panel_name(&self) -> &'static str {
        "SpectrumPanel"
    }

    fn closable(&self, _: &App) -> bool {
        true
    }
}

impl Panel for SpectrumPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(t!("Spectrum").into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        t!("Spectrum")
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for SpectrumPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.palette().clone();
        let mut content = v_flex()
            .size_full()
            .min_h_0()
            .track_focus(&self.focus_handle);
        if self.root.is_none() {
            content = content.child(t!("Open a project to browse spectra"));
        } else {
            content = content.child(self.toolbar(cx)).child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.file_list(cx, &palette))
                    .child(self.preview_stage(cx)),
            );
        }
        content.bg(palette.panel)
    }
}

impl SpectrumPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            root: None,
            files: Vec::new(),
            selected_file: None,
            preview: None,
            error: None,
            loading: false,
            visible: false,
            generation: 0,
            _load: None,
            preview_scroll: gpui::ScrollHandle::new(),
        }
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(cx.palette().line)
            .child(
                div()
                    .flex_1()
                    .text_color(cx.palette().text_secondary)
                    .child(t!("{count} spectrum files", count = self.files.len())),
            )
            .child(
                Button::new("spectrum-refresh", t!("Refresh"))
                    .compact()
                    .ghost()
                    .disabled(self.loading || self.selected_file.is_none())
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.invalidate();
                        panel.load_preview(cx);
                    })),
            )
            .child(
                Button::new("spectrum-open-mnova", t!("Open in MestReNova"))
                    .compact()
                    .ghost()
                    .trailing_icon(Icon::EXTERNAL_LINK)
                    .disabled(self.selected_file.is_none())
                    .on_click(cx.listener(|panel, _, _, cx| panel.open_selected(cx))),
            )
    }

    fn file_list(&self, cx: &mut Context<Self>, palette: &lattice_ui::Palette) -> impl IntoElement {
        v_flex()
            .w(px(190.))
            .flex_none()
            .min_h_0()
            .overflow_y_scrollbar()
            .border_r_1()
            .border_color(palette.line)
            .children(self.files.iter().map(|file| {
                let selected = self.selected_file.as_deref() == Some(file.as_str());
                let file_for_click = file.clone();
                Button::new(format!("spectrum-file:{file}"), file.clone())
                    .compact()
                    .ghost()
                    .selected(selected)
                    .max_w_full()
                    .truncate()
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        panel.select_file(&file_for_click, cx);
                    }))
            }))
    }

    fn preview_stage(&self, cx: &App) -> impl IntoElement {
        let body = match (&self.preview, &self.error, self.loading) {
            (Some(preview), _, _) => img(ImageSource::Render(preview.clone()))
                .max_w_full()
                .into_any_element(),
            (None, Some(error), _) => div()
                .max_w(px(PREVIEW_WIDTH))
                .text_color(cx.palette().text_secondary)
                .child(error.clone())
                .into_any_element(),
            (None, None, true) => div()
                .text_color(cx.palette().text_secondary)
                .child(t!("Exporting with MestReNova…"))
                .into_any_element(),
            (None, None, false) if self.files.is_empty() => {
                EmptyState::new(t!("No MestReNova files in this project")).into_any_element()
            }
            (None, None, false) => {
                EmptyState::new(t!("Choose a spectrum file to preview")).into_any_element()
            }
        };
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(
                div()
                    .id("spectrum-preview-scroll")
                    .absolute()
                    .inset_0()
                    .overflow_scroll()
                    .track_scroll(&self.preview_scroll)
                    .child(
                        div()
                            .min_w(px(PREVIEW_WIDTH))
                            .min_h(px(PREVIEW_HEIGHT))
                            .p(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(body),
                    ),
            )
            .child(FluidScrollbar::vertical(
                "spectrum-preview-scrollbar",
                &self.preview_scroll,
            ))
            .child(FluidScrollbar::horizontal(
                "spectrum-preview-scrollbar-x",
                &self.preview_scroll,
            ))
    }

    fn set_project(&mut self, root: &Path, files: &[FileNode], cx: &mut Context<Self>) {
        let files: Vec<_> = chemistry::mnova_files(files).map(str::to_owned).collect();
        if self.root.as_deref() == Some(root) && self.files == files {
            return;
        }
        self.root = Some(root.to_path_buf());
        self.files = files;
        if !self
            .selected_file
            .as_ref()
            .is_some_and(|file| self.files.iter().any(|candidate| candidate == file))
        {
            self.selected_file = self.files.first().cloned();
        }
        self.invalidate();
        self.load_preview(cx);
        cx.notify();
    }

    fn select_file(&mut self, file: &str, cx: &mut Context<Self>) {
        if self.selected_file.as_deref() == Some(file) {
            return;
        }
        self.selected_file = Some(file.to_owned());
        self.invalidate();
        self.load_preview(cx);
        cx.notify();
    }

    fn files_changed(&mut self, paths: Option<&[String]>, cx: &mut Context<Self>) {
        if paths.is_none_or(|paths| paths.iter().any(|path| is_mnova(Path::new(path)))) {
            self.invalidate();
            self.load_preview(cx);
            cx.notify();
        }
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.preview = None;
        self.error = None;
        self.loading = false;
        self._load = None;
    }

    fn load_preview(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(relative) = self.selected_file.clone() else {
            return;
        };
        if !should_load_preview(
            self.visible,
            self.loading,
            self.preview.is_some() || self.error.is_some(),
        ) {
            return;
        }
        let Some(engine) = crate::pdf::PdfEngine::global(cx).ok() else {
            self.error = Some(t!("PDF preview is unavailable").to_string());
            return;
        };
        let generation = self.generation;
        let export = cx.background_spawn(async move {
            chemistry::mnova_preview_pdf(&root, &relative, NonZeroUsize::MIN)
        });
        self.loading = true;
        self.preview = None;
        self.error = None;
        self._load = Some(cx.spawn(async move |panel, cx| {
            let result = match export.await {
                Ok(pdf) => crate::card_previews::first_page(&engine, pdf, cx)
                    .await
                    .ok_or_else(|| t!("Could not render the spectrum preview").to_string()),
                Err(error) => Err(error),
            };
            panel
                .update(cx, |panel, cx| {
                    if panel.generation != generation {
                        return;
                    }
                    panel.loading = false;
                    match result {
                        Ok(preview) => panel.preview = Some(preview),
                        Err(error) => panel.error = Some(error),
                    }
                    cx.notify();
                })
                .ok();
        }));
        cx.notify();
    }

    fn open_selected(&self, cx: &mut Context<Self>) {
        if let (Some(root), Some(file)) = (&self.root, &self.selected_file) {
            crate::opener::open_project_file(root, file, cx);
        }
    }
}

fn install(_: &mut Window, cx: &mut Context<Shell>) -> Box<dyn Any> {
    Box::new(cx.new(SpectrumPanel::new))
}

pub(super) const PART: Part = Part {
    install,
    views: &[View {
        kind: "spectrum",
        min: (360., 220.),
        panel: |shell| panel_handle(shell.parts.get::<Entity<SpectrumPanel>>().clone()),
    }],
    actions: |element, cx| {
        element
            .on_action(cx.listener(|this, _: &OpenSpectrum, window, cx| {
                this.open_spectrum(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSpectrumPanel, window, cx| {
                this.toggle_view("spectrum", window, cx);
            }))
    },
    commands,
};

impl Shell {
    pub(super) fn spectrum_project_opened(
        &mut self,
        root: &Path,
        files: &[FileNode],
        cx: &mut Context<Self>,
    ) {
        self.parts
            .get::<Entity<SpectrumPanel>>()
            .update(cx, |panel, cx| panel.set_project(root, files, cx));
    }

    pub(super) fn sync_spectrum(&mut self, cx: &mut Context<Self>) {
        let visible = self.view_on_screen("spectrum", cx);
        self.parts
            .get::<Entity<SpectrumPanel>>()
            .update(cx, |panel, cx| {
                panel.visible = visible;
                panel.load_preview(cx);
            });
    }

    pub(super) fn spectrum_files_changed(
        &mut self,
        paths: Option<&[String]>,
        cx: &mut Context<Self>,
    ) {
        self.parts
            .get::<Entity<SpectrumPanel>>()
            .update(cx, |panel, cx| panel.files_changed(paths, cx));
    }

    fn open_spectrum(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root(cx).is_none() {
            return;
        }
        self.show_view("spectrum", true, window, cx);
        self.sync_spectrum(cx);
    }
}

fn commands(context: &CommandContext, out: &mut Vec<Command>) {
    if context.in_project {
        out.push(
            Command::new(
                "panel-spectrum",
                t!("Show Spectrum panel"),
                Section::Layout,
                OpenSpectrum,
            )
            .keywords("chemistry spectrum mnova mestrelab nmr"),
        );
    }
}

fn should_load_preview(visible: bool, loading: bool, has_result: bool) -> bool {
    visible && !loading && !has_result
}

#[cfg(test)]
mod tests {
    use super::should_load_preview;

    #[test]
    fn preview_loads_only_when_visible_and_missing() {
        assert!(should_load_preview(true, false, false));
        assert!(!should_load_preview(false, false, false));
        assert!(!should_load_preview(true, true, false));
        assert!(!should_load_preview(true, false, true));
    }
}
