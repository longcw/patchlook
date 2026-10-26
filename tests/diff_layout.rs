use gpui_kit::component::diff::{
    Diff, DiffChangeIndicator, DiffFile, DiffHunkSeparator, DiffMode, DiffState,
};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext, Bounds, Context, Entity, IntoElement, Point, Render, ScrollDelta, Styled,
    TestAppContext, Window, WindowBounds, WindowOptions, point, px, size,
};

struct Viewer {
    state: Entity<DiffState>,
    wrap: bool,
}

impl Render for Viewer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Diff::new(&self.state)
            .header_visible(false)
            .change_indicator(DiffChangeIndicator::Bars)
            .soft_wrap(self.wrap)
            .hunk_separator(DiffHunkSeparator::Wave)
            .size_full()
    }
}

#[gpui_kit::test]
fn long_lines_keep_equal_panes_when_wrapping_scrolling_and_resizing(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let left = format!("LEFT {}", "abcdefgh你好 ".repeat(100));
    let right = format!("RIGHT {}", "0123456789世界 ".repeat(130));
    let patch = format!("--- a/test.rs\n+++ b/test.rs\n@@ -1 +1 @@\n-{left}\n+{right}\n");
    let (handle, viewer) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1000.), px(700.)),
                })),
                ..Default::default()
            },
            cx,
            |_, cx| {
                cx.new(|cx| Viewer {
                    state: cx.new(|cx| {
                        DiffState::new(DiffFile::parse(&patch).unwrap(), cx)
                            .with_mode(DiffMode::Split)
                    }),
                    wrap: false,
                })
            },
        )
        .unwrap()
    });
    let inspect = |window: &mut Window, cx: &mut gpui_kit::App| {
        // First frame records the viewport; the second lays out its code columns.
        window.render_frame(cx);
        window.render_frame(cx);
        let cells = window.find_all("source");
        let left = cells
            .iter()
            .find(|cell| cell.label().is_some_and(|label| label.starts_with("LEFT")))
            .unwrap()
            .bounds();
        let right = cells
            .iter()
            .find(|cell| cell.label().is_some_and(|label| label.starts_with("RIGHT")))
            .unwrap()
            .bounds();
        let viewport = window.find("diff-body").bounds();
        let divider = window.find("diff-pane-divider").bounds();
        assert_eq!(divider.top(), viewport.top());
        assert_eq!(divider.bottom(), viewport.bottom());
        assert!((divider.right() - viewport.center().x).abs() <= px(1.));
        assert!((left.size.width - right.size.width).abs() <= px(2.));
        assert!((right.left() - left.left() - viewport.size.width / 2.).abs() <= px(2.));
        assert!(left.right() <= viewport.center().x);
        assert!(right.right() <= viewport.right());
        (left, right)
    };
    cx.update_window(handle, |_, window, cx| {
        let before = inspect(window, cx);
        window.within(("new", 0_usize)).click_with_options(
            "source",
            gpui_kit::test::ClickOptions::new()
                .with_count(2)
                .with_offset(point(px(10.), px(10.))),
            cx,
        );
        window.press("secondary-c", cx);
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("RIGHT"),
            "text clipped out of the left pane must not intercept selection in the right pane"
        );
        let text_before = window.find_all("code-content");
        window.scroll(
            "diff-body",
            ScrollDelta::Pixels(point(px(-250.), px(0.))),
            cx,
        );
        let after = inspect(window, cx);
        let text_after = window.find_all("code-content");
        assert_eq!(text_before.len(), text_after.len());
        let left_shift = text_after[0].bounds().left() - text_before[0].bounds().left();
        let right_shift = text_after[1].bounds().left() - text_before[1].bounds().left();
        assert!(left_shift < px(0.), "the text must actually scroll");
        assert_eq!(
            left_shift, right_shift,
            "both sides scroll by the same offset"
        );
        assert_eq!(
            before, after,
            "horizontal scrolling must not move the pane boundaries"
        );
        viewer.update(cx, |viewer, cx| {
            viewer.wrap = true;
            cx.notify();
        });
        let wrapped = inspect(window, cx);
        assert!(
            wrapped.0.size.height > before.0.size.height,
            "long code must wrap to multiple visual lines"
        );
    })
    .unwrap();
    cx.simulate_window_resize(handle, size(px(700.), px(700.)));
    cx.update_window(handle, |_, window, cx| {
        inspect(window, cx);
        let state = viewer.read(cx).state.clone();
        state.update(cx, |state, cx| state.set_mode(DiffMode::Unified, cx));
        window.render_frame(cx);
        assert!(window.try_find("diff-pane-divider").is_none());
        state.update(cx, |state, cx| {
            state.set_mode(DiffMode::Split, cx);
            state.set_file_collapsed("test.rs", true, cx);
        });
        window.render_frame(cx);
        assert!(window.try_find("diff-pane-divider").is_none());
        state.update(cx, |state, cx| {
            state.set_file_collapsed("test.rs", false, cx)
        });
        inspect(window, cx);
        state.update(cx, |state, cx| state.set_files([], cx));
        window.render_frame(cx);
        assert!(window.try_find("diff-pane-divider").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn wave_disclosure_expands_hidden_context(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let patch = format!(
        "--- a/test.rs\n+++ b/test.rs\n@@ -1,22 +1,22 @@\n-before\n+after\n{}-last\n+changed\n",
        (0..20)
            .map(|i| format!(" context_{i}\n"))
            .collect::<String>()
    );
    let (handle, viewer) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1000.), px(900.)),
                })),
                ..Default::default()
            },
            cx,
            |_, cx| {
                cx.new(|cx| Viewer {
                    state: cx.new(|cx| {
                        DiffState::new(DiffFile::parse(&patch).unwrap(), cx)
                            .with_mode(DiffMode::Split)
                    }),
                    wrap: true,
                })
            },
        )
        .unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(
            !window
                .find_all("source")
                .iter()
                .any(|cell| cell.label() == Some("context_10"))
        );
        let state = viewer.read(cx).state.clone();
        assert!(state.read(cx).has_collapsed_context());
        assert_eq!(state.read(cx).files()[0].change_block_count(), 2);
        state.update(cx, |state, cx| state.expand_unchanged(cx));
        window.render_frame(cx);
        assert!(!state.read(cx).has_collapsed_context());
        assert_eq!(state.read(cx).files()[0].change_block_count(), 2);
        assert!(
            window
                .find_all("source")
                .iter()
                .any(|cell| cell.label() == Some("context_10"))
        );
        state.update(cx, |state, cx| state.collapse_unchanged(cx));
        window.render_frame(cx);
        assert!(state.read(cx).has_collapsed_context());
        assert_eq!(state.read(cx).files()[0].change_block_count(), 2);
        assert!(
            !window
                .find_all("source")
                .iter()
                .any(|cell| cell.label() == Some("context_10"))
        );
        // Three unchanged context lines remain visible next to each changed row.
        window.click(("expand", 4_usize), cx);
        window.render_frame(cx);
        assert!(
            window
                .find_all("source")
                .iter()
                .any(|cell| cell.label() == Some("context_10"))
        );
        window
            .within(("new", 4_usize))
            .click(("collapse-context", 4_usize), cx);
        window.render_frame(cx);
        assert!(state.read(cx).has_collapsed_context());
        assert!(state.read(cx).selected_lines().is_none());
        assert!(window.try_find(("expand", 4_usize)).is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn context_arrows_collapse_only_their_segment_in_both_modes(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let context = |prefix| {
        (0..10)
            .map(|i| format!(" {prefix}_{i}\n"))
            .collect::<String>()
    };
    let patch = format!(
        "--- a/test.rs\n+++ b/test.rs\n@@ -1,23 +1,23 @@\n-first\n+first_new\n{}-middle\n+middle_new\n{}-last\n+last_new\n",
        context("a"),
        context("b")
    );
    let (handle, viewer) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1000.), px(1000.)),
                })),
                ..Default::default()
            },
            cx,
            |_, cx| {
                cx.new(|cx| Viewer {
                    state: cx.new(|cx| DiffState::new(DiffFile::parse(&patch).unwrap(), cx)),
                    wrap: true,
                })
            },
        )
        .unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
        let state = viewer.read(cx).state.clone();
        for mode in [DiffMode::Split, DiffMode::Unified] {
            state.update(cx, |state, cx| {
                state.collapse_unchanged(cx);
                state.set_mode(mode, cx);
            });
            window.render_frame(cx);
            window.render_frame(cx);
            window.click(("expand", 4_usize), cx);
            window.click(("expand", 15_usize), cx);
            window.render_frame(cx);
            assert!(!state.read(cx).has_collapsed_context());
            let arrows = window.find_all(("collapse-context", 4_usize));
            assert_eq!(arrows.len(), if mode == DiffMode::Split { 2 } else { 1 });
            let row = window.within(("new", 4_usize));
            let source_left = row.find("source").bounds().left();
            let arrow = row.find(("collapse-context", 4_usize)).bounds();
            assert!(arrow.right() < source_left);
            assert_eq!(
                arrow.top(),
                row.find_all(("line", 4_usize))[0].bounds().top()
            );
            let side = if mode == DiffMode::Split {
                "old"
            } else {
                "new"
            };
            window
                .within((side, 4_usize))
                .click(("collapse-context", 4_usize), cx);
            window.render_frame(cx);
            assert!(window.try_find(("expand", 4_usize)).is_some());
            assert!(
                window
                    .find_all("source")
                    .iter()
                    .any(|cell| cell.label() == Some("b_4"))
            );
            assert!(
                !window
                    .find_all("source")
                    .iter()
                    .any(|cell| cell.label() == Some("a_4"))
            );
            assert!(state.read(cx).selected_lines().is_none());
            window.click(("expand", 4_usize), cx);
            window.render_frame(cx);
            assert_eq!(
                window
                    .within(("new", 4_usize))
                    .find("source")
                    .bounds()
                    .left(),
                source_left
            );

            // A global disclosure is still independently collapsible per segment.
            state.update(cx, |state, cx| {
                state.collapse_unchanged(cx);
                state.expand_unchanged(cx);
            });
            window.render_frame(cx);
            window
                .within(("new", 15_usize))
                .click(("collapse-context", 15_usize), cx);
            window.render_frame(cx);
            assert!(window.try_find(("expand", 15_usize)).is_some());
            assert!(
                window
                    .find_all("source")
                    .iter()
                    .any(|cell| cell.label() == Some("a_4"))
            );
            assert!(
                !window
                    .find_all("source")
                    .iter()
                    .any(|cell| cell.label() == Some("b_4"))
            );
        }
    })
    .unwrap();
}

