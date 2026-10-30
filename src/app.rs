use crate::{
    change_tree::{ChangeTree, file_status_color, stats_label},
    file_icons::file_icon,
    image_diff::{ImageDiff, ImageVersions, image_format},
    strings::{area_label, language_name, tr},
};
use gpui_kit::assets::IconName;
use gpui_kit::base::{SelectableText, TestSupportExt};
use gpui_kit::{
    component::{
        ActiveTheme, Disableable, Icon, Selectable, Sizable, Theme, ThemeMode, TitleBar,
        button::{Button, ButtonCustomVariant, ButtonVariants},
        diff::{
            Diff, DiffChangeIndicator, DiffFile, DiffHunkSeparator, DiffLinePosition, DiffMode,
            DiffState,
        },
        h_flex,
        input::{Input, InputEvent as TextInputEvent, InputState},
        menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
        resizable::{h_resizable, resizable_panel, v_resizable},
        scroll::{Scrollbar, ScrollbarMode},
        tooltip::Tooltip,
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};
use patchlook::{
    external_open::{ApplicationIcon, FileToOpen, OpenWith, applications_for},
    git::{
        ALL_BRANCHES, Branch, BranchScope, Change, Commit, Comparison, DiffStats, HistoryPage,
        RefKind, Repository, Worktree, parse_remote,
    },
    github,
    graph::{Graph, GraphRow},
    preferences::{Appearance, Language, Layout, Preferences},
    zed,
};
use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

const FILE_SELECTION_DELAY: Duration = Duration::from_millis(120);
const COPY_FEEDBACK_DURATION: Duration = Duration::from_secs(2);
const HISTORY_SEARCH_THRESHOLD: usize = 20;
const MAX_GRAPH_LANES: usize = 10;
/// The viewed-branch choice for the checked-out branch; no choice means all branches.
const HEAD_BRANCH: &str = "HEAD";
const GRAPH_LANE_WIDTH: f32 = 12.;

gpui_kit::actions!(patchlook, [About, OpenRepository, Refresh, Quit]);

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Repositories,
    History,
}

enum FileContent {
    Text(Vec<DiffFile>),
    Image(ImageVersions),
}

impl FileContent {
    fn load(repo: &Repository, change: &Change, comparison: &Comparison) -> anyhow::Result<Self> {
        if image_format(&change.path).is_some() {
            Ok(Self::Image(ImageVersions::decode(
                change,
                repo.file_versions(change, comparison)?,
            )?))
        } else {
            Ok(Self::Text(DiffFile::parse(
                &repo.patch(change, comparison)?,
            )?))
        }
    }
}

/// Lists and their initial preview are prepared together before replacing visible content.
struct PreparedChanges {
    changes: Vec<Change>,
    stats: Vec<Option<DiffStats>>,
    preview: Option<anyhow::Result<FileContent>>,
}

impl PreparedChanges {
    fn load(
        repo: &Repository,
        changes: Vec<Change>,
        comparison: &Comparison,
        preview: bool,
    ) -> anyhow::Result<Self> {
        let stats = repo.file_stats(&changes, comparison)?;
        let preview = changes
            .first()
            .filter(|_| preview)
            .map(|change| FileContent::load(repo, change, comparison));
        Ok(Self {
            changes,
            stats,
            preview,
        })
    }
}

struct ChangeState {
    loaded: bool,
    changes: Vec<Change>,
    file_stats: Vec<Option<DiffStats>>,
    change_tree: Option<ChangeTree>,
    total_stats: Option<DiffStats>,
    file_scroll: UniformListScrollHandle,
    selected_file: Option<usize>,
    diff: Entity<DiffState>,
    image: Option<Entity<ImageDiff>>,
    image_subscription: Option<Subscription>,
    _diff_subscription: Subscription,
    files_loading: bool,
    patch_loading: bool,
    empty_patch: bool,
    error: Option<String>,
    load_task: Option<Task<()>>,
    patch_task: Option<Task<()>>,
    last_file_selection: Option<Instant>,
}

impl ChangeState {
    fn new(layout: Layout, cx: &mut Context<Patchlook>) -> Self {
        let diff = cx.new(|cx| DiffState::new([], cx).with_mode(diff_mode(layout)));
        let subscription = cx.observe(&diff, |_, _, cx| cx.notify());
        Self {
            loaded: false,
            changes: Vec::new(),
            file_stats: Vec::new(),
            change_tree: None,
            total_stats: None,
            file_scroll: UniformListScrollHandle::new(),
            selected_file: None,
            diff,
            image: None,
            image_subscription: None,
            _diff_subscription: subscription,
            files_loading: false,
            patch_loading: false,
            empty_patch: false,
            error: None,
            load_task: None,
            patch_task: None,
            last_file_selection: None,
        }
    }

    fn cancel_pending(&mut self) {
        self.load_task = None;
        self.patch_task = None;
        self.last_file_selection = None;
        self.files_loading = false;
        if self.patch_loading {
            // A cancelled selection has not produced its preview yet.
            self.selected_file = None;
        }
        self.patch_loading = false;
    }
}

/// A history row; the working tree, when it has changes, sits above the commits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Working,
    Commit(usize),
}

struct HistoryState {
    files: ChangeState,
    commits: Vec<Commit>,
    working_changes: bool,
    selected: Option<Row>,
    /// A second row picked with Ctrl/Cmd-click; the files then compare the two rows.
    compare: Option<Row>,
    /// The last plainly clicked row, which a Ctrl/Cmd-click compares against
    /// even while that row's files are still loading.
    anchor: Option<Row>,
    graph: Graph,
    /// One layout row per loaded commit; empty while searching.
    graph_rows: Vec<GraphRow>,
    working_graph: Option<GraphRow>,
    scope: Option<BranchScope>,
    has_more: bool,
    revision: Option<String>,
    query: String,
    search_available: bool,
    scroll: UniformListScrollHandle,
    loading: bool,
    load_task: Option<Task<()>>,
}

impl HistoryState {
    /// Lays the graph out afresh for a newly loaded first page and reveals its selection.
    fn start(&mut self, start: &HistoryStart) {
        self.working_changes = start.working_changes;
        self.scope = start.scope.clone();
        self.selected = start.selected;
        self.anchor = start.selected;
        self.compare = None;
        self.graph = Graph::default();
        self.graph_rows.clear();
        self.working_graph = None;
        if start.working_changes
            && let Some(head) = &start.head_id
        {
            self.working_graph = Some(self.graph.expect(head));
        }
        self.extend_graph();
        if let Some(selected) = self.selected {
            let index = match selected {
                Row::Working => 0,
                Row::Commit(index) => index + usize::from(self.working_changes),
            };
            self.scroll.scroll_to_item(index, ScrollStrategy::Center);
        }
    }

    /// A filtered search result has gaps, so it shows no graph.
    fn extend_graph(&mut self) {
        if !self.query.is_empty() {
            return;
        }
        for commit in &self.commits[self.graph_rows.len()..] {
            self.graph_rows
                .push(self.graph.row(&commit.id, &commit.parents));
        }
    }

    /// The row with the most label and subject text sets the scrollable width.
    fn widest_row(&self) -> Option<usize> {
        let offset = usize::from(self.working_changes);
        self.commits
            .iter()
            .enumerate()
            .max_by_key(|(_, commit)| {
                let labels: usize = commit.refs.iter().map(|label| label.name.len() + 2).sum();
                labels + commit.subject.len()
            })
            .map(|(index, _)| index + offset)
    }

    fn graph_lanes(&self) -> usize {
        self.graph_rows
            .iter()
            .chain(&self.working_graph)
            .map(|row| row.width)
            .max()
            .unwrap_or(0)
            .min(MAX_GRAPH_LANES)
    }

    fn row_count(&self) -> usize {
        usize::from(self.working_changes) + self.commits.len()
    }

    fn row_at(&self, index: usize) -> Row {
        match index.checked_sub(usize::from(self.working_changes)) {
            Some(commit) => Row::Commit(commit),
            None => Row::Working,
        }
    }

    fn comparison_for(&self, selected: Row, compare: Option<Row>) -> Comparison {
        let (base, target) = match (selected, compare) {
            (Row::Working, None) => return Comparison::Working,
            (Row::Commit(index), None) => return Comparison::Commit(self.commits[index].clone()),
            (Row::Working, Some(Row::Commit(base))) | (Row::Commit(base), Some(Row::Working)) => {
                (base, None)
            }
            // The list runs newest first, so the higher index is the older base.
            (Row::Commit(a), Some(Row::Commit(b))) => (a.max(b), Some(a.min(b))),
            (Row::Working, Some(Row::Working)) => return Comparison::Working,
        };
        Comparison::Range {
            base: self.commits[base].id.clone(),
            target: target.map(|index| self.commits[index].id.clone()),
        }
    }

    fn comparison(&self) -> Option<Comparison> {
        Some(self.comparison_for(self.selected?, self.compare))
    }

    fn new(layout: Layout, cx: &mut Context<Patchlook>) -> Self {
        Self {
            files: ChangeState::new(layout, cx),
            commits: Vec::new(),
            working_changes: false,
            selected: None,
            compare: None,
            anchor: None,
            graph: Graph::default(),
            graph_rows: Vec::new(),
            working_graph: None,
            scope: None,
            has_more: false,
            revision: None,
            query: String::new(),
            search_available: false,
            scroll: UniformListScrollHandle::new(),
            loading: false,
            load_task: None,
        }
    }
}

enum HistoryRequest {
    Branch(Option<String>),
    Search,
    More,
}

pub struct Patchlook {
    preferences: Preferences,
    about: Entity<crate::about::AboutPanel>,
    external_files: Vec<tempfile::TempDir>,
    #[cfg(test)]
    preferences_directory: tempfile::TempDir,
    repository: Option<Repository>,
    branch: String,
    branches: Vec<Branch>,
    worktrees: Vec<Worktree>,
    repository_scroll: UniformListScrollHandle,
    branch_scroll: UniformListScrollHandle,
    toggled_branch_folders: std::collections::HashSet<String>,
    branch_tab: BranchTab,
    /// The diff line under the pointer, which the diff's context menu acts on.
    hovered_line: Option<DiffLinePosition>,
    view: View,
    history: HistoryState,
    repository_preview: bool,
    history_search: Entity<InputState>,
    branch_search: Entity<InputState>,
    /// `host:/path` entry for opening a repository over SSH; shown on demand.
    remote_input: Option<(Entity<InputState>, Subscription)>,
    commit_copied: bool,
    copy_feedback_task: Option<Task<()>>,
    loading: bool,
    error: Option<String>,
    load_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
    focus: FocusHandle,
}

