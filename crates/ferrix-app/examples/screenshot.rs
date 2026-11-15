//! Renders the app headlessly to a PNG, for reviewing UI changes without a display.
//!
//! ```sh
//! cargo run -p ferrix-app --example screenshot -- out.png [document] [--dark] [--new]
//! ```
//!
//! `--write` starts a written document and types formatted text into it; `--fit-page` and
//! `--zoom-in` change the zoom afterwards.
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
    if flag(&mut args, "--write") {
        script.extend(writing_script());
    }
    if flag(&mut args, "--features") {
        script.extend(features_script());
    }
    if flag(&mut args, "--fit-page") {
        script.push(Message::FitPage);
    }
    if flag(&mut args, "--fit-width") {
        script.push(Message::FitWidth);
    }
    if flag(&mut args, "--zoom-in") {
        script.extend([Message::ZoomIn, Message::ZoomIn, Message::ZoomIn]);
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

/// Creates a document and types a short formatted letter into it.
fn writing_script() -> Vec<Message> {
    use ferrix_app::{KeyCommand as K, WriteAction as A};
    use ferrix_compose::{Align, Motion, Toggle};
    let key = |k| Message::Write(A::Key(k));
    let typed = |t: &str| key(K::Insert(t.into()));
    let mut s = vec![
        Message::NewDialog,
        Message::CreateWriting,
        Message::Write(A::Style(ferrix_app::writer_style(1))),
        typed("Project Update"),
        key(K::Enter),
        typed("Dear team,"),
        key(K::Enter),
        typed("This document was written in "),
        key(K::Toggle(Toggle::Bold)),
        typed("Ferrix"),
        key(K::Toggle(Toggle::Bold)),
        typed(" and saved as an ordinary PDF that stays "),
        key(K::Toggle(Toggle::Italic)),
        typed("editable"),
        key(K::Toggle(Toggle::Italic)),
        typed(
            ". Text wraps at the margins, paragraphs reflow as you type, and every change can be undone.",
        ),
        key(K::Enter),
        Message::Write(A::Style(ferrix_app::writer_style(3))),
        typed("Next steps"),
        key(K::Enter),
        Message::Write(A::Align(Align::Justify)),
        typed(
            "Review the draft, then share the PDF. Anyone can read it; anyone with Ferrix can keep editing it. ",
        ),
        typed(
            "Selections, word-by-word movement and the formatting bar all work from the keyboard.",
        ),
    ];
    // Select the last word to show the selection.
    for _ in 0..9 {
        s.push(key(K::Move {
            motion: Motion::Left,
            extend: true,
        }));
    }
    s
}

/// Lists, a picture, a footer with page numbers and the find bar.
fn features_script() -> Vec<Message> {
    use ferrix_app::{KeyCommand as K, WriteAction as A};
    use ferrix_compose::{ImageData, ListKind};
    let key = |k| Message::Write(A::Key(k));
    let typed = |t: &str| key(K::Insert(t.into()));
    let picture = image::RgbaImage::from_fn(480, 160, |x, y| {
        let t = x as f32 / 480.0;
        image::Rgba([
            (40.0 + 180.0 * t) as u8,
            (90 + y / 4) as u8,
            (220.0 - 120.0 * t) as u8,
            255,
        ])
    });
    let mut png = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    vec![
        Message::NewDialog,
        Message::CreateWriting,
        Message::Write(A::Style(ferrix_app::writer_style(2))),
        typed("Launch checklist"),
        key(K::Enter),
        Message::Write(A::List(ListKind::Bullet)),
        typed("Freeze the feature set"),
        key(K::Enter),
        typed("Write the release notes"),
        key(K::Enter),
        key(K::Tab { back: false }),
        typed("Include screenshots"),
        key(K::Enter),
        key(K::Tab { back: true }),
        typed("Tag the release"),
        key(K::Enter),
        key(K::Enter),
        typed("Steps, in order:"),
        key(K::Enter),
        Message::Write(A::List(ListKind::Numbered)),
        typed("Build installers"),
        key(K::Enter),
        typed("Sign and notarize"),
        key(K::Enter),
        typed("Publish the release notes"),
        key(K::Enter),
        key(K::Enter),
        Message::Write(A::ImageLoaded(Ok(Box::new(
            ImageData::from_bytes(png).unwrap(),
        )))),
        typed("Figure 1: release timeline"),
        key(K::PageBreak),
        typed("Appendix"),
        Message::PageSetupDialog,
        Message::PageSetupFooter("Page {page} of {pages}".into()),
        Message::ApplyPageSetup,
        key(K::Move {
            motion: ferrix_compose::Motion::DocumentStart,
            extend: false,
        }),
        Message::FindOpen,
        Message::FindQuery("release".into()),
    ]
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
