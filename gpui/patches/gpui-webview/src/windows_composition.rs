use std::{cell::Cell, rc::Rc};

use self::input::{WebViewInput, attach_to_surface, rollback_move};
use anyhow::{Context as _, Result};
use gpui::{
    AnyWindowHandle, AsyncApp, Bounds, DispatchEventResult, Pixels, Window,
    WindowCompositionSurface, point, px, size,
};
use wry::WebViewExtWindows as _;

#[path = "windows_input.rs"]
mod input;
use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub(super) struct NativeWebViewSurface {
    surface: WindowCompositionSurface,
    input: Rc<WebViewInput>,
    window: AnyWindowHandle,
    parent_window: Cell<isize>,
    attached: Cell<bool>,
    cx: AsyncApp,
}

impl NativeWebViewSurface {
    pub(super) fn new(webview: &wry::WebView, window: &Window, cx: &gpui::App) -> Result<Self> {
        let parent_window = native_window_handle(window)?;
        let composition = window.enable_window_composition()?;
        let surface = composition.create_native_surface()?;
        let controller = webview
            .composition_controller()
            .context("Wry did not create a WebView2 composition controller")?;
        let visual = visual_for_surface(&surface)?;
        let input = Rc::new(WebViewInput {
            composition: controller.clone(),
            controller: webview.controller(),
            bounds: Cell::new(Bounds::default()),
            scale_factor: Cell::new(window.scale_factor()),
        });

        let result = unsafe { controller.SetRootVisualTarget(&visual) }
            .map_err(anyhow::Error::from)
            .and_then(|()| {
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
                })
            });
        if let Err(error) = result {
            if let Err(clear_error) = unsafe { controller.SetRootVisualTarget(None) } {
                log::warn!("clearing a failed WebView composition target: {clear_error}");
            }
            if let Err(remove_error) = composition.remove_surface(surface.id()) {
                log::warn!("removing a failed WebView composition surface: {remove_error:#}");
            }
            return Err(error.into());
        }

        Ok(Self {
            surface,
            input,
            window: window.window_handle(),
            parent_window: Cell::new(parent_window),
            attached: Cell::new(true),
            cx: cx.to_async(),
        })
    }

    pub(super) fn move_to(
        &mut self,
        webview: &wry::WebView,
        window: &Window,
        cx: &mut gpui::App,
    ) -> Result<()> {
        let target_window = window.window_handle();
        if self.window.window_id() == target_window.window_id() {
            return self.reattach();
        }

        let parent_window = native_window_handle(window)?;
        let old_visual = self
            .attached
            .get()
            .then(|| visual_for_surface(&self.surface))
            .transpose()?;
        let composition = window.enable_window_composition()?;
        let surface = composition.create_native_surface()?;
        let visual = match visual_for_surface(&surface) {
            Ok(visual) => visual,
            Err(error) => {
                if let Err(remove_error) = composition.remove_surface(surface.id()) {
                    log::warn!("removing a failed WebView composition surface: {remove_error:#}");
                }
                return Err(error);
            }
        };
        let input = Rc::new(WebViewInput {
            composition: self.input.composition.clone(),
            controller: webview.controller(),
            bounds: Cell::new(Bounds::default()),
            scale_factor: Cell::new(window.scale_factor()),
        });
        let old_parent_window = self.parent_window.get();
        if let Err(error) = attach_to_surface(
            webview,
            &composition,
            &surface,
            &visual,
            &input,
            parent_window,
            old_parent_window,
        ) {
            rollback_move(
                webview,
                &composition,
                &surface,
                &input,
                old_visual.as_ref(),
                parent_window,
                old_parent_window,
            );
            return Err(error);
        }

        self.replace_surface(surface, input, target_window, parent_window, cx);
        Ok(())
    }

    fn reattach(&self) -> Result<()> {
        if !self.attached.get() {
            let visual = visual_for_surface(&self.surface)?;
            unsafe { self.input.composition.SetRootVisualTarget(&visual)? };
            self.attached.set(true);
        }
        Ok(())
    }

    fn replace_surface(
        &mut self,
        surface: WindowCompositionSurface,
        input: Rc<WebViewInput>,
        target_window: AnyWindowHandle,
        parent_window: isize,
        cx: &mut gpui::App,
    ) {
        let old_window = self.window;
        let old_surface_id = self.surface.id();
        self.surface = surface;
        self.input = input;
        self.window = target_window;
        self.parent_window.set(parent_window);
        self.attached.set(true);
        let cx = cx.to_async();
        self.cx = cx.clone();
        cx.spawn(async move |cx| {
            let result = old_window.update(cx, |_, window, _| {
                window
                    .enable_window_composition()?
                    .remove_surface(old_surface_id)
            });
            if let Ok(Err(error)) = result {
                log::warn!("cannot release WebView composition surface: {error:#}");
            }
        })
        .detach();
    }

    pub(super) fn set_scale_factor(&self, scale_factor: f32) {
        self.input.scale_factor.set(scale_factor);
    }

    pub(super) fn set_bounds(&self, _: &wry::WebView, rect: wry::Rect) -> Result<()> {
        let scale_factor = self.input.scale_factor.get();
        let (position, extent) = logical_bounds(rect, scale_factor);
        let bounds = Bounds::new(
            point(px(position.x as f32), px(position.y as f32)),
            size(px(extent.width as f32), px(extent.height as f32)),
        )
        .to_device_pixels(scale_factor);
        self.surface.platform_surface()?.set_bounds(bounds)?;
        self.input.bounds.set(bounds);
        unsafe {
            self.input
                .controller
                .SetBounds(windows::Win32::Foundation::RECT {
                    left: 0,
                    top: 0,
                    right: bounds.size.width.0,
                    bottom: bounds.size.height.0,
                })?;
        }
        Ok(())
    }

    pub(super) fn set_geometry(
        &self,
        _: &wry::WebView,
        document: Bounds<Pixels>,
        visible: Bounds<Pixels>,
    ) -> Result<()> {
        let scale_factor = self.input.scale_factor.get();
        let visible_device = visible.to_device_pixels(scale_factor);
        let offset = document.origin - visible.origin;
        let left = (f32::from(offset.x) * scale_factor).round() as i32;
        let top = (f32::from(offset.y) * scale_factor).round() as i32;
        let width = (f32::from(document.size.width) * scale_factor).round() as i32;
        let height = (f32::from(document.size.height) * scale_factor).round() as i32;
        self.surface
            .platform_surface()?
            .set_bounds(visible_device)?;
        self.input
            .bounds
            .set(document.to_device_pixels(scale_factor));
        unsafe {
            self.input
                .controller
                .SetBounds(windows::Win32::Foundation::RECT {
                    left,
                    top,
                    right: left + width,
                    bottom: top + height,
                })?;
        }
        Ok(())
    }

    pub(super) fn set_visible(&self, visible: bool) -> Result<()> {
        self.surface.platform_surface()?.set_visible(visible)
    }

    pub(super) fn leave_window(&self, webview: &wry::WebView) {
        if let Err(error) = unsafe { self.input.composition.SetRootVisualTarget(None) } {
            log::warn!("detaching the WebView from its closing window: {error}");
        }
        if let Err(error) = self
            .surface
            .platform_surface()
            .and_then(|surface| surface.set_visible(false))
        {
            log::warn!("hiding the WebView in its closing window: {error:#}");
        }
        if let Err(error) = webview.focus_parent() {
            log::warn!("returning keyboard focus from the closing WebView: {error}");
        }
        self.attached.set(false);
    }

    pub(super) fn focus_parent(&self, webview: &wry::WebView) {
        if let Err(error) = webview.focus_parent() {
            log::warn!("returning keyboard focus to the GPUI window: {error}");
        }
    }

    pub(super) fn release(self, webview: Rc<wry::WebView>) {
        if let Err(error) = unsafe { self.input.composition.SetRootVisualTarget(None) } {
            log::warn!("clearing the WebView composition target: {error}");
        }
        self.cx
            .clone()
            .spawn(async move |cx| {
                let result = self.window.update(cx, |_, window, _| {
                    window
                        .enable_window_composition()?
                        .remove_surface(self.surface.id())
                });
                if let Ok(Err(error)) = result {
                    log::warn!("cannot release WebView composition surface: {error:#}");
                }
                drop(webview);
            })
            .detach();
    }
}

