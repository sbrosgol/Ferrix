//! Minimal hand-written PDFs, so engine tests don't depend on binary files or on the engine
//! under test to produce its own input.

/// One page of a fixture document.
pub(crate) struct Page {
    pub(crate) media: [f32; 4],
    pub(crate) crop: Option<[f32; 4]>,
    pub(crate) rotate: i32,
    pub(crate) content: String,
}

impl Page {
    /// US Letter with a 100×100 pt red square at the PDF origin and "Hello" at (72, 700).
    pub(crate) fn letter() -> Self {
        Self {
            media: [0.0, 0.0, 612.0, 792.0],
            crop: None,
            rotate: 0,
            content: "1 0 0 rg 0 0 100 100 re f\nBT /F1 24 Tf 0 g 72 700 Td (Hello) Tj ET".into(),
        }
    }

    pub(crate) fn rotated(mut self, degrees: i32) -> Self {
        self.rotate = degrees;
        self
    }

    pub(crate) fn cropped(mut self, crop: [f32; 4]) -> Self {
        self.crop = Some(crop);
        self
    }
}

/// Builds a PDF with the given pages, an Info title, and one outline entry per page.
pub(crate) fn pdf(title: &str, pages: &[Page]) -> Vec<u8> {
    // Object numbers: 1 catalog, 2 page tree, 3 font, 4 info, 5 outline root,
    // then per page i: page, content stream, outline item.
    let first_page = 6;
    let page_obj = |i: usize| first_page + i * 3;
    let mut objects: Vec<String> = Vec::new();

    objects.push("<< /Type /Catalog /Pages 2 0 R /Outlines 5 0 R >>".into());
    let kids: Vec<_> = (0..pages.len())
        .map(|i| format!("{} 0 R", page_obj(i)))
        .collect();
    objects.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        kids.join(" "),
        pages.len()
    ));
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    objects.push(format!("<< /Title ({title}) /Producer (ferrix-tests) >>"));
    let (first_item, last_item) = (page_obj(0) + 2, page_obj(pages.len() - 1) + 2);
    objects.push(format!(
        "<< /Type /Outlines /First {first_item} 0 R /Last {last_item} 0 R /Count {} >>",
        pages.len()
    ));

    for (i, page) in pages.iter().enumerate() {
        let n = page_obj(i);
        let rect = |r: [f32; 4]| format!("[{} {} {} {}]", r[0], r[1], r[2], r[3]);
        let crop = page
            .crop
            .map(|c| format!(" /CropBox {}", rect(c)))
            .unwrap_or_default();
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox {}{crop} /Rotate {} \
             /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            rect(page.media),
            page.rotate,
            n + 1
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            page.content.len(),
            page.content
        ));
        // Outline items are the third object of each page group, so siblings are 3 apart.
        let prev = if i > 0 {
            format!(" /Prev {} 0 R", n - 1)
        } else {
            String::new()
        };
        let next = if i + 1 < pages.len() {
            format!(" /Next {} 0 R", n + 5)
        } else {
            String::new()
        };
        objects.push(format!(
            "<< /Title (Page {}) /Parent 5 0 R{prev}{next} /Dest [{n} 0 R /Fit] >>",
            i + 1
        ));
    }

    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info 4 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}