impl Patchlook {
    pub fn new(initial: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (preferences, error) = match Preferences::read() {
            Ok(preferences) => (preferences, None),
            Err(error) => (Preferences::default(), Some(format!("{error:#}"))),
        };
        apply_appearance(preferences.appearance, window, cx);
        apply_language(preferences.language, cx);
        let history_search =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("search_commits_placeholder")));
        let branch_search =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("search_branches")));
        let subscriptions = vec![
            cx.subscribe(&branch_search, |_, _, event, cx| {
                if matches!(event, TextInputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&history_search, |this, input, event, cx| {
                if matches!(event, TextInputEvent::PressEnter { .. })
                    || (matches!(event, TextInputEvent::Change)
                        && input.read(cx).value().is_empty()
                        && !this.history.query.is_empty())
                {
                    this.submit_history_search(cx);
                }
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.preferences.appearance == Appearance::System {
                    Theme::sync_system_appearance(Some(window), cx);
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() && this.preferences.language == Language::System {
                    apply_language(Language::System, cx);
                    this.history_search.update(cx, |input, cx| {
                        input.set_placeholder(tr("search_commits_placeholder"), window, cx);
                    });
                    let placeholder = this.branch_tab.placeholder();
                    this.branch_search.update(cx, |input, cx| {
                        input.set_placeholder(tr(placeholder), window, cx);
                    });
                }
            }),
        ];
        let initial = initial.or_else(|| preferences.repository.clone());
        let mut app = Self {
            history: HistoryState::new(preferences.layout, cx),
            preferences,
            about: cx.new(crate::about::AboutPanel::new),
            external_files: Vec::new(),
            #[cfg(test)]
            preferences_directory: tempfile::tempdir().unwrap(),
            repository: None,
            branch: String::new(),
            branches: vec![],
            worktrees: vec![],
            repository_scroll: UniformListScrollHandle::new(),
            branch_scroll: UniformListScrollHandle::new(),
            toggled_branch_folders: Default::default(),
            branch_tab: BranchTab::Branches,
            hovered_line: None,
            view: View::History,
            repository_preview: false,

            history_search,
            branch_search,
            remote_input: None,

            commit_copied: false,
            copy_feedback_task: None,
            loading: false,

            error,
            load_task: None,
            _subscriptions: subscriptions,
            focus: cx.focus_handle(),
        };
        window.focus(&app.focus, cx);
        if let Some(path) = initial {
            app.load(path, cx);
        }
        app
    }

    fn show_about(&mut self, _: &About, window: &mut Window, cx: &mut Context<Self>) {
        self.about.update(cx, |about, cx| about.show(window, cx));
    }

    fn save_preferences(&mut self) {
        #[cfg(not(test))]
        let result = self.preferences.write();
        #[cfg(test)]
        let result = self
            .preferences
            .write_to(&self.preferences_directory.path().join("preferences.json"));
        if let Err(error) = result {
            self.error = Some(format!("{}: {error:#}", tr("settings_failed")));
        }
    }

    pub fn open_repository(&mut self, _: &OpenRepository, _: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(tr("open_repository").into()),
        });
        cx.spawn(async move |this, cx| match prompt.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |this, cx| this.load(path, cx));
                }
            }
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(error.to_string());
                    cx.notify();
                });
            }
            _ => {}
        })
        .detach();
    }

    fn toggle_remote_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.remote_input.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("remote_placeholder")));
        let subscription = cx.subscribe(&input, |this, input, event, cx| {
            if matches!(event, TextInputEvent::PressEnter { .. }) {
                let value = input.read(cx).value().to_string();
                match parse_remote(&value) {
                    Some(path) => {
                        this.remote_input = None;
                        this.error = None;
                        this.load(path, cx);
                    }
                    None => this.error = Some(tr("remote_invalid").to_owned()),
                }
                cx.notify();
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.remote_input = Some((input, subscription));
        cx.notify();
    }

    fn active_files(&self) -> &ChangeState {
        &self.history.files
    }

    fn active_files_mut(&mut self) -> &mut ChangeState {
        &mut self.history.files
    }

    fn current_commit(&self) -> Option<&Commit> {
        match (self.history.selected, self.history.compare) {
            (Some(Row::Commit(index)), None) => self.history.commits.get(index),
            _ => None,
        }
    }

    /// Index/worktree listings label each file by area; commit listings by status.
    fn shows_areas(&self) -> bool {
        self.history.selected == Some(Row::Working) && self.history.compare.is_none()
    }

    fn current_comparison(&self) -> Comparison {
        self.history.comparison().unwrap_or(Comparison::Working)
    }

    fn visible_error(&self) -> Option<&String> {
        self.error.as_ref().or(self.active_files().error.as_ref())
    }

    fn viewed_branch(&self) -> Option<&str> {
        self.repository
            .as_ref()
            .and_then(|repo| self.preferences.viewed_branch(&repo.main))
    }

    fn viewed_branch_name(&self) -> String {
        match self.viewed_branch() {
            None => tr("all_branches").to_owned(),
            Some(HEAD_BRANCH) => self.branch.clone(),
            Some(reference) => self
                .branches
                .iter()
                .find(|branch| branch.reference == reference)
                .map_or_else(|| self.branch.clone(), |branch| branch.name.clone()),
        }
    }

    fn browse_branch(&mut self, reference: Option<String>, cx: &mut Context<Self>) {
        if self.loading
            || (reference.as_deref() == self.viewed_branch()
                && self.history.files.loaded
                && !self.history.loading)
        {
            return;
        }
        self.load_history_page(HistoryRequest::Branch(reference), cx);
    }

    /// Opening a recent repository returns to the worktree it was last viewed in.
    fn load(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self
            .repository
            .as_ref()
            .is_some_and(|repo| repo.root == path || repo.main == path)
        {
            return;
        }
        let target = self
            .preferences
            .worktree(&path)
            .map_or(path, Path::to_path_buf);
        self.load_repository(target, cx);
    }

    fn switch_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.loading
            && self
                .repository
                .as_ref()
                .is_some_and(|repo| repo.root != path)
        {
            self.load_repository(path, cx);
        }
    }

    /// Opening/refreshing a repository replaces its two snapshots. Tab changes never call this.
    fn load_repository(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.load_task = None;
        self.history.files.cancel_pending();
        self.history.load_task = None;
        self.history.loading = false;
        self.loading = true;
        self.error = None;
        let preview = self.view != View::Repositories;
        let query = self.history.query.clone();
        let recent = self.preferences.recent_repositories.clone();
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    // A remembered worktree that was since removed falls back to the main one.
                    let repo = Repository::open(&path).or_else(|error| {
                        recent
                            .iter()
                            .find(|entry| entry.worktree.as_ref() == Some(&path))
                            .map_or(Err(error), |entry| Repository::open(&entry.path))
                    })?;
                    let reference = recent
                        .iter()
                        .find(|entry| entry.path == repo.main)
                        .and_then(|entry| entry.viewed_branch.clone());
                    let branches = repo.branches()?;
                    let worktrees = repo.worktrees()?;
                    let branch = repo.branch_name()?;
                    let reference = reference.as_deref();
                    let page =
                        repo.history_filtered(0, Some(history_revision(reference)), &query)?;
                    let start = first_history_row(&repo, &page, shows_head(reference), preview)?;
                    anyhow::Ok((repo, branch, (branches, worktrees), page, start))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((repo, branch, (branches, worktrees), page, start)) => {
                        let query = std::mem::take(&mut this.history.query);
                        this.history = HistoryState::new(this.preferences.layout, cx);
                        this.history.query = query;
                        this.repository_preview = false;
                        this.reset_copy_feedback();
                        // Entries saved for linked worktrees fold into the repository's one entry.
                        this.preferences.recent_repositories.retain(|entry| {
                            entry.path == repo.main
                                || !worktrees.iter().any(|worktree| worktree.path == entry.path)
                        });
                        this.preferences.remember_repository(repo.main.clone());
                        if worktrees.len() > 1 {
                            this.preferences.remember_worktree(repo.root.clone());
                        }
                        let linked = (repo.root != repo.main).then(|| repo.root.clone());
                        this.preferences.set_worktree(&repo.main, linked);
                        this.repository = Some(repo);
                        this.branch = branch;
                        this.branches = branches;
                        this.worktrees = worktrees;
                        this.save_preferences();
                        this.history.revision = page.revision;
                        this.history.commits = page.commits;
                        this.history.has_more = page.has_more;
                        this.history.search_available =
                            this.history.commits.len() > HISTORY_SEARCH_THRESHOLD;
                        this.history.start(&start);
                        this.apply_changes(start.prepared, cx);
                    }
                    Err(error) => this.error = Some(format!("{}: {error:#}", tr("load_failed"))),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub fn refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(repo) = &self.repository {
            self.load_repository(repo.root.clone(), cx);
        }
    }

    fn remove_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let Some(index) = self.repository.as_ref().and_then(|repo| {
            self.preferences
                .recent_repositories
                .iter()
                .position(|entry| entry.path == repo.main)
        }) else {
            return;
        };
        self.preferences.recent_repositories.remove(index);
        self.preferences.repository = None;
        let next = self
            .preferences
            .recent_repositories
            .get(index)
            .or_else(|| self.preferences.recent_repositories.last())
            .map(|entry| entry.path.clone());

        self.load_task = None;
        self.repository = None;
        self.branch.clear();
        self.branches.clear();
        self.branch_scroll = UniformListScrollHandle::new();
        self.history = HistoryState::new(self.preferences.layout, cx);
        self.repository_preview = false;
        self.reset_copy_feedback();
        self.error = None;
        self.history_search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.save_preferences();
        if let Some(path) = next {
            self.load(path, cx);
        }
        cx.notify();
    }

    fn switch_view(&mut self, view: View, cx: &mut Context<Self>) {
        if self.view == view || self.loading {
            return;
        }
        self.view = view;
        if view == View::Repositories {
            self.repository_preview = false;
        }
        self.ensure_view_loaded(cx);
        cx.notify();
    }

    fn ensure_view_loaded(&mut self, cx: &mut Context<Self>) {
        if self.repository.is_none() {
            return;
        }
        if !self.history.files.loaded && !self.history.loading {
            let reference = self.viewed_branch().map(str::to_owned);
            self.load_history_page(HistoryRequest::Branch(reference), cx);
        } else if self.view == View::History && !self.history.files.files_loading {
            self.ensure_history_preview(cx);
        }
    }

    fn ensure_history_preview(&mut self, cx: &mut Context<Self>) {
        if self.history.files.loaded
            && self.history.files.selected_file.is_none()
            && !self.history.files.changes.is_empty()
        {
            self.select_source_file(0, cx);
        }
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        if !self.loading && !self.history.loading && self.history.has_more {
            self.load_history_page(HistoryRequest::More, cx);
        }
    }

    fn submit_history_search(&mut self, cx: &mut Context<Self>) {
        if self.loading || (self.history.loading && self.history.revision.is_none()) {
            return;
        }
        self.history.query = self.history_search.read(cx).value().trim().to_owned();
        self.load_history_page(HistoryRequest::Search, cx);
    }

    /// A new branch resolves a new tip; searches and pagination stay on the loaded tip.
    fn load_history_page(&mut self, request: HistoryRequest, cx: &mut Context<Self>) {
        let Some(repo) = self.repository.clone() else {
            return;
        };
        let reset = !matches!(request, HistoryRequest::More);
        let branch = match request {
            HistoryRequest::Branch(reference) => Some(reference),
            HistoryRequest::Search | HistoryRequest::More => None,
        };
        self.history.load_task = None;
        if reset {
            self.history.files.cancel_pending();
        }
        self.history.loading = true;
        self.history.files.error = None;
        let offset = if reset { 0 } else { self.history.commits.len() };
        let reference = match &branch {
            Some(reference) => reference.as_deref(),
            None => self.viewed_branch(),
        };
        let head = shows_head(reference);
        let revision = match &branch {
            Some(reference) => Some(history_revision(reference.as_deref()).to_owned()),
            None => self.history.revision.clone(),
        };
        let query = self.history.query.clone();
        let preview = self.view == View::History;
        self.history.load_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let page = repo.history_filtered(offset, revision.as_deref(), &query)?;
                    let prepared = if reset {
                        Some(first_history_row(&repo, &page, head, preview)?)
                    } else {
                        None
                    };
                    anyhow::Ok((page, prepared))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.history.loading = false;
                match result {
                    Ok((page, prepared)) => {
                        if reset {
                            this.reset_copy_feedback();
                            this.history.commits.clear();
                            this.history.scroll = UniformListScrollHandle::new();
                            this.repository_preview = false;
                        }
                        this.history.revision = page.revision;
                        this.history.commits.extend(page.commits);
                        this.history.has_more = page.has_more;
                        if this.history.query.is_empty() {
                            this.history.search_available =
                                this.history.commits.len() > HISTORY_SEARCH_THRESHOLD;
                        }
                        if let Some(start) = prepared {
                            this.history.start(&start);
                            this.apply_changes(start.prepared, cx);
                            if this.view == View::History {
                                this.ensure_history_preview(cx);
                            }
                        } else {
                            this.history.extend_graph();
                        }
                        if let Some(reference) = branch {
                            let repo = this.repository.as_ref().unwrap();
                            // Lazy first entry and Tab changes do not rewrite preferences.
                            if this.preferences.viewed_branch(&repo.main) != reference.as_deref() {
                                this.preferences.set_viewed_branch(&repo.main, reference);
                                this.save_preferences();
                            }
                        }
                    }
                    Err(error) => this.history.files.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn reset_copy_feedback(&mut self) {
        self.copy_feedback_task = None;
        self.commit_copied = false;
    }

    /// A plain click selects one row; a Ctrl/Cmd-click compares it with the selected row.
    fn select_row(&mut self, row: Row, compare: bool, cx: &mut Context<Self>) {
        if self.loading || self.history.loading {
            return;
        }
        let (selected, compare) = match self.history.anchor {
            Some(anchor) if compare && anchor != row => (anchor, Some(row)),
            _ => (row, None),
        };
        self.history.anchor = Some(selected);
        if self.history.selected == Some(selected)
            && self.history.compare == compare
            && self.history.files.loaded
            && !self.history.files.files_loading
            && self.history.files.error.is_none()
        {
            return;
        }
        let Some(repo) = self.repository.clone() else {
            return;
        };
        let comparison = self.history.comparison_for(selected, compare);
        self.history.files.cancel_pending();
        self.history.files.files_loading = true;
        self.history.files.error = None;
        let preview = self.view == View::History;
        self.history.files.load_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let changes = repo.changes(&comparison)?;
                    PreparedChanges::load(&repo, changes, &comparison, preview)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.history.files.files_loading = false;
                match result {
                    Ok(prepared) => {
                        this.reset_copy_feedback();
                        this.repository_preview = false;
                        this.history.selected = Some(selected);
                        this.history.compare = compare;
                        this.apply_changes(prepared, cx);
                        if this.view == View::History {
                            this.ensure_history_preview(cx);
                        }
                    }
                    Err(error) => this.history.files.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn apply_changes(&mut self, prepared: PreparedChanges, cx: &mut Context<Self>) {
        let mut files = ChangeState::new(self.preferences.layout, cx);
        files.loaded = true;
        files.total_stats = Some(prepared.stats.iter().flatten().copied().sum());
        files.change_tree = self.repository.as_ref().map(|repo| {
            ChangeTree::new(
                &prepared.changes,
                &prepared.stats,
                &repo.root,
                cx,
                move |this, index, cx| {
                    if this.history.files.selected_file != Some(index) {
                        this.select_file(index, cx);
                    }
                },
            )
        });
        files.changes = prepared.changes;
        files.file_stats = prepared.stats;
        self.history.files = files;
        if let Some(preview) = prepared.preview {
            let files = &mut self.history.files;
            files.selected_file = Some(0);
            if let Some(tree) = &files.change_tree {
                tree.reveal_file(0, cx);
            }
            self.apply_file_result(preview, cx);
        }
    }

    fn apply_file_result(&mut self, result: anyhow::Result<FileContent>, cx: &mut Context<Self>) {
        let files = &mut self.history.files;
        match result {
            Ok(content) => {
                let (diffs, image) = match content {
                    FileContent::Text(diffs) => (diffs, None),
                    FileContent::Image(image) => (Vec::new(), Some(image)),
                };
                files.empty_patch = diffs.is_empty() && image.is_none();
                files.image = image.map(|image| cx.new(|_| ImageDiff::new(image)));
                files.image_subscription = files
                    .image
                    .as_ref()
                    .map(|image| cx.observe(image, |_, _, cx| cx.notify()));
                files.diff.update(cx, |diff, cx| diff.set_files(diffs, cx));
            }
            Err(error) => {
                files.image = None;
                files.image_subscription = None;
                files.diff.update(cx, |diff, cx| diff.set_files([], cx));
                files.error = Some(format!("{}: {error:#}", tr("load_failed")));
            }
        }
    }

    fn toggle_file_list(&mut self, cx: &mut Context<Self>) {
        self.preferences.tree_file_list = !self.preferences.tree_file_list;
        self.save_preferences();
        if let Some(index) = self.active_files().selected_file {
            if self.preferences.tree_file_list {
                if let Some(tree) = &self.active_files().change_tree {
                    tree.reveal_file(index, cx);
                }
            } else {
                self.active_files()
                    .file_scroll
                    .scroll_to_item(index, ScrollStrategy::Center);
            }
        }
        cx.notify();
    }

    fn select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.loading || self.active_files().files_loading {
            return;
        }
        if self.view == View::Repositories {
            if !self.repository_preview {
                let files = &mut self.history.files;
                // Entering from the tree must not reveal another file's retained preview.
                // A pending selection may still be displaying the previously loaded file.
                if files.selected_file != Some(index) || files.patch_loading {
                    files.diff.update(cx, |diff, cx| diff.set_files([], cx));
                    files.image = None;
                    files.image_subscription = None;
                    files.empty_patch = false;
                }
            }
            self.repository_preview = true;
        }
        self.select_source_file(index, cx);
        cx.notify();
    }

    fn select_source_file(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.loading || self.history.files.files_loading {
            return;
        }
        if self.history.files.selected_file == Some(index) && self.history.files.error.is_none() {
            return;
        }
        let Some(repo) = self.repository.clone() else {
            return;
        };
        let comparison = self.current_comparison();
        let files = &mut self.history.files;
        let change = files.changes[index].clone();
        let now = cx.background_executor().now();
        // Only a burst of selections needs to settle. An isolated selection
        // should start loading immediately, including its first preview.
        let debounce = files
            .last_file_selection
            .replace(now)
            .is_some_and(|previous| now.duration_since(previous) < FILE_SELECTION_DELAY);
        files.selected_file = Some(index);
        if let Some(tree) = &files.change_tree {
            tree.reveal_file(index, cx);
        }
        files
            .file_scroll
            .scroll_to_item(index, ScrollStrategy::Center);
        files.patch_task = None;
        files.patch_loading = true;
        files.error = None;
        files.empty_patch = false;
        files.patch_task = Some(cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(FILE_SELECTION_DELAY).await;
            }
            let result = cx
                .background_spawn(async move { FileContent::load(&repo, &change, &comparison) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.history.files.patch_loading = false;
                this.apply_file_result(result, cx);
                cx.notify();
            });
        }));
    }

    fn previous_file(&self) -> Option<usize> {
        self.active_files()
            .selected_file
            .and_then(|index| index.checked_sub(1))
    }

    fn next_file(&self) -> Option<usize> {
        self.active_files()
            .selected_file
            .map(|index| index + 1)
            .filter(|index| *index < self.active_files().changes.len())
    }

    /// Runs a link lookup off the main thread and opens the web page it finds.
    fn open_link(
        &mut self,
        job: impl FnOnce(Repository) -> anyhow::Result<Option<String>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repository.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { job(repo) }).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Some(url)) => cx.open_url(&url),
                    Ok(None) => {}
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens Zed in the worktree that has `commit` checked out, or the opened one.
    fn open_in_zed(
        &mut self,
        commit: Option<String>,
        file: Option<(PathBuf, usize)>,
        cx: &mut Context<Self>,
    ) {
        let worktrees = self.worktrees.clone();
        self.open_link(
            move |repo| {
                let worktree = match &commit {
                    Some(commit) => repo.worktree_for(commit, &worktrees)?,
                    None => repo.root.clone(),
                };
                let file = file.as_ref().map(|(path, line)| (path.as_path(), *line));
                zed::open(&worktree, file).map(|_| None)
            },
            cx,
        );
    }

    /// The commit whose files the diff shows, which picks the worktree to edit them in.
    fn shown_commit(&self) -> Option<String> {
        match self.current_comparison() {
            Comparison::Commit(commit) => Some(commit.id),
            Comparison::Range { target, .. } => target,
            Comparison::Working => None,
        }
    }

    /// The selected file at a diff line, for opening in an editor.
    fn line_target(&self, position: Option<&DiffLinePosition>) -> Option<(PathBuf, usize)> {
        let files = self.active_files();
        let change = files.changes.get(files.selected_file?)?;
        Some((
            change.path.clone(),
            position.map_or(1, DiffLinePosition::line),
        ))
    }

    fn commit_menu(
        &self,
        row: Row,
        menu: component::menu::PopupMenu,
        cx: &mut Context<Self>,
    ) -> component::menu::PopupMenu {
        let app = cx.entity().downgrade();
        let action = move |handler: fn(&mut Self, Row, &mut Context<Self>)| {
            let app = app.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let _ = app.update(cx, |this, cx| handler(this, row, cx));
            }
        };
        let id = match row {
            Row::Commit(index) => self.history.commits.get(index).map(|c| c.id.clone()),
            Row::Working => None,
        };
        let commit_items = |menu: component::menu::PopupMenu| {
            menu.item(PopupMenuItem::new(tr("open_pull_request")).on_click(action(
                |this, row, cx| {
                    if let Some(id) = this.row_commit_id(row) {
                        this.open_link(
                            move |repo| github::pull_request_url(&repo, &id).map(Some),
                            cx,
                        );
                    }
                },
            )))
            .item(
                PopupMenuItem::new(tr("open_commit_on_github")).on_click(action(
                    |this, row, cx| {
                        if let Some(id) = this.row_commit_id(row) {
                            this.open_link(
                                move |repo| github::published_commit_url(&repo, &id).map(Some),
                                cx,
                            );
                        }
                    },
                )),
            )
            .item(
                PopupMenuItem::new(tr("copy_commit_id")).on_click(action(|this, row, cx| {
                    if let Some(id) = this.row_commit_id(row) {
                        cx.write_to_clipboard(ClipboardItem::new_string(id));
                    }
                })),
            )
            .separator()
        };
        menu.when(id.is_some(), commit_items)
            .item(
                PopupMenuItem::new(tr("open_in_zed")).on_click(action(|this, row, cx| {
                    this.open_in_zed(this.row_commit_id(row), None, cx)
                })),
            )
            .item(
                PopupMenuItem::new(tr("compare_with_selected"))
                    .disabled(self.history.anchor.is_none_or(|anchor| anchor == row))
                    .on_click(action(|this, row, cx| this.select_row(row, true, cx))),
            )
    }

    fn row_commit_id(&self, row: Row) -> Option<String> {
        match row {
            Row::Commit(index) => self.history.commits.get(index).map(|c| c.id.clone()),
            Row::Working => None,
        }
    }

    /// Repos shows files or a diff in one pane; Compare shows both side by side.
    fn open_in_compare_button(&self, cx: &mut Context<Self>) -> Button {
        icon_button("open-in-compare", IconName::ExternalLink, "open_in_compare")
            .text_color(cx.theme().muted_foreground)
            .disabled(self.history.selected.is_none())
            .on_click(cx.listener(|this, _, _, cx| this.switch_view(View::History, cx)))
    }

    fn commit_message_button(&self, cx: &mut Context<Self>) -> Button {
        icon_button(
            "toggle-commit-message",
            IconName::PanelBottom,
            if self.preferences.show_commit_message {
                "hide_commit_message"
            } else {
                "show_commit_message"
            },
        )
        .text_color(cx.theme().muted_foreground)
        .selected(self.preferences.show_commit_message)
        .toggled(self.preferences.show_commit_message)
        .on_click(cx.listener(|this, _, _, cx| {
            this.preferences.show_commit_message = !this.preferences.show_commit_message;
            this.save_preferences();
            cx.notify();
        }))
    }

    fn commit_id_button(&self, commit: &Commit, cx: &mut Context<Self>) -> Button {
        let id = commit.id.clone();
        Button::new("copy-commit-id")
            .label(if self.commit_copied {
                tr("commit_copied").to_owned()
            } else {
                commit.short_id.clone()
            })
            .xsmall()
            .ghost()
            .min_w(px(72.))
            .text_color(cx.theme().muted_foreground)
            .tooltip(format!("{}\n{}", tr("copy_commit_id"), commit.id))
            .accessibility_label(tr("copy_commit_id"))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
                this.commit_copied = true;
                this.copy_feedback_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(COPY_FEEDBACK_DURATION).await;
                    let _ = this.update(cx, |this, cx| {
                        this.commit_copied = false;
                        cx.notify();
                    });
                }));
                cx.notify();
            }))
    }

    fn set_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        self.preferences.layout = layout;
        self.history
            .files
            .diff
            .update(cx, |diff, cx| diff.set_mode(diff_mode(layout), cx));
        self.save_preferences();
        cx.notify();
    }

    fn set_appearance(
        &mut self,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preferences.appearance = appearance;
        apply_appearance(appearance, window, cx);
        self.save_preferences();
        cx.notify();
    }

    fn set_language(&mut self, language: Language, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences.language = language;
        apply_language(language, cx);
        self.history_search.update(cx, |input, cx| {
            input.set_placeholder(tr("search_commits_placeholder"), window, cx);
        });
        let placeholder = self.branch_tab.placeholder();
        self.branch_search.update(cx, |input, cx| {
            input.set_placeholder(tr(placeholder), window, cx);
        });
        self.save_preferences();
        cx.notify();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BranchTab {
    Branches,
    Worktrees,
}

impl BranchTab {
    fn placeholder(self) -> &'static str {
        match self {
            Self::Branches => "search_branches",
            Self::Worktrees => "search_worktrees",
        }
    }
}

