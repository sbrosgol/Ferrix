//! Renders the app headlessly to a PNG, for reviewing UI changes without a display.
//!
//! ```sh
//! cargo run -p ferrix-app --example screenshot -- out.png [document] [--dark] [--new]
//! ```
//!
//! `--new` opens the New Document dialog before capturing. `SHOT_SECS` sets how long the app
//! runs first (default 3), `SHOT_SIZE=WxH` the window size in logical pixels.

#![allow(clippy::unwrap_used)]

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use ferrix_app::{Message, theme};
use iced_test::{
    Emulator,
    emulator::{Event, Mode},
    futures::futures::channel::mpsc,
};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |args: &mut Vec<String>, name: &str| {
        let found = args.iter().any(|a| a == name);
        args.retain(|a| a != name);
        found
    };
    let dark = flag(&mut args, "--dark");
    let new_dialog = flag(&mut args, "--new");
    let output = PathBuf::from(
        args.first()
            .expect("usage: screenshot <out.png> [doc] [--dark] [--new]"),
    );
    let document = args.get(1).map(PathBuf::from);

    let app = ferrix_app::application(document);
    let theme = if dark { theme::dark() } else { theme::light() };
    let duration = Duration::from_secs(
        std::env::var("SHOT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3),
    );

    let (sender, mut receiver) = mpsc::channel(100);
    let mut emulator = Emulator::new(sender, &app, Mode::Immediate, size().into());
    let start = Instant::now();
    let mut sent = !new_dialog;
    while start.elapsed() < duration {
        if let Ok(Event::Action(action)) = receiver.try_recv() {
            emulator.perform(&app, action);
        }
        if !sent && start.elapsed() > duration / 2 {
            emulator.update(&app, Message::NewDialog);
            sent = true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let shot = emulator.screenshot(&app, &theme, 2.0);

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
