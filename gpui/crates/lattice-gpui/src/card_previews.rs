//! What the editor's cards draw their previews with
//! (`lattice_editor::latex::previews`): KaTeX's picture of an equation, from
//! the math island's page ([`math_page`], macOS), and a figure, read as the
//! Tauri app's `read_project_asset` and `referenceAssetPreviewDataUrl` read
//! it:
//!
//! - a PNG, JPEG, WebP or SVG is shown as it is, and a CDXML drawing is shown
//!   as the SVG Chemex exports for it;
//! - an Mnova file is exported with MestReNova, then shown as a PDF; a PDF
//!   shows its first page, at most 720px wide, as PDF.js draws it
//!   there, through the PDF viewer's PDFium;
//! - any other format (EPS) has no preview, and a file that cannot be read
//!   (gone, larger than 50 MB) failed to load.
//!
//! The loader is the build chip's infinity loader (`InfinityLoader`).

#[cfg(target_os = "macos")]
mod math_page;

use std::{
    num::NonZeroUsize,
    path::Path,
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
};

use gpui::{AnyElement, App, AppContext as _, ElementId, ImageFormat, ImageSource, Pixels, Task};
use lattice_editor::latex::graphics::SpectrumOverlay;
use lattice_editor::latex::previews::{
    self, Figure, MathPicture, MathRequest, PreviewHost, SpectrumAnnotation,
};
use lattice_ui::ActiveLattice as _;

/// The largest figure read for a preview (`MAX_INLINE_ASSET_BYTES`).
const FIGURE_BYTES_MAX: u64 = 50 * 1024 * 1024;
/// A PDF figure's first page is drawn this wide (`referenceAssetPreviewDataUrl`
/// scales it to 720 CSS pixels, at most twice its own size).
const PDF_WIDTH: u32 = 720;

struct Host {
    #[cfg(target_os = "macos")]
    math: math_page::MathPage,
}

/// Draw the editor's card previews from now on.
pub fn install(cx: &mut App) {
    let host = Host {
        #[cfg(target_os = "macos")]
        math: math_page::MathPage::start(cx),
    };
    previews::install(Rc::new(host), cx);
}

/// What a figure is, by its extension (`asset_mime_type`).
enum Kind {
    Image(ImageFormat),
    Pdf,
    ChemDraw,
    Mnova,
    Other,
}

fn kind_of(path: &Path) -> Kind {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
    match extension.as_deref() {
        Some("png") => Kind::Image(ImageFormat::Png),
        Some("jpg" | "jpeg") => Kind::Image(ImageFormat::Jpeg),
        Some("webp") => Kind::Image(ImageFormat::Webp),
        Some("svg") => Kind::Image(ImageFormat::Svg),
        Some("pdf") => Kind::Pdf,
        Some("cdxml") => Kind::ChemDraw,
        Some("mnova") => Kind::Mnova,
        _ => Kind::Other,
    }
}

fn read_figure(path: &Path) -> Option<Vec<u8>> {
    let size = std::fs::metadata(path).ok()?.len();
    (size <= FIGURE_BYTES_MAX)
        .then(|| std::fs::read(path).ok())
        .flatten()
}

