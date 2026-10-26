use crate::strings::tr;
use anyhow::{Context as _, Result};
use diff_tool::git::{Change, FileVersions};
use gpui_kit::{
    assets::IconName,
    base::{ElementExt, TestSupportExt},
    component::{
        ActiveTheme, Disableable, Selectable, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        scroll::{Scrollbar, ScrollbarMode},
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};
use std::{path::Path, sync::Arc};

const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 8.;
const ZOOM_STEP: f32 = 1.25;
const IMAGE_PADDING: f32 = 16.;

pub fn image_format(path: &Path) -> Option<ImageFormat> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "webp" => Some(ImageFormat::Webp),
        "gif" => Some(ImageFormat::Gif),
        "svg" => Some(ImageFormat::Svg),
        "bmp" => Some(ImageFormat::Bmp),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        "ico" => Some(ImageFormat::Ico),
        _ => None,
    }
}

struct ImageVersion {
    image: Arc<RenderImage>,
    bytes: usize,
    dimensions: Size<Pixels>,
}

impl ImageVersion {
    fn decode(path: &Path, bytes: Vec<u8>) -> Result<Self> {
        let len = bytes.len();
        let format = image_format(path).context("Unsupported image format")?;
        let image = Image::from_bytes(format, bytes)
            .to_image_data(SvgRenderer::new(Arc::new(())))
            .with_context(|| format!("Cannot decode {}", path.display()))?;
        let scale = if format == ImageFormat::Svg {
            SMOOTH_SVG_SCALE_FACTOR
        } else {
            1.
        };
        let dimensions = image.size(0).map(|value| px(value.0 as f32 / scale));
        Ok(Self {
            image,
            bytes: len,
            dimensions,
        })
    }

    fn dimensions(&self) -> Size<Pixels> {
        self.dimensions
    }

    fn caption(&self) -> String {
        let size = self.dimensions;
        format!(
            "{} × {} · {:.1} KB",
            f32::from(size.width),
            f32::from(size.height),
            self.bytes as f64 / 1024.
        )
    }
}

pub struct ImageVersions {
    title: String,
    original: Option<ImageVersion>,
    modified: Option<ImageVersion>,
}

impl ImageVersions {
    pub fn decode(change: &Change, versions: FileVersions) -> Result<Self> {
        let old_path = change.previous_path.as_deref().unwrap_or(&change.path);
        Ok(Self {
            title: if old_path != change.path {
                format!("{} → {}", old_path.display(), change.path.display())
            } else {
                change.path.display().to_string()
            },
            original: versions
                .original
                .map(|bytes| ImageVersion::decode(old_path, bytes))
                .transpose()?,
            modified: versions
                .modified
                .map(|bytes| ImageVersion::decode(&change.path, bytes))
                .transpose()?,
        })
    }
}

pub struct ImageDiff {
    versions: ImageVersions,
    // None means fit both images at the same scale, preserving their relative size.
    zoom: Option<f32>,
    viewport: Size<Pixels>,
    scroll: [ScrollHandle; 2],
}

impl ImageDiff {
    pub fn new(versions: ImageVersions) -> Self {
        Self {
            versions,
            zoom: None,
            viewport: Size::default(),
            scroll: [ScrollHandle::new(), ScrollHandle::new()],
        }
    }

    pub fn title(&self) -> &str {
        &self.versions.title
    }

    fn fit_scale(&self) -> f32 {
        let width = (f32::from(self.viewport.width) / 2. - IMAGE_PADDING * 2.).max(1.);
        let height = (f32::from(self.viewport.height) - IMAGE_PADDING * 2.).max(1.);
        [&self.versions.original, &self.versions.modified]
            .into_iter()
            .flatten()
            .map(|image| {
                let size = image.dimensions();
                (width / f32::from(size.width)).min(height / f32::from(size.height))
            })
            .fold(1., f32::min)
    }

    fn scale(&self) -> f32 {
        self.zoom.unwrap_or_else(|| self.fit_scale())
    }

    fn set_zoom(&mut self, zoom: Option<f32>, cx: &mut Context<Self>) {
        self.zoom = zoom.map(|value| value.clamp(MIN_ZOOM, MAX_ZOOM));
        for scroll in &self.scroll {
            scroll.set_offset(Point::default());
        }
        cx.notify();
    }

