use crate::{
    image_diff::image_format,
    strings::{area_label, tr},
};
use diff_tool::git::{Area, Change, DiffStats};
use gpui_kit::{
    assets::IconName,
    base::{TestSupportExt, Tree, TreeItem, TreeState},
    component::{
        ActiveTheme, Icon, Sizable, h_flex,
        scroll::{Scrollbar, ScrollbarMode},
        tooltip::Tooltip,
    },
    prelude::FluentBuilder,
    *,
};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::Path,
    rc::Rc,
};

struct Node {
    file_index: Option<usize>,
    file_count: usize,
    status: char,
    icon: IconName,
    path: SharedString,
    stats: Option<DiffStats>,
    area: Option<Area>,
}

pub struct ChangeTree {
    state: Entity<TreeState>,
    nodes: Rc<HashMap<SharedString, Node>>,
    _subscription: Subscription,
}

#[derive(Default)]
struct Directory {
    directories: BTreeMap<OsString, Directory>,
    files: Vec<usize>,
}

impl Directory {
    fn from_changes(changes: &[Change]) -> Self {
        let mut root = Self::default();
        for (index, change) in changes.iter().enumerate() {
            let mut directory = &mut root;
            if let Some(parent) = change.path.parent() {
                for segment in parent.components() {
                    directory = directory
                        .directories
                        .entry(segment.as_os_str().to_owned())
                        .or_default();
                }
            }
            directory.files.push(index);
        }
        root
    }

    fn into_item(
        mut self,
        mut label: String,
        path: &Path,
        root: bool,
        changes: &[Change],
        stats: &[Option<DiffStats>],
        nodes: &mut HashMap<SharedString, Node>,
    ) -> TreeItem {
        let mut full_path = path.to_path_buf();
        // Compact only linear directory chains. Keep the repository root visible.
        while !root && self.files.is_empty() && self.directories.len() == 1 {
            let (name, child) = self.directories.pop_first().unwrap();
            label.push('/');
            label.push_str(&name.to_string_lossy());
            full_path.push(name);
            self = child;
        }
        let mut children = Vec::new();
        for (name, directory) in self.directories {
            children.push(directory.into_item(
                name.to_string_lossy().into_owned(),
                &full_path.join(&name),
                false,
                changes,
                stats,
                nodes,
            ));
        }
        self.files.sort_by(|a, b| {
            changes[*a]
                .path
                .file_name()
                .cmp(&changes[*b].path.file_name())
        });
        for index in self.files {
            let change = &changes[index];
            let id = file_id(index);
            nodes.insert(
                id.clone(),
                Node {
                    file_index: Some(index),
                    file_count: 1,
                    status: change.status,
                    icon: if image_format(&change.path).is_some() {
                        IconName::Image
                    } else {
                        IconName::FileText
                    },
                    path: change.path.display().to_string().into(),
                    stats: stats[index],
                    area: Some(change.area),
                },
            );
            children.push(TreeItem::new(
                id,
                change
                    .path
                    .file_name()
                    .unwrap_or(change.path.as_os_str())
                    .to_string_lossy()
                    .into_owned(),
            ));
        }
        let count = children
            .iter()
            .map(|child| nodes[&child.id].file_count)
            .sum();
        let stats = children
            .iter()
            .filter_map(|child| nodes[&child.id].stats)
            .sum();
        let id: SharedString = if root {
            "tree-root".into()
        } else {
            format!("tree-directory-{}", nodes.len()).into()
        };
        nodes.insert(
            id.clone(),
            Node {
                file_index: None,
                file_count: count,
                status: ' ',
                icon: IconName::Folder,
                path: full_path.display().to_string().into(),
                stats: Some(stats),
                area: None,
            },
        );
        TreeItem::new(id, label).children(children).expanded(true)
    }
}

fn file_id(index: usize) -> SharedString {
    format!("tree-file-{index}").into()
}

fn has_collapsed_directory(item: &TreeItem) -> bool {
    item.is_folder() && (!item.is_expanded() || item.children.iter().any(has_collapsed_directory))
}

fn set_directories_expanded(mut item: TreeItem, expanded: bool) -> TreeItem {
    if item.is_folder() {
        item.children = item
            .children
            .into_iter()
            .map(|child| set_directories_expanded(child, expanded))
            .collect();
        item = item.expanded(expanded);
    }
    item
}

