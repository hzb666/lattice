#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("This prototype is Windows-only.");
}

#[cfg(target_os = "windows")]
mod windows_prototype {
    use std::{rc::Rc, sync::mpsc};

    use anyhow::{Result, anyhow, bail};
    use gpui::{
        App, AppContext, Bounds, Context, InteractiveElement, IntoElement, MouseButton,
        ParentElement, Render, Styled, Window, WindowBounds, WindowComposition,
        WindowCompositionSurface, WindowOptions, canvas, deferred, div, px, rgb, size,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use webview2_com::{
        CoTaskMemPWSTR, CreateCoreWebView2CompositionControllerCompletedHandler,
        CreateCoreWebView2EnvironmentCompletedHandler, Microsoft::Web::WebView2::Win32::*,
    };
    use windows::{
        Win32::Foundation::{E_ABORT, E_POINTER, HWND, RECT},
        core::Interface,
    };

    const PAGE: &str = r#"<!doctype html>
<html><head><meta name="viewport" content="width=device-width,initial-scale=1">
<style>
html,body{margin:0;width:100%;height:100%;overflow:hidden;background:#122436}
main{height:100%;display:grid;grid-template-columns:repeat(3,1fr);gap:18px;padding:36px;box-sizing:border-box}
section{border-radius:24px;background:linear-gradient(145deg,#48c6ef,#6f86d6);box-shadow:0 24px 60px #0008}
section:nth-child(2){background:linear-gradient(145deg,#f6d365,#fda085)}
section:nth-child(3){background:linear-gradient(145deg,#84fab0,#8fd3f4)}
footer{position:absolute;left:36px;right:36px;bottom:36px;height:18%;border-radius:20px;background:linear-gradient(90deg,#8e2de2,#4a00e0)}
</style></head><body><main><section></section><section></section><section></section></main><footer></footer></body></html>"#;

    struct NativeWebView {
        _webview: ICoreWebView2,
        controller: ICoreWebView2Controller,
        _composition_controller: ICoreWebView2CompositionController,
        surface: WindowCompositionSurface,
    }

    impl NativeWebView {
        fn new(window: &Window, composition: &WindowComposition<'_>) -> Result<Self> {
            let raw_window = HasWindowHandle::window_handle(window)?;
            let hwnd = match raw_window.as_raw() {
                RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as _),
                _ => bail!("the prototype requires a Win32 window"),
            };
            let surface = composition.create_native_surface()?;
            let visual = surface.platform_surface()?.platform_handle()?;
            let visual = visual
                .downcast::<windows::core::IUnknown>()
                .map_err(|_| anyhow!("GPUI did not provide a DirectComposition visual"))?;

            let (environment_sender, environment_receiver) = mpsc::channel();
            CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
                Box::new(|handler| unsafe {
                    CreateCoreWebView2Environment(&handler)
                        .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |error, environment| {
                    let result = error.and_then(|_| {
                        environment.ok_or_else(|| windows::core::Error::from(E_POINTER))
                    });
                    environment_sender
                        .send(result)
                        .map_err(|_| windows::core::Error::from(E_ABORT))?;
                    Ok(())
                }),
            )?;
            let environment = webview2_com::wait_with_pump(environment_receiver)??;
            let environment = environment.cast::<ICoreWebView2Environment3>()?;

            let (controller_sender, controller_receiver) = mpsc::channel();
            CreateCoreWebView2CompositionControllerCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| unsafe {
                    environment
                        .CreateCoreWebView2CompositionController(hwnd, &handler)
                        .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |error, controller| {
                    let result = error.and_then(|_| {
                        controller.ok_or_else(|| windows::core::Error::from(E_POINTER))
                    });
                    controller_sender
                        .send(result)
                        .map_err(|_| windows::core::Error::from(E_ABORT))?;
                    Ok(())
                }),
            )?;
            let composition_controller = webview2_com::wait_with_pump(controller_receiver)??;
            unsafe { composition_controller.SetRootVisualTarget(&*visual)? };
            let controller = composition_controller.cast::<ICoreWebView2Controller>()?;
            unsafe { controller.SetIsVisible(true)? };
            let webview = unsafe { controller.CoreWebView2()? };
            let html = CoTaskMemPWSTR::from(PAGE);
            unsafe { webview.NavigateToString(*html.as_ref().as_pcwstr())? };

            Ok(Self {
                _webview: webview,
                controller,
                _composition_controller: composition_controller,
                surface,
            })
        }

        fn set_bounds(&self, bounds: Bounds<gpui::Pixels>, scale_factor: f32) -> Result<()> {
            let bounds = bounds.to_device_pixels(scale_factor);
            self.surface.platform_surface()?.set_bounds(bounds)?;
            unsafe {
                self.controller.SetBounds(RECT {
                    left: 0,
                    top: 0,
                    right: bounds.size.width.0,
                    bottom: bounds.size.height.0,
                })?;
                self.controller.SetIsVisible(true)?;
            }
            Ok(())
        }
    }

    struct Prototype {
        webview: Option<Rc<NativeWebView>>,
        overlay_open: bool,
    }

    impl Render for Prototype {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let mut root = div().size_full();
            if let Some(webview) = self.webview.clone() {
                root = root.child(
                    canvas(
                        move |bounds, window, _| {
                            if let Err(error) = webview.set_bounds(bounds, window.scale_factor()) {
                                log::error!("could not size the composition WebView: {error:#}");
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .size_full(),
                );
            } else {
                root = root.bg(rgb(0x7d2837));
            }

            if self.overlay_open {
                root = root.child(
                    deferred(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgb(0x08111c).opacity(0.48))
                            .child(
                                div()
                                    .w(px(420.))
                                    .h(px(230.))
                                    .rounded(px(20.))
                                    .border_1()
                                    .border_color(rgb(0xf3f5f7))
                                    .bg(rgb(0x26384a).opacity(0.94))
                                    .shadow_xl(),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.overlay_open = false;
                                    cx.notify();
                                }),
                            ),
                    )
                    .priority(1),
                );
            }

            root.child(
                deferred(
                    div()
                        .absolute()
                        .top(px(20.))
                        .right(px(20.))
                        .w(px(48.))
                        .h(px(48.))
                        .rounded(px(12.))
                        .bg(rgb(0xf2a65a))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.overlay_open = !this.overlay_open;
                                cx.notify();
                            }),
                        ),
                )
                .priority(2),
            )
        }
    }

    pub fn run() {
        gpui_kit::application()
            .with_assets(gpui_kit::assets::Assets)
            .run(|cx: &mut App| {
                gpui_kit::init(cx);
                let bounds = Bounds::centered(None, size(px(960.), px(680.)), cx);
                let result = cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |window, cx| {
                        let webview = window
                            .enable_window_composition()
                            .and_then(|composition| NativeWebView::new(window, &composition));
                        let webview = match webview {
                            Ok(webview) => Some(Rc::new(webview)),
                            Err(error) => {
                                eprintln!("failed to create composition WebView: {error:#}");
                                None
                            }
                        };
                        cx.new(|_| Prototype {
                            webview,
                            overlay_open: true,
                        })
                    },
                );
                if let Err(error) = result {
                    eprintln!("failed to open prototype window: {error:#}");
                    cx.quit();
                    return;
                }
                cx.activate(true);
            });
    }
}

#[cfg(target_os = "windows")]
fn main() {
    windows_prototype::run();
}
