use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, div, px, relative,
};
use gpui_kit::component::{Selectable as _, h_flex, v_flex};
use lattice_core::chemistry::{ProjectMolecule, ProjectMoleculeReport};
use lattice_i18n::t;
use lattice_ui::{
    ActiveLattice as _, TextRole, TextRoleStyle as _,
    components::{Button, EmptyState},
    tokens::SPACING_2,
};

use super::{MoleculeLayout, MoleculePanel, MoleculeView, formula, molecule_status};

impl Render for MoleculePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.palette().clone();
        let layout = super::molecule_layout(self.panel_bounds.get().size.width);
        let body = match (&self.root, &self.report, self.analysis.is_some()) {
            (None, _, _) => div()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(palette.text_secondary)
                .child(t!("Open a project to identify molecules"))
                .into_any_element(),
            (_, _, true) => div()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(palette.text_secondary)
                .child(t!("Analyzing ChemDraw structures…"))
                .into_any_element(),
            (_, Some(Err(error)), _) => div()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(palette.text_secondary)
                .child(format!("{}: {error}", t!("Could not scan the project")))
                .into_any_element(),
            (_, Some(Ok(report)), _) => workspace(self, report, layout, cx),
            (_, None, _) => div()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(palette.text_secondary)
                .child(t!("No molecule analysis available"))
                .into_any_element(),
        };
        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .min_h_0()
            .bg(palette.panel)
            .child(toolbar(self, cx))
            .child(body)
            .child(
                canvas(
                    {
                        let bounds = self.panel_bounds.clone();
                        move |next, window, _| {
                            if bounds.get().size != next.size {
                                window.request_animation_frame();
                            }
                            bounds.set(next);
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
    }
}

pub(super) fn toolbar(panel: &MoleculePanel, cx: &mut Context<MoleculePanel>) -> impl IntoElement {
    let summary = panel
        .report
        .as_ref()
        .and_then(|report| report.as_ref().ok())
        .map_or_else(
            || t!("{count} molecules", count = 0),
            |report| {
                let duplicates = report
                    .molecules
                    .iter()
                    .filter(|molecule| molecule.duplicate_of.is_some())
                    .count();
                format!(
                    "{} · {}",
                    t!("{count} molecules", count = report.molecules.len()),
                    t!("{count} duplicate structures", count = duplicates)
                )
            },
        );
    h_flex()
        .id("molecule-toolbar")
        .w_full()
        .min_w_0()
        .flex_none()
        .gap(SPACING_2)
        .px(px(12.))
        .py(px(8.))
        .border_b_1()
        .border_color(cx.palette().line)
        .bg(cx.palette().editor)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_role(TextRole::Caption, cx)
                .text_color(cx.palette().text_secondary)
                .child(summary),
        )
        .child(
            Button::new("molecule-view-list", t!("List"))
                .compact()
                .ghost()
                .selected(panel.view == MoleculeView::List)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.view = MoleculeView::List;
                    cx.notify();
                })),
        )
        .child(
            Button::new("molecule-view-gallery", t!("Gallery"))
                .compact()
                .ghost()
                .selected(panel.view == MoleculeView::Gallery)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.view = MoleculeView::Gallery;
                    cx.notify();
                })),
        )
        .child(
            Button::new("molecule-refresh", t!("Refresh"))
                .compact()
                .ghost()
                .disabled(panel.root.is_none() || panel.analysis.is_some())
                .on_click(cx.listener(|panel, _, _, cx| {
                    panel.invalidate();
                    panel.analyze(cx);
                })),
        )
}

pub(super) fn workspace(
    panel: &MoleculePanel,
    report: &ProjectMoleculeReport,
    layout: MoleculeLayout,
    cx: &mut Context<MoleculePanel>,
) -> AnyElement {
    let selected = panel.selected.is_some();
    let browser = browser(panel, report, layout, selected, cx);
    if layout == MoleculeLayout::Stacked {
        v_flex()
            .flex_1()
            .min_h_0()
            .child(browser)
            .when(selected, |content| content.child(panel.preview_panel(cx)))
            .into_any_element()
    } else {
        h_flex()
            .flex_1()
            .min_h_0()
            .child(browser)
            .when(selected, |content| content.child(panel.preview_panel(cx)))
            .into_any_element()
    }
}