/// A row of the worktree list.
enum WorktreeItem {
    Section(&'static str),
    /// A worktree opened lately, possibly of another repository.
    Recent(PathBuf),
    /// One of the current repository's worktrees.
    Current(usize),
}

/// A row of the branch tree.
enum BranchItem {
    All,
    Head,
    Folder {
        key: String,
        name: String,
        depth: usize,
        expanded: bool,
    },
    Branch {
        index: usize,
        name: String,
        depth: usize,
    },
}

#[derive(Default)]
struct BranchFolder {
    folders: std::collections::BTreeMap<String, BranchFolder>,
    branches: Vec<(String, usize)>,
}

/// What a fresh history page opens with.
struct HistoryStart {
    working_changes: bool,
    selected: Option<Row>,
    /// HEAD's commit when the page shows the checked-out branch, which the working tree sits on.
    head_id: Option<String>,
    scope: Option<BranchScope>,
    prepared: PreparedChanges,
}

/// The first history row is the working tree when the checked-out branch has changes.
fn first_history_row(
    repo: &Repository,
    page: &HistoryPage,
    head: bool,
    preview: bool,
) -> anyhow::Result<HistoryStart> {
    let working = repo.snapshot()?.changes;
    let working_changes = !working.is_empty();
    let head_id = repo.head_id()?.filter(|_| head);
    // Showing the checked-out branch opens on HEAD, which all branches may list lower down.
    let first = head_id
        .as_ref()
        .and_then(|id| page.commits.iter().position(|commit| &commit.id == id))
        .unwrap_or(0);
    let (selected, changes, comparison) = match page.commits.get(first) {
        _ if working_changes && head => (Some(Row::Working), working, Comparison::Working),
        Some(commit) => (
            Some(Row::Commit(first)),
            repo.commit_changes(commit)?,
            Comparison::Commit(commit.clone()),
        ),
        None => (None, Vec::new(), Comparison::Working),
    };
    let scope = match &page.revision {
        Some(tip) => repo.branch_scope(tip)?,
        None => None,
    };
    Ok(HistoryStart {
        working_changes,
        selected,
        head_id,
        scope,
        prepared: PreparedChanges::load(repo, changes, &comparison, preview)?,
    })
}

fn ref_chip(name: String, color: Hsla) -> Div {
    div()
        .flex_none()
        .px_1()
        .rounded_sm()
        .text_xs()
        .whitespace_nowrap()
        .bg(color.opacity(0.15))
        .text_color(color)
        .child(name)
}

fn ref_color(kind: RefKind, cx: &App) -> Hsla {
    match kind {
        RefKind::Head => cx.theme().primary,
        RefKind::Local => cx.theme().green,
        RefKind::Remote => cx.theme().blue,
        RefKind::Tag => cx.theme().yellow,
    }
}

/// Draws one row of the history graph; the working tree's node is hollow.
fn graph_canvas(
    row: Option<GraphRow>,
    lanes: usize,
    palette: [Hsla; 6],
    hollow: bool,
    background: Hsla,
) -> impl IntoElement {
    let x = move |bounds: Bounds<Pixels>, column: usize| {
        bounds.left() + px(GRAPH_LANE_WIDTH * (column as f32 + 0.5) + 4.)
    };
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let Some(row) = &row else {
                return;
            };
            let middle = bounds.top() + bounds.size.height / 2.;
            let mut line = |from: Point<Pixels>, to: Point<Pixels>, color: usize| {
                let mut path = PathBuilder::stroke(px(1.5));
                path.move_to(from);
                if from.x == to.x {
                    path.line_to(to);
                } else {
                    let half = (from.y + to.y) / 2.;
                    path.cubic_bezier_to(to, point(from.x, half), point(to.x, half));
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, palette[color % palette.len()]);
                }
            };
            let visible = |column: usize| column < lanes;
            for segment in row.top.iter().filter(|s| visible(s.from) && visible(s.to)) {
                line(
                    point(x(bounds, segment.from), bounds.top()),
                    point(x(bounds, segment.to), middle),
                    segment.color,
                );
            }
            for segment in row
                .bottom
                .iter()
                .filter(|s| visible(s.from) && visible(s.to))
            {
                line(
                    point(x(bounds, segment.from), middle),
                    point(x(bounds, segment.to), bounds.bottom()),
                    segment.color,
                );
            }
            if !visible(row.column) {
                return;
            }
            let center = point(x(bounds, row.column), middle);
            let dot = |radius: f32| {
                Bounds::new(
                    point(center.x - px(radius), center.y - px(radius)),
                    size(px(radius * 2.), px(radius * 2.)),
                )
            };
            let color = palette[row.color % palette.len()];
            window.paint_quad(fill(dot(4.), color).corner_radii(px(4.)));
            if hollow {
                window.paint_quad(fill(dot(2.5), background).corner_radii(px(2.5)));
            }
        },
    )
    .flex_none()
    .w(px(GRAPH_LANE_WIDTH * lanes as f32 + 8.))
    .h_full()
}

/// The revision a viewed-branch choice shows.
fn history_revision(reference: Option<&str>) -> &str {
    reference.unwrap_or(ALL_BRANCHES)
}

/// Whether a viewed-branch choice includes the checked-out branch and its working tree.
fn shows_head(reference: Option<&str>) -> bool {
    reference.is_none_or(|reference| reference == HEAD_BRANCH)
}

/// A worktree's folder name and the branch checked out there.
fn worktree_label(worktree: &Worktree) -> String {
    let folder = worktree
        .path
        .file_name()
        .unwrap_or(worktree.path.as_os_str())
        .to_string_lossy();
    let branch = worktree
        .branch
        .clone()
        .unwrap_or_else(|| worktree.head.chars().take(7).collect());
    format!("{folder} · {branch}")
}

fn apply_language(language: Language, cx: &mut App) {
    let language = language.resolve(sys_locale::get_locales());
    let code = language.code().expect("Resolved language has a locale");
    if &*component::locale() != code {
        component::set_locale(code);
        cx.refresh_windows();
    }
    crate::update_menus(cx);
}

fn diff_mode(layout: Layout) -> DiffMode {
    match layout {
        Layout::Split => DiffMode::Split,
        Layout::Inline => DiffMode::Unified,
    }
}

fn apply_appearance(appearance: Appearance, window: &mut Window, cx: &mut App) {
    cx.set_window_appearance(match appearance {
        Appearance::System => None,
        Appearance::Light => Some(WindowAppearance::Light),
        Appearance::Dark => Some(WindowAppearance::Dark),
    });
    match appearance {
        Appearance::System => Theme::sync_system_appearance(Some(window), cx),
        Appearance::Light => Theme::change(ThemeMode::Light, Some(window), cx),
        Appearance::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
    }
}

fn status_label(status: char) -> &'static str {
    tr(match status {
        'A' => "added",
        'D' => "deleted",
        'R' => "renamed",
        'C' => "copied",
        'T' => "type_changed",
        'U' => "conflict",
        _ => "modified",
    })
}

/// Map the same lowercase literal matches used by Git search back to UTF-8 source ranges.
fn search_match_ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let folded = text.to_lowercase();
    let matches = folded.match_indices(&query);
    if text.is_ascii() {
        return matches
            .map(|(start, value)| start..start + value.len())
            .collect();
    }

    // Lowercasing can change byte length or expand a character (for example İ).
    let mut folded_offset = 0;
    let spans = text
        .char_indices()
        .map(|(start, ch)| {
            let folded_start = folded_offset;
            folded_offset += ch.to_lowercase().map(char::len_utf8).sum::<usize>();
            (folded_start..folded_offset, start..start + ch.len_utf8())
        })
        .collect::<Vec<_>>();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (start, value) in matches {
        let first = spans.partition_point(|(folded, _)| folded.end <= start);
        let last = spans.partition_point(|(folded, _)| folded.start < start + value.len()) - 1;
        let range = spans[first].1.start..spans[last].1.end;
        if let Some(previous) = ranges
            .last_mut()
            .filter(|previous| previous.end > range.start)
        {
            previous.end = range.end;
        } else {
            ranges.push(range);
        }
    }
    ranges
}

fn appearance_label(appearance: Appearance) -> (IconName, &'static str) {
    match appearance {
        Appearance::Light => (IconName::Sun, "light"),
        Appearance::Dark => (IconName::Moon, "dark"),
        Appearance::System => (IconName::Monitor, "system"),
    }
}

fn icon_button(id: &'static str, icon: IconName, label: &'static str) -> Button {
    Button::new(id)
        .icon(icon)
        .small()
        .ghost()
        .tooltip(tr(label))
        .accessibility_label(tr(label))
}

fn application_menu_item(
    name: impl Into<SharedString>,
    icon: Option<ApplicationIcon>,
    fallback: IconName,
) -> PopupMenuItem {
    let name = name.into();
    let image: Option<ImageSource> = icon.map(|icon| match icon {
        ApplicationIcon::Png(bytes) => Arc::new(Image::from_bytes(ImageFormat::Png, bytes)).into(),
        ApplicationIcon::File(path) => path.into(),
    });
    PopupMenuItem::element(move |_, _| {
        let icon = match &image {
            Some(image) => img(image.clone())
                .size(px(16.))
                .object_fit(ObjectFit::Contain)
                .flex_none()
                .into_any_element(),
            None => Icon::new(fallback).size(px(16.)).into_any_element(),
        };
        h_flex()
            .id(name.clone())
            .test_support()
            .role(accesskit::Role::Label)
            .aria_label(name.clone())
            .gap_2()
            .child(icon)
            .child(name.clone())
    })
}

fn toolbar_divider(cx: &App) -> Div {
    div()
        .flex_none()
        .w(px(1.))
        .h(px(14.))
        .mx_1()
        .bg(cx.theme().border)
}

fn list_scrollbar(id: &'static str, handle: &UniformListScrollHandle) -> Div {
    div().absolute().inset_0().child(
        Scrollbar::vertical(handle)
            .id(id)
            .mode(ScrollbarMode::Scrolling)
            .viewport_from_layout()
            .styles(|styles| {
                styles
                    .track(|track| track.width(px(6.)))
                    .thumb(|thumb| thumb.width(px(6.)).inset(px(0.)).radius(px(2.)))
                    .thumb_hover(|thumb| thumb.width(px(6.)).inset(px(0.)))
                    .thumb_active(|thumb| thumb.width(px(6.)).inset(px(0.)))
            }),
    )
}