    pub fn toolbar(entity: &Entity<Self>, cx: &App) -> impl IntoElement {
        let state = entity.read(cx);
        h_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .px_2()
                    .child(if state.zoom.is_none() {
                        tr("image_fit").to_owned()
                    } else {
                        format!("{:.0}%", state.scale() * 100.)
                    }),
            )
            .children(
                [
                    (
                        "image-zoom-out",
                        Some(IconName::ZoomOut),
                        "image_zoom_out",
                        Some(false),
                    ),
                    (
                        "image-zoom-in",
                        Some(IconName::ZoomIn),
                        "image_zoom_in",
                        Some(true),
                    ),
                    ("image-fit", Some(IconName::Maximize), "image_fit", None),
                    ("image-actual", None, "image_actual", None),
                ]
                .map(|(id, icon, label, zoom_in)| {
                    let image = entity.clone();
                    Button::new(id)
                        .when_some(icon, |button, icon| button.icon(icon))
                        .when(id == "image-actual", |button| button.label("1:1"))
                        .small()
                        .ghost()
                        .tooltip(tr(label))
                        .accessibility_label(tr(label))
                        .selected(
                            (id == "image-fit" && state.zoom.is_none())
                                || (id == "image-actual" && state.zoom == Some(1.)),
                        )
                        .when(zoom_in.is_none(), |button| {
                            button.toggled(
                                (id == "image-fit" && state.zoom.is_none())
                                    || (id == "image-actual" && state.zoom == Some(1.)),
                            )
                        })
                        .disabled(match zoom_in {
                            Some(true) => state.scale() >= MAX_ZOOM,
                            Some(false) => state.scale() <= MIN_ZOOM,
                            None => false,
                        })
                        .on_click(move |_, _, cx| {
                            image.update(cx, |image, cx| {
                                let zoom = match zoom_in {
                                    Some(true) => Some(image.scale() * ZOOM_STEP),
                                    Some(false) => Some(image.scale() / ZOOM_STEP),
                                    None if id == "image-actual" => Some(1.),
                                    None => None,
                                };
                                image.set_zoom(zoom, cx);
                            })
                        })
                }),
            )
    }

    fn pane(&self, index: usize, window: &Window, cx: &App) -> impl IntoElement {
        let version = if index == 0 {
            &self.versions.original
        } else {
            &self.versions.modified
        };
        let viewport = size(self.viewport.width / 2., self.viewport.height);
        let padding = px(IMAGE_PADDING);
        let content = match version {
            Some(version) => {
                let dimensions = version.dimensions();
                let width = dimensions.width * self.scale();
                let height = dimensions.height * self.scale();
                div()
                    .w((width + padding * 2.).max(viewport.width))
                    .h((height + padding * 2.).max(viewport.height))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .flex_none()
                            .w(width)
                            .h(height)
                            .relative()
                            .child(checkerboard(cx))
                            .child(
                                img(version.image.clone())
                                    .id(("image-preview", index))
                                    .role(accesskit::Role::Image)
                                    .aria_label(format!(
                                        "{} · {}",
                                        tr(if index == 0 {
                                            "image_before"
                                        } else {
                                            "image_after"
                                        }),
                                        version.caption()
                                    ))
                                    .test_support()
                                    .size_full()
                                    .relative(),
                            ),
                    )
                    .into_any_element()
            }
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(tr("image_absent"))
                .into_any_element(),
        };
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .when(index == 1, |pane| {
                pane.border_l(px(1. / window.scale_factor()))
                    .border_color(cx.theme().border)
            })
            .child(
                div()
                    .id(("image-scroll", index))
                    .test_support()
                    .size_full()
                    .overflow_scroll()
                    .track_scroll(&self.scroll[index])
                    .child(content),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::new(&self.scroll[index]).mode(ScrollbarMode::Scrolling)),
            )
    }
}

impl Render for ImageDiff {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(
                h_flex()
                    .flex_none()
                    .h(px(28.))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .children(
                        [
                            (&self.versions.original, "image_before"),
                            (&self.versions.modified, "image_after"),
                        ]
                        .map(|(version, key)| {
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .px_3()
                                .gap_2()
                                .child(tr(key))
                                .when_some(version.as_ref(), |row, image| {
                                    row.child(div().truncate().child(image.caption()))
                                })
                        }),
                    ),
            )
            .child(
                h_flex()
                    .id("image-viewport")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .on_prepaint(move |bounds, _, cx| {
                        entity.update(cx, |image, cx| {
                            if image.viewport != bounds.size {
                                image.viewport = bounds.size;
                                cx.notify();
                            }
                        })
                    })
                    .child(self.pane(0, window, cx))
                    .child(self.pane(1, window, cx)),
            )
    }
}

