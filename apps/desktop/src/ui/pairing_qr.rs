//! Render shared QR modules on whole device pixels, with the specified quiet zone intact.

use gpui::{Bounds, IntoElement, canvas, fill, point, prelude::*, px, size};
use todo_client::device_pairing::Qr;
use todo_design::{Theme, hues, layout::component, palette};

pub fn tile(qr: Qr) -> impl IntoElement {
    let light = palette(Theme::Light, hues::MY_DAY);
    let bg = gpui::rgb(light.pane.to_u32());
    let ink = gpui::rgb(light.ink.to_u32());
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let scale = window.scale_factor();
            let step = (f32::from(bounds.size.width) * scale / qr.width as f32).floor() / scale;
            let x0 = (f32::from(bounds.origin.x) * scale).round() / scale;
            let y0 = (f32::from(bounds.origin.y) * scale).round() / scale;
            let module = px(step);
            let side = px(step * qr.width as f32);
            window.paint_quad(fill(
                Bounds::new(point(px(x0), px(y0)), size(side, side)),
                bg,
            ));
            for (idx, _) in qr.dark.iter().enumerate().filter(|(_, dark)| **dark) {
                let x = x0 + step * (idx % qr.width) as f32;
                let y = y0 + step * (idx / qr.width) as f32;
                window.paint_quad(fill(
                    Bounds::new(point(px(x), px(y)), size(module, module)),
                    ink,
                ));
            }
        },
    )
    .size(px(component::PAIRING_QR))
    .flex_none()
}
