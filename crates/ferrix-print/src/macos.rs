//! macOS: PDFKit builds the print operation; AppKit shows it as a sheet on our window.

#![allow(unsafe_code)]

use std::cell::RefCell;

use objc2::{AnyThread, MainThreadMarker, rc::Retained};
use objc2_app_kit::{NSPrintInfo, NSPrintOperation, NSView};
use objc2_foundation::{NSString, NSURL};
use objc2_pdf_kit::{PDFDocument, PDFPrintScalingMode};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::{PrintError, PrintJob, PrintOutcome};

thread_local! {
    /// Keeps the running operation alive while its sheet is open; replaced by the next job.
    static RUNNING: RefCell<Option<Retained<NSPrintOperation>>> = const { RefCell::new(None) };
}

/// Opens the print sheet on `window`. Must be called on the main thread; returns immediately.
pub fn print_with_sheet<W: HasWindowHandle + ?Sized>(
    window: &W,
    job: &PrintJob,
) -> Result<PrintOutcome, PrintError> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| PrintError::Unavailable("not called on the main thread".into()))?;
    let handle = window
        .window_handle()
        .map_err(|e| PrintError::Unavailable(e.to_string()))?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return Err(PrintError::Unavailable("not an AppKit window".into()));
    };
    // SAFETY: an AppKit window handle's `ns_view` is a valid `NSView` for as long as `handle`
    // is borrowed, and we are on the main thread.
    let view: &NSView = unsafe { appkit.ns_view.cast::<NSView>().as_ref() };
    let ns_window = view
        .window()
        .ok_or_else(|| PrintError::Unavailable("view is not in a window".into()))?;

    let path = job
        .path
        .to_str()
        .ok_or_else(|| PrintError::Unavailable("file path is not valid UTF-8".into()))?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    // SAFETY: standard allocate-and-initialise; returns `None` if the file can't be read.
    let document = unsafe { PDFDocument::initWithURL(PDFDocument::alloc(), &url) }
        .ok_or_else(|| PrintError::Platform("PDFKit could not open the document".into()))?;

    let info = NSPrintInfo::sharedPrintInfo();
    // SAFETY: `document` and `info` are valid objects; called on the main thread.
    let operation = unsafe {
        document.printOperationForPrintInfo_scalingMode_autoRotate(
            Some(&info),
            PDFPrintScalingMode::PageScaleDownToFit,
            true,
            mtm,
        )
    }
    .ok_or_else(|| PrintError::Platform("PDFKit could not create a print operation".into()))?;
    operation.setJobTitle(Some(&NSString::from_str(&job.title)));

    // SAFETY: `ns_window` is a live window on the main thread. With no delegate there is no
    // completion callback, so the null context pointer is never read.
    unsafe {
        operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
            &ns_window,
            None,
            None,
            std::ptr::null_mut(),
        );
    }
    RUNNING.with(|running| running.replace(Some(operation)));
    Ok(PrintOutcome::SheetOpened)
}
