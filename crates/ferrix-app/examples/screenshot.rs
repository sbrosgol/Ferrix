//! Renders the app headlessly to a PNG, for reviewing UI changes without a display.
//!
//! ```sh
//! cargo run -p ferrix-app --example screenshot -- out.png [document.pdf] [--dark]
//! ```

#![allow(clippy::unwrap_used)]

use std::{path::PathBuf, time::Duration};

use ferrix_app::theme;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let dark = args.iter().any(|a| a == "--dark");
    args.retain(|a| a != "--dark");
    let output = PathBuf::from(
        args.first()
            .expect("usage: screenshot <out.png> [doc.pdf] [--dark]"),
    );
    let document = args.get(1).map(PathBuf::from);

    let app = ferrix_app::application(document);
    let theme = if dark { theme::dark() } else { theme::light() };
    let shot = iced_test::screenshot(
        &app,
        &theme,
        size(),
        2.0,
        Duration::from_secs(
            std::env::var("SHOT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
        ),
    );

    image::RgbaImage::from_raw(shot.size.width, shot.size.height, shot.rgba.to_vec())
        .unwrap()
        .save(&output)
        .unwrap();
    println!(
        "{} ({}×{})",
        output.display(),
        shot.size.width,
        shot.size.height
    );
}

/// Viewport in logical pixels, overridable with `SHOT_SIZE=WxH`.
fn size() -> (f32, f32) {
    std::env::var("SHOT_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((1180.0, 860.0))
}
