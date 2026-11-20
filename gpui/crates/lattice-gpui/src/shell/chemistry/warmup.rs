use std::{path::PathBuf, sync::Arc, time::Duration};

use chemex_cdxml::DrawingDocument;
use gpui::{AppContext as _, Context, Task};

use super::ChemistryPanel;

#[derive(Default)]
pub(super) struct Warmup {
    pub allowed: bool,
    done: bool,
    task: Option<Task<()>>,
}

impl Warmup {
    pub fn reset(&mut self) {
        self.pause();
        self.done = false;
    }

    pub fn pause(&mut self) {
        self.task = None;
    }

    pub fn start(
        &mut self,
        drawing: (PathBuf, Arc<DrawingDocument>, Vec<String>),
        cx: &mut Context<ChemistryPanel>,
    ) {
        if !cfg!(windows)
            || !self.allowed
            || self.done
            || self.task.is_some()
            || drawing.2.is_empty()
        {
            return;
        }
        let (root, document, groups) = drawing;
        self.task = Some(cx.spawn(async move |panel, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            for group in groups {
                let root = root.clone();
                let document = document.clone();
                cx.background_spawn(async move {
                    if let Err(error) =
                        lattice_core::chemistry::preview_group_svg(&root, &document, &group)
                    {
                        log::debug!("Could not prewarm ChemDraw group {group}: {error}");
                    }
                })
                .await;
            }
            panel.update(cx, |panel, _| panel.warmup.done = true).ok();
        }));
    }
}
