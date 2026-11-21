use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use gpui::{RenderImage, SvgRenderer};
use lattice_core::util::sha256_hex;

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMAGES: usize = 32;

#[derive(Default)]
pub(super) struct PreviewCache {
    images: Mutex<VecDeque<(String, Arc<RenderImage>)>>,
}

impl PreviewCache {
    pub fn render(&self, svg: &[u8], renderer: &SvgRenderer) -> Result<Arc<RenderImage>, String> {
        let key = sha256_hex(svg);
        {
            let mut images = self
                .images
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(index) = images.iter().position(|(saved, _)| *saved == key)
                && let Some(entry) = images.remove(index)
            {
                let image = entry.1.clone();
                images.push_back(entry);
                return Ok(image);
            }
        }
        let image = renderer
            .render_single_frame(svg, 2.)
            .map_err(|error| error.to_string())?;
        if image_bytes(&image) <= MAX_BYTES {
            let mut images = self
                .images
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            images.retain(|(saved, _)| *saved != key);
            images.push_back((key, image.clone()));
            while images.len() > MAX_IMAGES
                || images
                    .iter()
                    .map(|(_, image)| image_bytes(image))
                    .sum::<usize>()
                    > MAX_BYTES
            {
                images.pop_front();
            }
        }
        Ok(image)
    }
}

fn image_bytes(image: &RenderImage) -> usize {
    image.as_bytes(0).map_or(0, <[u8]>::len)
}
