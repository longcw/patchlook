use crate::{file_icons::APP_ICON, strings::tr};
use gpui_kit::{
    assets::IconName,
    base::{
        Dialog, DialogTitle, SelectableText, TestSupportExt, TextSelection,
        motion::{Presence, Transition},
    },
    component::{
        ActiveTheme, Sizable,
        button::{Button, ButtonVariants},
        h_flex, v_flex,
    },
    *,
};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

#[derive(Clone, Copy, PartialEq, Eq)]
enum Visibility {
    Hidden,
    Open,
    Closing,
}

/// A window-owned dialog keeps its focus trap and backdrop until its exit finishes.
pub struct AboutPanel {
    visibility: Visibility,
    license: bool,
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
}

impl AboutPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            visibility: Visibility::Hidden,
            license: false,
            focus: cx.focus_handle(),
            previous_focus: None,
        }
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.visibility == Visibility::Open {
            return;
        }
        if self.visibility == Visibility::Hidden {
            self.previous_focus = window.focused(cx);
        }
        self.visibility = Visibility::Open;
        self.license = false;
        TextSelection::clear(window, cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        TextSelection::clear(window, cx);
        if self.license {
            self.license = false;
            window.focus(&self.focus, cx);
        } else {
            self.visibility = Visibility::Closing;
        }
        cx.notify();
    }

    fn overview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("about-content")
            .test_support()
            .items_center()
            .gap_4()
            .py_2()
            .child(img(APP_ICON).size(px(88.)).flex_none())
            .child(
                v_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr("app_name")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                tr("about_version").replace("{version}", env!("CARGO_PKG_VERSION")),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .text_sm()
                    .text_center()
                    .child(tr("about_description")),
            )
            .child(
                Button::new("about-license")
                    .label(format!(
                        "{} · {}",
                        tr("open_source_license"),
                        env!("CARGO_PKG_LICENSE")
                    ))
                    .icon(IconName::FileText)
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.license = true;
                        TextSelection::clear(window, cx);
                        window.focus(&this.focus, cx);
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .justify_center()
                    .gap_2()
                    .child(
                        Button::new("about-github")
                            .label("GitHub")
                            .icon(IconName::Github)
                            .small()
                            .on_click(|_, _, cx| cx.open_url(REPOSITORY)),
                    )
                    .child(
                        Button::new("about-issues")
                            .label(tr("report_issue"))
                            .icon(IconName::ExternalLink)
                            .small()
                            .ghost()
                            .on_click(|_, _, cx| {
                                cx.open_url(concat!(env!("CARGO_PKG_REPOSITORY"), "/issues"))
                            }),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr("about_built_with")),
            )
    }
}

impl Render for AboutPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.visibility == Visibility::Hidden {
            return div().into_any_element();
        }
        let open = self.visibility == Visibility::Open;
        let motion = cx.theme().motion_tokens();
        let (duration, easing) = if open {
            (motion.duration_normal, motion.easing_enter.clone())
        } else {
            (motion.duration_fast, motion.easing_exit.clone())
        };
        let presence = Presence::new(ElementId::from(("about-presence", cx.entity_id())), open)
            .transition(Transition::new(duration).easing(easing))
            .sample(window, cx);
        if !presence.should_render() {
            self.visibility = Visibility::Hidden;
            if let Some(focus) = self.previous_focus.take() {
                window.defer(cx, move |window, cx| window.focus(&focus, cx));
            }
            return div().into_any_element();
        }

        let margin = cx.theme().spacing_tokens().lg;
        let viewport = window.viewport_size();
        let title = if self.license {
            format!(
                "{} · {}",
                tr("open_source_license"),
                env!("CARGO_PKG_LICENSE")
            )
        } else {
            tr("about").to_owned()
        };
        let body = if self.license {
            div()
                .id("app-license-text")
                .test_support()
                .text_sm()
                .child(SelectableText::new(
                    "app-license",
                    include_str!("../LICENSE"),
                ))
                .into_any_element()
        } else {
            self.overview(cx).into_any_element()
        };

        Dialog::new(cx)
            .focus_handle(self.focus.clone())
            .on_close(cx.listener(|this, _, window, cx| this.cancel(window, cx)))
            .backdrop(
                div()
                    .size_full()
                    .bg(cx.theme().overlay)
                    .opacity(presence.progress),
            )
            .popup(
                v_flex()
                    .id("about-surface")
                    .test_support()
                    .occlude()
                    .w(px(if self.license { 600. } else { 448. }))
                    .max_w((viewport.width - margin * 2.).max(px(0.)))
                    .max_h((viewport.height - margin * 2.).max(px(0.)))
                    .p_4()
                    .gap_4()
                    .bg(cx.theme().tokens.background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius_lg)
                    .opacity(presence.progress)
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_2()
                            .child(
                                DialogTitle::new()
                                    .flex_1()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(
                                Button::new("about-close")
                                    .icon(IconName::X)
                                    .xsmall()
                                    .ghost()
                                    .tooltip(tr("dismiss"))
                                    .accessibility_label(tr("dismiss"))
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.cancel(window, cx)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("about-scroll")
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(body),
                    ),
            )
            .into_any_element()
    }
}