impl Patchlook {
    fn application_menu(&self, cx: &App) -> impl IntoElement {
        let fallback_focus = self.focus.clone();
        // Block the titlebar's native drag hitbox around the whole trigger.
        div().occlude().child(
            icon_button("application-menu", IconName::Menu, "application_menu")
                .text_color(cx.theme().muted_foreground)
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, window, cx| {
                    menu.action_context(
                        window.focused(cx).unwrap_or_else(|| fallback_focus.clone()),
                    )
                    .menu(tr("about"), Box::new(About))
                    .separator()
                    .menu(tr("quit"), Box::new(Quit))
                }),
        )
    }

    /// Switches among the repository's worktrees without adding Recent entries.
    fn worktree_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app = cx.entity().downgrade();
        let current = self.repository.as_ref().map(|repo| repo.root.clone());
        let label = self
            .worktrees
            .iter()
            .find(|worktree| Some(&worktree.path) == current.as_ref())
            .map(worktree_label)
            .unwrap_or_default();
        Button::new("worktree")
            .icon(IconName::FolderOpen)
            .child(div().max_w(px(220.)).truncate().text_xs().child(label))
            .small()
            .ghost()
            .tooltip(tr("switch_worktree"))
            .accessibility_label(tr("switch_worktree"))
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, cx| {
                let Some(entity) = app.upgrade() else {
                    return menu;
                };
                let this = entity.read(cx);
                let current = this.repository.as_ref().map(|repo| repo.root.clone());
                this.worktrees.iter().fold(
                    menu.min_w(px(260.))
                        .max_h(px(360.))
                        .scrollable(true)
                        .check_side(component::Side::Right),
                    |menu, worktree| {
                        let app = app.clone();
                        let path = worktree.path.clone();
                        menu.item(
                            PopupMenuItem::new(worktree_label(worktree))
                                .checked(current.as_ref() == Some(&worktree.path))
                                .on_click(move |_, _, cx| {
                                    let _ = app.update(cx, |this, cx| {
                                        this.switch_worktree(path.clone(), cx)
                                    });
                                }),
                        )
                    },
                )
            })
    }

    fn language_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app = cx.entity().downgrade();
        Button::new("language")
            .icon(IconName::Languages)
            .child(div().text_xs().child(tr("language_short")))
            .small()
            .ghost()
            .tooltip(tr("language"))
            .accessibility_label(tr("language"))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, cx| {
                let Some(entity) = app.upgrade() else {
                    return menu;
                };
                let selected = entity.read(cx).preferences.language;
                Language::ALL.into_iter().fold(
                    menu.min_w(px(210.))
                        .max_h(px(360.))
                        .scrollable(true)
                        .check_side(component::Side::Right),
                    |menu, language| {
                        let app = app.clone();
                        menu.item(
                            PopupMenuItem::new(language_name(language))
                                .checked(selected == language)
                                .on_click(move |_, window, cx| {
                                    let _ = app.update(cx, |this, cx| {
                                        this.set_language(language, window, cx)
                                    });
                                }),
                        )
                    },
                )
            })
    }

    fn external_file_target(&self) -> Option<FileToOpen> {
        let files = self.active_files();
        if self.loading || files.files_loading || files.patch_loading || self.history.loading {
            return None;
        }
        Some(FileToOpen {
            repository: self.repository.clone()?,
            change: files.changes.get(files.selected_file?)?.clone(),
            comparison: self.current_comparison(),
        })
    }

    fn open_external(&mut self, target: FileToOpen, application: OpenWith, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { target.launch(&application) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Some(directory)) => this.external_files.push(directory),
                    Ok(None) => {}
                    Err(error) => {
                        this.error = Some(format!("{}: {error:#}", tr("open_file_failed")))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn choose_external_program(&mut self, target: FileToOpen, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(tr("select_application").into()),
        });
        cx.spawn(async move |this, cx| match prompt.await {
            Ok(Ok(Some(paths))) => {
                if let Some(program) = paths.into_iter().next() {
                    let _ = this.update(cx, |this, cx| {
                        this.open_external(target, OpenWith::Program(program), cx)
                    });
                }
            }
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(format!("{}: {error:#}", tr("open_file_failed")));
                    cx.notify();
                });
            }
            _ => {}
        })
        .detach();
    }

    fn open_with_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let target = self.external_file_target();
        let app = cx.entity().downgrade();
        icon_button("open-with", IconName::ExternalLink, "open_with")
            .when(target.is_some(), |button| {
                button.text_color(cx.theme().muted_foreground)
            })
            .disabled(target.is_none())
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, cx| {
                let Some(target) = target.clone() else {
                    return menu;
                };
                menu = menu
                    .min_w(px(220.))
                    .max_h(px(360.))
                    .scrollable(true)
                    .label(tr(if target.change.status == 'D' {
                        "open_previous_version"
                    } else {
                        "open_working_file"
                    }));
                let mut choices = vec![(
                    tr("system_default_application").to_owned(),
                    OpenWith::Default,
                    None,
                )];
                match applications_for(&target.path()) {
                    Ok(applications) => {
                        choices.extend(applications.into_iter().map(|application| {
                            (
                                application.name,
                                OpenWith::Recommended(application.id),
                                application.icon,
                            )
                        }))
                    }
                    Err(error) => {
                        let _ = app.update(cx, |this, cx| {
                            this.error = Some(format!("{}: {error:#}", tr("applications_failed")));
                            cx.notify();
                        });
                    }
                }
                for (name, application, icon) in choices {
                    let app = app.clone();
                    let target = target.clone();
                    menu = menu.item(
                        application_menu_item(name, icon, IconName::Monitor).on_click(
                            move |_, _, cx| {
                                let _ = app.update(cx, |this, cx| {
                                    this.open_external(target.clone(), application.clone(), cx)
                                });
                            },
                        ),
                    );
                }
                let app = app.clone();
                menu.separator().item(
                    application_menu_item(tr("other_application"), None, IconName::FolderOpen)
                        .on_click(move |_, _, cx| {
                            let _ = app.update(cx, |this, cx| {
                                this.choose_external_program(target.clone(), cx)
                            });
                        }),
                )
            })
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let branch = self.viewed_branch_name();
        let branch_tooltip = tr("viewed_branch_tooltip")
            .replace("{branch}", &branch)
            .replace("{worktree}", &self.branch);
        let repository_name = self.repository.as_ref().map(|repo| {
            repo.main
                .file_name()
                .unwrap_or(repo.main.as_os_str())
                .to_string_lossy()
                .into_owned()
        });
        // Controls must hide the underlying Drag hitbox from Windows' native
        // hit testing, which runs before ordinary mouse/click event handlers.
        TitleBar::new().bg(cx.theme().title_bar).child(
            h_flex()
                .size_full()
                .min_w_0()
                .pr_2()
                .gap_2()
                .when(
                    cfg!(any(target_os = "windows", target_os = "linux")),
                    |bar| bar.child(self.application_menu(cx)),
                )
                .child(
                    Button::new("open-repository")
                        .occlude()
                        .icon(IconName::FolderOpen)
                        .small()
                        .ghost()
                        .tooltip(
                            self.repository
                                .as_ref()
                                .map(|repo| {
                                    format!("{}\n{}", tr("open_repository"), repo.root.display())
                                })
                                .unwrap_or_else(|| tr("open_repository").to_owned()),
                        )
                        .accessibility_label(tr("open_repository"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_repository(&OpenRepository, window, cx)
                        })),
                )
                .child(
                    div()
                        .max_w(px(180.))
                        .min_w_0()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(repository_name.unwrap_or_else(|| tr("app_name").to_owned())),
                )
                .when(self.worktrees.len() > 1, |view| {
                    view.child(div().occlude().child(self.worktree_menu(cx)))
                })
                .when(self.repository.is_some(), |view| {
                    view.child(
                        h_flex()
                            .id("toolbar-branch")
                            .test_support()
                            .role(accesskit::Role::Label)
                            .aria_label(branch_tooltip.clone())
                            .gap_1()
                            .max_w(px(320.))
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .tooltip(move |window, cx| {
                                Tooltip::new(branch_tooltip.clone()).build(window, cx)
                            })
                            .child(Icon::new(IconName::GitBranch).xsmall())
                            .child(div().truncate().child(branch)),
                    )
                })
                .child(
                    icon_button("refresh", IconName::RefreshCw, "refresh")
                        .occlude()
                        .xsmall()
                        .text_color(cx.theme().muted_foreground)
                        .disabled(self.repository.is_none() || self.loading)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.refresh(&Refresh, window, cx)),
                        ),
                )
                .child(div().flex_1())
                // Keep the dropdown's click-handling wrapper above the blocker.
                .child(div().occlude().child(self.language_menu(cx)))
                .child({
                    let (icon, current) = appearance_label(self.preferences.appearance);
                    let (_, next) = appearance_label(self.preferences.appearance.next());
                    let label = tr("theme_cycle")
                        .replace("{current}", tr(current))
                        .replace("{next}", tr(next));
                    Button::new("theme")
                        .occlude()
                        .icon(icon)
                        .small()
                        .ghost()
                        .tooltip(label.clone())
                        .accessibility_label(label)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_appearance(this.preferences.appearance.next(), window, cx);
                        }))
                }),
        )
    }

    fn repositories(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(32.))
                    .px_3()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr("recent_repositories"))
                    .child(self.preferences.recent_repositories.len().to_string())
                    .child(div().flex_1())
                    .child(
                        icon_button("add-remote-repository", IconName::Globe, "open_remote")
                            .xsmall()
                            .text_color(cx.theme().muted_foreground)
                            .selected(self.remote_input.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_remote_input(window, cx)
                            })),
                    )
                    .child(
                        icon_button("add-repository", IconName::Plus, "open_repository")
                            .xsmall()
                            .text_color(cx.theme().muted_foreground)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_repository(&OpenRepository, window, cx)
                            })),
                    )
                    .child(
                        icon_button("remove-repository", IconName::Minus, "remove_repository")
                            .xsmall()
                            .when(self.repository.is_some() && !self.loading, |button| {
                                button.text_color(cx.theme().muted_foreground)
                            })
                            .disabled(self.repository.is_none() || self.loading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.remove_repository(window, cx)
                            })),
                    ),
            )
            .when_some(self.remote_input.as_ref(), |view, (input, _)| {
                view.child(
                    div()
                        .flex_none()
                        .px_2()
                        .pb_2()
                        .on_action(
                            cx.listener(|this, _: &component::input::Escape, window, cx| {
                                this.toggle_remote_input(window, cx)
                            }),
                        )
                        .child(
                            Input::new(input)
                                .id("remote-repository-input")
                                .aria_label(tr("open_remote"))
                                .small(),
                        ),
                )
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "repositories-list",
                            self.preferences.recent_repositories.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|index| this.repository_row(index, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.repository_scroll)
                        .size_full(),
                    )
                    .child(list_scrollbar(
                        "repositories-scrollbar",
                        &self.repository_scroll,
                    )),
            )
            .into_any_element()
    }

    fn repository_row(&self, index: usize, cx: &mut Context<Self>) -> Div {
        let path = self.preferences.recent_repositories[index].path.clone();
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let selected = self
            .repository
            .as_ref()
            .is_some_and(|repo| repo.main == path);
        div().h(px(50.)).px_2().pb_1().child(
            Button::new(("repository", index))
                .custom(
                    ButtonCustomVariant::new(cx)
                        .hover(cx.theme().list_hover)
                        .active(cx.theme().secondary_active),
                )
                .w_full()
                .h_full()
                .justify_start()
                .selected(selected)
                .tooltip(path.display().to_string())
                .accessibility_label(path.display().to_string())
                .child(
                    h_flex()
                        .gap_2()
                        .w_full()
                        .min_w_0()
                        .child(
                            Icon::new(IconName::Folder)
                                .small()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            v_flex()
                                .min_w_0()
                                .flex_1()
                                .items_start()
                                .child(div().text_sm().truncate().child(name))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .truncate()
                                        .child(path.display().to_string()),
                                ),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.load(path.clone(), cx))),
        )
    }

    /// Flattens the branch tree into visible rows: all branches, the checked-out
    /// branch, then Local and each remote with their `/`-separated folders.
    fn branch_items(&self, cx: &App) -> Vec<BranchItem> {
        if self.repository.is_none() {
            return Vec::new();
        }
        let mut sections: Vec<(String, String, BranchFolder)> = Vec::new();
        for (index, branch) in self.branches.iter().enumerate() {
            let (key, label, path) = if branch.remote {
                let (remote, path) = branch.name.split_once('/').unwrap_or(("", &branch.name));
                (format!("remote/{remote}"), remote.to_owned(), path)
            } else {
                (
                    "local".to_owned(),
                    tr("local_branch").to_owned(),
                    &*branch.name,
                )
            };
            let section = match sections
                .iter()
                .position(|(existing, _, _)| *existing == key)
            {
                Some(position) => position,
                None => {
                    sections.push((key, label, BranchFolder::default()));
                    sections.len() - 1
                }
            };
            let mut folder = &mut sections[section].2;
            let mut parts: Vec<_> = path.split('/').collect();
            let leaf = parts.pop().unwrap_or_default();
            for part in parts {
                folder = folder.folders.entry(part.to_owned()).or_default();
            }
            folder.branches.push((leaf.to_owned(), index));
        }
        let query = self.branch_search.read(cx).value().trim().to_lowercase();
        if !query.is_empty() {
            // Matches are listed flat by full name so a hit inside a folder is never hidden.
            return self
                .branches
                .iter()
                .enumerate()
                .filter(|(_, branch)| branch.name.to_lowercase().contains(&query))
                .map(|(index, branch)| BranchItem::Branch {
                    index,
                    name: branch.name.clone(),
                    depth: 0,
                })
                .collect();
        }
        let mut items = vec![BranchItem::All, BranchItem::Head];
        for (key, label, folder) in sections {
            let expanded = self.branch_folder_expanded(&key);
            items.push(BranchItem::Folder {
                key: key.clone(),
                name: label,
                depth: 0,
                expanded,
            });
            if expanded {
                self.push_branch_folder(&mut items, &key, &folder, 1);
            }
        }
        items
    }

    fn push_branch_folder(
        &self,
        items: &mut Vec<BranchItem>,
        key: &str,
        folder: &BranchFolder,
        depth: usize,
    ) {
        for (name, child) in &folder.folders {
            let key = format!("{key}/{name}");
            let expanded = self.branch_folder_expanded(&key);
            items.push(BranchItem::Folder {
                key: key.clone(),
                name: name.clone(),
                depth,
                expanded,
            });
            if expanded {
                self.push_branch_folder(items, &key, child, depth + 1);
            }
        }
        for (name, index) in &folder.branches {
            items.push(BranchItem::Branch {
                index: *index,
                name: name.clone(),
                depth,
            });
        }
    }

    /// Remote sections start collapsed and everything else expanded; a click flips that.
    fn branch_folder_expanded(&self, key: &str) -> bool {
        let collapsed_by_default = key.starts_with("remote/") && !key[7..].contains('/');
        collapsed_by_default == self.toggled_branch_folders.contains(key)
    }

    /// Worktrees opened lately in any repository, then the current repository's.
    fn worktree_items(&self, cx: &App) -> Vec<WorktreeItem> {
        let query = self.branch_search.read(cx).value().trim().to_lowercase();
        let matches = |path: &Path, branch: Option<&str>| {
            query.is_empty()
                || path.to_string_lossy().to_lowercase().contains(&query)
                || branch.is_some_and(|branch| branch.to_lowercase().contains(&query))
        };
        let recent: Vec<_> = self
            .preferences
            .recent_worktrees
            .iter()
            .filter(|path| {
                !self
                    .worktrees
                    .iter()
                    .any(|worktree| &worktree.path == *path)
            })
            .filter(|path| matches(path, None))
            .map(|path| WorktreeItem::Recent(path.clone()))
            .collect();
        let mut items = Vec::new();
        if !recent.is_empty() {
            items.push(WorktreeItem::Section("recent_worktrees"));
            items.extend(recent);
        }
        if self.repository.is_some() {
            items.push(WorktreeItem::Section("repository_worktrees"));
            items.extend(
                self.worktrees
                    .iter()
                    .enumerate()
                    .filter(|(_, worktree)| matches(&worktree.path, worktree.branch.as_deref()))
                    .map(|(index, _)| WorktreeItem::Current(index)),
            );
        }
        items
    }

    fn set_branch_tab(&mut self, tab: BranchTab, window: &mut Window, cx: &mut Context<Self>) {
        self.branch_tab = tab;
        self.branch_search.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(tr(tab.placeholder()), window, cx);
        });
        self.branch_scroll = UniformListScrollHandle::new();
        cx.notify();
    }

    fn branch_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = match self.branch_tab {
            BranchTab::Branches => self.branch_items(cx).len(),
            BranchTab::Worktrees => self.worktree_items(cx).len(),
        };
        let tab = |id: &'static str, label: String, tab: BranchTab, cx: &mut Context<Self>| {
            Button::new(id)
                .label(label)
                .xsmall()
                .ghost()
                .selected(self.branch_tab == tab)
                .on_click(
                    cx.listener(move |this, _, window, cx| this.set_branch_tab(tab, window, cx)),
                )
        };
        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(32.))
                    .px_2()
                    .gap_1()
                    .child(tab(
                        "branches-tab",
                        format!("{} {}", tr("branches"), self.branches.len()),
                        BranchTab::Branches,
                        cx,
                    ))
                    .child(tab(
                        "worktrees-tab",
                        format!("{} {}", tr("worktrees"), self.worktrees.len()),
                        BranchTab::Worktrees,
                        cx,
                    )),
            )
            .child(
                div()
                    .flex_none()
                    .px_2()
                    .pb_2()
                    .on_action(
                        cx.listener(|this, _: &component::input::Escape, window, cx| {
                            this.branch_search
                                .update(cx, |input, cx| input.set_value("", window, cx));
                            window.focus(&this.focus, cx);
                        }),
                    )
                    .child(
                        Input::new(&self.branch_search)
                            .id("branch-search-input")
                            .aria_label(tr("search_branches"))
                            .small()
                            .cleanable(true),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "branches-list",
                            count,
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                match this.branch_tab {
                                    BranchTab::Branches => {
                                        let items = this.branch_items(cx);
                                        range
                                            .filter_map(|index| {
                                                let item = items.get(index)?;
                                                Some(this.branch_row(index, item, cx))
                                            })
                                            .collect::<Vec<_>>()
                                    }
                                    BranchTab::Worktrees => {
                                        let items = this.worktree_items(cx);
                                        range
                                            .filter_map(|index| {
                                                let item = items.get(index)?;
                                                Some(this.worktree_row(index, item, cx))
                                            })
                                            .collect::<Vec<_>>()
                                    }
                                }
                            }),
                        )
                        .track_scroll(&self.branch_scroll)
                        .size_full(),
                    )
                    .child(list_scrollbar("branches-scrollbar", &self.branch_scroll)),
            )
            .into_any_element()
    }

    fn worktree_row(&self, index: usize, item: &WorktreeItem, cx: &mut Context<Self>) -> Div {
        let (path, label, caption) = match item {
            WorktreeItem::Section(key) => {
                return div()
                    .h(px(30.))
                    .px_3()
                    .pt_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(key));
            }
            WorktreeItem::Recent(path) => {
                let folder = path.file_name().unwrap_or(path.as_os_str());
                let parent = path.parent().map(|parent| parent.display().to_string());
                (path.clone(), folder.to_string_lossy().into_owned(), parent)
            }
            WorktreeItem::Current(index) => {
                let worktree = &self.worktrees[*index];
                (worktree.path.clone(), worktree_label(worktree), None)
            }
        };
        let current = self
            .repository
            .as_ref()
            .is_some_and(|repo| repo.root == path);
        let recent = matches!(item, WorktreeItem::Recent(_));
        div().h(px(30.)).px_2().pb_1().child(
            Button::new(("worktree", index))
                .custom(
                    ButtonCustomVariant::new(cx)
                        .hover(cx.theme().list_hover)
                        .active(cx.theme().secondary_active),
                )
                .selected(current)
                .w_full()
                .h_full()
                .justify_start()
                .disabled(self.loading)
                .tooltip(path.display().to_string())
                .accessibility_label(label.clone())
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .child(
                            Icon::new(if current {
                                IconName::Eye
                            } else {
                                IconName::FolderOpen
                            })
                            .xsmall()
                            .text_color(cx.theme().muted_foreground),
                        )
                        .child(div().flex_none().text_sm().child(label))
                        .when_some(caption, |row, caption| {
                            row.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(caption),
                            )
                        }),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if recent {
                        this.load(path.clone(), cx);
                    } else {
                        this.switch_worktree(path.clone(), cx);
                    }
                })),
        )
    }

    fn branch_row(&self, index: usize, item: &BranchItem, cx: &mut Context<Self>) -> Div {
        let (reference, name, depth, icon, caption) = match item {
            BranchItem::All => (
                None,
                tr("all_branches").to_owned(),
                0,
                IconName::GitBranch,
                None,
            ),
            BranchItem::Head => (
                Some(HEAD_BRANCH.to_owned()),
                self.branch.clone(),
                0,
                IconName::GitBranch,
                Some(tr("head_branch")),
            ),
            BranchItem::Folder {
                name,
                depth,
                expanded,
                ..
            } => (
                None,
                name.clone(),
                *depth,
                if *expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                },
                None,
            ),
            BranchItem::Branch { index, name, depth } => {
                let branch = &self.branches[*index];
                (
                    Some(branch.reference.clone()),
                    name.clone(),
                    *depth,
                    IconName::GitBranch,
                    branch.is_head.then(|| tr("worktree_branch")),
                )
            }
        };
        let folder = match item {
            BranchItem::Folder { key, .. } => Some(key.clone()),
            _ => None,
        };
        let selected = folder.is_none() && self.viewed_branch() == reference.as_deref();
        let tooltip = match item {
            BranchItem::Branch { index, .. } => self.branches[*index].name.clone(),
            _ => name.clone(),
        };
        div().h(px(30.)).px_2().pb_1().child(
            Button::new(("branch", index))
                .custom(
                    ButtonCustomVariant::new(cx)
                        .hover(cx.theme().list_hover)
                        .active(cx.theme().secondary_active),
                )
                .selected(selected)
                .w_full()
                .h_full()
                .justify_start()
                .disabled(self.loading && folder.is_none())
                .tooltip(tooltip.clone())
                .accessibility_label(tooltip)
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .pl(px(14. * depth as f32))
                        .child(
                            Icon::new(if selected { IconName::Eye } else { icon })
                                .xsmall()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .when(folder.is_some(), |text| {
                                    text.text_color(cx.theme().muted_foreground)
                                })
                                .child(name),
                        )
                        .when_some(caption, |row, caption| {
                            row.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(caption),
                            )
                        }),
                )
                .on_click(cx.listener(move |this, _, _, cx| match &folder {
                    Some(key) => {
                        if !this.toggled_branch_folders.remove(key) {
                            this.toggled_branch_folders.insert(key.clone());
                        }
                        cx.notify();
                    }
                    None => this.browse_branch(reference.clone(), cx),
                })),
        )
    }

    fn sidebar_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        base::Tabs::new("sidebar-tabs")
            .flex()
            .flex_none()
            .h(px(32.))
            .bg(cx.theme().tab_bar)
            .children(
                [
                    (View::Repositories, "repositories"),
                    (View::History, "history"),
                ]
                .map(|(view, key)| {
                    let selected = self.view == view;
                    base::Tab::new(key)
                        .flex_1()
                        .h_full()
                        .text_sm()
                        .selected(selected)
                        .accessibility_label(tr(key))
                        .bg(if selected {
                            cx.theme().sidebar
                        } else {
                            cx.theme().tab_bar
                        })
                        .text_color(if selected {
                            cx.theme().foreground
                        } else {
                            cx.theme().muted_foreground
                        })
                        .when(!selected, |tab| {
                            tab.hover(|style| style.bg(cx.theme().list_hover))
                        })
                        .child(tr(key))
                        .on_click(cx.listener(move |this, _, _, cx| this.switch_view(view, cx)))
                }),
            )
    }

    fn diff_header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let compare_button =
            (self.view == View::Repositories).then(|| self.open_in_compare_button(cx));
        let file = self.active_files().diff.read(cx).files().first();
        let image = self
            .active_files()
            .image
            .as_ref()
            .map(|image| image.read(cx));
        let selected_path = self
            .active_files()
            .selected_file
            .and_then(|i| self.active_files().changes.get(i))
            .map(|change| change.path.as_path());
        let path = image
            .map(ImageDiff::path)
            .or_else(|| file.map(|file| std::path::Path::new(file.path().as_str())))
            .or(selected_path);
        let title = image
            .map(|image| image.title().to_owned())
            .or_else(|| {
                file.map(|file| match (file.original_path(), file.modified_path()) {
                    (Some(old), Some(new)) if old != new => format!("{old} → {new}"),
                    _ => file.path().to_string(),
                })
            })
            .or_else(|| selected_path.map(|path| path.display().to_string()));
        let folded_context = self.active_files().diff.read(cx).has_collapsed_context();
        h_flex()
            .flex_none()
            .h(px(32.))
            .px_2()
            .gap_1()
            .border_b(px(1. / window.scale_factor()))
            .border_color(cx.theme().border)
            .when(self.view == View::Repositories, |row| {
                row.child(
                    icon_button("back-to-files", IconName::ArrowLeft, "back_to_files")
                        .text_color(cx.theme().muted_foreground)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.repository_preview = false;
                            cx.notify();
                        })),
                )
            })
            .children(compare_button)
            .when_some(path, |row, path| {
                row.child(self.open_with_menu(cx)).child(file_icon(path))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .truncate()
                    .child(title.unwrap_or_default()),
            )
            .when_some(file, |row, file| {
                row.child(
                    h_flex()
                        .flex_none()
                        .gap_2()
                        .px_2()
                        .text_xs()
                        .child(
                            div().text_color(cx.theme().muted_foreground).child(
                                tr("diff_blocks")
                                    .replace("{count}", &file.change_block_count().to_string()),
                            ),
                        )
                        .child(
                            div()
                                .text_color(cx.theme().success)
                                .child(format!("+{}", file.additions())),
                        )
                        .child(
                            div()
                                .text_color(cx.theme().danger)
                                .child(format!("−{}", file.deletions())),
                        ),
                )
            })
            .child(
                icon_button("previous-file", IconName::ChevronUp, "previous_file")
                    .when(self.previous_file().is_some(), |button| {
                        button.text_color(cx.theme().muted_foreground)
                    })
                    .disabled(self.previous_file().is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(index) = this.previous_file() {
                            this.select_file(index, cx);
                        }
                    })),
            )
            .child(
                icon_button("next-file", IconName::ChevronDown, "next_file")
                    .when(self.next_file().is_some(), |button| {
                        button.text_color(cx.theme().muted_foreground)
                    })
                    .disabled(self.next_file().is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(index) = this.next_file() {
                            this.select_file(index, cx);
                        }
                    })),
            )
            .child(toolbar_divider(cx))
            .when(self.active_files().image.is_none(), |row| {
                row.child(
                    icon_button(
                        "toggle-context",
                        if folded_context {
                            IconName::ChevronsUpDown
                        } else {
                            IconName::ChevronsDownUp
                        },
                        if folded_context {
                            "expand_context"
                        } else {
                            "collapse_context"
                        },
                    )
                    .when(file.is_some(), |button| {
                        button.text_color(cx.theme().muted_foreground)
                    })
                    .disabled(file.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.active_files().diff.update(cx, |diff, cx| {
                            if diff.has_collapsed_context() {
                                diff.expand_unchanged(cx);
                            } else {
                                diff.collapse_unchanged(cx);
                            }
                        });
                    })),
                )
                .child(
                    icon_button("soft-wrap", IconName::TextWrap, "soft_wrap")
                        .text_color(cx.theme().muted_foreground)
                        .selected(self.preferences.soft_wrap)
                        .toggled(self.preferences.soft_wrap)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.preferences.soft_wrap = !this.preferences.soft_wrap;
                            this.save_preferences();
                            cx.notify();
                        })),
                )
                .children(
                    [
                        (Layout::Split, "split", IconName::Columns2),
                        (Layout::Inline, "inline", IconName::Rows2),
                    ]
                    .map(|(layout, key, icon)| {
                        icon_button(key, icon, key)
                            .text_color(cx.theme().muted_foreground)
                            .selected(self.preferences.layout == layout)
                            .toggled(self.preferences.layout == layout)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.set_layout(layout, cx)),
                            )
                    }),
                )
            })
            .when_some(self.active_files().image.as_ref(), |row, image| {
                row.child(ImageDiff::toolbar(image, cx))
            })
            .when(self.current_commit().is_some(), |row| {
                row.child(toolbar_divider(cx))
                    .child(self.commit_message_button(cx))
            })
    }

    fn files(&self, cx: &mut Context<Self>) -> AnyElement {
        let tree_view = self.view == View::Repositories || self.preferences.tree_file_list;
        let tree = self
            .active_files()
            .change_tree
            .as_ref()
            .filter(|_| tree_view);
        v_flex()
            .size_full()
            .min_h_0()
            .bg(if self.view == View::Repositories {
                cx.theme().background
            } else {
                cx.theme().sidebar
            })
            .child(
                h_flex()
                    .flex_none()
                    .h(px(32.))
                    .pl_3()
                    .pr_2()
                    .when(!tree_view, |row| row.pr_4())
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr("files"))
                    .child(self.active_files().changes.len().to_string())
                    .when(self.view != View::Repositories, |row| {
                        row.child(
                            icon_button(
                                "toggle-file-list",
                                if self.preferences.tree_file_list {
                                    IconName::List
                                } else {
                                    IconName::FolderTree
                                },
                                if self.preferences.tree_file_list {
                                    "flat_file_list"
                                } else {
                                    "tree_file_list"
                                },
                            )
                            .ml_1()
                            .icon(
                                Icon::new(if self.preferences.tree_file_list {
                                    IconName::List
                                } else {
                                    IconName::FolderTree
                                })
                                .text_color(cx.theme().muted_foreground),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_file_list(cx))),
                        )
                        .child(self.file_stats_button(cx))
                    })
                    .child(div().flex_1())
                    .when_some(
                        tree.filter(|_| self.view == View::Repositories),
                        |row, tree| {
                            let expand = tree.has_collapsed_directories(cx);
                            row.child(
                                icon_button(
                                    "toggle-tree-directories",
                                    if expand {
                                        IconName::ChevronsUpDown
                                    } else {
                                        IconName::ChevronsDownUp
                                    },
                                    if expand {
                                        "expand_directories"
                                    } else {
                                        "collapse_directories"
                                    },
                                )
                                .disabled(self.active_files().changes.is_empty())
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        if let Some(tree) = &this.active_files().change_tree {
                                            tree.toggle_directories(cx);
                                        }
                                    },
                                )),
                            )
                            .child(self.file_stats_button(cx))
                            .child(toolbar_divider(cx))
                        },
                    )
                    .when_some(self.active_files().total_stats, |row, stats| {
                        row.child(
                            h_flex()
                                .gap_2()
                                .id("change-totals")
                                .test_support()
                                .role(accesskit::Role::Label)
                                .aria_label(format!(
                                    "{} +{} −{}",
                                    tr("total_changes"),
                                    stats.additions,
                                    stats.deletions
                                ))
                                .child(
                                    div()
                                        .text_color(cx.theme().success)
                                        .child(format!("+{}", stats.additions)),
                                )
                                .child(
                                    div()
                                        .text_color(cx.theme().danger)
                                        .child(format!("−{}", stats.deletions)),
                                ),
                        )
                    })
                    .when(
                        self.view == View::Repositories && self.current_commit().is_some(),
                        |row| {
                            row.child(toolbar_divider(cx))
                                .child(self.commit_message_button(cx))
                        },
                    )
                    .when(self.view == View::Repositories, |row| {
                        row.child(self.open_in_compare_button(cx))
                    }),
            )
            .child(if let Some(tree) = tree {
                div()
                    .flex_1()
                    .min_h_0()
                    .child(tree.render(
                        match self.view {
                            View::Repositories => Some(self.viewed_branch_name()),
                            View::History => None,
                        },
                        self.preferences.show_file_stats,
                        self.shows_areas(),
                        cx.listener(|this, index: &usize, _, cx| this.select_file(*index, cx)),
                        cx,
                    ))
                    .into_any_element()
            } else {
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "files-list",
                            self.active_files().changes.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|index| this.file_row(index, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.active_files().file_scroll)
                        .size_full(),
                    )
                    .child(list_scrollbar(
                        "files-list-scrollbar",
                        &self.active_files().file_scroll,
                    ))
                    .into_any_element()
            })
            .into_any_element()
    }

    fn file_stats_button(&self, cx: &mut Context<Self>) -> Button {
        icon_button(
            "toggle-file-stats",
            IconName::Diff,
            if self.preferences.show_file_stats {
                "hide_file_stats"
            } else {
                "show_file_stats"
            },
        )
        .when(self.view != View::Repositories, |button| {
            button.icon(Icon::new(IconName::Diff).text_color(cx.theme().muted_foreground))
        })
        .selected(self.preferences.show_file_stats)
        .toggled(self.preferences.show_file_stats)
        .on_click(cx.listener(|this, _, _, cx| {
            this.preferences.show_file_stats = !this.preferences.show_file_stats;
            this.save_preferences();
            cx.notify();
        }))
    }

    fn file_row(&self, index: usize, cx: &mut Context<Self>) -> Div {
        let change = &self.active_files().changes[index];
        let filename = change
            .path
            .file_name()
            .unwrap_or(change.path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let caption = if self.shows_areas() {
            area_label(change.area)
        } else {
            status_label(change.status)
        };
        let status_color = file_status_color(change.status, cx);
        div().h(px(50.)).px_2().pb_1().child(
            Button::new(("file", index))
                .custom(
                    ButtonCustomVariant::new(cx)
                        .hover(cx.theme().list_hover)
                        .active(cx.theme().secondary_active),
                )
                .w_full()
                .h_full()
                .justify_start()
                .px_2()
                .border_0()
                .selected(self.active_files().selected_file == Some(index))
                .tooltip(change.path.display().to_string())
                .accessibility_label(format!("{}, {}", change.path.display(), caption))
                .child(
                    h_flex()
                        .gap_2()
                        .w_full()
                        .min_w_0()
                        .child(file_icon(&change.path).size(px(20.)))
                        .child(
                            v_flex()
                                .min_w_0()
                                .flex_1()
                                .items_start()
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_sm()
                                                .text_color(status_color)
                                                .truncate()
                                                .child(filename),
                                        )
                                        .when_some(
                                            self.active_files()
                                                .file_stats
                                                .get(index)
                                                .copied()
                                                .filter(|_| self.preferences.show_file_stats),
                                            |row, stats| row.child(stats_label(stats, cx)),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap_2()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(
                                            div().flex_1().min_w_0().truncate().child(
                                                change
                                                    .path
                                                    .parent()
                                                    .filter(|path| !path.as_os_str().is_empty())
                                                    .map(|path| path.display().to_string())
                                                    .unwrap_or_else(|| ".".into()),
                                            ),
                                        )
                                        .child(div().flex_none().child(caption)),
                                ),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.select_file(index, cx))),
        )
    }

    fn history(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .min_h_0()
            .bg(if self.view == View::Repositories {
                cx.theme().background
            } else {
                cx.theme().sidebar
            })
            .child(
                h_flex()
                    .flex_none()
                    .h(px(32.))
                    .px_3()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr("commits"))
                    .child(self.history.commits.len().to_string())
                    .when_some(self.comparison_label(), |row, label| {
                        row.child(div().flex_1()).child(
                            div()
                                .id("history-comparison")
                                .test_support()
                                .min_w_0()
                                .truncate()
                                .child(label),
                        )
                    })
                    .when(
                        self.view != View::History && self.history.compare.is_none(),
                        |row| {
                            row.child(div().flex_1())
                                .child(Icon::new(IconName::GitBranch).xsmall())
                                .child(
                                    div()
                                        .id("history-branch")
                                        .test_support()
                                        .min_w_0()
                                        .truncate()
                                        .child(self.viewed_branch_name()),
                                )
                        },
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "commits-list",
                            self.history.row_count() + usize::from(self.history.has_more),
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|index| {
                                        if index == this.history.row_count() {
                                            this.load_more_row(cx).into_any_element()
                                        } else {
                                            this.history_row(index, cx)
                                        }
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .with_horizontal_sizing_behavior(
                            ListHorizontalSizingBehavior::Unconstrained,
                        )
                        .with_width_from_item(self.history.widest_row())
                        .track_scroll(&self.history.scroll)
                        .size_full(),
                    )
                    .child(list_scrollbar(
                        "commits-list-scrollbar",
                        &self.history.scroll,
                    ))
                    .child(
                        div().absolute().inset_0().child(
                            Scrollbar::horizontal(&self.history.scroll)
                                .id("commits-list-hscrollbar")
                                .mode(ScrollbarMode::Scrolling)
                                .viewport_from_layout(),
                        ),
                    ),
            )
            .when(
                self.history.search_available || !self.history.query.is_empty(),
                |view| {
                    view.child(
                        h_flex()
                            .flex_none()
                            .p_2()
                            .gap_1()
                            .on_action(cx.listener(
                                |this, _: &component::input::Escape, window, cx| {
                                    window.focus(&this.focus, cx);
                                },
                            ))
                            .child(
                                Input::new(&self.history_search)
                                    .id("history-search-input")
                                    .aria_label(tr("search_commits_placeholder"))
                                    .small()
                                    .cleanable(true)
                                    .flex_1(),
                            )
                            .child(
                                icon_button("search-commits", IconName::Search, "search_commits")
                                    .small()
                                    .disabled(
                                        self.loading
                                            || (self.history.loading
                                                && self.history.revision.is_none()),
                                    )
                                    .on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.submit_history_search(cx)
                                        }),
                                    ),
                            ),
                    )
                },
            )
            .into_any_element()
    }

    fn load_more_row(&self, cx: &mut Context<Self>) -> Div {
        div().h(px(50.)).px_2().pb_1().child(
            Button::new("more-commits")
                .label(tr("load_more"))
                .small()
                .ghost()
                .size_full()
                .loading(self.history.loading)
                .on_click(cx.listener(|this, _, _, cx| this.load_more(cx))),
        )
    }

    fn history_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let row = self.history.row_at(index);
        let scope = self.history.scope.as_ref();
        let (subject, highlights, author, date, labels, inherited, graph) = match row {
            Row::Working => (
                tr("uncommitted_changes").to_owned(),
                Vec::new(),
                tr("worktree_branch").to_owned(),
                String::new(),
                Vec::new(),
                false,
                self.history.working_graph.clone(),
            ),
            Row::Commit(index) => {
                let commit = &self.history.commits[index];
                let mut labels: Vec<_> = commit
                    .refs
                    .iter()
                    .map(|label| (label.name.clone(), ref_color(label.kind, cx)))
                    .collect();
                if let Some(scope) = scope.filter(|scope| scope.fork.as_ref() == Some(&commit.id)) {
                    let fork = tr("fork_point").replace("{branch}", &scope.base);
                    labels.push((fork, cx.theme().muted_foreground));
                }
                (
                    commit.subject.clone(),
                    self.commit_highlights(commit, &commit.subject, cx),
                    commit.author.clone(),
                    commit.date.clone(),
                    labels,
                    // Commits the branch inherited from its base read dimmer than its own.
                    scope.is_some_and(|scope| !scope.own.contains(&commit.id)),
                    self.history.graph_rows.get(index).cloned(),
                )
            }
        };
        let lanes = self.history.graph_lanes();
        let palette = [
            cx.theme().blue,
            cx.theme().green,
            cx.theme().magenta,
            cx.theme().yellow,
            cx.theme().cyan,
            cx.theme().red,
        ];
        h_flex()
            .h(px(50.))
            .pr_2()
            .when(lanes == 0, |element| element.pl_2())
            .when(lanes > 0, |element| {
                element.child(graph_canvas(
                    graph,
                    lanes,
                    palette,
                    row == Row::Working,
                    cx.theme().sidebar,
                ))
            })
            .child(
                div().flex_1().min_w_0().h_full().pb_1().child(
                    Button::new(("commit", index))
                        .custom(
                            ButtonCustomVariant::new(cx)
                                .hover(cx.theme().list_hover)
                                .active(cx.theme().secondary_active),
                        )
                        .w_full()
                        .h_full()
                        .justify_start()
                        .selected(
                            self.history.selected == Some(row) || self.history.compare == Some(row),
                        )
                        .tooltip(format!("{subject}\n{}", tr("compare_hint")))
                        .accessibility_label(format!("{subject}, {author}, {date}"))
                        .child(
                            v_flex()
                                .w_full()
                                .min_w_0()
                                .items_start()
                                .gap_1()
                                .child(
                                    h_flex()
                                        .w_full()
                                        .min_w_0()
                                        .gap_1()
                                        .children(
                                            labels
                                                .into_iter()
                                                .map(|(name, color)| ref_chip(name, color)),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_sm()
                                                .whitespace_nowrap()
                                                .when(row == Row::Working, |text| text.italic())
                                                .when(inherited, |text| {
                                                    text.text_color(cx.theme().muted_foreground)
                                                })
                                                .child(
                                                    StyledText::new(subject)
                                                        .with_highlights(highlights),
                                                ),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .text_xs()
                                        .whitespace_nowrap()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(author)
                                        .child(date),
                                ),
                        )
                        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                            let modifiers = event.modifiers();
                            this.select_row(row, modifiers.control || modifiers.platform, cx)
                        })),
                ),
            )
            .id(("history-row", index))
            .context_menu({
                let app = cx.entity().downgrade();
                move |menu, _, cx| match app.upgrade() {
                    Some(app) => app.update(cx, |this, cx| this.commit_menu(row, menu, cx)),
                    None => menu,
                }
            })
            .into_any_element()
    }

    /// Names both sides while two rows are compared.
    fn comparison_label(&self) -> Option<String> {
        let Comparison::Range { base, target } = self.history.comparison()? else {
            return None;
        };
        let short = |id: &str| {
            self.history
                .commits
                .iter()
                .find(|commit| commit.id == id)
                .map_or_else(|| id.to_owned(), |commit| commit.short_id.clone())
        };
        Some(format!(
            "{} → {}",
            short(&base),
            target.map_or_else(|| tr("worktree_branch").to_owned(), |id| short(&id))
        ))
    }

    fn commit_highlights(
        &self,
        commit: &Commit,
        text: &str,
        cx: &App,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let query = self.history.query.to_lowercase();
        if query.is_empty() || commit.id.starts_with(&query) {
            return Vec::new();
        }
        search_match_ranges(text, &query)
            .into_iter()
            .map(|range| {
                (
                    range,
                    HighlightStyle {
                        background_color: Some(cx.theme().yellow.opacity(0.3)),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    fn detail(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let content = if (self.loading || self.active_files().files_loading || self.history.loading)
            && !self.active_files().loaded
        {
            self.message("loading", None, cx)
        } else if self.view == View::Repositories
            && !self.repository_preview
            && self.history.selected.is_some()
        {
            self.files(cx)
        } else if self.active_files().patch_loading
            && self.active_files().diff.read(cx).files().is_empty()
            && self.active_files().image.is_none()
        {
            self.message("loading_diff", None, cx)
        } else if self.active_files().empty_patch {
            self.message("no_patch", Some("no_patch_body"), cx)
        } else if self.visible_error().is_some()
            && self.active_files().diff.read(cx).files().is_empty()
            && self.active_files().image.is_none()
        {
            self.message("load_failed", None, cx)
        } else if let Some(image) = &self.active_files().image {
            image.clone().into_any_element()
        } else if self.active_files().selected_file.is_some() {
            let app = cx.entity().downgrade();
            let hover_app = app.clone();
            let click_app = app.clone();
            div()
                .id("diff-lines")
                .size_full()
                .child(
                    Diff::new(&self.active_files().diff)
                        .when_some(
                            cx.theme().highlight_theme.style.editor_foreground,
                            |diff, color| diff.text_color(color),
                        )
                        .change_indicator(DiffChangeIndicator::Bars)
                        .header_visible(false)
                        .soft_wrap(self.preferences.soft_wrap)
                        .hunk_separator(DiffHunkSeparator::Wave)
                        .on_line_hover(move |position, _, cx| {
                            let _ = hover_app.update(cx, |this, _| {
                                this.hovered_line = position.cloned();
                            });
                        })
                        // Cmd/Ctrl-click opens the line in Zed, like an editor's go-to.
                        .on_line_click(move |position, event, _, cx| {
                            let modifiers = event.modifiers();
                            if modifiers.platform || modifiers.control {
                                let _ = click_app.update(cx, |this, cx| {
                                    let target = this.line_target(Some(position));
                                    if target.is_some() {
                                        this.open_in_zed(this.shown_commit(), target, cx);
                                    }
                                });
                            }
                        })
                        .size_full(),
                )
                .context_menu(move |menu, _, cx| {
                    let Some(entity) = app.upgrade() else {
                        return menu;
                    };
                    let this = entity.read(cx);
                    let line = this.line_target(this.hovered_line.as_ref());
                    let file = this.line_target(None);
                    let open = |target: Option<(PathBuf, usize)>| {
                        let app = app.clone();
                        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            let _ = app.update(cx, |this, cx| {
                                this.open_in_zed(this.shown_commit(), target.clone(), cx)
                            });
                        }
                    };
                    menu.when_some(
                        line.clone().filter(|_| this.hovered_line.is_some()),
                        |menu, target| {
                            let label =
                                tr("open_line_in_zed").replace("{line}", &target.1.to_string());
                            menu.item(PopupMenuItem::new(label).on_click(open(line.clone())))
                        },
                    )
                    .item(PopupMenuItem::new(tr("open_file_in_zed")).on_click(open(file)))
                })
                .into_any_element()
        } else if self.history.row_count() == 0 {
            if self.history.query.is_empty() {
                self.message("no_commits_title", Some("no_commits_body"), cx)
            } else {
                self.message("no_matching_commits", Some("no_matching_commits_body"), cx)
            }
        } else {
            self.message("no_files", None, cx)
        };
        v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .when(
                self.view != View::Repositories || self.repository_preview,
                |view| view.child(self.diff_header(window, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(content),
            )
            .when_some(
                self.current_commit()
                    .filter(|_| self.preferences.show_commit_message),
                |view, commit| {
                    view.child(
                        v_flex()
                            .id("commit-message")
                            .test_support()
                            .flex_none()
                            .max_h(px(200.))
                            .overflow_y_scroll()
                            .px_3()
                            .py_2()
                            .gap_1()
                            .border_t(px(1. / window.scale_factor()))
                            .border_color(cx.theme().border)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
                            )
                            .child(
                                div().text_sm().font_weight(FontWeight::SEMIBOLD).child(
                                    SelectableText::new(
                                        SharedString::from(format!("commit-subject-{}", commit.id)),
                                        commit.subject.clone(),
                                    )
                                    .document_order(0),
                                ),
                            )
                            .when(!commit.body.is_empty(), |view| {
                                view.child(
                                    div().text_sm().child(
                                        SelectableText::new(
                                            SharedString::from(format!(
                                                "commit-body-{}",
                                                commit.id
                                            )),
                                            commit.body.clone(),
                                        )
                                        .document_order(1),
                                    ),
                                )
                            })
                            .child(
                                h_flex()
                                    .flex_wrap()
                                    .gap_x_3()
                                    .gap_y_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.commit_id_button(commit, cx))
                                    .child(commit.author.clone())
                                    .child(commit.date.clone())
                                    .children(commit.refs.iter().map(|label| {
                                        ref_chip(label.name.clone(), ref_color(label.kind, cx))
                                    }))
                                    .when(commit.parents.len() > 1, |row| {
                                        row.child(tr("first_parent"))
                                    }),
                            ),
                    )
                },
            )
            .into_any_element()
    }

    fn message(&self, title: &'static str, body: Option<&'static str>, cx: &App) -> AnyElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child(tr(title)),
            )
            .when_some(body, |view, body| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(tr(body)),
                )
            })
            .into_any_element()
    }
}