pub fn stats_label(stats: Option<DiffStats>, cx: &App) -> impl IntoElement {
    h_flex()
        .id("file-diff-stats")
        .test_support()
        .flex_none()
        .gap_2()
        .text_xs()
        .role(accesskit::Role::Label)
        .map(|row| match stats {
            Some(stats) => row
                .aria_label(format!("+{} −{}", stats.additions, stats.deletions))
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
            None => row
                .aria_label(tr("binary_file"))
                .text_color(cx.theme().muted_foreground)
                .child(tr("binary_file")),
        })
}

impl ChangeTree {
    pub fn new<T: 'static>(
        changes: &[Change],
        stats: &[Option<DiffStats>],
        repository: &Path,
        cx: &mut Context<T>,
    ) -> Self {
        let mut nodes = HashMap::new();
        let root = Directory::from_changes(changes).into_item(
            repository
                .file_name()
                .unwrap_or(repository.as_os_str())
                .to_string_lossy()
                .into_owned(),
            Path::new(""),
            true,
            changes,
            stats,
            &mut nodes,
        );
        nodes.get_mut(&root.id).unwrap().path = repository.display().to_string().into();
        let state = cx.new(|cx| TreeState::new(cx).items(vec![root]));
        let subscription = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            nodes: Rc::new(nodes),
            _subscription: subscription,
        }
    }

    pub fn has_collapsed_directories(&self, cx: &App) -> bool {
        self.state
            .read(cx)
            .entry(0)
            .is_some_and(|root| has_collapsed_directory(root.item()))
    }

    pub fn toggle_directories(&self, cx: &mut App) {
        let expand = self.has_collapsed_directories(cx);
        self.state.update(cx, |state, cx| {
            let Some(root) = state.entry(0).map(|entry| entry.item().clone()) else {
                return;
            };
            let selected = state.selected_item().map(|item| item.id.clone());
            state.set_items(vec![set_directories_expanded(root, expand)], cx);
            state.set_selected_index(selected.and_then(|id| state.index_of(&id)), cx);
            state.scroll_to_item(0, ScrollStrategy::Top);
        });
    }

    pub fn reveal_file(&self, index: usize, cx: &mut App) {
        let id = file_id(index);
        self.state.update(cx, |state, cx| {
            state.reveal_item(&id, ScrollStrategy::Center, cx);
            state.set_selected_index(state.index_of(&id), cx);
        });
    }

    pub fn render(
        &self,
        branch: String,
        show_stats: bool,
        working: bool,
        cx: &App,
        on_file: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let nodes = self.nodes.clone();
        let on_file = Rc::new(on_file);
        let scroll = self
            .state
            .read_with(cx, |state, _| state.scroll_handle().clone());
        div()
            .relative()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(
                Tree::new(&self.state)
                    .size_full()
                    .list_style(StyleRefinement::default().size_full())
                    .item(move |_, entry, state, _, cx| {
                        let item = entry.item();
                        let node = &nodes[&item.id];
                        let on_file = on_file.clone();
                        let file_index = node.file_index;
                        let path = node.path.clone();
                        let status_color = match node.status {
                            'A' => cx.theme().success,
                            'D' => cx.theme().danger,
                            _ => cx.theme().warning,
                        };
                        h_flex()
                            .id(item.id.clone())
                            .test_support()
                            .h(px(28.))
                            .w_full()
                            .min_w_0()
                            .pr_2()
                            .pl(px(8. + entry.depth() as f32 * 18.))
                            .gap_2()
                            .text_sm()
                            .cursor_pointer()
                            .tooltip(move |window, cx| Tooltip::new(path.clone()).build(window, cx))
                            .when(state.is_selected(), |row| {
                                row.bg(cx.theme().secondary_active)
                            })
                            .hover(|style| {
                                style.bg(if state.is_selected() {
                                    cx.theme().secondary_active
                                } else {
                                    cx.theme().list_hover
                                })
                            })
                            .child(
                                h_flex()
                                    .w(px(12.))
                                    .flex_none()
                                    .justify_center()
                                    .text_xs()
                                    .text_color(status_color)
                                    .when(entry.is_folder(), |slot| {
                                        slot.child(
                                            Icon::new(if entry.is_expanded() {
                                                IconName::ChevronDown
                                            } else {
                                                IconName::ChevronRight
                                            })
                                            .xsmall()
                                            .text_color(cx.theme().muted_foreground),
                                        )
                                    })
                                    .when(file_index.is_some(), |slot| {
                                        slot.child(node.status.to_string())
                                    }),
                            )
                            .child(
                                Icon::new(node.icon)
                                    .small()
                                    .flex_none()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(div().min_w_0().truncate().child(item.label.clone()))
                            .when(file_index.is_none(), |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(
                                            tr("tree_file_count")
                                                .replace("{count}", &node.file_count.to_string()),
                                        ),
                                )
                            })
                            .when(entry.is_root(), |row| {
                                row.child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(branch.clone()),
                                )
                            })
                            .when(working && file_index.is_some(), |row| {
                                row.child(div().flex_1()).child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(area_label(node.area.unwrap())),
                                )
                            })
                            .when(
                                show_stats && (file_index.is_some() || !entry.is_expanded()),
                                |row| {
                                    row.when(!(working && file_index.is_some()), |row| {
                                        row.child(div().flex_1())
                                    })
                                    .child(stats_label(node.stats, cx))
                                },
                            )
                            .when_some(file_index, |row, index| {
                                row.on_click(move |_, window, cx| on_file(index, window, cx))
                            })
                            .into_any_element()
                    }),
            )
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::vertical(&scroll)
                        .mode(ScrollbarMode::Scrolling)
                        .viewport_from_layout()
                        .styles(|styles| {
                            styles
                                .track(|track| track.width(px(6.)))
                                .thumb(|thumb| thumb.width(px(6.)).inset(px(0.)).radius(px(2.)))
                                .thumb_hover(|thumb| thumb.width(px(6.)).inset(px(0.)))
                                .thumb_active(|thumb| thumb.width(px(6.)).inset(px(0.)))
                        }),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use diff_tool::git::Area;
    use gpui_kit::test::TestWindowExt;
    use std::cell::Cell;

    fn changes() -> Vec<Change> {
        [
            "README.md",
            "crates/model/src/specs/a.rs",
            "crates/model/src/specs/b.rs",
            ".codex/project-state.md",
            "docs/guides/setup.md",
            "docs/index.md",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, path)| Change {
            path: path.into(),
            previous_path: None,
            status: if index == 1 { 'D' } else { 'M' },
            area: Area::Staged,
        })
        .collect()
    }

    fn stats() -> Vec<Option<DiffStats>> {
        (0..changes().len())
            .map(|index| {
                (index != 5).then_some(DiffStats {
                    additions: index + 1,
                    deletions: index,
                })
            })
            .collect()
    }

    #[test]
    fn directories_are_compacted_counted_and_sorted_without_losing_file_indices() {
        let changes = changes();
        let mut nodes = HashMap::new();
        let root = Directory::from_changes(&changes).into_item(
            "repository".into(),
            Path::new(""),
            true,
            &changes,
            &stats(),
            &mut nodes,
        );
        assert_eq!(root.label.as_str(), "repository");
        assert_eq!(nodes[&root.id].file_count, 6);
        assert_eq!(
            nodes[&root.id].stats,
            Some(DiffStats {
                additions: 15,
                deletions: 10
            })
        );
        assert_eq!(
            root.children
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            [".codex", "crates/model/src/specs", "docs", "README.md"]
        );
        let compact = &root.children[1];
        assert_eq!(nodes[&compact.id].file_count, 2);
        assert_eq!(
            nodes[&compact.id].stats,
            Some(DiffStats {
                additions: 5,
                deletions: 3
            })
        );
        assert_eq!(nodes[&compact.children[0].id].file_index, Some(1));
        assert_eq!(nodes[&compact.children[0].id].status, 'D');
        assert_eq!(root.children[2].children[0].label.as_str(), "guides");
        assert_eq!(root.children[2].children[1].label.as_str(), "index.md");
        for (index, change) in changes.iter().enumerate() {
            assert_eq!(
                nodes[&file_id(index)].path.as_str(),
                change.path.to_str().unwrap()
            );
        }
    }

    #[test]
    fn same_path_in_index_and_worktree_keeps_separate_stats_and_selection_ids() {
        let changes = [Area::Staged, Area::Unstaged].map(|area| Change {
            path: "src/shared.rs".into(),
            previous_path: None,
            status: 'M',
            area,
        });
        let stats = [
            Some(DiffStats {
                additions: 3,
                deletions: 1,
            }),
            Some(DiffStats {
                additions: 7,
                deletions: 2,
            }),
        ];
        let mut nodes = HashMap::new();
        let root = Directory::from_changes(&changes).into_item(
            "repository".into(),
            Path::new(""),
            true,
            &changes,
            &stats,
            &mut nodes,
        );
        assert_eq!(root.children[0].children.len(), 2);
        assert_eq!(
            nodes[&root.id].stats,
            Some(DiffStats {
                additions: 10,
                deletions: 3
            })
        );
        for index in 0..2 {
            let node = &nodes[&file_id(index)];
            assert_eq!(node.file_index, Some(index));
            assert_eq!(node.area, Some(changes[index].area));
            assert_eq!(node.stats, stats[index]);
        }
    }

    struct Viewer {
        tree: ChangeTree,
        show_stats: bool,
        opened: Rc<Cell<Option<usize>>>,
    }
    impl Render for Viewer {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let opened = self.opened.clone();
            self.tree.render(
                "main".into(),
                self.show_stats,
                false,
                cx,
                move |index, _, _| opened.set(Some(index)),
            )
        }
    }

    #[gpui_kit::test]
    fn folder_clicks_fold_and_leaf_clicks_open_the_original_file(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, viewer) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| Viewer {
                    tree: ChangeTree::new(&changes(), &stats(), Path::new("repository"), cx),
                    show_stats: false,
                    opened: Rc::new(Cell::new(None)),
                })
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            let folder = viewer
                .read(cx)
                .tree
                .nodes
                .iter()
                .find(|(_, node)| {
                    node.file_index.is_none() && node.path.as_str() == "crates/model/src/specs"
                })
                .unwrap()
                .0
                .clone();
            assert!(window.try_find(file_id(1)).is_some());
            window.click(folder.clone(), cx);
            assert!(window.try_find(file_id(1)).is_none());
            assert_eq!(viewer.read(cx).opened.get(), None);
            window.click(folder.clone(), cx);
            window.click(file_id(2), cx);
            assert_eq!(viewer.read(cx).opened.get(), Some(2));
            window.click(folder, cx);
            viewer.update(cx, |viewer, cx| viewer.tree.reveal_file(1, cx));
            window.render_frame(cx);
            assert!(window.try_find(file_id(1)).is_some());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn directory_toggle_and_stats_follow_visible_tree_state(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, viewer) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| Viewer {
                    tree: ChangeTree::new(&changes(), &stats(), Path::new("repository"), cx),
                    show_stats: false,
                    opened: Rc::new(Cell::new(None)),
                })
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            assert!(window.find_all("file-diff-stats").is_empty());
            viewer.update(cx, |viewer, cx| {
                viewer.show_stats = true;
                cx.notify();
            });
            window.render_frame(cx);
            assert_eq!(window.find_all("file-diff-stats").len(), changes().len());
            assert_eq!(
                window.within(file_id(1)).find("file-diff-stats").label(),
                Some("+2 −1")
            );
            assert_eq!(
                window.within(file_id(5)).find("file-diff-stats").label(),
                Some(tr("binary_file"))
            );
            let folder = viewer
                .read(cx)
                .tree
                .nodes
                .iter()
                .find(|(_, node)| node.path.as_str() == "crates/model/src/specs")
                .unwrap()
                .0
                .clone();
            window.click(folder.clone(), cx);
            window.render_frame(cx);
            assert_eq!(
                window.within(folder).find("file-diff-stats").label(),
                Some("+5 −3")
            );
            assert!(viewer.read(cx).tree.has_collapsed_directories(cx));

            viewer.update(cx, |viewer, cx| viewer.tree.toggle_directories(cx));
            window.render_frame(cx);
            assert!(!viewer.read(cx).tree.has_collapsed_directories(cx));
            assert!(window.try_find(file_id(1)).is_some());
            viewer.update(cx, |viewer, cx| viewer.tree.toggle_directories(cx));
            window.render_frame(cx);
            assert!(viewer.read(cx).tree.has_collapsed_directories(cx));
            assert!(window.try_find(file_id(0)).is_none());
            assert_eq!(window.find_all("file-diff-stats").len(), 1);
            assert_eq!(
                window.within("tree-root").find("file-diff-stats").label(),
                Some("+15 −10")
            );

            // Opening only the root after collapse-all leaves its children folded.
            window.click("tree-root", cx);
            window.render_frame(cx);
            assert!(window.try_find(file_id(0)).is_some());
            assert!(window.try_find(file_id(1)).is_none());
            assert!(viewer.read(cx).tree.has_collapsed_directories(cx));
            viewer.update(cx, |viewer, cx| {
                viewer.show_stats = false;
                cx.notify();
            });
            window.render_frame(cx);
            assert!(window.find_all("file-diff-stats").is_empty());
            viewer.update(cx, |viewer, cx| viewer.tree.toggle_directories(cx));
            window.render_frame(cx);
            window.click(file_id(2), cx);
            assert_eq!(viewer.read(cx).opened.get(), Some(2));
        })
        .unwrap();
    }
}
