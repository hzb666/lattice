use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use super::{native, snapshot, transaction, Workspace};
use tempfile::TempDir;

struct Fixture {
    scratch: TempDir,
    original: PathBuf,
    stage: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let scratch = tempfile::tempdir().unwrap();
        let original = scratch.path().join("original");
        let stage = scratch.path().join("stage");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&stage).unwrap();
        Self { scratch, original, stage }
    }

    fn prepare(&self) -> Workspace {
        Workspace::prepare(&self.original, &self.stage).unwrap()
    }
}

#[test]
fn reconciles_changes_and_refreshes_citations_without_touching_checkpoints() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.original.join(".git")).unwrap();
    fs::write(fixture.original.join(".git/marker"), "original git").unwrap();
    fs::write(fixture.original.join("paper.tex"), "draft").unwrap();
    fs::write(fixture.original.join("remove.tex"), "draft").unwrap();
    fs::write(fixture.original.join("refs.bib"), "original bibliography").unwrap();
    let mut workspace = fixture.prepare();
    assert!(fixture.stage.join(".git/HEAD").is_file());
    fs::write(fixture.stage.join(".git/marker"), "checkpoint").unwrap();
    fs::write(fixture.stage.join("paper.tex"), "agent edit").unwrap();
    fs::remove_file(fixture.stage.join("remove.tex")).unwrap();
    fs::create_dir(fixture.stage.join("parts")).unwrap();
    fs::write(fixture.stage.join("parts/new.tex"), "new content").unwrap();

    assert_eq!(
        vec!["paper.tex", "parts", "parts/new.tex", "remove.tex"]
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>(),
        workspace.reconcile().unwrap()
    );
    assert_eq!("agent edit", fs::read_to_string(fixture.original.join("paper.tex")).unwrap());
    assert_eq!("new content", fs::read_to_string(fixture.original.join("parts/new.tex")).unwrap());
    assert!(!fixture.original.join("remove.tex").exists());
    assert!(workspace.reconcile().unwrap().is_empty());

    fs::write(fixture.original.join("refs.bib"), "trusted citation edit").unwrap();
    workspace.refresh().unwrap();
    assert_eq!(
        "trusted citation edit",
        fs::read_to_string(fixture.stage.join("refs.bib")).unwrap()
    );
    assert_eq!("checkpoint", fs::read_to_string(fixture.stage.join(".git/marker")).unwrap());
    assert_eq!("original git", fs::read_to_string(fixture.original.join(".git/marker")).unwrap());
    assert!(workspace.reconcile().unwrap().is_empty());
}

#[test]
fn rejects_internal_configuration_and_protected_asset_changes_as_a_batch() {
    let fixture = Fixture::new();
    fs::write(fixture.original.join("paper.tex"), "draft").unwrap();
    let mut workspace = fixture.prepare();
    fs::write(fixture.stage.join("paper.tex"), "agent edit").unwrap();
    for name in
        [".latexmkrc", "LATEXMKRC", ".envrc", ".gitmodules", "references.BıB", "sample.MnOvA"]
    {
        let file = fixture.stage.join(name);
        fs::write(&file, "untrusted contents").unwrap();
        assert!(workspace.reconcile().unwrap_err().contains("protected"), "{name}");
        assert!(!fixture.original.join(name).exists());
        fs::remove_file(file).unwrap();
    }
    assert_eq!("draft", fs::read_to_string(fixture.original.join("paper.tex")).unwrap());
    for name in [".lattice", ".overleaf", ".papers", ".research", ".omp", ".CHEM"] {
        let dir = fixture.stage.join(name);
        fs::create_dir(&dir).unwrap();
        assert!(workspace.reconcile().unwrap_err().contains("protected"), "{name}");
        fs::remove_dir(dir).unwrap();
    }
    assert_eq!(vec![PathBuf::from("paper.tex")], workspace.reconcile().unwrap());
}

