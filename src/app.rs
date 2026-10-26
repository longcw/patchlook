use crate::{
    change_tree::{ChangeTree, stats_label},
    image_diff::{ImageDiff, ImageVersions, image_format},
    strings::{area_label, language_name, tr},
};
use diff_tool::{
    git::{Branch, Change, Commit, DiffStats, Repository, Snapshot},
    preferences::{Appearance, Language, Layout, Preferences},
};
use gpui_kit::assets::IconName;
use gpui_kit::base::TestSupportExt;
use gpui_kit::{
    component::{
        ActiveTheme, Disableable, Icon, Selectable, Sizable, Theme, ThemeMode, TitleBar,
        button::{Button, ButtonCustomVariant, ButtonVariants},
        diff::{Diff, DiffChangeIndicator, DiffFile, DiffHunkSeparator, DiffMode, DiffState},
        h_flex,
        menu::{DropdownMenu, PopupMenuItem},
        resizable::{h_resizable, resizable_panel, v_resizable},
        scroll::{Scrollbar, ScrollbarMode},
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};
use std::path::PathBuf;

gpui_kit::actions!(diff_tool, [OpenRepository, Refresh, Quit]);

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Repositories,
    Working,
    History,
}

pub struct DiffTool {
    preferences: Preferences,
    repository: Option<Repository>,
    branch: String,
    branches: Vec<Branch>,
    repository_scroll: UniformListScrollHandle,
    branch_scroll: UniformListScrollHandle,
    view: View,
    changes: Vec<Change>,
    file_stats: Vec<Option<DiffStats>>,
    change_tree: Option<ChangeTree>,
    tree_file_list: bool,
    show_file_stats: bool,
    total_stats: Option<DiffStats>,
    file_scroll: UniformListScrollHandle,
    commit_scroll: UniformListScrollHandle,
    commits: Vec<Commit>,
    has_more: bool,
    selected_commit: Option<usize>,
    selected_file: Option<usize>,
    diff: Entity<DiffState>,
    image: Option<Entity<ImageDiff>>,
    image_subscription: Option<Subscription>,
    loading: bool,
    files_loading: bool,
    patch_loading: bool,
    empty_patch: bool,
    soft_wrap: bool,
    error: Option<String>,
    load_task: Option<Task<()>>,
    commit_task: Option<Task<()>>,
    patch_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
    focus: FocusHandle,
}

