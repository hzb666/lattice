use std::{cell::Cell, rc::Rc};

use gpui::{
    DispatchPhase, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    RenderImage, ScrollHandle, Styled as _, canvas,
};

pub(super) const MIN_SCALE: f32 = 0.1;
pub(super) const MAX_SCALE: f32 = 3.;
pub(super) const ZOOM_STEP: f32 = 0.1;
pub(super) const RENDER_SCALE: f32 = 2.;
pub(super) const PREVIEW_PADDING: f32 = 20.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PreviewFit {
    Both,
    Width,
    Height,
    Manual,
}

pub(super) fn preview_size(preview: &RenderImage) -> gpui::Size<f32> {
    preview.size(0).map(|length| length.0 as f32 / RENDER_SCALE)
}

pub(super) fn pan_offset(
    offset: Point<Pixels>,
    delta: Point<Pixels>,
    overflow: Point<Pixels>,
) -> Point<Pixels> {
    (offset + delta).clamp(&(-overflow), &Point::default())
}

pub(super) fn zoom_step(scale: f32, direction: f32) -> f32 {
    (((scale + direction * ZOOM_STEP) / ZOOM_STEP).round() * ZOOM_STEP).clamp(MIN_SCALE, MAX_SCALE)
}

pub(super) fn fit_scale(natural: gpui::Size<f32>, room: gpui::Size<f32>, fit: PreviewFit) -> f32 {
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

pub(super) fn pan_events(
    drag: Rc<Cell<Option<Point<Pixels>>>>,
    scroll: ScrollHandle,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |_, (), window, _| {
            let move_drag = drag.clone();
            let move_scroll = scroll;
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
                if phase != DispatchPhase::Capture || move_drag.get().is_none() {
                    return;
                }
                if event.pressed_button != Some(MouseButton::Left) {
                    move_drag.set(None);
                    window.refresh();
                } else if let Some(last) = move_drag.replace(Some(event.position)) {
                    move_scroll.set_offset(pan_offset(
                        move_scroll.offset(),
                        event.position - last,
                        move_scroll.max_offset(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{px, size};

    #[test]
    fn fit_modes_and_zoom_remain_bounded() {
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

    #[test]
    fn pan_offset_stays_inside_scroll_bounds() {
        assert_eq!(
            gpui::point(px(-10.), px(-20.)),
            pan_offset(
                Point::default(),
                gpui::point(px(-20.), px(-30.)),
                gpui::point(px(10.), px(20.))
            )
        );
    }
}