impl PreviewHost for Host {
    fn math(
        &self,
        request: &MathRequest,
        cx: &mut App,
    ) -> Task<anyhow::Result<Option<MathPicture>>> {
        #[cfg(target_os = "macos")]
        {
            let picture = self.math.picture(request.clone());
            cx.spawn(async move |_| {
                picture
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("the math page stopped")))
            })
        }
        // Elsewhere the card keeps the equation's source.
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (request, cx);
            Task::ready(Ok(None))
        }
    }

    fn figure(
        &self,
        project_root: &Path,
        path: &Path,
        spectrum: Option<NonZeroUsize>,
        overlay: Option<SpectrumOverlay>,
        cx: &mut App,
    ) -> Task<Figure> {
        let path = path.to_path_buf();
        match kind_of(&path) {
            Kind::Other => Task::ready(if path.is_file() {
                Figure::Unavailable
            } else {
                Figure::Failed
            }),
            Kind::Image(format) => {
                let read = cx.background_spawn(async move { read_figure(&path) });
                cx.spawn(async move |_| match read.await {
                    Some(bytes) => Figure::Shown(ImageSource::Image(Arc::new(
                        gpui::Image::from_bytes(format, bytes),
                    ))),
                    None => Figure::Failed,
                })
            }
            Kind::Pdf => {
                let engine = crate::pdf::PdfEngine::global(cx).ok();
                cx.spawn(async move |cx| {
                    let Some(engine) = engine else {
                        return Figure::Failed;
                    };
                    match first_page(&engine, path, cx).await {
                        Some(image) => Figure::Shown(ImageSource::Render(image)),
                        None => Figure::Failed,
                    }
                })
            }
            Kind::ChemDraw => {
                let project_root = project_root.to_path_buf();
                let svg_renderer = cx.svg_renderer();
                let read = cx.background_spawn(async move {
                    let relative = path.strip_prefix(&project_root).ok()?.to_str()?;
                    let svg = lattice_core::chemistry::drawing_preview_svg(&project_root, relative)
                        .ok()??;
                    svg_renderer.render_single_frame(&svg, 2.).ok()
                });
                cx.spawn(async move |_| match read.await {
                    Some(image) => Figure::Shown(ImageSource::Render(image)),
                    None => Figure::Failed,
                })
            }
            Kind::Mnova => {
                let Some(engine) = crate::pdf::PdfEngine::global(cx).ok() else {
                    return Task::ready(Figure::Failed);
                };
                let project_root = project_root.to_path_buf();
                let relative = path
                    .strip_prefix(&project_root)
                    .ok()
                    .and_then(Path::to_str)
                    .map(str::to_owned);
                let spectrum = spectrum.unwrap_or(NonZeroUsize::MIN);
                let formula_root = project_root.clone();
                let export = cx.background_spawn(async move {
                    let relative = relative?;
                    let pdf = lattice_core::chemistry::mnova_preview_pdf(
                        &project_root,
                        &relative,
                        spectrum,
                    )
                    .ok()?;
                    let annotation = match overlay {
                        Some(overlay) => {
                            let text = lattice_core::chemistry::compound_formula_display(
                                &formula_root,
                                &overlay.compound,
                            )
                            .ok()??;
                            Some(SpectrumAnnotation {
                                text,
                                x: overlay.x,
                                y: overlay.y,
                                size: overlay.size,
                            })
                        }
                        None => None,
                    };
                    Some((pdf, annotation))
                });
                cx.spawn(async move |cx| {
                    let Some((pdf, annotation)) = export.await else {
                        return Figure::Failed;
                    };
                    match first_page(&engine, pdf, cx).await {
                        Some(image) => annotation.map_or(
                            Figure::Shown(ImageSource::Render(image.clone())),
                            |annotation| Figure::Spectrum(ImageSource::Render(image), annotation),
                        ),
                        None => Figure::Failed,
                    }
                })
            }
        }
    }

    fn compound(&self, project_root: &Path, compound_id: &str, cx: &mut App) -> Task<Figure> {
        let project_root = project_root.to_path_buf();
        let compound_id = compound_id.to_string();
        let svg_renderer = cx.svg_renderer();
        let read = cx.background_spawn(async move {
            let svg = lattice_core::chemistry::compound_preview_svg(&project_root, &compound_id)
                .ok()??;
            svg_renderer.render_single_frame(&svg, 2.).ok()
        });
        cx.spawn(async move |_| match read.await {
            Some(image) => Figure::Shown(ImageSource::Render(image)),
            None => Figure::Failed,
        })
    }

    fn loader(&self, id: ElementId, size: Pixels, cx: &App) -> AnyElement {
        crate::build::chip_mark::infinity_loader(id, size, cx.palette())
    }
}

/// A PDF figure's first page, [`PDF_WIDTH`] pixels wide.
pub(crate) async fn first_page(
    engine: &Arc<crate::pdf::PdfEngine>,
    path: std::path::PathBuf,
    cx: &mut gpui::AsyncApp,
) -> Option<Arc<gpui::RenderImage>> {
    let bytes = cx
        .background_executor()
        .spawn(async move { read_figure(&path) })
        .await?;
    let doc = engine.new_document();
    let drawn = async {
        let info = engine.open(doc, bytes).await.ok()?.ok()?;
        let page = engine
            .render(crate::pdf::RenderRequest {
                doc,
                generation: info.generation,
                page: 0,
                width_px: PDF_WIDTH,
                region: None,
                dark: false,
                cancel: Arc::new(AtomicBool::new(false)),
            })
            .await
            .ok()?
            .ok()?;
        Some(page.image)
    }
    .await;
    engine.close(doc);
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn figures_preview_the_formats_the_tauri_app_previews() {
        assert!(matches!(
            kind_of(Path::new("figs/Plot.PNG")),
            Kind::Image(ImageFormat::Png)
        ));
        assert!(matches!(
            kind_of(Path::new("a.jpeg")),
            Kind::Image(ImageFormat::Jpeg)
        ));
        assert!(matches!(
            kind_of(Path::new("a.svg")),
            Kind::Image(ImageFormat::Svg)
        ));
        assert!(matches!(kind_of(Path::new("a.pdf")), Kind::Pdf));
        assert!(matches!(kind_of(Path::new("a.cdxml")), Kind::ChemDraw));
        assert!(matches!(kind_of(Path::new("a.mnova")), Kind::Mnova));
        assert!(matches!(kind_of(Path::new("a.cdx")), Kind::Other));
        assert!(matches!(kind_of(Path::new("a.eps")), Kind::Other));
        assert!(matches!(kind_of(Path::new("a")), Kind::Other));
    }
}
