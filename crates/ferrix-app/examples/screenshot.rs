//! Renders the app headlessly to a PNG, for reviewing UI changes without a display.
//!
//! ```sh
//! cargo run -p ferrix-app --example screenshot -- out.png [document] [--dark] [--new]
//! ```
//!
//! `--new` opens the New Document dialog, `--rotate` rotates the current page and `--recover`
//! accepts the crash-recovery offer, each partway through the run, before capturing. `SHOT_SECS` sets how long the app
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
    let mut script: Vec<Message> = Vec::new();
    if flag(&mut args, "--new") {
        script.push(Message::NewDialog);
    }
    if flag(&mut args, "--rotate") {
        script.push(Message::Rotate(ferrix_types::Rotation::R90));
    }
    if flag(&mut args, "--recover") {
        script.push(Message::Recovery(ferrix_app::Recovery::Recover));
    }
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
    // Scripted messages go out in the middle half of the run, after loading settles.
    let steps = script.len() as u32;
    let mut script = script.into_iter().enumerate().peekable();
    while start.elapsed() < duration {
        if let Ok(Event::Action(action)) = receiver.try_recv() {
            emulator.perform(&app, action);
        }
        if let Some((i, _)) = script.peek()
            && start.elapsed() > duration / 2 + duration * (*i as u32) / (4 * steps)
            && let Some((_, message)) = script.next()
        {
            emulator.update(&app, message);
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
