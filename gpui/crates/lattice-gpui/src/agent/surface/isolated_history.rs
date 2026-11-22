use std::{collections::BTreeSet, path::PathBuf};

use gpui::{AppContext as _, Context};
use lattice_i18n::t;

use super::{SynaraSurface, runtime::AgentRuntime};
use crate::agent::checkpoints::Snapshot;

#[derive(Default)]
pub(super) struct IsolatedHistory {
    pub(super) enabled: bool,
    generation: u64,
    applying: bool,
    queued: Option<Snapshot>,
    published: Option<Snapshot>,
    error: Option<String>,
}

impl IsolatedHistory {
    pub(super) fn reset(&mut self) {
        *self = Self {
            generation: self.generation.wrapping_add(1),
            ..Self::default()
        };
    }
}

impl SynaraSurface {
    pub(super) fn queue_isolated_history(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        if snapshot
            .entries
            .iter()
            .any(|entry| entry.checkpoint_ref.starts_with("provider-diff:"))
        {
            return;
        }
        let history = &mut self.isolated_history;
        if history.applying {
            history.queued = Some(snapshot);
            return;
        }
        if history.published.as_ref() == Some(&snapshot) {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let paths = checkpoint_paths(history.published.as_ref(), &snapshot);
        history.applying = true;
        let generation = history.generation;
        let supervisor = AgentRuntime::get(cx).supervisor().clone();
        let apply = cx
            .background_spawn(async move { supervisor.reconcile_project_workspace(&root, &paths) });
        cx.spawn(async move |this, cx| {
            let result = apply.await;
            this.update(cx, |this, cx| {
                if this.isolated_history.generation == generation {
                    this.finish_isolated_history(snapshot, result, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn finish_isolated_history(
        &mut self,
        snapshot: Snapshot,
        result: Result<Vec<std::path::PathBuf>, String>,
        cx: &mut Context<Self>,
    ) {
        let history = &mut self.isolated_history;
        history.applying = false;
        let next = history.queued.take();
        match result {
            Ok(_) => {
                history.error = None;
                history.published = Some(snapshot.clone());
                self.publish_history(snapshot, cx);
            }
            Err(error) if history.error.as_ref() != Some(&error) => {
                log::warn!("Could not publish isolated agent changes: {error}");
                history.error = Some(error);
                crate::notices::warning(
                    t!("Agent changes could not be applied to the project."),
                    cx,
                );
            }
            Err(_) => {}
        }
        if let Some(next) = next {
            self.queue_isolated_history(next, cx);
        }
    }
}

fn checkpoint_paths(previous: Option<&Snapshot>, next: &Snapshot) -> BTreeSet<PathBuf> {
    let previous = previous.filter(|snapshot| snapshot.active_thread_id == next.active_thread_id);
    let old = previous.into_iter().flat_map(|snapshot| &snapshot.entries);
    let added = next
        .entries
        .iter()
        .filter(|entry| previous.is_none_or(|snapshot| !snapshot.entries.contains(entry)));
    let removed = old.filter(|entry| !next.entries.contains(entry));
    added
        .chain(removed)
        .flat_map(|entry| &entry.files)
        .map(|file| PathBuf::from(&file.path))
        .collect()
}
