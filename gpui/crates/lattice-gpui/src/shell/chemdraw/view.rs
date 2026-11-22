use gpui::{Render, relative, uniform_list};
use gpui_kit::component::{
    Selectable as _,
    menu::{ContextMenuExt as _, PopupMenuItem},
};
use lattice_core::chemistry::is_cdxml;
use lattice_ui::components::{LatticeTooltip, TreeRow};

use super::*;

impl Render for ChemDrawPanel {
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
                content.child(group_list(self, layout, cx))
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

pub(super) fn group_list(
    panel: &ChemDrawPanel,
    layout: ChemistryLayout,
    cx: &mut Context<ChemDrawPanel>,
) -> gpui::AnyElement {
    let all = Button::new("chemistry-compound-all", t!("All"))
        .compact()
        .ghost()
        .selected(panel.compound_id.is_none())
        .on_click(cx.listener(|this, _, _, cx| this.select_compound(None, cx)));
    let source = Button::new("chemistry-all-source", t!("Source"))
        .compact()
        .ghost()
        .selected(panel.compound_id.is_none() && panel.all_preview == AllPreview::Source)
        .on_click(cx.listener(|this, _, _, cx| this.select_all_preview(AllPreview::Source, cx)));
    let rendered = Button::new("chemistry-all-rendered", t!("Rendered"))
        .compact()
        .ghost()
        .selected(panel.compound_id.is_none() && panel.all_preview == AllPreview::Rendered)
        .disabled(panel.numbers.as_ref().is_none_or(BTreeMap::is_empty))
        .on_click(cx.listener(|this, _, _, cx| this.select_all_preview(AllPreview::Rendered, cx)));
    let compounds = panel.groups.iter().map(|compound_id| {
        let id = compound_id.clone();
        let label = panel
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
            .selected(panel.compound_id.as_deref() == Some(id.as_str()))
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
        .when(panel.compound_id.is_none(), |bar| {
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

pub(super) fn render_file_list<T, F>(
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
                                            crate::opener::open_project_file(root, &menu_path, cx);
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

pub(super) fn workspace(
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