#[test]
fn change_blocks_group_adjacent_replacements_and_separate_context_and_hunks() {
    let file = DiffFile::parse(
        "--- a/test.rs\n+++ b/test.rs\n@@ -1,3 +1,5 @@\n-old1\n-old2\n+new1\n+new2\n+new3\n context\n+extra\n@@ -20 +22,0 @@\n-deleted\n",
    ).unwrap().remove(0);
    assert_eq!(file.change_block_count(), 3);
}

#[gpui_kit::test]
fn text_diff_stays_still_after_an_interior_trackpad_release(cx: &mut TestAppContext) {
    use gpui_kit::{InputEvent, ScrollWheelEvent, TouchPhase};
    use std::time::Duration;
    cx.update(gpui_kit::init);
    for mode in [DiffMode::Split, DiffMode::Unified] {
        let mut patch = "--- /dev/null\n+++ b/long.txt\n@@ -0,0 +1,300 @@\n".to_owned();
        for index in 0..300 {
            patch.push_str(&format!("+line {index} {}\n", "wrapped text ".repeat(15)));
        }
        let (handle, _) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Point::default(),
                        size(px(900.), px(500.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |_, cx| {
                    cx.new(|cx| Viewer {
                        state: cx.new(|cx| {
                            DiffState::new(DiffFile::parse(&patch).unwrap(), cx).with_mode(mode)
                        }),
                        wrap: true,
                    })
                },
            )
            .unwrap()
        });
        for direction in [-1., 1.] {
            let (label, baseline) = cx
                .update_window(handle, |_, window, cx| {
                    window.render_frame(cx);
                    window.render_frame(cx);
                    let position = window.find("diff-body").bounds().center();
                    // Ordinary wheel input first places us well inside the diff.
                    window.scroll("diff-body", ScrollDelta::Lines(point(0., -25.)), cx);
                    for (dy, phase) in [
                        (30.3 * direction, TouchPhase::Started),
                        (50.2 * direction, TouchPhase::Moved),
                        (0., TouchPhase::Ended),
                    ] {
                        window.dispatch_event(
                            ScrollWheelEvent {
                                position,
                                delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                                touch_phase: phase,
                                ..Default::default()
                            }
                            .to_platform_input(),
                            cx,
                        );
                    }
                    window.render_frame(cx);
                    let rows = window.find_all("source");
                    let row = &rows[rows.len() / 2];
                    (row.label().unwrap().to_owned(), row.bounds())
                })
                .unwrap();
            cx.executor().advance_clock(Duration::from_millis(16));
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                let rows = window.find_all("source");
                let row = rows
                    .iter()
                    .find(|row| row.label() == Some(label.as_str()))
                    .unwrap();
                assert_eq!(
                    row.bounds(),
                    baseline,
                    "the code must not move after releasing away from either edge"
                );
            })
            .unwrap();
        }
    }
}