impl DiffTool {
    pub fn new(initial: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (preferences, error) = match Preferences::read() {
            Ok(preferences) => (preferences, None),
            Err(error) => (Preferences::default(), Some(format!("{error:#}"))),
        };
        apply_appearance(preferences.appearance, window, cx);
        apply_language(preferences.language, cx);
        let diff = cx.new(|cx| DiffState::new([], cx).with_mode(diff_mode(preferences.layout)));
        let subscriptions = vec![
            cx.observe(&diff, |_, _, cx| cx.notify()),
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.preferences.appearance == Appearance::System {
                    Theme::sync_system_appearance(Some(window), cx);
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() && this.preferences.language == Language::System {
                    apply_language(Language::System, cx);
                }
            }),
        ];
        let initial = initial.or_else(|| preferences.repository.clone());
        let mut app = Self {
            preferences,
            repository: None,
            branch: String::new(),
            branches: vec![],
            repository_scroll: UniformListScrollHandle::new(),
            branch_scroll: UniformListScrollHandle::new(),
            view: View::Working,
            changes: vec![],
            file_stats: vec![],
            change_tree: None,
            tree_file_list: false,
            show_file_stats: false,
            total_stats: None,
            file_scroll: UniformListScrollHandle::new(),
            commit_scroll: UniformListScrollHandle::new(),
            commits: vec![],
            has_more: false,
            selected_commit: None,
            selected_file: None,
            diff,
            image: None,
            image_subscription: None,
            loading: false,
            files_loading: false,
            patch_loading: false,
            empty_patch: false,
            soft_wrap: true,
            error,
            load_task: None,
            commit_task: None,
            patch_task: None,
            _subscriptions: subscriptions,
            focus: cx.focus_handle(),
        };
        window.focus(&app.focus, cx);
        if let Some(path) = initial {
            app.load(path, cx);
        }
        app
    }

    fn save_preferences(&mut self) {
        if let Err(error) = self.preferences.write() {
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

    fn clear_file(&mut self, cx: &mut Context<Self>) {
        self.patch_task = None;
        self.selected_file = None;
        self.patch_loading = false;
        self.empty_patch = false;
        self.image = None;
        self.image_subscription = None;
        self.diff.update(cx, |diff, cx| diff.set_files([], cx));
    }

    fn clear_detail(&mut self, cx: &mut Context<Self>) {
        self.commit_task = None;
        self.clear_file(cx);
        self.changes.clear();
        self.file_stats.clear();
        self.change_tree = None;
        self.total_stats = None;
        self.selected_commit = None;
        self.files_loading = false;
    }

    fn viewed_branch(&self) -> Option<&str> {
        self.repository
            .as_ref()
            .and_then(|repo| self.preferences.viewed_branch(&repo.root))
    }

    fn viewed_branch_name(&self) -> String {
        self.viewed_branch()
            .and_then(|reference| {
                self.branches
                    .iter()
                    .find(|branch| branch.reference == reference)
            })
            .map(|branch| branch.name.clone())
            .unwrap_or_else(|| self.branch.clone())
    }

    fn browse_branch(&mut self, reference: Option<String>, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        if let Some(repo) = &self.repository {
            let path = repo.root.clone();
            self.preferences.set_viewed_branch(&path, reference);
            self.save_preferences();
            self.load(path, cx);
        }
    }

    fn load(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.load_task = None;
        self.clear_detail(cx);
        self.commits.clear();
        self.has_more = false;
        self.loading = true;
        self.error = None;
        let history = self.view != View::Working;
        let preferences = self.preferences.clone();
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let repo = Repository::open(&path)?;
                    let snapshot = repo.snapshot()?;
                    let branches = repo.branches()?;
                    let reference = preferences.viewed_branch(&repo.root);
                    let page = if history {
                        let revision = match reference {
                            Some(reference) => Some(
                                branches
                                    .iter()
                                    .find(|branch| branch.reference == reference)
                                    .ok_or_else(|| {
                                        anyhow::anyhow!("{}: {reference}", tr("branch_missing"))
                                    })?
                                    .revision
                                    .as_str(),
                            ),
                            None => None,
                        };
                        Some(repo.history(0, revision)?)
                    } else {
                        None
                    };
                    let stats = if history {
                        None
                    } else {
                        Some(repo.file_stats(&snapshot.changes, None)?)
                    };
                    anyhow::Ok((repo, snapshot, branches, page, stats))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((repo, Snapshot { branch, changes }, branches, page, stats)) => {
                        this.preferences.remember_repository(repo.root.clone());
                        this.repository = Some(repo);
                        this.branch = branch;
                        this.branches = branches;
                        this.commit_scroll = UniformListScrollHandle::new();
                        this.file_scroll = UniformListScrollHandle::new();
                        this.save_preferences();
                        if let Some(page) = page {
                            this.commits = page.commits;
                            this.has_more = page.has_more;
                            if !this.commits.is_empty() {
                                this.select_commit(0, cx);
                            }
                        } else {
                            this.set_changes(changes, stats.unwrap(), cx);
                            if !this.changes.is_empty() {
                                this.select_file(0, cx);
                            }
                        }
                    }
                    Err(error) => {
                        this.error = Some(format!("{}: {error:#}", tr("load_failed")));
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub fn refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(repo) = &self.repository {
            self.load(repo.root.clone(), cx);
        }
    }

    fn switch_view(&mut self, view: View, cx: &mut Context<Self>) {
        if self.view == view || self.loading {
            return;
        }
        self.view = view;
        if let Some(repo) = &self.repository {
            self.load(repo.root.clone(), cx);
        }
        cx.notify();
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repository.clone() else {
            return;
        };
        if self.loading {
            return;
        }
        self.loading = true;
        let offset = self.commits.len();
        let revision = self.commits.first().map(|commit| commit.id.clone());
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { repo.history(offset, revision.as_deref()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(page) => {
                        this.commits.extend(page.commits);
                        this.has_more = page.has_more;
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn select_commit(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(repo) = self.repository.clone() else {
            return;
        };
        let commit = self.commits[index].clone();
        self.clear_detail(cx);
        self.selected_commit = Some(index);
        self.files_loading = true;
        self.error = None;
        self.commit_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let changes = repo.commit_changes(&commit)?;
                    let stats = repo.file_stats(&changes, Some(&commit))?;
                    anyhow::Ok((changes, stats))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.files_loading = false;
                match result {
                    Ok((changes, stats)) => {
                        this.set_changes(changes, stats, cx);
                        if !this.changes.is_empty() && this.view != View::Repositories {
                            this.select_file(0, cx);
                        }
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn set_changes(
        &mut self,
        changes: Vec<Change>,
        stats: Vec<Option<DiffStats>>,
        cx: &mut Context<Self>,
    ) {
        self.total_stats = Some(stats.iter().flatten().copied().sum());
        self.change_tree = self
            .repository
            .as_ref()
            .map(|repo| ChangeTree::new(&changes, &stats, &repo.root, cx));
        self.changes = changes;
        self.file_stats = stats;
    }

    fn toggle_file_list(&mut self, cx: &mut Context<Self>) {
        self.tree_file_list = !self.tree_file_list;
        if let Some(index) = self.selected_file {
            if self.tree_file_list {
                if let Some(tree) = &self.change_tree {
                    tree.reveal_file(index, cx);
                }
            } else {
                self.file_scroll
                    .scroll_to_item(index, ScrollStrategy::Center);
            }
        }
        cx.notify();
    }

    fn select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(repo) = self.repository.clone() else {
            return;
        };
        let change = self.changes[index].clone();
        let commit = self
            .selected_commit
            .map(|index| self.commits[index].clone());
        self.selected_file = Some(index);
        if let Some(tree) = &self.change_tree {
            tree.reveal_file(index, cx);
        }
        self.file_scroll
            .scroll_to_item(index, ScrollStrategy::Center);
        self.patch_task = None;
        self.patch_loading = true;
        self.error = None;
        self.empty_patch = false;
        self.patch_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if image_format(&change.path).is_some() {
                        let versions = repo.file_versions(&change, commit.as_ref())?;
                        Ok::<_, anyhow::Error>((
                            vec![],
                            Some(ImageVersions::decode(&change, versions)?),
                        ))
                    } else {
                        let patch = repo.patch(&change, commit.as_ref())?;
                        Ok((DiffFile::parse(&patch)?, None))
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.patch_loading = false;
                match result {
                    Ok((files, image)) => {
                        this.empty_patch = files.is_empty() && image.is_none();
                        this.image = image.map(|image| cx.new(|_| ImageDiff::new(image)));
                        this.image_subscription = this
                            .image
                            .as_ref()
                            .map(|image| cx.observe(image, |_, _, cx| cx.notify()));
                        this.diff.update(cx, |diff, cx| diff.set_files(files, cx));
                    }
                    Err(error) => {
                        this.image = None;
                        this.image_subscription = None;
                        this.diff.update(cx, |diff, cx| diff.set_files([], cx));
                        this.error = Some(format!("{}: {error:#}", tr("load_failed")));
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn previous_file(&self) -> Option<usize> {
        self.selected_file.and_then(|index| index.checked_sub(1))
    }

    fn next_file(&self) -> Option<usize> {
        self.selected_file
            .map(|index| index + 1)
            .filter(|index| *index < self.changes.len())
    }

    fn set_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        self.preferences.layout = layout;
        self.diff
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

    fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        self.preferences.language = language;
        apply_language(language, cx);
        self.save_preferences();
        cx.notify();
    }
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

impl DiffTool {
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
                                .on_click(move |_, _, cx| {
                                    let _ =
                                        app.update(cx, |this, cx| this.set_language(language, cx));
                                }),
                        )
                    },
                )
            })
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let repository_name = self.repository.as_ref().map(|repo| {
            repo.root
                .file_name()
                .unwrap_or(repo.root.as_os_str())
                .to_string_lossy()
                .into_owned()
        });
        TitleBar::new().bg(cx.theme().title_bar).child(
            h_flex()
                .size_full()
                .min_w_0()
                .pr_2()
                .gap_2()
                .child(
                    Button::new("open-repository")
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
                .when(self.repository.is_some(), |view| {
                    view.child(
                        h_flex()
                            .gap_1()
                            .max_w(px(160.))
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(Icon::new(IconName::GitBranch).xsmall())
                            .child(div().truncate().child(self.branch.clone())),
                    )
                })
                .child(
                    icon_button("refresh", IconName::RefreshCw, "refresh")
                        .xsmall()
                        .text_color(cx.theme().muted_foreground)
                        .disabled(self.repository.is_none() || self.loading)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.refresh(&Refresh, window, cx)),
                        ),
                )
                .child(div().flex_1())
                .child(self.language_menu(cx))
                .child({
                    let (icon, current) = appearance_label(self.preferences.appearance);
                    let (_, next) = appearance_label(self.preferences.appearance.next());
                    let label = tr("theme_cycle")
                        .replace("{current}", tr(current))
                        .replace("{next}", tr(next));
                    Button::new("theme")
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
                        icon_button("add-repository", IconName::Plus, "open_repository")
                            .xsmall()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_repository(&OpenRepository, window, cx)
                            })),
                    ),
            )
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
            .is_some_and(|repo| repo.root == path);
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

    fn branch_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = if self.repository.is_some() {
            self.branches.len() + 1
        } else {
            0
        };
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
                    .child(tr("branches"))
                    .child(self.branches.len().to_string()),
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
                                range
                                    .map(|index| this.branch_row(index, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.branch_scroll)
                        .size_full(),
                    )
                    .child(list_scrollbar("branches-scrollbar", &self.branch_scroll)),
            )
            .into_any_element()
    }

    fn branch_row(&self, index: usize, cx: &mut Context<Self>) -> Div {
        let branch = index
            .checked_sub(1)
            .and_then(|index| self.branches.get(index));
        let reference = branch.map(|branch| branch.reference.clone());
        let selected = self.viewed_branch() == reference.as_deref();
        let name = branch
            .map(|branch| branch.name.clone())
            .unwrap_or_else(|| tr("head_branch").to_owned());
        let group = SharedString::from(format!("branch-row-{index}"));
        let caption = branch.map(|branch| {
            if branch.is_head {
                "worktree_branch"
            } else if branch.remote {
                "remote_branch"
            } else {
                "local_branch"
            }
        });
        let action_reference = reference.clone();
        div().h(px(36.)).px_2().pb_1().child(
            h_flex()
                .id(("branch-row", index))
                .group(group.clone())
                .size_full()
                .rounded_md()
                .when(selected, |row| row.bg(cx.theme().secondary_active))
                .hover(|style| {
                    style.bg(if selected {
                        cx.theme().secondary_active
                    } else {
                        cx.theme().list_hover
                    })
                })
                .child(
                    Button::new(("branch", index))
                        .custom(ButtonCustomVariant::new(cx))
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .justify_start()
                        .disabled(self.loading)
                        .tooltip(name.clone())
                        .accessibility_label(name.clone())
                        .child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .gap_2()
                                .child(
                                    Icon::new(if selected {
                                        IconName::Eye
                                    } else {
                                        IconName::GitBranch
                                    })
                                    .xsmall(),
                                )
                                .child(div().flex_1().min_w_0().truncate().text_sm().child(name))
                                .when_some(caption, |row, key| {
                                    row.child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(tr(key)),
                                    )
                                }),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.browse_branch(reference.clone(), cx)
                        })),
                )
                .child(
                    div()
                        .flex_none()
                        .pr_1()
                        .invisible()
                        .group_hover(group, |style| style.visible())
                        .child(
                            Button::new(("browse-branch", index))
                                .icon(IconName::ArrowRight)
                                .xsmall()
                                .custom(
                                    ButtonCustomVariant::new(cx)
                                        .hover(cx.theme().foreground.opacity(0.12))
                                        .active(cx.theme().foreground.opacity(0.18)),
                                )
                                .disabled(self.loading)
                                .tooltip(tr("browse_branch"))
                                .accessibility_label(tr("browse_branch"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.browse_branch(action_reference.clone(), cx)
                                })),
                        ),
                ),
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
                    (View::Working, "working_tree"),
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
        let file = self.diff.read(cx).files().first();
        let path = file.map(|file| file.path().clone());
        let title = self
            .image
            .as_ref()
            .map(|image| image.read(cx).title().to_owned())
            .or_else(|| {
                file.map(|file| match (file.original_path(), file.modified_path()) {
                    (Some(old), Some(new)) if old != new => format!("{old} → {new}"),
                    _ => file.path().to_string(),
                })
            })
            .or_else(|| {
                self.selected_file
                    .and_then(|i| self.changes.get(i))
                    .map(|change| change.path.display().to_string())
            });
        let folded_context = self.diff.read(cx).has_collapsed_context();
        let collapsed = path
            .as_ref()
            .is_some_and(|path| self.diff.read(cx).is_file_collapsed(path));
        h_flex()
            .flex_none()
            .h(px(32.))
            .px_2()
            .gap_1()
            .border_b(px(1. / window.scale_factor()))
            .border_color(cx.theme().border)
            .when(self.view == View::Repositories, |row| {
                row.child(
                    icon_button("back-to-files", IconName::ArrowLeft, "back_to_files").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.clear_file(cx);
                            this.error = None;
                            cx.notify();
                        }),
                    ),
                )
            })
            .when_some(path, |row, path| {
                row.child(
                    icon_button(
                        "collapse-file",
                        if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        },
                        if collapsed {
                            "expand_file"
                        } else {
                            "collapse_file"
                        },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.diff.update(cx, |diff, cx| {
                            diff.set_file_collapsed(&path, !diff.is_file_collapsed(&path), cx)
                        });
                    })),
                )
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
            .when(self.image.is_none(), |row| {
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
                    .when(file.is_some() && !collapsed, |button| {
                        button.text_color(cx.theme().muted_foreground)
                    })
                    .disabled(file.is_none() || collapsed)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.diff.update(cx, |diff, cx| {
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
                        .selected(self.soft_wrap)
                        .toggled(self.soft_wrap)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.soft_wrap = !this.soft_wrap;
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
            .when_some(self.image.as_ref(), |row, image| {
                row.child(ImageDiff::toolbar(image, cx))
            })
    }

    fn files(&self, cx: &mut Context<Self>) -> AnyElement {
        let tree_view = self.view == View::Repositories || self.tree_file_list;
        let tree = self.change_tree.as_ref().filter(|_| tree_view);
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
                    .child(self.changes.len().to_string())
                    .when(self.view != View::Repositories, |row| {
                        row.child(
                            icon_button(
                                "toggle-file-list",
                                if self.tree_file_list {
                                    IconName::List
                                } else {
                                    IconName::FolderTree
                                },
                                if self.tree_file_list {
                                    "flat_file_list"
                                } else {
                                    "tree_file_list"
                                },
                            )
                            .ml_1()
                            .icon(
                                Icon::new(if self.tree_file_list {
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
                                .disabled(self.changes.is_empty())
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        if let Some(tree) = &this.change_tree {
                                            tree.toggle_directories(cx);
                                        }
                                    },
                                )),
                            )
                            .child(self.file_stats_button(cx))
                            .child(toolbar_divider(cx))
                        },
                    )
                    .when_some(self.total_stats, |row, stats| {
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
                    }),
            )
            .child(if let Some(tree) = tree {
                let app = cx.entity().downgrade();
                div()
                    .flex_1()
                    .min_h_0()
                    .child(tree.render(
                        self.viewed_branch_name(),
                        self.show_file_stats,
                        self.view == View::Working,
                        cx,
                        move |index, _, cx| {
                            let _ = app.update(cx, |app, cx| app.select_file(index, cx));
                        },
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
                            self.changes.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|index| this.file_row(index, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.file_scroll)
                        .size_full(),
                    )
                    .child(list_scrollbar("files-list-scrollbar", &self.file_scroll))
                    .into_any_element()
            })
            .into_any_element()
    }

    fn file_stats_button(&self, cx: &mut Context<Self>) -> Button {
        icon_button(
            "toggle-file-stats",
            IconName::Diff,
            if self.show_file_stats {
                "hide_file_stats"
            } else {
                "show_file_stats"
            },
        )
        .when(self.view != View::Repositories, |button| {
            button.icon(Icon::new(IconName::Diff).text_color(cx.theme().muted_foreground))
        })
        .selected(self.show_file_stats)
        .toggled(self.show_file_stats)
        .on_click(cx.listener(|this, _, _, cx| {
            this.show_file_stats = !this.show_file_stats;
            cx.notify();
        }))
    }

    fn file_row(&self, index: usize, cx: &mut Context<Self>) -> Div {
        let change = &self.changes[index];
        let filename = change
            .path
            .file_name()
            .unwrap_or(change.path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let caption = if self.view == View::Working {
            area_label(change.area)
        } else {
            status_label(change.status)
        };
        let status_color = match change.status {
            'A' => cx.theme().success,
            'D' => cx.theme().danger,
            _ => cx.theme().warning,
        };
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
                .selected(self.selected_file == Some(index))
                .tooltip(change.path.display().to_string())
                .accessibility_label(format!("{}, {}", change.path.display(), caption))
                .child(
                    h_flex()
                        .gap_2()
                        .w_full()
                        .min_w_0()
                        .child(
                            div()
                                .w(px(16.))
                                .flex_none()
                                .text_color(status_color)
                                .child(change.status.to_string()),
                        )
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
                                                .truncate()
                                                .child(filename),
                                        )
                                        .when_some(
                                            self.file_stats
                                                .get(index)
                                                .copied()
                                                .filter(|_| self.show_file_stats),
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
                                                    .map(|path| path.display().to_string())
                                                    .unwrap_or_default(),
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
                    .child(self.commits.len().to_string())
                    .child(div().flex_1())
                    .child(Icon::new(IconName::GitBranch).xsmall())
                    .child(div().min_w_0().truncate().child(self.viewed_branch_name())),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "commits-list",
                            self.commits.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|index| this.commit_row(index, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.commit_scroll)
                        .size_full(),
                    )
                    .child(list_scrollbar(
                        "commits-list-scrollbar",
                        &self.commit_scroll,
                    )),
            )
            .when(self.has_more, |view| {
                view.child(
                    div().flex_none().p_2().child(
                        Button::new("more-commits")
                            .label(tr("load_more"))
                            .small()
                            .ghost()
                            .w_full()
                            .loading(self.loading)
                            .on_click(cx.listener(|this, _, _, cx| this.load_more(cx))),
                    ),
                )
            })
            .into_any_element()
    }

    fn commit_row(&self, index: usize, cx: &mut Context<Self>) -> Div {
        let commit = &self.commits[index];
        div().h(px(50.)).px_2().pb_1().child(
            Button::new(("commit", index))
                .custom(
                    ButtonCustomVariant::new(cx)
                        .hover(cx.theme().list_hover)
                        .active(cx.theme().secondary_active),
                )
                .w_full()
                .h_full()
                .justify_start()
                .selected(self.selected_commit == Some(index))
                .tooltip(commit.subject.clone())
                .accessibility_label(format!(
                    "{}, {}, {}",
                    commit.subject, commit.author, commit.date
                ))
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .items_start()
                        .gap_1()
                        .child(div().text_sm().truncate().child(commit.subject.clone()))
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().flex_1().truncate().child(commit.author.clone()))
                                .child(commit.date.clone()),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.select_commit(index, cx))),
        )
    }

    fn detail(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let content = if (self.loading && self.commits.is_empty()) || self.files_loading {
            self.message("loading", None, cx)
        } else if self.patch_loading
            && self.diff.read(cx).files().is_empty()
            && self.image.is_none()
        {
            self.message("loading_diff", None, cx)
        } else if self.empty_patch {
            self.message("no_patch", Some("no_patch_body"), cx)
        } else if self.error.is_some()
            && self.diff.read(cx).files().is_empty()
            && self.image.is_none()
        {
            self.message("load_failed", None, cx)
        } else if let Some(image) = &self.image {
            image.clone().into_any_element()
        } else if self.view == View::Repositories
            && self.selected_file.is_none()
            && self.selected_commit.is_some()
        {
            self.files(cx)
        } else if self.selected_file.is_some() {
            Diff::new(&self.diff)
                .change_indicator(DiffChangeIndicator::Bars)
                .header_visible(false)
                .soft_wrap(self.soft_wrap)
                .hunk_separator(DiffHunkSeparator::Wave)
                .size_full()
                .into_any_element()
        } else if self.view == View::Working && self.changes.is_empty() {
            self.message("clean_title", Some("clean_body"), cx)
        } else if self.view != View::Working && self.commits.is_empty() {
            self.message("no_commits_title", Some("no_commits_body"), cx)
        } else {
            self.message("no_files", None, cx)
        };
        v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .when(
                self.view != View::Repositories || self.selected_file.is_some(),
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
                self.selected_commit.and_then(|i| self.commits.get(i)),
                |view, commit| {
                    view.child(
                        v_flex()
                            .id("commit-message")
                            .flex_none()
                            .max_h(px(200.))
                            .overflow_y_scroll()
                            .px_3()
                            .py_2()
                            .gap_1()
                            .border_t(px(1. / window.scale_factor()))
                            .border_color(cx.theme().border)
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(commit.subject.clone()),
                            )
                            .when(!commit.body.is_empty(), |view| {
                                view.child(div().text_sm().child(commit.body.clone()))
                            })
                            .child(
                                h_flex()
                                    .gap_3()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(commit.short_id.clone())
                                    .child(commit.author.clone())
                                    .child(commit.date.clone())
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

impl Render for DiffTool {
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
                v_resizable("repositories-sidebar")
                    .child(
                        resizable_panel()
                            .size(px(230.))
                            .child(self.repositories(cx)),
                    )
                    .child(
                        resizable_panel()
                            .overflow_hidden()
                            .child(self.branch_list(cx)),
                    )
                    .into_any_element()
            } else if self.view == View::History {
                v_resizable("history-sidebar")
                    .child(resizable_panel().size(px(350.)).child(self.history(cx)))
                    .child(resizable_panel().overflow_hidden().child(self.files(cx)))
                    .into_any_element()
            } else {
                self.files(cx)
            };
            let detail = if self.view == View::Repositories {
                h_resizable("repository-history")
                    .child(
                        resizable_panel()
                            .size(px(320.))
                            .size_range(px(210.)..px(600.))
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
            .key_context("DiffTool")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::open_repository))
            .on_action(cx.listener(Self::refresh))
            .child(self.toolbar(cx))
            .when_some(self.error.clone(), |view, error| {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use diff_tool::git::Area;
    use gpui_kit::component::scroll::ScrollbarHandle;
    use gpui_kit::test::TestWindowExt;

    #[gpui_kit::test]
    fn file_navigation_keeps_header_and_code_until_replacement_is_ready(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("next.rs"), "NEXT\n").unwrap();
        let root = directory.path().to_path_buf();
        let (handle, app) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| DiffTool {
                    preferences: Preferences::default(),
                    repository: Some(Repository { root }),
                    branch: "main".into(),
                    branches: vec![],
                    repository_scroll: UniformListScrollHandle::new(),
                    branch_scroll: UniformListScrollHandle::new(),
                    view: View::Working,
                    changes: ["current.rs", "next.rs"]
                        .map(|path| Change {
                            path: path.into(),
                            previous_path: None,
                            status: 'A',
                            area: Area::Untracked,
                        })
                        .to_vec(),
                    file_stats: vec![],
                    change_tree: None,
                    tree_file_list: false,
                    show_file_stats: false,
                    total_stats: None,
                    file_scroll: UniformListScrollHandle::new(),
                    commit_scroll: UniformListScrollHandle::new(),
                    commits: vec![],
                    has_more: false,
                    selected_commit: None,
                    selected_file: Some(0),
                    diff: cx.new(|cx| {
                        DiffState::new(
                            DiffFile::parse(
                                "--- /dev/null\n+++ b/current.rs\n@@ -0,0 +1 @@\n+CURRENT\n",
                            )
                            .unwrap(),
                            cx,
                        )
                    }),
                    image: None,
                    image_subscription: None,
                    loading: false,
                    files_loading: false,
                    patch_loading: false,
                    empty_patch: false,
                    soft_wrap: true,
                    error: None,
                    load_task: None,
                    commit_task: None,
                    patch_task: None,
                    _subscriptions: vec![],
                    focus: cx.focus_handle(),
                })
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            // The adjacent resize handle must not consume the list scrollbar's clicks.
            app.update(cx, |app, cx| {
                app.changes.extend((2..42).map(|index| Change {
                    path: format!("file-{index}.rs").into(),
                    previous_path: None,
                    status: 'A',
                    area: Area::Untracked,
                }));
                cx.notify();
            });
            window.render_frame(cx);
            window.render_frame(cx);
            let scroll = app.read(cx).file_scroll.clone();
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
            let header = window.find("collapse-file").bounds();
            app.update(cx, |app, cx| app.select_file(1, cx));
            assert!(app.read(cx).patch_loading);
            assert_eq!(
                app.read(cx).diff.read(cx).files()[0].path().as_str(),
                "current.rs"
            );
            window.render_frame(cx);
            assert_eq!(header, window.find("collapse-file").bounds());
            assert!(
                window
                    .find_all("source")
                    .iter()
                    .any(|line| line.label() == Some("CURRENT"))
            );
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            assert!(!app.read(cx).patch_loading);
            assert_eq!(
                app.read(cx).diff.read(cx).files()[0].path().as_str(),
                "next.rs"
            );
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
                app.changes.push(Change {
                    path: "picture.png".into(),
                    previous_path: None,
                    status: 'A',
                    area: Area::Untracked,
                });
                app.select_file(app.changes.len() - 1, cx);
            });
            window.render_frame(cx);
            assert!(window.try_find("soft-wrap").is_some());
            assert!(app.read(cx).image.is_none());
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            assert!(app.read(cx).image.is_some());
            assert!(app.read(cx).diff.read(cx).files().is_empty());
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
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(app.read(cx).image.is_none());
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
                app.commits = commits;
                app.view = View::Repositories;
                app.select_commit(0, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            assert_eq!(app.read(cx).selected_file, None);
            let repo_row = window.find(("repository", 0usize)).bounds();
            let commit_row = window.find(("commit", 0usize)).bounds();
            let file_row = window.find("tree-file-0").bounds();
            assert!(repo_row.right() <= commit_row.left());
            assert!(commit_row.right() <= file_row.left());
            assert!(!window.find(("browse-branch", 1usize)).visible());
            window.hover(("branch", 1usize), cx);
            assert!(window.find(("browse-branch", 1usize)).visible());
            window.click("tree-file-0", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(app.read(cx).selected_file.is_some());
            window.click("back-to-files", cx);
            assert_eq!(app.read(cx).selected_file, None);
            assert_eq!(app.read(cx).selected_commit, Some(0));
            assert!(window.try_find("tree-file-0").is_some());
            app.update(cx, |app, cx| {
                app.view = View::History;
                app.select_file(0, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            for view in [View::History, View::Working] {
                app.update(cx, |app, cx| {
                    app.view = view;
                    app.show_file_stats = false;
                    cx.notify();
                });
                window.render_frame(cx);
                window.render_frame(cx);
                let selected = app.read(cx).selected_file;
                let diff_path = app.read(cx).diff.read(cx).files()[0].path().clone();
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
                assert!(app.read(cx).tree_file_list);
                let total_right = window.find("change-totals").bounds().right();
                for stats in window.find_all("file-diff-stats") {
                    assert!((stats.bounds().right() - total_right).abs() < px(1.));
                }
                window.click("toggle-file-list", cx);
                window.render_frame(cx);
                assert!(window.try_find(("file", 0usize)).is_some());
                assert_eq!(app.read(cx).selected_file, selected);
                assert!(!app.read(cx).patch_loading);
                assert_eq!(app.read(cx).diff.read(cx).files()[0].path(), &diff_path);
            }
        })
        .unwrap();
        // Drain menu dismissal between languages, as the native event loop does.
        let original_locale = component::locale().to_string();
        let (selected, diff_path) = cx.update(|cx| {
            (
                app.read(cx).selected_file,
                app.read(cx).diff.read(cx).files()[0].path().clone(),
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
                assert_eq!(app.read(cx).selected_file, selected);
                assert_eq!(app.read(cx).diff.read(cx).files()[0].path(), &diff_path);
                assert!(!app.read(cx).patch_loading);
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
    }
}
