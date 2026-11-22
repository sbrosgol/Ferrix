//! Measures the app against the M1 exit criteria (docs/roadmap.md), one JSON line per file:
//!
//! - `first_paint_ms`: from asking to open the file to its first page on screen, through the
//!   real app (iced's headless emulator), as the app itself reports it (`ferrix::perf`).
//! - `frame_ms`: per scroll step, the UI thread's work: handling the scroll and building the
//!   view. Layout and GPU drawing aren't included (the emulator has no GPU), so this is a lower
//!   bound on a frame; 60 fps needs the whole frame under 16.7 ms.
//! - `full_frame_ms`: the same plus laying out the widget tree and recording its drawing (the
//!   CPU side of a frame); only rasterizing, which the GPU does, is left out.
//! - `render_ms`: engine time per page at the zoom a 1180 px window fits, on a 2× display,
//!   which sets how fast pages fill in while scrolling.
//!
//! ```sh
//! cargo run --release -p ferrix-app --example bench -- file.pdf…
//! ```
//! `cargo xtask bench` runs it over the corpus and checks the thresholds.

#![allow(clippy::unwrap_used)]

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use ferrix_app::Message;
use ferrix_engine::{DocSource, PdfEngine, RenderFlags, TileRequest};
use ferrix_engine_pdfium::PdfiumEngine;
use ferrix_types::{PageIndex, PixelRect, Rotation};
use iced_test::{
    Emulator,
    emulator::{Event, Mode},
    futures::futures::channel::mpsc,
};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

/// Window size, as the app opens by default.
const WINDOW: (f32, f32) = (1180.0, 860.0);
const SCALE_FACTOR: f32 = 2.0;
const SCROLL_STEPS: usize = 200;
const RENDER_PAGES: u32 = 50;

/// Collects the app's `first paint` perf events.
struct Capture(Arc<Mutex<Option<f64>>>);

impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if event.metadata().target() != "ferrix::perf" {
            return;
        }
        struct Visit<'a>(&'a Mutex<Option<f64>>);
        impl tracing::field::Visit for Visit<'_> {
            fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
                if field.name() == "first_paint_ms" {
                    *self.0.lock().unwrap() = Some(value);
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
        }
        event.record(&mut Visit(&self.0));
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i]
}

fn main() {
    let files: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    assert!(!files.is_empty(), "usage: bench <file.pdf>…");
    let first_paint = Arc::new(Mutex::new(None));
    tracing_subscriber::registry()
        .with(Capture(first_paint.clone()))
        .init();

    for file in files {
        *first_paint.lock().unwrap() = None;
        let app = ferrix_app::application(Some(file.clone()));
        let (sender, mut receiver) = mpsc::channel(100);
        let mut emulator = Emulator::new(
            sender,
            &app,
            Mode::Immediate,
            iced::Size::new(WINDOW.0, WINDOW.1),
        );
        // Wait for the first page, running the app's tasks as they finish.
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut pump = |emulator: &mut Emulator<_>| {
            while let Ok(Event::Action(action)) = receiver.try_recv() {
                emulator.perform(&app, action);
            }
        };
        while first_paint.lock().unwrap().is_none() && Instant::now() < deadline {
            pump(&mut emulator);
            std::thread::sleep(Duration::from_micros(200));
        }
        let paint = *first_paint.lock().unwrap();

        // Scroll down the document a little at a time, timing each frame's UI-thread work.
        let mut frames = Vec::with_capacity(SCROLL_STEPS);
        if paint.is_some() {
            for step in 0..SCROLL_STEPS {
                let start = Instant::now();
                emulator.update(&app, Message::PageScrolled(step as f32 * 60.0, 0.0));
                drop(emulator.view(&app));
                frames.push(start.elapsed().as_secs_f64() * 1000.0);
                pump(&mut emulator);
            }
        }
        frames.sort_by(f64::total_cmp);
        // Full frames: the same, plus building the widget tree's layout and recording its
        // drawing, on a headless renderer (rasterizing is left to the GPU and not counted).
        let mut full = Vec::new();
        if paint.is_some() {
            full = full_frames(&app, emulator.into_state());
        }
        full.sort_by(f64::total_cmp);

        let renders = render_times(&file);
        let line = serde_json::json!({
            "file": file.display().to_string(),
            "first_paint_ms": paint,
            "frame_ms": { "p50": percentile(&frames, 0.5), "p95": percentile(&frames, 0.95), "max": frames.last().copied().unwrap_or(0.0) },
            "full_frame_ms": { "p50": percentile(&full, 0.5), "p95": percentile(&full, 0.95) },
            "render_ms": { "p50": percentile(&renders, 0.5), "p95": percentile(&renders, 0.95), "pages": renders.len() },
        });
        println!("{line}");
    }
}

/// Times update + view + layout + draw for scroll steps, outside the emulator (which can't
/// lay out more than once).
fn full_frames<P>(app: &P, (mut state, window): (P::State, iced::window::Id)) -> Vec<f64>
where
    P: iced::Program<Message = Message, Theme = iced::Theme>,
{
    use iced::advanced::renderer::Headless;
    use iced_runtime::user_interface::{Cache, UserInterface};

    let Some(mut renderer) = iced::futures::executor::block_on(<P::Renderer as Headless>::new(
        iced::Font::DEFAULT,
        iced::Pixels(16.0),
        Some("tiny-skia"),
    )) else {
        return Vec::new();
    };
    let theme = ferrix_app::theme::light();
    let style = iced::advanced::renderer::Style {
        text_color: app.style(&state, &theme).text_color,
    };
    let size = iced::Size::new(WINDOW.0, WINDOW.1);
    let mut cache = Cache::default();
    let mut times = Vec::new();
    for step in 0..60 {
        let start = Instant::now();
        drop(app.update(&mut state, Message::PageScrolled(step as f32 * 300.0, 0.0)));
        let mut ui = UserInterface::build(app.view(&state, window), size, cache, &mut renderer);
        ui.draw(
            &mut renderer,
            &theme,
            &style,
            iced::mouse::Cursor::Unavailable,
        );
        cache = ui.into_cache();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times
}

/// Engine time to render each of the first pages whole, at the width of the default window.
fn render_times(file: &std::path::Path) -> Vec<f64> {
    let Ok(mut engine) = PdfiumEngine::new() else {
        return Vec::new();
    };
    let Ok(info) = engine.open(DocSource::Path(file.to_path_buf()), None) else {
        return Vec::new();
    };
    let mut times = Vec::new();
    for i in 0..info.page_count.min(RENDER_PAGES) {
        let Ok(page) = engine.page_info(info.id, PageIndex(i)) else {
            continue;
        };
        // Fit width in the page view (window less sidebar and margins), at most 125 %.
        let logical_width = WINDOW.0 - 221.0 - 56.0 - 12.0;
        let zoom = (logical_width / (page.size.width * 96.0 / 72.0)).min(1.25);
        let scale = zoom * 96.0 / 72.0 * SCALE_FACTOR;
        let (w, h) = page.size.to_pixels(scale);
        let start = Instant::now();
        let ok = engine
            .render_tile(&TileRequest {
                doc: info.id,
                page: PageIndex(i),
                scale,
                rotation: Rotation::R0,
                region: PixelRect::new(0, 0, w.min(3072), h.min(3072)),
                flags: RenderFlags::default(),
            })
            .is_ok();
        if ok {
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    times.sort_by(f64::total_cmp);
    times
}