impl Render for Patchlook {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.repository.is_none()
            && self.preferences.recent_repositories.is_empty()
            && self.view != View::Repositories
        {
            if self.loading {
                self.message("loading", None, cx)
            } else {
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .child(
                        Icon::new(IconName::GitBranch)
                            .size(px(36.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        v_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(tr("welcome_title")),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(tr("welcome_body")),
                            ),
                    )
                    .child(
                        Button::new("welcome-open")
                            .primary()
                            .label(tr("open_repository"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_repository(&OpenRepository, window, cx)
                            })),
                    )
                    .into_any_element()
            }
        } else {
            let sidebar = if self.view == View::Repositories {
                let ratio = self.preferences.repository_sidebar_ratio.unwrap_or(0.5);
                v_resizable("repositories-sidebar")
                    .on_resize(cx.listener(
                        |this, state: &Entity<component::resizable::ResizableState>, _, cx| {
                            let sizes = state.read(cx).sizes();
                            if sizes.len() == 2 && sizes[0] + sizes[1] > px(0.) {
                                this.preferences.repository_sidebar_ratio =
                                    Some(sizes[0] / (sizes[0] + sizes[1]));
                                this.save_preferences();
                            }
                        },
                    ))
                    .child(
                        resizable_panel()
                            .size(px(0.))
                            .flex_grow(ratio)
                            .child(self.repositories(cx)),
                    )
                    .child(
                        resizable_panel()
                            .size(px(0.))
                            .flex_grow(1. - ratio)
                            .child(self.branch_list(cx)),
                    )
                    .into_any_element()
            } else {
                v_resizable("history-sidebar")
                    .child(resizable_panel().size(px(350.)).child(self.history(cx)))
                    .child(resizable_panel().overflow_hidden().child(self.files(cx)))
                    .into_any_element()
            };
            let detail = if self.view == View::Repositories {
                h_resizable("repository-history")
                    .child(
                        resizable_panel()
                            .size(px(560.))
                            .size_range(px(210.)..px(1200.))
                            .child(self.history(cx)),
                    )
                    .child(
                        resizable_panel()
                            .overflow_hidden()
                            .child(self.detail(window, cx)),
                    )
                    .into_any_element()
            } else {
                self.detail(window, cx)
            };
            h_resizable("workspace")
                .child(
                    resizable_panel()
                        .size(px(300.))
                        .size_range(px(230.)..px(500.))
                        .fixed()
                        .child(
                            v_flex()
                                .size_full()
                                .min_h_0()
                                .bg(cx.theme().sidebar)
                                .child(self.sidebar_tabs(cx))
                                .child(div().flex_1().min_h_0().child(sidebar)),
                        ),
                )
                .child(resizable_panel().overflow_hidden().child(detail))
                .into_any_element()
        };
        v_flex()
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .font_family(cx.theme().font_family.clone())
            .text_size(cx.theme().font_size)
            .key_context("Patchlook")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::open_repository))
            .on_action(cx.listener(Self::refresh))
            .on_action(cx.listener(Self::show_about))
            .child(self.toolbar(cx))
            .when_some(self.visible_error().cloned(), |view, error| {
                view.child(
                    h_flex()
                        .flex_none()
                        .gap_3()
                        .px_4()
                        .py_2()
                        .bg(cx.theme().danger.opacity(0.12))
                        .child(div().flex_1().text_sm().child(error))
                        .child(
                            Button::new("dismiss-error")
                                .icon(IconName::X)
                                .small()
                                .ghost()
                                .tooltip(tr("dismiss"))
                                .accessibility_label(tr("dismiss"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.error = None;
                                    this.active_files_mut().error = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(body),
            )
            .child(self.about.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use gpui_kit::component::scroll::ScrollbarHandle;
    use gpui_kit::test::{TestQueryExt, TestWindowExt};
    use patchlook::git::Area;

    #[test]
    fn failed_initial_preview_does_not_discard_the_file_list() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(directory.path())
                .args(["init", "-b", "main"])
                .output()
                .unwrap()
                .status
                .success()
        );
        std::fs::write(directory.path().join("broken.png"), "not an image").unwrap();
        let repository = Repository::open(directory.path()).unwrap();
        let prepared = PreparedChanges::load(
            &repository,
            vec![Change {
                path: "broken.png".into(),
                previous_path: None,
                status: 'A',
                area: Area::Untracked,
            }],
            &Comparison::Working,
            true,
        )
        .unwrap();
        assert_eq!(prepared.changes.len(), 1);
        assert!(prepared.preview.unwrap().is_err());
    }

    fn repository_sidebar_ratio(window: &mut Window) -> f32 {
        let top = window.find("repositories").bounds().bottom();
        let divider = window
            .within("repositories-sidebar")
            .find(("resizable-handle", 0usize))
            .bounds()
            .center()
            .y;
        (divider - top) / (window.viewport_size().height - top)
    }

    #[test]
    fn search_highlights_preserve_case_insensitive_utf8_ranges() {
        for (text, query, expected) in [
            ("Fix FIX fixes", "fix", vec!["Fix", "FIX", "fix"]),
            ("更新中文，中文说明", "中文", vec!["中文", "中文"]),
            ("ÉCOLE école", "école", vec!["ÉCOLE", "école"]),
            ("İstanbul", "i", vec!["İ"]),
            ("ΟΣ ος", "ος", vec!["ΟΣ", "ος"]),
            ("literal [.] match", "[.]", vec!["[.]"]),
            ("no matches", " ", vec![]),
        ] {
            let ranges = search_match_ranges(text, query);
            assert_eq!(
                ranges
                    .iter()
                    .map(|range| &text[range.clone()])
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                ranges.is_empty(),
                query.trim().is_empty()
                    || !text.to_lowercase().contains(&query.trim().to_lowercase())
            );
        }
    }

    fn test_app(window: &mut Window, cx: &mut Context<Patchlook>) -> Patchlook {
        Patchlook {
            history: HistoryState::new(Layout::Split, cx),
            preferences: Preferences::default(),
            about: cx.new(crate::about::AboutPanel::new),
            external_files: Vec::new(),
            preferences_directory: tempfile::tempdir().unwrap(),
            repository: None,
            branch: "main".into(),
            branches: vec![],
            worktrees: vec![],
            repository_scroll: UniformListScrollHandle::new(),
            branch_scroll: UniformListScrollHandle::new(),
            toggled_branch_folders: Default::default(),
            branch_tab: BranchTab::Branches,
            hovered_line: None,
            view: View::History,
            repository_preview: false,

            history_search: cx.new(|cx| InputState::new(window, cx)),
            branch_search: cx.new(|cx| InputState::new(window, cx)),
            remote_input: None,

            commit_copied: false,
            copy_feedback_task: None,
            loading: false,

            error: None,
            load_task: None,
            _subscriptions: vec![],
            focus: cx.focus_handle(),
        }
    }

    #[gpui_kit::test]
    fn isolated_file_selections_load_immediately_but_bursts_wait_for_the_last_file(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let names = ["go.mod", "go.sum", "main.go"];
        for name in names {
            std::fs::write(directory.path().join(name), format!("{name}\n")).unwrap();
        }
        let (_, app) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut app = test_app(window, cx);
                    app.repository = Some(Repository {
                        root: directory.path().into(),
                        main: directory.path().into(),
                    });
                    app.history.files.loaded = true;
                    app.history.files.changes = names
                        .into_iter()
                        .map(|path| Change {
                            path: path.into(),
                            previous_path: None,
                            status: 'A',
                            area: Area::Untracked,
                        })
                        .collect();
                    app
                })
            })
            .unwrap()
        });
        let select = |index, cx: &mut TestAppContext| {
            cx.update(|cx| app.update(cx, |app, cx| app.select_file(index, cx)));
            cx.run_until_parked();
        };
        let verify = |path: &str, loading: bool, cx: &mut TestAppContext| {
            cx.update(|cx| {
                let files = &app.read(cx).history.files;
                assert_eq!(files.patch_loading, loading);
                assert_eq!(files.diff.read(cx).files()[0].path().as_str(), path);
            });
        };
        // No clock advancement: the first file is available as soon as I/O finishes.
        select(0, cx);
        verify("go.mod", false, cx);
        select(1, cx);
        verify("go.mod", true, cx);
        cx.executor().advance_clock(FILE_SELECTION_DELAY / 2);
        select(2, cx);
        cx.executor()
            .advance_clock(FILE_SELECTION_DELAY - Duration::from_millis(1));
        cx.run_until_parked();
        verify("go.mod", true, cx);
        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        verify("main.go", false, cx);
        // Once the burst settles, another uncached file needs no extra timer.
        select(1, cx);
        verify("go.sum", false, cx);
    }

    #[gpui_kit::test]
    fn titlebar_buttons_exclude_background_hitboxes_and_still_handle_clicks(
        cx: &mut TestAppContext,
    ) {
        use std::{cell::RefCell, rc::Rc};

        struct ToolbarProbe {
            app: Entity<Patchlook>,
            background: Rc<RefCell<Option<Hitbox>>>,
        }
        impl Render for ToolbarProbe {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let background = self.background.clone();
                let toolbar = self
                    .app
                    .update(cx, |app, cx| app.toolbar(cx).into_any_element());
                div()
                    .relative()
                    .size_full()
                    .child(
                        canvas(
                            move |bounds, window, _| {
                                *background.borrow_mut() =
                                    Some(window.insert_hitbox(bounds, HitboxBehavior::Normal));
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(toolbar)
            }
        }

        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        git(directory.path(), &["init", "-b", "main"]);
        std::fs::write(directory.path().join("refresh.txt"), "Refresh me\n").unwrap();
        let repository = Repository::open(directory.path()).unwrap();
        let background = Rc::new(RefCell::new(None));
        let (handle, probe) = cx.update(|cx| {
            gpui_kit::open_window(TitleBar::window_options(), cx, |window, cx| {
                let app = cx.new(|cx| {
                    let mut app = test_app(window, cx);
                    app.repository = Some(repository);
                    app
                });
                cx.new(|_| ToolbarProbe {
                    app,
                    background: background.clone(),
                })
            })
            .unwrap()
        });
        let app = cx.update(|cx| probe.read(cx).app.clone());
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            let mut controls = vec!["open-repository", "refresh", "language", "theme"];
            if cfg!(any(target_os = "windows", target_os = "linux")) {
                controls.push("application-menu");
            } else {
                assert!(window.try_find("application-menu").is_none());
            }
            for id in controls {
                let bounds = window.find(id).bounds();
                for position in [bounds.center(), bounds.origin + point(px(1.), px(1.))] {
                    window.simulate_mouse_move(position, cx);
                    window.render_frame(cx);
                    // Windows checks the full hit-test chain, not only hovered items.
                    // This API tests membership in that same chain, so scroll-only
                    // blocking would not incorrectly pass as a native drag exclusion.
                    assert!(
                        !background
                            .borrow()
                            .as_ref()
                            .unwrap()
                            .should_handle_scroll(window),
                        "{id} still exposes its titlebar background to native hit testing"
                    );
                }
            }
            let refresh = window.find("refresh").bounds();
            let language = window.find("language").bounds();
            let blank = point((refresh.right() + language.left()) / 2., refresh.center().y);
            window.simulate_mouse_move(blank, cx);
            window.render_frame(cx);
            assert!(
                background
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .should_handle_scroll(window)
            );

            let next = app.read(cx).preferences.appearance.next();
            window.click("theme", cx);
            assert_eq!(app.read(cx).preferences.appearance, next);
            window.render_frame(cx);
            window.click("language", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("popup-menu").is_some());
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("open-repository", cx);
        })
        .unwrap();
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|_| None);
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.click("refresh", cx);
            assert!(app.read(cx).loading);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let app = app.read(cx);
            assert!(!app.loading);
            assert!(
                app.history
                    .files
                    .changes
                    .iter()
                    .any(|change| change.path == std::path::Path::new("refresh.txt"))
            );
        });
    }

    #[gpui_kit::test]
    fn application_menu_opens_about_and_dispatches_quit(cx: &mut TestAppContext) {
        use std::{cell::Cell, rc::Rc};

        struct MenuProbe(Entity<Patchlook>);
        impl Render for MenuProbe {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    // The extra test trigger sits outside Patchlook on macOS;
                    // give it the same ancestor action route as the real toolbar.
                    .on_action(cx.listener(|this, action: &About, window, cx| {
                        this.0
                            .update(cx, |app, cx| app.show_about(action, window, cx));
                    }))
                    .child(self.0.clone())
                    .when(
                        !cfg!(any(target_os = "windows", target_os = "linux")),
                        |view| {
                            // Exercise the same trigger on macOS test hosts without
                            // adding it to the macOS application's real toolbar.
                            view.child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .child(self.0.read(cx).application_menu(cx)),
                            )
                        },
                    )
            }
        }
        let quits = Rc::new(Cell::new(0));
        let on_quit = quits.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            cx.on_action(move |_: &Quit, _| on_quit.set(on_quit.get() + 1));
        });
        let (handle, probe) = cx.update(|cx| {
            gpui_kit::open_window(TitleBar::window_options(), cx, |window, cx| {
                let app = cx.new(|cx| test_app(window, cx));
                window.focus(&app.read(cx).focus.clone(), cx);
                cx.new(|_| MenuProbe(app))
            })
            .unwrap()
        });
        let focus = cx.update(|cx| probe.read(cx).0.read(cx).focus.clone());
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("application-menu", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let menu = window.within("popup-menu");
            assert_eq!(menu.find(0usize).label(), Some(tr("about")));
            assert_eq!(menu.find(2usize).label(), Some(tr("quit")));
            assert!(menu.try_find(3usize).is_none());
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("popup-menu").is_none());
            assert_eq!(window.focused(cx), Some(focus));
            // The trigger is keyboard reachable; Enter opens its dropdown.
            for _ in 0..16 {
                window.press("tab", cx);
                if window.find("application-menu").focused() == Some(true) {
                    break;
                }
            }
            assert_eq!(window.find("application-menu").focused(), Some(true));
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("popup-menu").is_some());
            window.within("popup-menu").click(0usize, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("about-content").visible());
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.click("application-menu", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("up", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(quits.get(), 1);
    }

    #[gpui_kit::test]
    fn about_links_and_license_dialog_work_without_a_repository(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (handle, app) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| test_app(window, cx))
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            window.focus(&app.read(cx).focus.clone(), cx);
            window.render_frame(cx);
            window.dispatch_action(Box::new(About), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            assert!(window.find("about-content").visible());
            window.click("about-github", cx);
        })
        .unwrap();
        assert_eq!(
            cx.opened_url().as_deref(),
            Some(env!("CARGO_PKG_REPOSITORY"))
        );
        cx.update_window(handle, |_, window, cx| window.click("about-issues", cx))
            .unwrap();
        assert_eq!(
            cx.opened_url().as_deref(),
            Some(concat!(env!("CARGO_PKG_REPOSITORY"), "/issues"))
        );
        cx.update_window(handle, |_, window, cx| window.click("about-license", cx))
            .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            assert!(window.find("app-license-text").visible());
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("about-surface").visible());
            assert!(window.try_find("app-license-text").is_none());
            assert!(window.find("about-content").visible());
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("about-surface").is_none());
            assert_eq!(window.focused(cx), Some(app.read(cx).focus.clone()));
            window.render_frame(cx);
            window.dispatch_action(Box::new(About), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("about-surface").visible());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn about_is_centered_and_keeps_focus_until_each_dismissal_finishes(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, app) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| test_app(window, cx))
            })
            .unwrap()
        });
        let (enter, exit) = cx.update(|cx| {
            let motion = cx.theme().motion_tokens();
            (motion.duration_normal, motion.duration_fast)
        });
        cx.update_window(handle, |_, window, cx| {
            window.focus(&app.read(cx).focus.clone(), cx);
            window.render_frame(cx);
            assert!(window.try_find("about-surface").is_none());
        })
        .unwrap();
        for dismissal in ["button", "escape", "backdrop"] {
            cx.update_window(handle, |_, window, cx| {
                window.dispatch_action(Box::new(About), cx);
                window.render_frame(cx);
            })
            .unwrap();
            cx.executor().advance_clock(enter);
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                let bounds = window.find("about-surface").bounds();
                let viewport = window.viewport_size();
                assert!((bounds.center().y - viewport.height / 2.).abs() < px(1.));
                match dismissal {
                    "button" => window.click("about-close", cx),
                    "escape" => window.press("escape", cx),
                    _ => {
                        window.dispatch_event(
                            MouseDownEvent {
                                position: point(px(1.), px(1.)),
                                button: MouseButton::Left,
                                click_count: 1,
                                modifiers: Modifiers::default(),
                                first_mouse: false,
                            }
                            .to_platform_input(),
                            cx,
                        );
                    }
                }
                window.render_frame(cx);
                assert!(window.find("about-surface").visible());
                assert_ne!(window.focused(cx), Some(app.read(cx).focus.clone()));
            })
            .unwrap();
            cx.executor().advance_clock(exit / 2);
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                assert!(window.find("about-surface").visible());
            })
            .unwrap();
            cx.executor().advance_clock(exit);
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                assert!(window.try_find("about-surface").is_none());
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(handle, |_, window, cx| {
                assert_eq!(window.focused(cx), Some(app.read(cx).focus.clone()));
            })
            .unwrap();
        }
    }

    fn git(directory: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(directory)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.test",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[gpui_kit::test]
    fn open_with_menu_offers_program_selection_and_waits_for_the_visible_file(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        git(directory.path(), &["init", "-b", "main"]);
        std::fs::write(directory.path().join("file.txt"), "Working file").unwrap();
        let (handle, app) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut app = test_app(window, cx);
                    app.load(directory.path().to_owned(), cx);
                    app
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            assert_eq!(
                app.read(cx).external_file_target().unwrap().change.path,
                PathBuf::from("file.txt")
            );
            window.click("open-with", cx);
            window.render_frame(cx);
            window.render_frame(cx);
            assert!(
                window
                    .find_by_label(tr("system_default_application"))
                    .visible()
            );
            // Recommended apps vary by machine; Up wraps to the final "Other" item.
            window.press("up", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|options| {
            assert!(options.files && !options.multiple);
            None
        });
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            app.update(cx, |app, cx| {
                assert!(app.external_files.is_empty());
                app.active_files_mut().patch_loading = true;
                assert!(app.external_file_target().is_none());
                cx.notify();
            });
            window.render_frame(cx);
            window.click("open-with", cx);
            assert!(
                window
                    .try_find_by_label(tr("system_default_application"))
                    .is_none()
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn removing_repositories_persists_the_list_and_clears_their_content(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let repositories = ["alpha", "beta", "gamma"].map(|name| {
            let path = directory.path().join(name);
            std::fs::create_dir(&path).unwrap();
            git(&path, &["init", "-b", "main"]);
            std::fs::write(path.join("file.txt"), name).unwrap();
            git(&path, &["add", "."]);
            git(&path, &["commit", "-qm", "Initial"]);
            std::fs::write(path.join("file.txt"), "Working change").unwrap();
            Repository::open(&path).unwrap()
        });
        let (handle, app) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut app = test_app(window, cx);
                    for repo in &repositories {
                        app.preferences.remember_repository(repo.root.clone());
                    }
                    app.load(repositories[1].root.clone(), cx);
                    app
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            app.update(cx, |app, cx| {
                assert!(!app.history.files.changes.is_empty());
                app.switch_view(View::Repositories, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            app.update(cx, |app, cx| {
                assert!(!app.history.commits.is_empty());
                app.history.query = "old search".into();
                app.history_search
                    .update(cx, |input, cx| input.set_value("old search", window, cx));
            });
        })
        .unwrap();

        // Remove a middle entry, then the last entry, then the only remaining entry.
        for (removed, next, remaining) in [(1, Some(2), 2), (2, Some(0), 1), (0, None, 0)] {
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                window.render_frame(cx);
                assert!(window.try_find("about").is_none());
                assert!(
                    window.find("add-repository").bounds().right()
                        <= window.find("remove-repository").bounds().left()
                );
                assert_eq!(
                    app.read(cx).repository.as_ref().unwrap().root,
                    repositories[removed].root
                );
                window.click("remove-repository", cx);
                window.render_frame(cx);
                let state = app.read(cx);
                assert!(state.repository.is_none());
                assert!(state.history.files.changes.is_empty());
                assert!(state.history.commits.is_empty());
                assert!(state.history.query.is_empty());
                assert!(state.history_search.read(cx).value().is_empty());
                assert!(state.branches.is_empty());
                assert_eq!(state.preferences.recent_repositories.len(), remaining);
                // Disabled both while the replacement loads and when nothing is selected.
                window.click("remove-repository", cx);
                assert_eq!(
                    app.read(cx).preferences.recent_repositories.len(),
                    remaining
                );
            })
            .unwrap();
            cx.run_until_parked();
            cx.update(|cx| {
                let state = app.read(cx);
                let next_path = next.map(|index| repositories[index].root.clone());
                assert_eq!(
                    state.repository.as_ref().map(|repo| repo.root.clone()),
                    next_path
                );
                assert_eq!(state.preferences.repository, next_path);
                assert_eq!(state.history.commits.len(), usize::from(next.is_some()));
                assert!(state.visible_error().is_none());
                let saved: Preferences = serde_json::from_slice(
                    &std::fs::read(state.preferences_directory.path().join("preferences.json"))
                        .unwrap(),
                )
                .unwrap();
                assert_eq!(saved.repository, next_path);
                assert_eq!(saved.recent_repositories.len(), remaining);
                assert!(
                    !saved
                        .recent_repositories
                        .iter()
                        .any(|entry| entry.path == repositories[removed].root)
                );
            });
            assert!(repositories[removed].root.join(".git").is_dir());
            assert_eq!(
                std::fs::read_to_string(repositories[removed].root.join("file.txt")).unwrap(),
                "Working change"
            );
        }
    }

    #[gpui_kit::test]
    fn history_lists_the_working_tree_and_compares_any_two_rows(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-b", "main"]);
        for content in ["one\n", "two\n", "three\n"] {
            std::fs::write(root.join("file.txt"), content).unwrap();
            git(root, &["add", "."]);
            git(root, &["commit", "-qm", content.trim()]);
        }
        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        let repo = Repository::open(root).unwrap();
        let (handle, app) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut app = test_app(window, cx);
                    app.view = View::History;
                    app.repository = Some(repo.clone());
                    app.ensure_view_loaded(cx);
                    app
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        let paths = |cx: &mut TestAppContext| {
            app.read_with(cx, |app, _| {
                let files = &app.history.files.changes;
                files
                    .iter()
                    .map(|change| change.path.clone())
                    .collect::<Vec<_>>()
            })
        };
        app.read_with(cx, |app, _| {
            assert!(app.history.working_changes);
            assert_eq!(app.history.row_count(), 4);
            assert_eq!(app.history.selected, Some(Row::Working));
        });
        assert_eq!(paths(cx), [PathBuf::from("new.txt")]);

        // Picking the newer row first still diffs from the older commit.
        app.update(cx, |app, cx| {
            app.select_row(Row::Commit(0), false, cx);
            app.select_row(Row::Commit(2), true, cx);
        });
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let app = app.read(cx);
            let commits = &app.history.commits;
            assert_eq!(
                app.history.comparison(),
                Some(Comparison::Range {
                    base: commits[2].id.clone(),
                    target: Some(commits[0].id.clone()),
                })
            );
            let label = format!("{} → {}", commits[2].short_id, commits[0].short_id);
            assert!(window.try_find("history-comparison").is_some());
            assert_eq!(app.comparison_label(), Some(label));
            assert!(app.current_commit().is_none());
        })
        .unwrap();
        assert_eq!(paths(cx), [PathBuf::from("file.txt")]);

        app.update(cx, |app, cx| {
            app.select_row(Row::Commit(2), false, cx);
            app.select_row(Row::Working, true, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            paths(cx),
            [PathBuf::from("file.txt"), PathBuf::from("new.txt")]
        );
        app.update(cx, |app, cx| app.select_row(Row::Commit(1), false, cx));
        cx.run_until_parked();
        app.read_with(cx, |app, _| assert_eq!(app.history.compare, None));
    }

    #[gpui_kit::test]
    fn all_branches_open_on_head_with_a_folder_tree_of_branches(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-b", "main"]);
        git(root, &["commit", "-qm", "Base", "--allow-empty"]);
        for branch in ["longc/one", "longc/two"] {
            git(root, &["switch", "-qc", branch, "main"]);
            git(root, &["commit", "-qm", branch, "--allow-empty"]);
        }
        git(root, &["switch", "-q", "main"]);
        let (handle, app) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| test_app(window, cx))
            })
            .unwrap()
        });
        let names = |app: &Patchlook, cx: &App| {
            app.branch_items(cx)
                .into_iter()
                .map(|item| match item {
                    BranchItem::All => "*".to_owned(),
                    BranchItem::Head => "HEAD".to_owned(),
                    BranchItem::Folder { name, depth, .. } => format!("{depth}{name}/"),
                    BranchItem::Branch { name, depth, .. } => format!("{depth}{name}"),
                })
                .collect::<Vec<_>>()
        };
        app.update(cx, |app, cx| app.load(root.canonicalize().unwrap(), cx));
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            app.update(cx, |app, cx| {
                // Newer branch commits come first, but the checked-out HEAD is selected.
                assert_eq!(app.history.commits.len(), 3);
                let Some(Row::Commit(head)) = app.history.selected else {
                    panic!("HEAD is not selected");
                };
                assert_eq!(app.history.commits[head].subject, "Base");
                assert_eq!(
                    names(app, cx),
                    ["*", "HEAD", "0Local/", "1longc/", "2one", "2two", "1main"]
                );
                app.toggled_branch_folders.insert("local/longc".into());
                assert_eq!(names(app, cx), ["*", "HEAD", "0Local/", "1longc/", "1main"]);
                // A filter lists matches flat by full name, even inside collapsed folders.
                app.branch_search
                    .update(cx, |input, cx| input.set_value("TWO", window, cx));
                assert_eq!(names(app, cx), ["0longc/two"]);
                app.branch_search
                    .update(cx, |input, cx| input.set_value("", window, cx));
                app.browse_branch(Some(HEAD_BRANCH.into()), cx);
            })
        })
        .unwrap();
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert_eq!(app.history.commits.len(), 1);
            assert_eq!(app.viewed_branch_name(), "main");
        });
    }

    #[gpui_kit::test]
    fn worktrees_share_one_recent_entry_and_reopen_where_left(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("project");
        let linked = directory.path().join("project-feature");
        std::fs::create_dir(&main).unwrap();
        git(&main, &["init", "-b", "main"]);
        std::fs::write(main.join("file.txt"), "one\n").unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "-qm", "Initial"]);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                linked.to_str().unwrap(),
            ],
        );
        let (main, linked) = (main.canonicalize().unwrap(), linked.canonicalize().unwrap());
        let (handle, app) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut app = test_app(window, cx);
                    // Entries saved for a worktree before worktree support fold into one.
                    app.preferences
                        .remember_repository(linked.canonicalize().unwrap());
                    app
                })
            })
            .unwrap()
        });
        let open = |path: &std::path::Path, switch: bool, cx: &mut TestAppContext| {
            app.update(cx, |app, cx| {
                if switch {
                    app.switch_worktree(path.to_owned(), cx)
                } else {
                    app.load(path.to_owned(), cx)
                }
            });
            cx.run_until_parked();
        };
        let state = |cx: &mut TestAppContext| {
            app.read_with(cx, |app, _| {
                let recent: Vec<_> = app
                    .preferences
                    .recent_repositories
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect();
                let remembered = app.preferences.worktree(&main).map(Path::to_path_buf);
                (
                    app.repository.as_ref().unwrap().root.clone(),
                    recent,
                    remembered,
                )
            })
        };

        open(&linked, false, cx);
        assert_eq!(
            state(cx),
            (linked.clone(), vec![main.clone()], Some(linked.clone()))
        );
        app.read_with(cx, |app, _| assert_eq!(app.worktrees.len(), 2));
        // Worktrees of other repositories opened lately head the Worktrees tab.
        let elsewhere = PathBuf::from("/elsewhere/other-feature");
        let rows = |cx: &mut TestAppContext| {
            app.update(cx, |app, cx| {
                app.worktree_items(cx)
                    .into_iter()
                    .map(|item| match item {
                        WorktreeItem::Section(key) => key.to_owned(),
                        WorktreeItem::Recent(path) => path.display().to_string(),
                        WorktreeItem::Current(index) => worktree_label(&app.worktrees[index]),
                    })
                    .collect::<Vec<_>>()
            })
        };
        app.update(cx, |app, _| {
            app.preferences.remember_worktree(elsewhere.clone())
        });
        let recent = [
            "recent_worktrees".to_owned(),
            elsewhere.display().to_string(),
        ];
        let current = ["project · main", "project-feature · feature"].map(str::to_owned);
        let expected = [&recent[..], &["repository_worktrees".to_owned()], &current].concat();
        assert_eq!(rows(cx), expected);
        cx.update_window(handle, |_, window, cx| {
            app.update(cx, |app, cx| {
                app.branch_search
                    .update(cx, |input, cx| input.set_value("feature", window, cx))
            })
        })
        .unwrap();
        let expected = [
            &recent[..],
            &["repository_worktrees".to_owned()],
            &current[1..],
        ]
        .concat();
        assert_eq!(rows(cx), expected);
        open(&main, true, cx);
        assert_eq!(state(cx), (main.clone(), vec![main.clone()], None));
        open(&linked, true, cx);
        // Reopening the Recent entry returns to the worktree viewed last.
        app.update(cx, |app, _| app.repository = None);
        open(&main, false, cx);
        assert_eq!(
            state(cx),
            (linked.clone(), vec![main.clone()], Some(linked))
        );
    }

    #[gpui_kit::test]
    fn tabs_retain_snapshots_without_rereading_git(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-b", "main"]);
        std::fs::write(root.join("alpha.rs"), "ALPHA\n").unwrap();
        std::fs::write(root.join("beta.rs"), "BETA\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "Initial"]);
        git(root, &["branch", "older"]);
        std::fs::write(root.join("beta.rs"), "HISTORY\n").unwrap();
        git(root, &["commit", "-qam", "Second"]);
        std::fs::write(root.join("alpha.rs"), "WORKING\n").unwrap();
        let repo = Repository::open(root).unwrap();
        let (handle, app) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut app = test_app(window, cx);
                    app.repository = Some(repo.clone());
                    app.branches = repo.branches().unwrap();
                    app.preferences.remember_repository(repo.root.clone());
                    app
                })
            })
            .unwrap()
        });
        // Load while Repos is visible, then reveal History before the load finishes.
        cx.update(|cx| {
            app.update(cx, |app, cx| {
                app.switch_view(View::Repositories, cx);
                app.ensure_view_loaded(cx);
                app.switch_view(View::History, cx);
                assert!(app.history.loading);
            })
        });
        cx.run_until_parked();
        cx.executor().advance_clock(FILE_SELECTION_DELAY);
        cx.run_until_parked();
        cx.update(|cx| {
            let app = app.read(cx);
            // A dirty working tree is the first history row and opens selected.
            assert_eq!(app.history.selected, Some(Row::Working));
            assert_eq!(app.history.commits.len(), 2);
            assert_eq!(app.history.files.changes[0].path, PathBuf::from("alpha.rs"));
            assert_eq!(
                app.history.files.diff.read(cx).files()[0].path().as_str(),
                "alpha.rs"
            );
            assert!(!app.history.files.patch_loading);
        });
        // No Git reads are possible now: revisiting loaded Tabs must still work.
        std::fs::rename(root.join(".git"), root.join("git-hidden")).unwrap();
        let (history_diff, revision) = cx.update(|cx| {
            let app = app.read(cx);
            (
                app.history.files.diff.entity_id(),
                app.history.revision.clone(),
            )
        });
        for target in [View::Repositories, View::History, View::Repositories] {
            cx.update_window(handle, |_, window, cx| {
                app.update(cx, |app, cx| app.switch_view(target, cx));
                window.render_frame(cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update(|cx| {
                let app = app.read(cx);
                assert!(!app.loading && !app.history.loading && !app.history.files.files_loading);
                assert!(app.visible_error().is_none());
                assert_eq!(app.history.files.diff.entity_id(), history_diff);
                assert_eq!(app.history.revision, revision);
                assert_eq!(app.history.selected, Some(Row::Working));
                assert_eq!(app.history.files.selected_file, Some(0));
            });
        }
        // Reopening the selected file in Repos reuses the preview, including its identity.
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("tree-file-0", cx);
            window.render_frame(cx);
            assert!(app.read(cx).repository_preview);
            assert!(!app.read(cx).history.files.patch_loading);
            window.click("back-to-files", cx);
            window.render_frame(cx);
            window.click("tree-file-0", cx);
            assert!(app.read(cx).repository_preview);
            assert_eq!(app.read(cx).history.files.diff.entity_id(), history_diff);
        })
        .unwrap();
        std::fs::rename(root.join("git-hidden"), root.join(".git")).unwrap();
        // Another branch keeps the working-tree row but opens on its newest commit.
        cx.update(|cx| {
            app.update(cx, |app, cx| {
                // Set the saved choice in memory so the test never writes real preferences.
                app.preferences
                    .set_viewed_branch(&repo.root, Some("refs/heads/older".into()));
                app.load_history_page(HistoryRequest::Branch(Some("refs/heads/older".into())), cx);
            })
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let app = app.read(cx);
            assert!(app.history.working_changes);
            assert_eq!(app.history.selected, Some(Row::Commit(0)));
            assert_eq!(app.history.commits.len(), 1);
            assert_eq!(app.history.commits[0].subject, "Initial");
            assert_eq!(app.history.files.changes.len(), 2);
        });
        // The title bar names the viewed branch while the worktree stays on main.
        for view in [View::Repositories, View::History] {
            cx.update_window(handle, |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.preferences.tree_file_list = true;
                    app.switch_view(view, cx);
                });
                window.render_frame(cx);
                window.render_frame(cx);
                let label = tr("viewed_branch_tooltip")
                    .replace("{branch}", "older")
                    .replace("{worktree}", "main");
                assert_eq!(window.find("toolbar-branch").label(), Some(label.as_str()));
                assert_eq!(
                    window.try_find("tree-branch").is_some(),
                    view == View::Repositories
                );
                assert_eq!(
                    window.try_find("history-branch").is_some(),
                    view == View::Repositories
                );
            })
            .unwrap();
        }
        assert_eq!(repo.branch_name().unwrap(), "main");
        // An empty search result is a loaded snapshot too, and survives all Tab changes.
        cx.update(|cx| {
            app.update(cx, |app, cx| {
                app.history.query = "not present in either commit".into();
                app.load_history_page(HistoryRequest::Search, cx);
            })
        });
        cx.run_until_parked();
        std::fs::rename(root.join(".git"), root.join("git-hidden")).unwrap();
        cx.update(|cx| {
            app.update(cx, |app, cx| {
                assert!(app.history.files.loaded);
                assert!(app.history.commits.is_empty());
                for view in [View::Repositories, View::History, View::Repositories] {
                    app.switch_view(view, cx);
                    assert!(!app.history.loading);
                }
            })
        });
        cx.run_until_parked();
        cx.update(|cx| assert!(app.read(cx).visible_error().is_none()));
    }

    #[gpui_kit::test]
    fn repository_tree_opens_the_requested_preview_without_revealing_cached_content(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-b", "main"]);
        std::fs::write(root.join("alpha.rs"), "ALPHA_CONTENT\n").unwrap();
        std::fs::write(root.join("beta.rs"), "BETA_CONTENT\n").unwrap();
        std::fs::write(
            root.join("image.png"),
            include_bytes!("../tests/fixtures/images/sample.png"),
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "Preview fixture"]);
        let repo = Repository::open(root).unwrap();
        let commit = repo.history(0, None).unwrap().commits.remove(0);

        // Both a cached text diff and a cached image must stay hidden at this boundary.
        for previous in [0, 2] {
            let changes = repo.commit_changes(&commit).unwrap();
            let stats = repo
                .file_stats(&changes, &Comparison::Commit(commit.clone()))
                .unwrap();
            let content = FileContent::load(
                &repo,
                &changes[previous],
                &Comparison::Commit(commit.clone()),
            )
            .unwrap();
            let (handle, app) = cx.update(|cx| {
                gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                    cx.new(|cx| {
                        let mut app = test_app(window, cx);
                        app.view = View::Repositories;
                        app.repository = Some(repo.clone());
                        app.history.commits = vec![commit.clone()];
                        app.history.selected = Some(Row::Commit(0));
                        app.apply_changes(
                            PreparedChanges {
                                changes,
                                stats,
                                preview: None,
                            },
                            cx,
                        );
                        app.history.files.selected_file = Some(previous);
                        app.apply_file_result(Ok(content), cx);
                        app
                    })
                })
                .unwrap()
            });
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                window.render_frame(cx);
                window.click("tree-file-1", cx);
            })
            .unwrap();
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                let app = app.read(cx);
                assert!(app.repository_preview);
                assert_eq!(app.history.files.selected_file, Some(1));
                assert!(app.history.files.patch_loading);
                assert!(app.history.files.diff.read(cx).files().is_empty());
                assert!(app.history.files.image.is_none());
                assert!(window.find_all("source").is_empty());
                assert!(window.try_find("image-fit").is_none());
            })
            .unwrap();
            // The first selection starts I/O without a debounce timer, while
            // the in-flight frame above still hides the previous preview.
            cx.run_until_parked();
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                assert_eq!(
                    app.read(cx).history.files.diff.read(cx).files()[0]
                        .path()
                        .as_str(),
                    "beta.rs"
                );
                window.click("back-to-files", cx);
                window.render_frame(cx);
                window.click("tree-file-1", cx);
                window.render_frame(cx);
                assert!(!app.read(cx).history.files.patch_loading);
                assert_eq!(
                    app.read(cx).history.files.diff.read(cx).files()[0]
                        .path()
                        .as_str(),
                    "beta.rs"
                );

                // Switching within the preview still preserves its contents while loading.
                app.update(cx, |app, cx| app.select_file(0, cx));
                assert_eq!(
                    app.read(cx).history.files.diff.read(cx).files()[0]
                        .path()
                        .as_str(),
                    "beta.rs"
                );
                window.click("back-to-files", cx);
                window.render_frame(cx);
                window.click("tree-file-0", cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                // Reentering the same pending selection must not expose the previous file either.
                assert!(app.read(cx).history.files.patch_loading);
                assert!(app.read(cx).history.files.diff.read(cx).files().is_empty());
                assert!(window.find_all("source").is_empty());
            })
            .unwrap();
            cx.executor().advance_clock(FILE_SELECTION_DELAY);
            cx.run_until_parked();
            cx.update(|cx| {
                assert_eq!(
                    app.read(cx).history.files.diff.read(cx).files()[0]
                        .path()
                        .as_str(),
                    "alpha.rs"
                );
            });
        }
    }

    #[gpui_kit::test]
    fn file_navigation_keeps_header_and_code_until_replacement_is_ready(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("next.rs"), "NEXT\n").unwrap();
        let root = directory.path().to_path_buf();
        let (handle, app) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut app = test_app(window, cx);
                    app.repository = Some(Repository {
                        main: root.clone(),
                        root,
                    });
                    app.history.files.loaded = true;
                    app.history.files.changes = ["current.rs", "next.rs"]
                        .map(|path| Change {
                            path: path.into(),
                            previous_path: None,
                            status: 'A',
                            area: Area::Untracked,
                        })
                        .to_vec();
                    app.history.files.selected_file = Some(0);
                    app.history.files.diff.update(cx, |diff, cx| {
                        diff.set_files(
                            DiffFile::parse(
                                "--- /dev/null\n+++ b/current.rs\n@@ -0,0 +1 @@\n+CURRENT\n",
                            )
                            .unwrap(),
                            cx,
                        )
                    });
                    app
                })
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            // The adjacent resize handle must not consume the list scrollbar's clicks.
            app.update(cx, |app, cx| {
                app.active_files_mut()
                    .changes
                    .extend((2..42).map(|index| Change {
                        path: format!("file-{index}.rs").into(),
                        previous_path: None,
                        status: 'A',
                        area: Area::Untracked,
                    }));
                cx.notify();
            });
            window.render_frame(cx);
            window.render_frame(cx);
            let scroll = app.read(cx).active_files().file_scroll.clone();
            // Scrolling reveals the auto-hiding bar before interacting with it.
            scroll.set_offset(point(px(0.), -px(20.)));
            window.render_frame(cx);
            let bounds = scroll.viewport_bounds();
            let position = point(bounds.right() - px(3.), bounds.bottom() - px(8.));
            window.dispatch_event(
                MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            window.dispatch_event(
                MouseDownEvent {
                    position,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            window.dispatch_event(
                MouseUpEvent {
                    position,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            assert!(
                scroll.offset().y < -px(20.),
                "the scrollbar at the pane edge must receive the click"
            );
            let header = window.find("next-file").bounds();
            app.update(cx, |app, cx| app.select_file(1, cx));
            assert!(app.read(cx).active_files().patch_loading);
            assert_eq!(
                app.read(cx).active_files().diff.read(cx).files()[0]
                    .path()
                    .as_str(),
                "current.rs"
            );
            window.render_frame(cx);
            assert_eq!(header, window.find("next-file").bounds());
            assert!(
                window
                    .find_all("source")
                    .iter()
                    .any(|line| line.label() == Some("CURRENT"))
            );
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(!app.read(cx).active_files().patch_loading);
            assert_eq!(
                app.read(cx).active_files().diff.read(cx).files()[0]
                    .path()
                    .as_str(),
                "next.rs"
            );
        });
        // Repeated selection cancels the pending load, preserving the first preview.
        for index in [0, 1] {
            cx.run_until_parked();
            cx.executor().advance_clock(FILE_SELECTION_DELAY / 2);
            cx.update(|cx| app.update(cx, |app, cx| app.select_file(index, cx)));
        }
        cx.run_until_parked();
        cx.executor()
            .advance_clock(FILE_SELECTION_DELAY - Duration::from_millis(1));
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(app.read(cx).active_files().patch_loading);
            assert_eq!(
                app.read(cx).active_files().diff.read(cx).files()[0]
                    .path()
                    .as_str(),
                "next.rs"
            );
        });
        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            assert!(!app.read(cx).active_files().patch_loading);
            assert_eq!(
                app.read(cx).active_files().diff.read(cx).files()[0]
                    .path()
                    .as_str(),
                "next.rs"
            );
            app.update(cx, |app, cx| app.select_file(1, cx));
            assert!(!app.read(cx).active_files().patch_loading);
            window.render_frame(cx);
            window.render_frame(cx);
            assert!(
                window
                    .find_all("source")
                    .iter()
                    .any(|line| line.label() == Some("NEXT"))
            );
        })
        .unwrap();
        std::fs::write(
            directory.path().join("picture.png"),
            include_bytes!("../tests/fixtures/images/sample.png"),
        )
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            app.update(cx, |app, cx| {
                app.active_files_mut().changes.push(Change {
                    path: "picture.png".into(),
                    previous_path: None,
                    status: 'A',
                    area: Area::Untracked,
                });
                app.select_file(app.active_files().changes.len() - 1, cx);
            });
            window.render_frame(cx);
            assert!(window.try_find("soft-wrap").is_some());
            assert!(app.read(cx).active_files().image.is_none());
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(FILE_SELECTION_DELAY);
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            assert!(app.read(cx).active_files().image.is_some());
            assert!(app.read(cx).active_files().diff.read(cx).files().is_empty());
            assert!(window.try_find("soft-wrap").is_none());
            assert!(window.try_find("image-fit").is_some());
            window.click("image-actual", cx);
            assert!(window.find("image-actual").checked().unwrap());
            app.update(cx, |app, cx| app.select_file(1, cx));
            window.render_frame(cx);
            assert!(window.try_find("image-fit").is_some());
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(FILE_SELECTION_DELAY);
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(app.read(cx).active_files().image.is_none());
            assert!(window.try_find("image-fit").is_none());
            assert!(window.try_find("soft-wrap").is_some());
        })
        .unwrap();
        // Repository mode keeps the same commit selected while opening and leaving a file.
        for args in [
            vec!["init", "-b", "main"],
            vec!["add", "--all"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.test",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "Repository overview",
                "-m",
                "Full commit message",
            ],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(directory.path())
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let repository = Repository::open(directory.path()).unwrap();
        let commits = repository.history(0, None).unwrap().commits;
        let branches = repository.branches().unwrap();
        cx.update_window(handle, |_, window, cx| {
            app.update(cx, |app, cx| {
                app.preferences.remember_repository(repository.root.clone());
                app.repository = Some(repository);
                app.branches = branches;
                app.history.commits = commits;
                app.view = View::Repositories;
                app.select_row(Row::Commit(0), false, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            assert_eq!(app.read(cx).active_files().selected_file, None);
            assert!((repository_sidebar_ratio(window) - 0.5).abs() < 0.01);
            let repo_row = window.find(("repository", 0usize)).bounds();
            let commit_row = window.find(("commit", 0usize)).bounds();
            let file_row = window.find("tree-file-0").bounds();
            assert!(repo_row.right() <= commit_row.left());
            assert!(commit_row.right() <= file_row.left());
            // Selecting the message after searching must copy the message, not the input.
            app.update(cx, |app, cx| {
                app.history.search_available = true;
                app.history_search.update(cx, |input, cx| {
                    input.focus(window, cx);
                    input.set_value("search text", window, cx);
                });
            });
            window.render_frame(cx);
            window.render_frame(cx);
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            assert_eq!(window.focused(cx), Some(app.read(cx).focus.clone()));
            assert_eq!(
                app.read(cx).history_search.read(cx).value().as_str(),
                "search text"
            );
            app.read(cx)
                .history_search
                .clone()
                .update(cx, |input, cx| input.focus(window, cx));
            window.render_frame(cx);
            let message = window.find("commit-message").bounds();
            let position = message.origin + point(px(30.), px(16.));
            window.dispatch_event(
                MouseDownEvent {
                    position,
                    button: MouseButton::Left,
                    click_count: 2,
                    modifiers: Modifiers::default(),
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseUpEvent {
                    position,
                    button: MouseButton::Left,
                    click_count: 2,
                    modifiers: Modifiers::default(),
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            let selected = gpui_kit::base::TextSelection::selected_text(window, cx);
            assert_eq!(selected, "Repository");
            assert_eq!(window.focused(cx), Some(app.read(cx).focus.clone()));
            cx.write_to_clipboard(ClipboardItem::new_string("sentinel".into()));
            window.dispatch_action(Box::new(component::input::Copy), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().unwrap(),
                "Repository"
            );
            window.click("copy-commit-id", cx);
            assert!(app.read(cx).commit_copied);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().unwrap(),
                app.read(cx).history.commits[0].id
            );
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(COPY_FEEDBACK_DURATION);
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            assert!(!app.read(cx).commit_copied);
            window.click("tree-file-0", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(FILE_SELECTION_DELAY);
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(app.read(cx).active_files().selected_file.is_some());
            window.click("back-to-files", cx);
            assert!(!app.read(cx).repository_preview);
            assert_eq!(app.read(cx).active_files().selected_file, Some(0));
            assert_eq!(app.read(cx).history.selected, Some(Row::Commit(0)));
            assert!(window.try_find("tree-file-0").is_some());
            app.update(cx, |app, cx| {
                app.view = View::History;
                app.select_file(0, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(FILE_SELECTION_DELAY);
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            app.update(cx, |app, cx| {
                let repo = app.repository.as_ref().unwrap();
                let prepared = PreparedChanges::load(
                    repo,
                    app.history.files.changes.clone(),
                    &app.current_comparison(),
                    true,
                )
                .unwrap();
                app.apply_changes(prepared, cx);
            });
            for view in [View::History] {
                app.update(cx, |app, cx| {
                    app.view = view;
                    app.preferences.show_file_stats = false;
                    cx.notify();
                });
                window.render_frame(cx);
                window.render_frame(cx);
                let selected = app.read(cx).active_files().selected_file;
                let diff_path = app.read(cx).active_files().diff.read(cx).files()[0]
                    .path()
                    .clone();
                assert!(window.find_all("file-diff-stats").is_empty());
                window.click("toggle-file-stats", cx);
                window.render_frame(cx);
                assert!(!window.find_all("file-diff-stats").is_empty());
                let total_right = window.find("change-totals").bounds().right();
                for stats in window.find_all("file-diff-stats") {
                    assert!((stats.bounds().right() - total_right).abs() < px(1.));
                }
                window.click("toggle-file-list", cx);
                window.render_frame(cx);
                window.render_frame(cx);
                assert!(window.try_find("tree-file-0").is_some());
                assert!(app.read(cx).preferences.tree_file_list);
                let total_right = window.find("change-totals").bounds().right();
                for stats in window.find_all("file-diff-stats") {
                    assert!((stats.bounds().right() - total_right).abs() < px(1.));
                }
                window.click("toggle-file-list", cx);
                window.render_frame(cx);
                assert!(window.try_find(("file", 0usize)).is_some());
                assert_eq!(app.read(cx).active_files().selected_file, selected);
                assert!(!app.read(cx).active_files().patch_loading);
                assert_eq!(
                    app.read(cx).active_files().diff.read(cx).files()[0].path(),
                    &diff_path
                );
            }
        })
        .unwrap();
        // The footer toggle is available in both history layouts and keeps parsed content intact.
        cx.update_window(handle, |_, window, cx| {
            app.update(cx, |app, cx| app.switch_view(View::History, cx));
            window.render_frame(cx);
            let diff = app.read(cx).history.files.diff.entity_id();
            let selected = app.read(cx).history.selected;
            assert!(window.find("toggle-commit-message").checked().unwrap());
            assert!(window.try_find("commit-message").is_some());
            window.click("toggle-commit-message", cx);
            window.render_frame(cx);
            assert!(!window.find("toggle-commit-message").checked().unwrap());
            assert!(window.try_find("commit-message").is_none());
            let saved: Preferences = serde_json::from_slice(
                &std::fs::read(
                    app.read(cx)
                        .preferences_directory
                        .path()
                        .join("preferences.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert!(!saved.show_commit_message);
            app.update(cx, |app, cx| app.switch_view(View::Repositories, cx));
            window.render_frame(cx);
            assert!(window.try_find("commit-message").is_none());
            window.click("toggle-commit-message", cx);
            window.render_frame(cx);
            assert!(window.try_find("commit-message").is_some());
            assert_eq!(app.read(cx).history.files.diff.entity_id(), diff);
            assert_eq!(app.read(cx).history.selected, selected);
            assert!(!app.read(cx).history.files.patch_loading);
            app.update(cx, |app, cx| app.switch_view(View::History, cx));
            window.render_frame(cx);
            window.click("soft-wrap", cx);
            window.render_frame(cx);
            let saved: Preferences = serde_json::from_slice(
                &std::fs::read(
                    app.read(cx)
                        .preferences_directory
                        .path()
                        .join("preferences.json"),
                )
                .unwrap(),
            )
            .unwrap();
            let current = &app.read(cx).preferences;
            assert_eq!(saved.soft_wrap, current.soft_wrap);
            assert_eq!(saved.tree_file_list, current.tree_file_list);
            assert_eq!(saved.show_file_stats, current.show_file_stats);
            assert_eq!(saved.show_commit_message, current.show_commit_message);
        })
        .unwrap();
        // Drain menu dismissal between languages, as the native event loop does.
        let original_locale = component::locale().to_string();
        let (selected, diff_path) = cx.update(|cx| {
            (
                app.read(cx).active_files().selected_file,
                app.read(cx).active_files().diff.read(cx).files()[0]
                    .path()
                    .clone(),
            )
        });
        for language in Language::ALL {
            cx.update_window(handle, |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.preferences.language = language;
                    apply_language(language, cx);
                    cx.notify();
                });
                window.render_frame(cx);
                assert_eq!(
                    window.find("open-repository").label(),
                    Some(tr("open_repository"))
                );
                assert_eq!(window.find("soft-wrap").label(), Some(tr("soft_wrap")));
                assert_eq!(app.read(cx).active_files().selected_file, selected);
                assert_eq!(
                    app.read(cx).active_files().diff.read(cx).files()[0].path(),
                    &diff_path
                );
                assert!(!app.read(cx).active_files().patch_loading);
                window.click("language", cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                assert!(
                    window.try_find("popup-menu").is_some(),
                    "Menu did not open for {language:?}"
                );
                for (index, item_language) in Language::ALL.into_iter().enumerate() {
                    let item = window.within("popup-menu").find(index);
                    assert_eq!(item.label(), Some(language_name(item_language)));
                    assert!(item.bounds().right() <= window.viewport_size().width);
                    assert!(item.bounds().bottom() <= window.viewport_size().height);
                }
                window.press("escape", cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
        component::set_locale(&original_locale);
        // Commit changes and their first diff replace the old snapshot together.
        std::fs::write(directory.path().join("later.rs"), "LATER\n").unwrap();
        for args in [
            vec!["add", "later.rs"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.test",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "Later commit",
            ],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(directory.path())
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let latest = Repository::open(directory.path())
            .unwrap()
            .history(0, None)
            .unwrap()
            .commits
            .remove(0);
        cx.update_window(handle, |_, window, cx| {
            app.update(cx, |app, cx| {
                app.view = View::History;
                app.history.commits.push(latest);
                let files = app
                    .active_files()
                    .changes
                    .iter()
                    .map(|change| change.path.clone())
                    .collect::<Vec<_>>();
                let old_patch = app.active_files_mut().diff.read(cx).files()[0]
                    .path()
                    .clone();
                app.select_row(Row::Commit(1), false, cx);
                assert_eq!(app.history.selected, Some(Row::Commit(0)));
                assert_eq!(
                    app.active_files_mut()
                        .changes
                        .iter()
                        .map(|change| change.path.clone())
                        .collect::<Vec<_>>(),
                    files
                );
                assert_eq!(
                    app.active_files_mut().diff.read(cx).files()[0].path(),
                    &old_patch
                );
            });
            window.render_frame(cx);
            assert!(
                window
                    .find_all("source")
                    .iter()
                    .any(|line| line.label() == Some("NEXT"))
            );
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            assert_eq!(app.read(cx).history.selected, Some(Row::Commit(1)));
            assert_eq!(
                app.read(cx).active_files().changes[0].path,
                PathBuf::from("later.rs")
            );
            assert_eq!(
                app.read(cx).active_files().diff.read(cx).files()[0]
                    .path()
                    .as_str(),
                "later.rs"
            );
            assert!(!app.read(cx).active_files().patch_loading);
            app.update(cx, |app, cx| {
                app.load(directory.path().join("missing-repository"), cx)
            });
            assert_eq!(app.read(cx).history.selected, Some(Row::Commit(1)));
            window.render_frame(cx);
            assert!(
                window
                    .find_all("source")
                    .iter()
                    .any(|line| line.label() == Some("LATER"))
            );
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(app.read(cx).error.is_some());
            assert_eq!(app.read(cx).history.selected, Some(Row::Commit(1)));
            assert_eq!(
                app.read(cx).active_files().diff.read(cx).files()[0]
                    .path()
                    .as_str(),
                "later.rs"
            );
        });
        // A new window gets its initial ratio from serialized preferences, not old layout state.
        cx.update(|cx| {
            app.update(cx, |app, _| {
                app.view = View::Repositories;
                app.error = None;
                app.preferences.repository_sidebar_ratio = Some(0.35);
                app.preferences =
                    serde_json::from_slice(&serde_json::to_vec(&app.preferences).unwrap()).unwrap();
            })
        });
        let (restored, _) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, _| app.clone()).unwrap()
        });
        cx.update_window(restored, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            assert!((repository_sidebar_ratio(window) - 0.35).abs() < 0.01);
        })
        .unwrap();
        // Repos and History share one retained snapshot across Tab switches.
        let (history_diff, commit) = cx.update(|cx| {
            let app = app.read(cx);
            (app.history.files.diff.entity_id(), app.history.selected)
        });
        for target in [View::History, View::Repositories, View::History] {
            cx.update_window(handle, |_, window, cx| {
                app.update(cx, |app, cx| app.switch_view(target, cx));
                window.render_frame(cx);
                let app = app.read(cx);
                assert!(!app.loading);
                assert_eq!(app.history.files.diff.entity_id(), history_diff);
                assert_eq!(app.history.selected, commit);
            })
            .unwrap();
        }
    }
}