#[test]
fn checkpoint_writeback_rejects_unrelated_changes_before_publishing() {
    let fixture = Fixture::new();
    fs::write(fixture.original.join("paper.tex"), "draft").unwrap();
    let mut workspace = fixture.prepare();
    fs::write(fixture.stage.join("paper.tex"), "proposal").unwrap();
    fs::create_dir(fixture.stage.join("sections")).unwrap();
    fs::write(fixture.stage.join("sections/new.tex"), "section").unwrap();

    let allowed = [PathBuf::from("paper.tex")].into();
    let error = workspace.reconcile_checkpoint(&allowed).unwrap_err();
    assert!(error.contains("outside the selected checkpoint"), "{error}");
    assert_eq!("draft", fs::read_to_string(fixture.original.join("paper.tex")).unwrap());
    assert!(!fixture.original.join("sections").exists());

    let allowed = [PathBuf::from("paper.tex"), PathBuf::from("sections/new.tex")].into();
    workspace.reconcile_checkpoint(&allowed).unwrap();
    assert_eq!("section", fs::read_to_string(fixture.original.join("sections/new.tex")).unwrap());
}

#[test]
fn writer_conflicts_do_not_reset_the_base_or_allow_refresh_to_discard_staged_work() {
    let fixture = Fixture::new();
    let original = fixture.original.join("paper.tex");
    let stage = fixture.stage.join("paper.tex");
    fs::write(&original, "draft").unwrap();
    let mut workspace = fixture.prepare();
    fs::write(&original, "writer edit").unwrap();
    fs::write(&stage, "agent edit").unwrap();
    assert!(workspace.reconcile().unwrap_err().contains("writer change"));
    assert!(workspace.refresh().unwrap_err().contains("unpublished changes"));
    assert!(workspace.reconcile().unwrap_err().contains("writer change"));
    assert_eq!("writer edit", fs::read_to_string(&original).unwrap());
    assert_eq!("agent edit", fs::read_to_string(&stage).unwrap());
    fs::write(&stage, "draft").unwrap();
    workspace.refresh().unwrap();
    assert_eq!("writer edit", fs::read_to_string(&stage).unwrap());
}

#[test]
fn refuses_multi_link_and_alternate_stream_payloads() {
    let fixture = Fixture::new();
    let outside = fixture.scratch.path().join("outside.bib");
    fs::write(&outside, "private").unwrap();
    let mut workspace = fixture.prepare();
    let alias = fixture.stage.join("notes.tex");
    fs::hard_link(&outside, &alias).unwrap();
    assert!(workspace.reconcile().unwrap_err().contains("hard link"));
    assert!(!fixture.original.join("notes.tex").exists());
    fs::remove_file(alias).unwrap();
    fs::write(fixture.stage.join("notes.tex"), "agent").unwrap();
    fs::write(fixture.stage.join("notes.tex:payload"), "alternate stream").unwrap();
    assert!(workspace.reconcile().unwrap_err().contains("alternate data streams"));
    assert_eq!("private", fs::read_to_string(outside).unwrap());
}

#[test]
fn a_live_stage_writer_blocks_publication() {
    let fixture = Fixture::new();
    let mut workspace = fixture.prepare();
    let stage = fixture.stage.join("new.tex");
    fs::write(&stage, "edit").unwrap();
    let writer = OpenOptions::new().write(true).open(&stage).unwrap();
    assert!(workspace.reconcile().is_err());
    assert!(!fixture.original.join("new.tex").exists());
    drop(writer);
    assert_eq!(vec![PathBuf::from("new.tex")], workspace.reconcile().unwrap());
}

#[test]
fn source_namespace_changes_between_snapshot_and_publish_refuse_the_batch() {
    let fixture = Fixture::new();
    let mut workspace = fixture.prepare();
    fs::write(fixture.stage.join("new.tex"), "ordinary edit").unwrap();
    let original = snapshot::Snapshot::read(&workspace.original).unwrap();
    let stage = snapshot::Snapshot::read(&workspace.staging).unwrap();
    let steps = snapshot::plan(&workspace.baseline, &stage, &original, true).unwrap();
    fs::write(fixture.stage.join("late.bib"), "unpublished bibliography").unwrap();
    let error =
        transaction::run(&stage, &original, steps, &mut workspace.recovery_required).unwrap_err();
    assert!(error.to_string().contains("changed before publication"));
    assert!(native::children(&workspace.original.directory).unwrap().is_empty());
    assert!(!workspace.recovery_required);
}

