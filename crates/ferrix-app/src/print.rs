//! Windows raster printing: page images come from our engine (ADR-0006).

use ferrix_engine::{DocSource, PdfEngine, RenderFlags, TileRequest};
use ferrix_print::{Bitmap, PageSource, Parent, PrintJob, PrintOutcome};
use ferrix_types::{DocId, PageIndex, PixelRect, Rotation};
use iced::futures::executor::block_on;

use crate::{app::Worker, worker::flatten};

/// Shows the print dialog and spools `job`. Blocking: run it off the UI thread.
pub(crate) fn print_on_windows(
    engine: Worker,
    parent: Parent,
    job: &PrintJob,
) -> Result<PrintOutcome, String> {
    let path = job.path.clone();
    let doc = flatten(block_on(
        engine.call(move |e| e.open(DocSource::Path(path), None)),
    ))?
    .id;
    let mut pages = EnginePages {
        engine: engine.clone(),
        doc,
    };
    let result =
        ferrix_print::print_with_dialog(parent, job, &mut pages).map_err(|e| e.to_string());
    let _ = block_on(engine.call(move |e| e.close(doc)));
    result
}

/// Page images for raster printing, rendered synchronously on the print thread.
struct EnginePages {
    engine: Worker,
    doc: DocId,
}

impl PageSource for EnginePages {
    fn page_size(&mut self, page: u32) -> Result<(f32, f32), String> {
        let doc = self.doc;
        let info = block_on(self.engine.call(move |e| e.page_info(doc, PageIndex(page))));
        flatten(info).map(|info| (info.size.width, info.size.height))
    }

    fn render(&mut self, page: u32, scale: f32, rotate: bool) -> Result<Bitmap, String> {
        let doc = self.doc;
        let tile = block_on(self.engine.call(move |e| {
            let size = e.page_info(doc, PageIndex(page))?.size;
            let (mut width, mut height) = size.to_pixels(scale);
            if rotate {
                std::mem::swap(&mut width, &mut height);
            }
            e.render_tile(&TileRequest {
                doc,
                page: PageIndex(page),
                scale,
                rotation: if rotate { Rotation::R90 } else { Rotation::R0 },
                region: PixelRect::new(0, 0, width, height),
                flags: RenderFlags::default(),
            })
        }));
        flatten(tile).map(|tile| Bitmap {
            width: tile.region.width,
            height: tile.region.height,
            rgba: tile.pixels,
        })
    }
}
