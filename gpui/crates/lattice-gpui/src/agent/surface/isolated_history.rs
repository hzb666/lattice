use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use gpui::{AppContext as _, Context};
use lattice_i18n::t;

use super::{SynaraSurface, runtime::AgentRuntime};
use crate::agent::checkpoints::{Entry, Snapshot};

#[derive(Default)]
pub(super) struct IsolatedHistory {
    pub(super) enabled: bool,
    generation: u64,
    applying: bool,
    queued: Option<Snapshot>,
    published: BTreeMap<String, Snapshot>,
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
        if !finalized(&snapshot) {
            return;
        }
        if self.isolated_history.applying {
            self.isolated_history.queued = Some(snapshot);
            return;
        }
        let previous = self
            .isolated_history
            .published
            .get(&snapshot.active_thread_id);
        if previous == Some(&snapshot) && self.thread.as_deref() == Some(&snapshot.active_thread_id)
        {
            return;
        }
        let targets = checkpoint_targets(previous, &snapshot);
        if targets.is_empty() {
            self.finish_isolated_history(snapshot, Ok(()), cx);
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let supervisor = match AgentRuntime::get(cx).supervisor_for(&root) {
            Ok(supervisor) => supervisor,
            Err(error) => {
                self.finish_isolated_history(snapshot, Err(error), cx);
                return;
            }
        };
        self.isolated_history.applying = true;
        let generation = self.isolated_history.generation;
        let apply = cx.background_spawn(async move {
            supervisor
                .reconcile_project_workspace(&root, &targets)
                .map(|_| ())
        });
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
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        let history = &mut self.isolated_history;
        history.applying = false;
        let next = history.queued.take();
        match result {
            Ok(_) => {
                history.error = None;
                history
                    .published
                    .insert(snapshot.active_thread_id.clone(), snapshot.clone());
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

fn finalized(snapshot: &Snapshot) -> bool {
    let thread = URL_SAFE_NO_PAD.encode(&snapshot.active_thread_id);
    let prefix = format!("refs/lattice/checkpoints/{thread}/turn/");
    snapshot.entries.iter().all(|entry| {
        entry.thread_id == snapshot.active_thread_id
            && entry.turn_count > 0
            && entry.checkpoint_ref == format!("{prefix}{}", entry.turn_count)
    })
}

fn checkpoint_files(snapshot: &Snapshot) -> BTreeSet<(&str, &str)> {
    snapshot
        .entries
        .iter()
        .flat_map(|entry| {
            entry
                .files
                .iter()
                .map(|file| (entry.checkpoint_ref.as_str(), file.path.as_str()))
        })
        .collect()
}

fn checkpoint_targets(previous: Option<&Snapshot>, next: &Snapshot) -> BTreeMap<PathBuf, String> {
    let Some(previous) = previous else {
        return BTreeMap::new();
    };
    let previous_files = checkpoint_files(previous);
    let next_files = checkpoint_files(next);
    let mut removed = changed_files(previous, &next_files);
    let mut added = changed_files(next, &previous_files);
    removed.sort_unstable_by_key(|(entry, _)| Reverse(entry.turn_count));
    added.sort_unstable_by_key(|(entry, _)| entry.turn_count);
    let thread = URL_SAFE_NO_PAD.encode(&next.active_thread_id);
    let mut targets = BTreeMap::new();
    for (entry, path) in removed {
        let turn = URL_SAFE_NO_PAD.encode(&entry.turn_id);
        targets.insert(
            PathBuf::from(path),
            format!("refs/lattice/checkpoints/{thread}/turn-start/{turn}"),
        );
    }
    for (entry, path) in added {
        targets.insert(PathBuf::from(path), entry.checkpoint_ref.clone());
    }
    targets
}

fn changed_files<'a>(
    snapshot: &'a Snapshot,
    other: &BTreeSet<(&str, &str)>,
) -> Vec<(&'a Entry, &'a str)> {
    snapshot
        .entries
        .iter()
        .flat_map(|entry| {
            entry.files.iter().filter_map(move |file| {
                let path = file.path.as_str();
                (!other.contains(&(entry.checkpoint_ref.as_str(), path))).then_some((entry, path))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(checkpoint_ref: &str) -> Snapshot {
        crate::agent::checkpoints::parse_snapshot(&json!({
            "type": "lattice:project-history",
            "activeThreadId": "thread",
            "entries": [{
                "id": "agent:thread:turn",
                "label": "First title",
                "timestamp": "2026-11-23T00:00:00Z",
                "threadId": "thread",
                "threadTitle": "First title",
                "turnId": "turn",
                "turnCount": 1,
                "checkpointRef": checkpoint_ref,
                "files": [{"path": "paper.tex", "kind": "modified", "additions": 1, "deletions": 0}]
            }]
        }))
        .unwrap()
    }

    #[test]
    fn only_the_active_threads_completed_turn_can_supply_checkpoint_paths() {
        assert!(!finalized(&snapshot(
            "refs/lattice/checkpoints/dGhyZWFk/turn-live/dHVybg"
        )));
        assert!(!finalized(&snapshot(
            "refs/lattice/checkpoints/b3RoZXI/turn/1"
        )));
        assert!(!finalized(&snapshot("provider-diff:turn")));
        assert!(finalized(&snapshot(
            "refs/lattice/checkpoints/dGhyZWFk/turn/1"
        )));
    }

    #[test]
    fn a_replayed_or_renamed_checkpoint_cannot_authorize_writes() {
        let first = snapshot("refs/lattice/checkpoints/dGhyZWFk/turn/1");
        assert!(checkpoint_targets(None, &first).is_empty());
        let mut next = first.clone();
        next.entries[0].thread_title = "Updated title".into();
        assert!(checkpoint_targets(Some(&first), &next).is_empty());
        next.entries.clear();
        let path = PathBuf::from("paper.tex");
        assert_eq!(
            BTreeMap::from([(
                path.clone(),
                "refs/lattice/checkpoints/dGhyZWFk/turn/1".into()
            )]),
            checkpoint_targets(Some(&next), &first)
        );
        assert_eq!(
            BTreeMap::from([(
                path,
                "refs/lattice/checkpoints/dGhyZWFk/turn-start/dHVybg".into()
            )]),
            checkpoint_targets(Some(&first), &next)
        );
    }

    #[test]
    fn batched_changes_use_the_latest_completion_and_earliest_undo_baseline() {
        let mut both = snapshot("refs/lattice/checkpoints/dGhyZWFk/turn/1");
        let mut later = both.entries[0].clone();
        later.turn_id = "later".into();
        later.turn_count = 2;
        later.checkpoint_ref = "refs/lattice/checkpoints/dGhyZWFk/turn/2".into();
        both.entries.insert(0, later);
        let empty = Snapshot {
            active_thread_id: both.active_thread_id.clone(),
            entries: vec![],
        };
        let path = PathBuf::from("paper.tex");
        assert_eq!(
            "refs/lattice/checkpoints/dGhyZWFk/turn/2",
            checkpoint_targets(Some(&empty), &both)[&path]
        );
        assert_eq!(
            "refs/lattice/checkpoints/dGhyZWFk/turn-start/dHVybg",
            checkpoint_targets(Some(&both), &empty)[&path]
        );
    }
}
