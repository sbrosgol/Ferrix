//! Windows: the `PrintDlgEx` common dialog, then GDI spooling of pages rasterized by the
//! caller (our PDFium engine), so output matches the screen.

#![allow(unsafe_code)]

use std::{ffi::c_void, mem, ptr};

use windows_sys::Win32::{
    Foundation::GlobalFree,
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteDC, GetDeviceCaps, HDC,
        HORZRES, LOGPIXELSX, SRCCOPY, StretchDIBits, VERTRES,
    },
    Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage},
    UI::Controls::Dialogs::{
        PD_NOCURRENTPAGE, PD_NOSELECTION, PD_PAGENUMS, PD_RESULT_PRINT, PD_RETURNDC,
        PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGEXW, PRINTPAGERANGE, PrintDlgExW,
        START_PAGE_GENERAL,
    },
};

use crate::{PageSource, Parent, PrintError, PrintJob, PrintOutcome, fit_to_paper};

/// Rasterizing above this resolution makes spool files huge with no visible gain for most
/// documents.
const MAX_RASTER_DPI: f32 = 300.0;
const MAX_RANGES: usize = 16;

/// Shows the print dialog and, if confirmed, prints every selected page. Blocking: call it
/// from a background thread.
pub fn print_with_dialog(
    parent: Parent,
    job: &PrintJob,
    pages: &mut dyn PageSource,
) -> Result<PrintOutcome, PrintError> {
    let Parent::Win32(owner) = parent else {
        return Err(PrintError::Unavailable(
            "no parent window for the print dialog".into(),
        ));
    };
    let Some(selection) = show_dialog(owner as *mut c_void, job.page_count)? else {
        return Ok(PrintOutcome::Cancelled);
    };
    let printer = Printer(selection.dc);
    let result = spool(&printer, job, &selection.pages, pages);
    // `printer` deletes the DC on drop, after spooling finished or was aborted.
    result.map(|()| PrintOutcome::Submitted)
}

struct Selection {
    dc: HDC,
    /// Zero-based page indices in print order.
    pages: Vec<u32>,
}

fn show_dialog(owner: *mut c_void, page_count: u32) -> Result<Option<Selection>, PrintError> {
    let mut ranges: [PRINTPAGERANGE; MAX_RANGES] = unsafe { mem::zeroed() };
    ranges[0] = PRINTPAGERANGE {
        nFromPage: 1,
        nToPage: page_count,
    };
    // SAFETY: PRINTDLGEXW is a plain C struct; all-zero is a valid initial state.
    let mut dialog: PRINTDLGEXW = unsafe { mem::zeroed() };
    dialog.lStructSize = mem::size_of::<PRINTDLGEXW>() as u32;
    dialog.hwndOwner = owner;
    dialog.Flags = PD_RETURNDC | PD_NOSELECTION | PD_NOCURRENTPAGE | PD_USEDEVMODECOPIESANDCOLLATE;
    dialog.nPageRanges = 1;
    dialog.nMaxPageRanges = MAX_RANGES as u32;
    dialog.lpPageRanges = ranges.as_mut_ptr();
    dialog.nMinPage = 1;
    dialog.nMaxPage = page_count;
    dialog.nCopies = 1;
    dialog.nStartPage = START_PAGE_GENERAL;

    // SAFETY: `dialog` is fully initialised and `ranges` outlives the call.
    let hr = unsafe { PrintDlgExW(&mut dialog) };
    // SAFETY: the dialog allocated these global handles (or left them null); we own them now.
    unsafe {
        if !dialog.hDevMode.is_null() {
            GlobalFree(dialog.hDevMode);
        }
        if !dialog.hDevNames.is_null() {
            GlobalFree(dialog.hDevNames);
        }
    }
    if hr != 0 {
        return Err(PrintError::Platform(format!(
            "PrintDlgEx failed (HRESULT {hr:#010x})"
        )));
    }
    if dialog.dwResultAction != PD_RESULT_PRINT {
        if !dialog.hDC.is_null() {
            // SAFETY: we own the DC returned by the dialog.
            unsafe { DeleteDC(dialog.hDC) };
        }
        return Ok(None);
    }
    if dialog.hDC.is_null() {
        return Err(PrintError::Platform(
            "the printer returned no device context".into(),
        ));
    }

    let pages = if dialog.Flags & PD_PAGENUMS != 0 {
        ranges[..(dialog.nPageRanges as usize).min(MAX_RANGES)]
            .iter()
            .flat_map(|r| r.nFromPage.min(r.nToPage)..=r.nToPage.max(r.nFromPage))
            .filter(|&n| n >= 1 && n <= page_count)
            .map(|n| n - 1)
            .collect()
    } else {
        (0..page_count).collect()
    };
    Ok(Some(Selection {
        dc: dialog.hDC,
        pages,
    }))
}