fn logical_bounds(
    rect: wry::Rect,
    scale_factor: f32,
) -> (wry::dpi::LogicalPosition<f64>, wry::dpi::LogicalSize<f64>) {
    (
        rect.position.to_logical(f64::from(scale_factor)),
        rect.size.to_logical(f64::from(scale_factor)),
    )
}

fn native_window_handle(window: &Window) -> Result<isize> {
    let handle =
        HasWindowHandle::window_handle(window).context("reading the native window handle")?;
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as isize),
        _ => anyhow::bail!("the GPUI window is not a Win32 window"),
    }
}

fn visual_for_surface(surface: &WindowCompositionSurface) -> Result<windows::core::IUnknown> {
    let platform_handle = surface.platform_surface()?.platform_handle()?;
    let visual = *platform_handle
        .downcast::<windows_062::core::IUnknown>()
        .map_err(|_| anyhow::anyhow!("GPUI did not provide a DirectComposition visual"))?;
    Ok(visual_for_wry(&visual))
}

fn visual_for_wry(visual: &windows_062::core::IUnknown) -> windows::core::IUnknown {
    let retained = visual.clone();
    let raw = <windows_062::core::IUnknown as windows_062::core::Interface>::as_raw(&retained);
    std::mem::forget(retained);
    // Both crate versions wrap the same COM ABI; transfer the cloned interface reference.
    unsafe { <windows::core::IUnknown as windows::core::Interface>::from_raw(raw.cast()) }
}
