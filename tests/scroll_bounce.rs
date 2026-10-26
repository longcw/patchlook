use gpui_kit::{
    AppContext, Context, InputEvent, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollDelta, ScrollHandle, ScrollWheelEvent, StatefulInteractiveElement, Styled,
    TestAppContext, TouchPhase, Window, WindowOptions, base::ScrollBounce, div, point, px,
    test::TestWindowExt,
};
use std::time::Duration;

struct Viewport(ScrollHandle);
impl Render for Viewport {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ScrollBounce::new(
            "bounce",
            &self.0,
            div()
                .id("viewport")
                .w(px(400.))
                .h(px(240.))
                .overflow_y_scroll()
                .track_scroll(&self.0)
                .child(div().w_full().h(px(12000.))),
        )
        .enabled(true)
    }
}

fn packet(window: &mut Window, cx: &mut gpui_kit::App, dy: f32, phase: TouchPhase) {
    window.dispatch_event(
        ScrollWheelEvent {
            position: point(px(100.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            touch_phase: phase,
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    );
}

#[gpui_kit::test]
fn releasing_a_trackpad_in_the_middle_never_displaces_the_viewport(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let scroll = ScrollHandle::new();
    let (handle, _) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|_| Viewport(scroll.clone()))
        })
        .unwrap()
    });
    let origin = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            scroll.set_offset(point(px(0.), px(-500.)));
            window.render_frame(cx);
            let origin = scroll.bounds().origin;
            for (dy, phase) in [
                (-30., TouchPhase::Started),
                (-50., TouchPhase::Moved),
                (0., TouchPhase::Ended),
            ] {
                packet(window, cx, dy, phase);
            }
            window.render_frame(cx);
            origin
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(scroll.offset().y, px(-580.));
        assert_eq!(
            scroll.bounds().origin,
            origin,
            "releasing inside the content must not start an edge spring"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn fractional_precise_scroll_does_not_mistake_roundoff_for_an_edge(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let scroll = ScrollHandle::new();
    let (handle, _) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|_| Viewport(scroll.clone()))
        })
        .unwrap()
    });
    let origin = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            scroll.set_offset(point(px(0.), px(-4000.125)));
            window.render_frame(cx);
            let origin = scroll.bounds().origin;
            packet(window, cx, -33.3, TouchPhase::Moved);
            window.render_frame(cx);
            origin
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(scroll.offset().y < px(-4000.) && scroll.offset().y > px(-4100.));
        assert_eq!(
            scroll.bounds().origin,
            origin,
            "fractional deltas inside the content are fully consumed"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn real_top_and_bottom_edges_still_bounce_and_return(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    for direction in [1., -1.] {
        let scroll = ScrollHandle::new();
        let (handle, _) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|_| Viewport(scroll.clone()))
            })
            .unwrap()
        });
        let (origin, edge) = cx
            .update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                let edge = if direction > 0. {
                    px(0.)
                } else {
                    -scroll.max_offset().y
                };
                scroll.set_offset(point(px(0.), edge));
                window.render_frame(cx);
                let origin = scroll.bounds().origin;
                packet(window, cx, direction * 100., TouchPhase::Started);
                window.render_frame(cx);
                assert!((scroll.bounds().origin.y - origin.y) * direction > px(0.));
                assert_eq!(scroll.offset().y, edge);
                packet(window, cx, 0., TouchPhase::Ended);
                window.render_frame(cx);
                (origin, edge)
            })
            .unwrap();
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!((scroll.bounds().origin.y - origin.y) * direction > px(0.));
            assert_eq!(scroll.offset().y, edge);
        })
        .unwrap();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(scroll.bounds().origin, origin);
            assert_eq!(scroll.offset().y, edge);
        })
        .unwrap();
    }
}