#[test]
fn overlapping_roots_and_reparse_ancestors_are_refused() {
    let fixture = Fixture::new();
    let nested = fixture.original.join("nested");
    fs::create_dir(&nested).unwrap();
    assert!(Workspace::prepare(&fixture.original, &nested).err().unwrap().contains("overlap"));
    let alias = fixture.scratch.path().join("alias");
    match std::os::windows::fs::symlink_dir(&fixture.original, &alias) {
        Ok(()) => {
            assert!(Workspace::prepare(&alias.join("nested"), &fixture.stage).is_err());
            fs::remove_dir(&alias).unwrap();
        }
        Err(error) if error.raw_os_error() == Some(1314) => {}
        Err(error) => panic!("create reparse fixture: {error}"),
    }
}

#[test]
fn reopen_creates_a_stage_under_a_trusted_parent_and_reuses_its_git() {
    let fixture = Fixture::new();
    let stage = fixture.stage.join("stable");
    fs::write(fixture.original.join("paper.tex"), "draft").unwrap();
    let workspace = Workspace::reopen(&fixture.original, &stage).unwrap();
    assert_eq!("draft", fs::read_to_string(stage.join("paper.tex")).unwrap());
    assert!(stage.join(".git/HEAD").is_file());
    drop(workspace);
    fs::write(stage.join(".git/marker"), "checkpoint").unwrap();
    let mut reused = Workspace::reopen(&fixture.original, &stage).unwrap();
    assert!(reused.reconcile().unwrap().is_empty());
    assert_eq!("checkpoint", fs::read_to_string(stage.join(".git/marker")).unwrap());
}

#[test]
fn checkpoint_git_rejects_config_alternates_and_unowned_refs() {
    let fixture = Fixture::new();
    drop(fixture.prepare());
    let config = fixture.stage.join(".git/config");
    fs::write(&config, "[core]\n\tworktree = C:/Users/Public\n").unwrap();
    assert!(Workspace::reopen(&fixture.original, &fixture.stage).is_err());
    fs::write(&config, snapshot::CHECKPOINT_CONFIG).unwrap();

    let alternates = fixture.stage.join(".git/objects/info/alternates");
    fs::write(&alternates, fixture.original.to_string_lossy().as_bytes()).unwrap();
    assert!(Workspace::reopen(&fixture.original, &fixture.stage).is_err());
    fs::remove_file(alternates).unwrap();

    let unexpected = fixture.stage.join(".git/refs/remotes/origin/main");
    fs::create_dir_all(unexpected.parent().unwrap()).unwrap();
    fs::write(&unexpected, format!("{}\n", "0".repeat(40))).unwrap();
    assert!(Workspace::reopen(&fixture.original, &fixture.stage).is_err());
    fs::remove_file(&unexpected).unwrap();
    fs::remove_dir(unexpected.parent().unwrap()).unwrap();
    fs::remove_dir(unexpected.parent().unwrap().parent().unwrap()).unwrap();

    let packed_refs = fixture.stage.join(".git/packed-refs");
    fs::write(&packed_refs, format!("{} refs/remotes/origin/main\n", "0".repeat(40))).unwrap();
    assert!(Workspace::reopen(&fixture.original, &fixture.stage).is_err());
    fs::remove_file(packed_refs).unwrap();

    let outside = fixture.scratch.path().join("outside-object");
    fs::write(&outside, "object").unwrap();
    let alias = fixture.stage.join(".git/objects/info/linked-object");
    fs::hard_link(outside, &alias).unwrap();
    assert!(Workspace::reopen(&fixture.original, &fixture.stage).is_err());
}

