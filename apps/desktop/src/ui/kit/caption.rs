//! Windows caption controls. GPUI owns their non-client hit tests and native actions.

use gpui::{
    Bounds, Div, IntoElement, PathBuilder, Window, WindowControlArea, canvas, div, fill, point,
    prelude::*, px, size,
};

use crate::theme::{Palette, chrome, color, desktop, with_alpha};

/// Overrides for the hidden gallery. Live windows use GPUI hover/press and native window state.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum CaptionPreview {
    #[default]
    Live,
    MaximizeHover,
    CloseHover,
}

#[derive(Clone, Copy)]
enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

fn glyph(glyph: Glyph) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            // Snap filled strokes as a unit, so a 1 logical px rule doesn't straddle pixels
            // at 150%. Use the same device-aligned stroke for the diagonals of the close glyph.
            let origin = window.pixel_snap_point(bounds.origin);
            let side = window.pixel_snap(px(desktop::CAPTION_GLYPH));
            let stroke = window.pixel_snap(px(desktop::CAPTION_STROKE));
            let overlap = window.pixel_snap(px(desktop::CAPTION_STROKE * 2.));
            let mid = window.pixel_snap((side - stroke) / 2.);
            let ink = window.text_style().color;
            let mut rule = |x, y, w, h| {
                window.paint_quad(fill(Bounds::new(origin + point(x, y), size(w, h)), ink));
            };
            match glyph {
                Glyph::Minimize => rule(px(0.), mid, side, stroke),
                Glyph::Maximize | Glyph::Restore => {
                    let restore = matches!(glyph, Glyph::Restore);
                    let (top, right) = if restore {
                        // Only the exposed top and right edges of the back window.
                        rule(overlap, px(0.), side - overlap, stroke);
                        rule(overlap, stroke, stroke, overlap - stroke);
                        rule(side - stroke, stroke, stroke, side - overlap - stroke);
                        rule(
                            side - overlap,
                            side - overlap - stroke,
                            overlap - stroke,
                            stroke,
                        );
                        (overlap, side - overlap)
                    } else {
                        (px(0.), side)
                    };
                    rule(px(0.), top, right, stroke);
                    rule(px(0.), side - stroke, right, stroke);
                    rule(px(0.), top + stroke, stroke, side - top - stroke * 2.);
                    rule(
                        right - stroke,
                        top + stroke,
                        stroke,
                        side - top - stroke * 2.,
                    );
                }
                Glyph::Close => {
                    let inset = stroke / std::f32::consts::SQRT_2;
                    for (start, end) in [(px(0.), side), (side, px(0.))] {
                        let sign = if end > start { 1. } else { -1. };
                        let mut path = PathBuilder::fill();
                        for (i, (x, y)) in [
                            (start, inset),
                            (start + inset * sign, px(0.)),
                            (end, side - inset),
                            (end - inset * sign, side),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            if i == 0 {
                                path.move_to(origin + point(x, y));
                            } else {
                                path.line_to(origin + point(x, y));
                            }
                        }
                        path.close();
                        if let Ok(path) = path.build() {
                            window.paint_path(path, ink);
                        }
                    }
                }
            }
        },
    )
    .size(px(desktop::CAPTION_GLYPH))
    .flex_none()
}

/// Minimize, maximize/restore and close, after a drag area filling the rest of their parent.
pub fn window_controls(p: &Palette, window: &Window, preview: CaptionPreview) -> Div {
    let active = preview != CaptionPreview::Live || window.is_window_active();
    let maximized = preview == CaptionPreview::CloseHover || window.is_maximized();
    let white = color(chrome::CAPTION_WHITE);
    let (base, hover, pressed) = if p.night() {
        (
            white,
            chrome::CAPTION_HOVER_DARK,
            chrome::CAPTION_PRESSED_DARK,
        )
    } else {
        (
            p.ink,
            chrome::CAPTION_HOVER_LIGHT,
            chrome::CAPTION_PRESSED_LIGHT,
        )
    };
    let ink = if active { p.ink } else { p.ink_2 };
    let control = |id, symbol, area, preview_hover| {
        let button = div()
            .id(id)
            .flex_none()
            .w(px(desktop::CAPTION_WIDTH))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(ink)
            // In particular, Max maps to HTMAXBUTTON, which Windows uses for snap layouts.
            .window_control_area(area)
            .child(glyph(symbol));
        if matches!(symbol, Glyph::Close) {
            let hover = color(chrome::CAPTION_CLOSE);
            let pressed = color(chrome::CAPTION_CLOSE_PRESSED);
            button
                .hover(move |s| s.bg(hover).text_color(white))
                .active(move |s| s.bg(pressed).text_color(white))
                .when(preview_hover, |b| b.bg(hover).text_color(white))
        } else {
            let hover = with_alpha(base, hover);
            let pressed = with_alpha(base, pressed);
            button
                .hover(move |s| s.bg(hover))
                .active(move |s| s.bg(pressed))
                .when(preview_hover, |b| b.bg(hover))
        }
    };
    div()
        .flex_1()
        .h_full()
        .flex()
        .justify_end()
        .child(
            div()
                .flex_1()
                .h_full()
                .window_control_area(WindowControlArea::Drag),
        )
        .child(control(
            "win-min",
            Glyph::Minimize,
            WindowControlArea::Min,
            false,
        ))
        .child(control(
            "win-max",
            if maximized {
                Glyph::Restore
            } else {
                Glyph::Maximize
            },
            WindowControlArea::Max,
            preview == CaptionPreview::MaximizeHover,
        ))
        .child(control(
            "win-close",
            Glyph::Close,
            WindowControlArea::Close,
            preview == CaptionPreview::CloseHover,
        ))
}