/// Owns a printer device context.
struct Printer(HDC);

impl Drop for Printer {
    fn drop(&mut self) {
        // SAFETY: the DC came from PrintDlgEx and is deleted exactly once.
        unsafe { DeleteDC(self.0) };
    }
}

fn spool(
    printer: &Printer,
    job: &PrintJob,
    pages: &[u32],
    source: &mut dyn PageSource,
) -> Result<(), PrintError> {
    let dc = printer.0;
    let title: Vec<u16> = job.title.encode_utf16().chain([0]).collect();
    let doc = DOCINFOW {
        cbSize: mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: title.as_ptr(),
        lpszOutput: ptr::null(),
        lpszDatatype: ptr::null(),
        fwType: 0,
    };
    // SAFETY: `dc` is a valid printer DC and `doc` points to live data for the call.
    if unsafe { StartDocW(dc, &doc) } <= 0 {
        return Err(PrintError::Platform("the printer refused the job".into()));
    }
    for &page in pages {
        if let Err(e) = print_page(dc, page, source) {
            // SAFETY: a document was started on `dc` and is abandoned once.
            unsafe { AbortDoc(dc) };
            return Err(e);
        }
    }
    // SAFETY: matches the successful StartDocW above.
    if unsafe { EndDoc(dc) } <= 0 {
        return Err(PrintError::Platform(
            "the printer could not finish the job".into(),
        ));
    }
    Ok(())
}

fn print_page(dc: HDC, page: u32, source: &mut dyn PageSource) -> Result<(), PrintError> {
    let render_error = |reason| PrintError::Render {
        page: page + 1,
        reason,
    };
    // SAFETY: `dc` is a valid printer DC for all GDI calls below.
    let (width, height, dpi) = unsafe {
        (
            GetDeviceCaps(dc, HORZRES as i32) as f32,
            GetDeviceCaps(dc, VERTRES as i32) as f32,
            GetDeviceCaps(dc, LOGPIXELSX as i32) as f32,
        )
    };
    if width <= 0.0 || height <= 0.0 || dpi <= 0.0 {
        return Err(PrintError::Platform(
            "the printer reported an empty page".into(),
        ));
    }

    let size = source.page_size(page).map_err(render_error)?;
    let device_per_point = dpi / 72.0;
    let (rotate, fit) = fit_to_paper(size, (width / device_per_point, height / device_per_point));
    let raster_scale = dpi.min(MAX_RASTER_DPI) / 72.0 * fit;
    let mut bitmap = source
        .render(page, raster_scale, rotate)
        .map_err(render_error)?;

    let (w, h) = if rotate { (size.1, size.0) } else { size };
    let dest_w = (w * device_per_point * fit).round() as i32;
    let dest_h = (h * device_per_point * fit).round() as i32;
    let x = ((width as i32) - dest_w) / 2;
    let y = ((height as i32) - dest_h) / 2;

    // GDI expects BGRA.
    for px in bitmap.rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: bitmap.width as i32,
            biHeight: -(bitmap.height as i32), // negative: top-down rows
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [unsafe { mem::zeroed() }],
    };
    // SAFETY: `bitmap.rgba` holds width*height*4 bytes matching `info`; `dc` is valid.
    let drawn = unsafe {
        StartPage(dc);
        let lines = StretchDIBits(
            dc,
            x,
            y,
            dest_w,
            dest_h,
            0,
            0,
            bitmap.width as i32,
            bitmap.height as i32,
            bitmap.rgba.as_ptr().cast(),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
        EndPage(dc);
        lines
    };
    if drawn == 0 {
        return Err(render_error(
            "the printer driver rejected the page image".into(),
        ));
    }
    Ok(())
}