#[test]
fn checkpoint_repository_supports_synara_tree_capture_without_project_git() {
    let fixture = Fixture::new();
    fs::write(fixture.original.join("paper.tex"), "draft").unwrap();
    fs::remove_dir(&fixture.stage).unwrap();
    let workspace = Workspace::reopen(&fixture.original, &fixture.stage).unwrap();
    let run_git = |args: &[&str], index: &Path| {
        let mut command = crate::commands::command("git");
        command
            .current_dir(&workspace.staging.path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_INDEX_FILE", index)
            .env("GIT_AUTHOR_NAME", "Lattice")
            .env("GIT_AUTHOR_EMAIL", "lattice@localhost")
            .env("GIT_COMMITTER_NAME", "Lattice")
            .env("GIT_COMMITTER_EMAIL", "lattice@localhost")
            .args(args)
            .output()
    };
    let index = fixture.scratch.path().join("checkpoint.index");
    let repository =
        run_git(&["rev-parse", "--is-inside-work-tree"], &index).unwrap_or_else(|error| {
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
            panic!("Git for Windows is required for this checkpoint integration test")
        });
    assert!(repository.status.success(), "{}", String::from_utf8_lossy(&repository.stderr));
    let added = run_git(&["add", "-A", "--", "."], &index).unwrap();
    assert!(added.status.success(), "{}", String::from_utf8_lossy(&added.stderr));
    let tree = run_git(&["write-tree"], &index).unwrap();
    assert!(tree.status.success(), "{}", String::from_utf8_lossy(&tree.stderr));
    let tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();
    let commit = run_git(&["commit-tree", &tree, "-m", "Agent checkpoint"], &index).unwrap();
    assert!(commit.status.success(), "{}", String::from_utf8_lossy(&commit.stderr));
    let commit = String::from_utf8_lossy(&commit.stdout).trim().to_string();
    let reference = "refs/lattice/checkpoints/bootstrap/turn-1";
    let update = run_git(&["update-ref", reference, &commit], &index).unwrap();
    assert!(update.status.success(), "{}", String::from_utf8_lossy(&update.stderr));
    let found = run_git(&["show-ref", "--verify", reference], &index).unwrap();
    assert!(found.status.success(), "{}", String::from_utf8_lossy(&found.stderr));
    let packed = run_git(&["pack-refs", "--all", "--prune"], &index).unwrap();
    assert!(packed.status.success(), "{}", String::from_utf8_lossy(&packed.stderr));
    assert!(fixture.stage.join(".git/packed-refs").is_file());
    drop(workspace);
    assert!(Workspace::reopen(&fixture.original, &fixture.stage).is_ok());
}

#[test]
fn reopen_refuses_divergence_including_a_stage_with_unpublished_deletions() {
    let fixture = Fixture::new();
    fs::write(fixture.original.join("paper.tex"), "draft").unwrap();
    drop(fixture.prepare());
    let file = fixture.stage.join("paper.tex");
    fs::write(&file, "unpublished").unwrap();
    assert!(Workspace::reopen(&fixture.original, &fixture.stage)
        .err()
        .unwrap()
        .contains("recovery"));
    assert_eq!("unpublished", fs::read_to_string(&file).unwrap());
    fs::remove_file(&file).unwrap();
    assert!(Workspace::reopen(&fixture.original, &fixture.stage).is_err());
    assert!(!file.exists());
    let inside = fixture.original.join("stage");
    assert!(Workspace::reopen(&fixture.original, &inside).is_err());
    assert!(!inside.exists());
}

#[test]
fn collision_during_publish_restores_previous_changes() {
    let fixture = Fixture::new();
    let earlier = fixture.original.join("a.tex");
    fs::write(&earlier, "original").unwrap();
    let mut workspace = fixture.prepare();
    fs::write(fixture.stage.join("a.tex"), "agent edit").unwrap();
    fs::write(fixture.stage.join("z.tex"), "agent new").unwrap();
    let original = snapshot::Snapshot::read(&workspace.original).unwrap();
    let stage = snapshot::Snapshot::read(&workspace.staging).unwrap();
    let steps = snapshot::plan(&workspace.baseline, &stage, &original, true).unwrap();
    fs::write(fixture.original.join("z.tex"), "writer new").unwrap();

    assert!(transaction::run(&stage, &original, steps, &mut workspace.recovery_required).is_err());
    assert!(!workspace.recovery_required);
    assert_eq!("original", fs::read_to_string(&earlier).unwrap());
    assert_eq!("writer new", fs::read_to_string(fixture.original.join("z.tex")).unwrap());
    assert!(native::children(&workspace.original.directory)
        .unwrap()
        .iter()
        .all(|entry| !entry.name.starts_with(super::RECOVERY_PREFIX)));
}