fn browser(
    panel: &MoleculePanel,
    report: &ProjectMoleculeReport,
    layout: MoleculeLayout,
    selected: bool,
    cx: &mut Context<MoleculePanel>,
) -> AnyElement {
    let palette = cx.palette().clone();
    let entries = report.errors.iter().map(|error| {
        div()
            .w_full()
            .border_b_1()
            .border_color(palette.line)
            .p(px(10.))
            .text_color(palette.text_secondary)
            .child(format!(
                "{} · {}",
                error.file,
                t!(
                    "Could not analyze this drawing: {reason}",
                    reason = &error.message
                )
            ))
            .into_any_element()
    });
    let molecules = if panel.view == MoleculeView::List {
        report
            .molecules
            .iter()
            .map(|molecule| list_row(panel, molecule, cx).into_any_element())
            .collect::<Vec<_>>()
    } else {
        report
            .molecules
            .iter()
            .map(|molecule| gallery_card(panel, molecule, cx).into_any_element())
            .collect::<Vec<_>>()
    };
    let empty = report.molecules.is_empty() && report.errors.is_empty();
    let list = if panel.view == MoleculeView::List {
        v_flex().w_full().children(molecules).into_any_element()
    } else {
        div()
            .flex()
            .flex_wrap()
            .content_start()
            .gap(px(8.))
            .p(px(10.))
            .children(molecules)
            .into_any_element()
    };
    let body = v_flex()
        .id("molecule-results")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .children(entries)
        .when(empty, |body| {
            body.child(EmptyState::new(t!("No numbered molecules found")))
        })
        .when(!empty, |body| body.child(list));
    div()
        .id("molecule-browser")
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .when(layout == MoleculeLayout::Stacked, |browser| {
            browser
                .w_full()
                .flex_1()
                .when(selected, |browser| browser.max_h(relative(0.52)))
                .when(selected, |browser| browser.border_b_1())
        })
        .when(layout == MoleculeLayout::Wide, |browser| {
            browser.w(px(300.)).flex_none().border_r_1()
        })
        .border_color(cx.palette().line)
        .child(body)
        .into_any_element()
}

fn list_row(
    panel: &MoleculePanel,
    molecule: &ProjectMolecule,
    cx: &mut Context<MoleculePanel>,
) -> impl IntoElement {
    let location = molecule.location.clone();
    let selected = panel.selected.as_ref() == Some(&location);
    let formula = formula(molecule);
    let status = molecule_status(molecule);
    v_flex()
        .id(format!("molecule-row:{}:{}", location.file, location.key))
        .w_full()
        .gap(px(3.))
        .p(px(10.))
        .cursor_pointer()
        .when(selected, |row| row.bg(cx.palette().hover))
        .when(!selected, |row| row.hover(|row| row.bg(cx.palette().hover)))
        .border_b_1()
        .border_color(cx.palette().line)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.select_molecule(location.clone(), cx);
        }))
        .child(
            h_flex()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_role(TextRole::BodyCompact, cx)
                        .text_color(cx.palette().text)
                        .child(molecule.location.key.clone()),
                )
                .child(
                    div()
                        .text_role(TextRole::BodyCompact, cx)
                        .text_color(cx.palette().accent)
                        .child(formula),
                ),
        )
        .child(
            div()
                .text_role(TextRole::Micro, cx)
                .text_color(cx.palette().text_secondary)
                .child(molecule.location.file.clone()),
        )
        .when_some(status, |row, status| {
            row.child(
                div()
                    .text_role(TextRole::Micro, cx)
                    .text_color(cx.palette().text_secondary)
                    .child(status),
            )
        })
}

fn gallery_card(
    panel: &MoleculePanel,
    molecule: &ProjectMolecule,
    cx: &mut Context<MoleculePanel>,
) -> impl IntoElement {
    let location = molecule.location.clone();
    let selected = panel.selected.as_ref() == Some(&location);
    let formula = formula(molecule);
    let status = molecule_status(molecule);
    v_flex()
        .id(format!("molecule-card:{}:{}", location.file, location.key))
        .w(relative(0.47))
        .min_w(px(150.))
        .min_h(px(118.))
        .gap(px(5.))
        .p(px(10.))
        .rounded(px(6.))
        .cursor_pointer()
        .border_1()
        .border_color(if selected {
            cx.palette().accent
        } else {
            cx.palette().line
        })
        .when(selected, |card| card.bg(cx.palette().hover))
        .when(!selected, |card| {
            card.hover(|card| card.bg(cx.palette().hover))
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.select_molecule(location.clone(), cx);
        }))
        .child(
            div()
                .text_role(TextRole::Title, cx)
                .text_color(cx.palette().text)
                .child(formula),
        )
        .child(
            div()
                .text_role(TextRole::BodyCompact, cx)
                .text_color(cx.palette().accent)
                .child(molecule.location.key.clone()),
        )
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_role(TextRole::Micro, cx)
                .text_color(cx.palette().text_secondary)
                .child(molecule.location.file.clone()),
        )
        .when_some(status, |card, status| {
            card.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_role(TextRole::Micro, cx)
                    .text_color(cx.palette().text_secondary)
                    .child(status),
            )
        })
}