fn checkerboard(cx: &App) -> impl IntoElement {
    let background = cx.theme().background;
    let foreground = cx.theme().muted;
    canvas(
        move |bounds, _, _| bounds,
        move |bounds, _, window, _| {
            window.paint_quad(fill(bounds, background));
            let cell = px(8.);
            let visible = bounds.intersect(&window.content_mask().bounds);
            let first_row = ((visible.top() - bounds.top()) / cell).floor() as usize;
            let first_column = ((visible.left() - bounds.left()) / cell).floor() as usize;
            let last_row = ((visible.bottom() - bounds.top()) / cell).ceil() as usize;
            let last_column = ((visible.right() - bounds.left()) / cell).ceil() as usize;
            for row in first_row..last_row {
                for column in first_column..last_column {
                    if (row + column) % 2 == 0 {
                        let origin = bounds.origin + point(cell * column as f32, cell * row as f32);
                        window.paint_quad(fill(
                            Bounds::new(
                                origin,
                                size(
                                    cell.min(bounds.right() - origin.x),
                                    cell.min(bounds.bottom() - origin.y),
                                ),
                            ),
                            foreground,
                        ));
                    }
                }
            }
        },
    )
    .absolute()
    .inset_0()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use gpui_kit::test::TestWindowExt;

    #[test]
    fn common_image_formats_decode_and_corrupt_data_is_reported() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/images");
        for extension in ["png", "jpg", "webp", "gif", "bmp", "tiff", "ico", "svg"] {
            let path = root.join(format!("sample.{extension}"));
            let image = ImageVersion::decode(&path, std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(image.dimensions(), size(px(32.), px(32.)), "{extension}");
        }
        assert!(ImageVersion::decode(Path::new("broken.png"), vec![0, 1, 2]).is_err());
        assert!(image_format(Path::new("PHOTO.JPEG")).is_some());
        assert!(image_format(Path::new("archive.zip")).is_none());
    }

    fn rectangle(width: usize, height: usize) -> ImageVersion {
        ImageVersion::decode(Path::new("fixture.svg"), format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"><rect width="{width}" height="{height}" fill="#268cd2"/></svg>"##
        ).into_bytes()).unwrap()
    }

    #[gpui_kit::test]
    fn images_fit_at_equal_scale_and_zoom_scroll_inside_equal_panes(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, image) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Point::default(),
                        size(px(1000.), px(600.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |_, cx| {
                    cx.new(|_| {
                        ImageDiff::new(ImageVersions {
                            title: "fixture.svg".into(),
                            original: Some(rectangle(1000, 800)),
                            modified: Some(rectangle(500, 400)),
                        })
                    })
                },
            )
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            let panes = [
                window.find(("image-scroll", 0usize)),
                window.find(("image-scroll", 1usize)),
            ];
            let left = panes[0].bounds();
            let right = panes[1].bounds();
            assert!((left.size.width - right.size.width).abs() <= px(1.));
            let pictures = [
                window.find(("image-preview", 0usize)),
                window.find(("image-preview", 1usize)),
            ];
            assert_eq!(pictures.len(), 2);
            assert!(pictures[0].bounds().size.width < left.size.width);
            assert!(
                (pictures[0].bounds().size.width - pictures[1].bounds().size.width * 2.).abs()
                    <= px(1.)
            );
            image.update(cx, |image, cx| image.set_zoom(Some(1.), cx));
            window.render_frame(cx);
            window.render_frame(cx);
            assert_eq!(
                window.find(("image-preview", 0usize)).bounds().size.width,
                px(1000.)
            );
            assert_eq!(
                window.find(("image-scroll", 0usize)).bounds().size.width,
                left.size.width
            );
            let before = window.find(("image-preview", 0usize)).bounds();
            image.update(cx, |image, cx| {
                image.scroll[0].set_offset(point(px(-200.), px(-100.)));
                cx.notify();
            });
            window.render_frame(cx);
            assert!(window.find(("image-preview", 0usize)).bounds().left() < before.left());
            image.update(cx, |image, cx| image.set_zoom(None, cx));
            window.render_frame(cx);
            assert_eq!(image.read(cx).scroll[0].offset(), Point::default());
            assert!(window.find(("image-preview", 0usize)).bounds().size.width < left.size.width);
        })
        .unwrap();
    }
}
