//! Runs the PDF engine on a dedicated thread so the UI thread never blocks on PDFium.
//!
//! In M2 this is replaced by an IPC client to sandboxed engine hosts (ADR-0004); the async
//! `call` interface stays the same.

use std::{sync::mpsc, thread};

use ferrix_engine::PdfEngine;
use iced::futures::channel::oneshot;

type Job<E> = Box<dyn FnOnce(&mut E) + Send>;

/// A cloneable handle to the engine thread.
#[derive(Debug)]
pub(crate) struct EngineWorker<E> {
    jobs: mpsc::Sender<Job<E>>,
}

impl<E> Clone for EngineWorker<E> {
    fn clone(&self) -> Self {
        Self {
            jobs: self.jobs.clone(),
        }
    }
}

impl<E: PdfEngine + 'static> EngineWorker<E> {
    /// Moves `engine` onto its own thread, which runs until every handle is dropped.
    pub(crate) fn spawn(mut engine: E) -> std::io::Result<Self> {
        let (jobs, rx) = mpsc::channel::<Job<E>>();
        thread::Builder::new()
            .name("ferrix-engine".into())
            .spawn(move || {
                for job in rx {
                    job(&mut engine);
                }
            })?;
        Ok(Self { jobs })
    }

    /// Runs `f` on the engine thread and returns its result.
    pub(crate) async fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut E) -> R + Send + 'static,
    ) -> Result<R, EngineUnavailable> {
        let (tx, rx) = oneshot::channel();
        let job: Job<E> = Box::new(move |engine| {
            // The receiver may have been dropped if the UI no longer needs the result.
            let _ = tx.send(f(engine));
        });
        self.jobs.send(job).map_err(|_| EngineUnavailable)?;
        rx.await.map_err(|_| EngineUnavailable)
    }
}

/// The engine thread has stopped (it panicked).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EngineUnavailable;

impl std::fmt::Display for EngineUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the PDF engine is not available")
    }
}

/// Collapses an engine call's two failure modes into a message for the UI.
pub(crate) fn flatten<T>(
    result: Result<Result<T, ferrix_engine::EngineError>, EngineUnavailable>,
) -> Result<T, String> {
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Runs blocking work (file I/O, PDF writing) on its own thread so the UI never waits on it.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = oneshot::channel();
    thread::Builder::new()
        .name("ferrix-io".into())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .map_err(|e| e.to_string())?;
    rx.await
        .unwrap_or_else(|_| Err("background work stopped unexpectedly".into()))
}
