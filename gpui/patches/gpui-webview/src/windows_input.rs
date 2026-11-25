use std::cell::Cell;

use anyhow::Result;
use gpui::{
    Bounds, DevicePixels, DispatchEventResult, Modifiers, MouseButton, NavigationDirection, Pixels,
    PlatformInput, ScrollDelta, WindowCompositionSurface,
};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use windows::Win32::{
    Foundation::POINT,
    UI::WindowsAndMessaging::{XBUTTON1, XBUTTON2},
};
use wry::WebViewExtWindows as _;

pub(super) struct WebViewInput {
    pub(super) composition: ICoreWebView2CompositionController,
    pub(super) controller: ICoreWebView2Controller,
    pub(super) bounds: Cell<Bounds<DevicePixels>>,
    pub(super) scale_factor: Cell<f32>,
}

struct MouseInput {
    kind: COREWEBVIEW2_MOUSE_EVENT_KIND,
    button: Option<MouseButton>,
    modifiers: Modifiers,
    data: u32,
    position: gpui::Point<Pixels>,
}

impl WebViewInput {
    pub(super) fn send(&self, event: &PlatformInput) -> Result<()> {
        match event {
            PlatformInput::MouseDown(event) => {
                unsafe {
                    self.controller
                        .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)?;
                }
                self.send_mouse(MouseInput {
                    kind: mouse_button_kind(event.button, true),
                    button: Some(event.button),
                    modifiers: event.modifiers,
                    data: navigation_button_data(event.button),
                    position: event.position,
                })?;
            }
            PlatformInput::MouseUp(event) => self.send_mouse(MouseInput {
                kind: mouse_button_kind(event.button, false),
                button: None,
                modifiers: event.modifiers,
                data: navigation_button_data(event.button),
                position: event.position,
            })?,
            PlatformInput::MouseMove(event) => self.send_mouse(MouseInput {
                kind: COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
                button: event.pressed_button,
                modifiers: event.modifiers,
                data: 0,
                position: event.position,
            })?,
            PlatformInput::MouseExited(event) => self.send_mouse(MouseInput {
                kind: COREWEBVIEW2_MOUSE_EVENT_KIND_LEAVE,
                button: event.pressed_button,
                modifiers: event.modifiers,
                data: 0,
                position: event.position,
            })?,
            PlatformInput::Scroll(event) => {
                let delta = scroll_amount(event.delta);
                for (kind, amount) in [
                    (COREWEBVIEW2_MOUSE_EVENT_KIND_WHEEL, delta.y),
                    (COREWEBVIEW2_MOUSE_EVENT_KIND_HORIZONTAL_WHEEL, -delta.x),
                ] {
                    if amount != 0. {
                        self.send_mouse(MouseInput {
                            kind,
                            button: None,
                            modifiers: event.modifiers,
                            data: amount as i32 as u32,
                            position: event.position,
                        })?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn send_mouse(&self, input: MouseInput) -> Result<()> {
        let mut virtual_keys = COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE;
        if input.modifiers.control {
            virtual_keys |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_CONTROL;
        }
        if input.modifiers.shift {
            virtual_keys |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_SHIFT;
        }
        if let Some(button) = input.button {
            virtual_keys |= mouse_button_key(button);
        }
        let point = if input.kind == COREWEBVIEW2_MOUSE_EVENT_KIND_LEAVE {
            POINT::default()
        } else {
            mouse_point(input.position, self.bounds.get(), self.scale_factor.get())
        };
        unsafe {
            self.composition
                .SendMouseInput(input.kind, virtual_keys, input.data, point)?;
        }
        Ok(())
    }
}

pub(super) fn attach_to_surface(
    webview: &wry::WebView,
    composition: &gpui::WindowComposition<'_>,
    surface: &WindowCompositionSurface,
    visual: &windows::core::IUnknown,
    input: &std::rc::Rc<WebViewInput>,
    parent_window: isize,
    old_parent_window: isize,
) -> Result<()> {
    surface.platform_surface()?.set_visible(false)?;
    if parent_window != old_parent_window {
        webview.reparent(parent_window)?;
    }
    unsafe { input.composition.SetRootVisualTarget(visual)? };
    composition.on_input(surface.id(), {
        let input = input.clone();
        move |event| {
            if let Err(error) = input.send(event) {
                log::error!("sending input to composition WebView: {error:#}");
            }
            DispatchEventResult {
                propagate: false,
                default_prevented: false,
            }
        }
    })?;
    Ok(())
}

pub(super) fn rollback_move(
    webview: &wry::WebView,
    composition: &gpui::WindowComposition<'_>,
    surface: &WindowCompositionSurface,
    input: &WebViewInput,
    old_visual: Option<&windows::core::IUnknown>,
    parent_window: isize,
    old_parent_window: isize,
) {
    if let Err(error) = unsafe { input.composition.SetRootVisualTarget(old_visual) } {
        log::warn!("restoring the previous WebView visual target: {error}");
    }
    if parent_window != old_parent_window
        && let Err(error) = webview.reparent(old_parent_window)
    {
        log::warn!("restoring the previous WebView parent window: {error}");
    }
    if let Err(error) = composition.remove_surface(surface.id()) {
        log::warn!("removing a failed WebView composition surface: {error:#}");
    }
}

fn mouse_button_kind(button: MouseButton, down: bool) -> COREWEBVIEW2_MOUSE_EVENT_KIND {
    match (button, down) {
        (MouseButton::Left, true) => COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
        (MouseButton::Left, false) => COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP,
        (MouseButton::Right, true) => COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_DOWN,
        (MouseButton::Right, false) => COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_UP,
        (MouseButton::Middle, true) => COREWEBVIEW2_MOUSE_EVENT_KIND_MIDDLE_BUTTON_DOWN,
        (MouseButton::Middle, false) => COREWEBVIEW2_MOUSE_EVENT_KIND_MIDDLE_BUTTON_UP,
        (MouseButton::Navigate(_), true) => COREWEBVIEW2_MOUSE_EVENT_KIND_X_BUTTON_DOWN,
        (MouseButton::Navigate(_), false) => COREWEBVIEW2_MOUSE_EVENT_KIND_X_BUTTON_UP,
    }
}

fn mouse_button_key(button: MouseButton) -> COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS {
    match button {
        MouseButton::Left => COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON,
        MouseButton::Right => COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_RIGHT_BUTTON,
        MouseButton::Middle => COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_MIDDLE_BUTTON,
        MouseButton::Navigate(NavigationDirection::Back) => {
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_X_BUTTON1
        }
        MouseButton::Navigate(NavigationDirection::Forward) => {
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_X_BUTTON2
        }
    }
}

fn navigation_button_data(button: MouseButton) -> u32 {
    match button {
        MouseButton::Navigate(NavigationDirection::Back) => u32::from(XBUTTON1),
        MouseButton::Navigate(NavigationDirection::Forward) => u32::from(XBUTTON2),
        _ => 0,
    }
}

fn mouse_point(
    position: gpui::Point<Pixels>,
    bounds: Bounds<DevicePixels>,
    scale_factor: f32,
) -> POINT {
    POINT {
        x: (f32::from(position.x) * scale_factor).round() as i32 - bounds.origin.x.0,
        y: (f32::from(position.y) * scale_factor).round() as i32 - bounds.origin.y.0,
    }
}

fn scroll_amount(delta: ScrollDelta) -> gpui::Point<f32> {
    match delta {
        ScrollDelta::Pixels(delta) => delta.map(f32::from),
        ScrollDelta::Lines(delta) => delta * 120.,
    }
}

#[cfg(test)]
mod tests {
    use super::{mouse_point, scroll_amount};
    use gpui::{Bounds, DevicePixels, ScrollDelta, point, px, size};

    #[test]
    fn mouse_coordinates_are_relative_to_the_composition_surface() {
        let point = mouse_point(
            point(px(70.), px(55.)),
            Bounds::new(
                point(DevicePixels(100), DevicePixels(50)),
                size(DevicePixels(400), DevicePixels(300)),
            ),
            2.,
        );
        assert_eq!((point.x, point.y), (40, 60));
    }

    #[test]
    fn clipped_surface_mouse_coordinates_remain_relative_to_the_document() {
        let point = mouse_point(
            point(px(30.), px(40.)),
            Bounds::new(
                point(DevicePixels(20), DevicePixels(10)),
                size(DevicePixels(600), DevicePixels(800)),
            ),
            2.,
        );
        assert_eq!((point.x, point.y), (40, 70));
    }

    #[test]
    fn scroll_deltas_keep_pixel_and_wheel_units() {
        assert_eq!(
            scroll_amount(ScrollDelta::Pixels(point(px(2.), px(-3.)))),
            point(2., -3.)
        );
        assert_eq!(
            scroll_amount(ScrollDelta::Lines(point(0.5, -1.))),
            point(60., -120.)
        );
    }
}
