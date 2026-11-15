//! Tables: layout, editing, structure changes, export.

#![allow(clippy::unwrap_used)]

use ferrix_compose::{
    CharFormat, Document, Editor, Fonts, Layout, Motion, Opened, Paragraph, ParagraphStyle,
    Position, Rasterizer, TableOp, TextRange, export_pdf, open_pdf,
};

fn editor(paragraphs: &[&str]) -> Editor {
    let mut doc = Document::new();
    doc.paragraphs = paragraphs
        .iter()
        .map(|t| Paragraph::new(ParagraphStyle::Normal).with_text(t, CharFormat::default()))
        .collect();
    Editor::new(doc, Fonts::bundled())
}

fn texts(e: &Editor) -> Vec<String> {
    e.document()
        .paragraphs
        .iter()
        .map(Paragraph::text)
        .collect()
}

/// Cell contents as rows of columns of text (joining a cell's paragraphs with `|`).
fn grid(e: &Editor) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in &e.document().paragraphs {
        let Some(cell) = p.cell else { continue };
        let (r, c) = (cell.row as usize, cell.col as usize);
        if rows.len() <= r {
            rows.resize(r + 1, Vec::new());
        }
        if rows[r].len() <= c {
            rows[r].resize(c + 1, String::new());
        }
        if !seen.insert((r, c)) {
            rows[r][c].push('|');
        }
        rows[r][c].push_str(&p.text());
    }
    rows
}

/// A 2 × 3 table with the caret in its first cell, inside "Before"/"After" text.
fn table() -> Editor {
    let mut e = editor(&["Before", ""]);
    e.move_caret(Motion::DocumentEnd, false);
    e.insert_table(2, 3);
    e
}

#[test]
fn inserting_a_table_lays_out_a_grid() {
    let e = table();
    assert_eq!(
        e.document().paragraphs.len(),
        1 + 6 + 1,
        "before, six cells, after"
    );
    assert!(e.in_table());
    assert_eq!(e.focus(), Position::new(1, 0));
    assert_eq!(grid(&e).len(), 2);
    assert!(grid(&e).iter().all(|r| r.len() == 3));

    let page = &e.layout().pages[0];
    // Cells of a row sit side by side, left to right, at the same height.
    let cell_lines: Vec<_> = page
        .lines
        .iter()
        .filter(|l| (1..=3).contains(&l.block))
        .collect();
    assert_eq!(cell_lines.len(), 3);
    assert!(
        cell_lines
            .windows(2)
            .all(|w| w[0].left < w[1].left && w[0].top == w[1].top)
    );
    // The paragraph after the table is below it.
    let after = page.lines.iter().find(|l| l.block == 7).unwrap();
    assert!(after.top > cell_lines[0].top + 30.0);
    // Borders: two horizontal and four vertical per row.
    assert_eq!(page.rules.len(), 2 * (2 + 4));

    // No tables in tables.
    let mut e = e;
    e.insert_table(2, 2);
    assert_eq!(e.document().paragraphs.len(), 8);
}

#[test]
fn typing_tab_and_rows_growing() {
    let mut e = table();
    e.type_text("Name");
    e.tab(false);
    e.type_text("Qty");
    e.tab(false);
    e.type_text("Price");
    e.tab(false);
    e.type_text("Apples");
    assert_eq!(grid(&e)[0], ["Name", "Qty", "Price"]);
    assert_eq!(grid(&e)[1][0], "Apples");

    // Shift+Tab goes back; Tab from the last cell adds a row.
    e.tab(true);
    assert_eq!(e.selected_text(), "Price");
    e.tab(false);
    e.tab(false);
    e.tab(false);
    assert_eq!(grid(&e).len(), 2);
    e.tab(false);
    assert_eq!(grid(&e).len(), 3, "a new row");
    assert_eq!(
        e.document().paragraphs[e.focus().block].cell.unwrap().row,
        2
    );

    // Enter makes a second paragraph in the same cell; the row grows.
    let before = e.layout().pages[0]
        .rules
        .iter()
        .map(|r| r.thickness)
        .fold(0.0, f32::max);
    e.type_text("one");
    e.enter();
    e.type_text("two");
    let after = e.layout().pages[0]
        .rules
        .iter()
        .map(|r| r.thickness)
        .fold(0.0, f32::max);
    assert!(after > before, "the row is taller");
    assert_eq!(grid(&e)[2][0], "one|two");
}

#[test]
fn deleting_never_merges_cells() {
    let mut e = table();
    e.type_text("A");
    e.tab(false);
    e.type_text("B");
    let cells = e.document().paragraphs.len();

    // Backspace at the start of a cell, Delete at the end of one: nothing happens.
    e.select(TextRange::caret(Position::new(2, 0)));
    e.backspace(false);
    e.select(TextRange::caret(Position::new(1, 1)));
    e.delete_forward(false);
    assert_eq!(grid(&e)[0][..2], ["A", "B"]);
    assert_eq!(e.document().paragraphs.len(), cells);

    // A selection across cells is cleared cell by cell; the table keeps its shape.
    e.select(TextRange::between(Position::new(0, 3), Position::new(2, 1)));
    e.backspace(false);
    assert_eq!(texts(&e)[0], "Bef");
    assert_eq!(grid(&e)[0][..2], ["", ""]);
    assert_eq!(e.document().paragraphs.len(), cells);
    e.undo();
    assert_eq!(texts(&e)[0], "Before");
    assert_eq!(grid(&e)[0][..2], ["A", "B"]);

    // Typing over such a selection puts the text in the first part.
    e.select(TextRange::between(Position::new(1, 0), Position::new(2, 1)));
    e.type_text("X");
    assert_eq!(grid(&e)[0][..2], ["X", ""]);
}

#[test]
fn rows_and_columns_are_added_and_removed() {
    let mut e = table();
    e.type_text("a");
    let original = e.document().clone();

    e.table_op(TableOp::RowBelow);
    assert_eq!(grid(&e).len(), 3);
    assert_eq!(
        e.document().paragraphs[e.focus().block].cell.unwrap().row,
        1
    );
    e.table_op(TableOp::ColumnRight);
    assert!(grid(&e).iter().all(|r| r.len() == 4));
    e.table_op(TableOp::DeleteColumn);
    e.table_op(TableOp::DeleteRow);
    assert_eq!(grid(&e).len(), 2);
    assert!(grid(&e).iter().all(|r| r.len() == 3));
    assert_eq!(grid(&e)[0][0], "a");

    for _ in 0..4 {
        e.undo();
    }
    assert_eq!(e.document(), &original);

    e.table_op(TableOp::DeleteTable);
    assert_eq!(texts(&e), ["Before", "", ""]);
    assert!(e.document().paragraphs.iter().all(|p| p.cell.is_none()));
    e.undo();
    assert_eq!(e.document(), &original);
}

#[test]
fn caret_moves_by_column_and_clicks_find_cells() {
    let mut e = table();
    e.tab(false); // cell (0, 1)
    e.type_text("top");
    e.move_caret(Motion::Down, false);
    let cell = e.document().paragraphs[e.focus().block].cell.unwrap();
    assert_eq!((cell.row, cell.col), (1, 1), "Down stays in the column");
    e.move_caret(Motion::Down, false);
    assert!(
        e.document().paragraphs[e.focus().block].cell.is_none(),
        "out below the table"
    );

    // Clicking in the middle of cell (1, 2).
    let page = &e.layout().pages[0];
    let line = page
        .lines
        .iter()
        .find(|l| {
            e.document().paragraphs[l.block]
                .cell
                .is_some_and(|c| (c.row, c.col) == (1, 2))
        })
        .unwrap()
        .clone();
    e.click(
        0,
        line.left + 5.0,
        line.top + line.height / 2.0,
        ferrix_compose::Clicks::Single,
        false,
    );
    let cell = e.document().paragraphs[e.focus().block].cell.unwrap();
    assert_eq!((cell.row, cell.col), (1, 2));
}

#[test]
fn long_tables_break_between_rows() {
    let mut e = editor(&[""]);
    e.insert_table(80, 2);
    let pages = &e.layout().pages;
    assert!(pages.len() >= 2);
    let doc = e.document();
    // Every row's cells are on one page.
    for row in 0..80u32 {
        let on: Vec<usize> = pages
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.lines
                    .iter()
                    .any(|l| doc.paragraphs[l.block].cell.is_some_and(|c| c.row == row))
            })
            .map(|(i, _)| i)
            .collect();
        assert_eq!(on.len(), 1, "row {row} split over pages {on:?}");
    }
    let bottom = doc.page.height - doc.page.margins.bottom;
    for page in pages {
        for rule in &page.rules {
            assert!(rule.top + rule.thickness <= bottom + 0.01);
        }
    }
}

#[test]
fn tables_draw_save_and_reopen() {
    let mut e = table();
    e.type_text("cell text");
    let doc = e.document().clone();
    assert_eq!(Document::from_json(&doc.to_json()).unwrap(), doc);

    let mut rasterizer = Rasterizer::new();
    let raster = e.rasterize(&mut rasterizer, 0, 1.0).unwrap();
    let rule = &e.layout().pages[0].rules[0];
    let i = ((rule.top + 0.3) as usize * raster.width as usize + (rule.x + 20.0) as usize) * 4;
    assert!(
        raster.pixels[i] < 240,
        "border drawn: {rule:?} pixel {:?}",
        &raster.pixels[i..i + 4]
    );

    let mut fonts = Fonts::bundled();
    let layout = Layout::new(&doc, &mut fonts);
    let pdf = export_pdf(&doc, &layout, &fonts).unwrap();
    assert_eq!(open_pdf(&pdf).unwrap(), Opened::Editable(doc));
    let tagged = String::from_utf8_lossy(&pdf);
    assert!(tagged.contains("/TD") && tagged.contains("/TR") && tagged.contains("/Table"));
}

#[test]
fn html_tables_round_trip() {
    use ferrix_compose::{fragment_from_html, fragment_to_html};
    let mut e = table();
    e.type_text("x");
    e.select_all();
    let html = fragment_to_html(&e.copy_fragment().unwrap());
    assert!(html.contains("<table") && html.contains("<td"));
    let back = fragment_from_html(&html).unwrap();
    let cells = back
        .paragraphs()
        .iter()
        .filter(|p| p.cell.is_some())
        .count();
    assert_eq!(cells, 6);

    // Pasting a table into a document gives it its own table number.
    let mut target = editor(&["Intro"]);
    target.move_caret(Motion::DocumentEnd, false);
    target.paste_fragment(&back);
    let tables: std::collections::BTreeSet<u32> = target
        .document()
        .paragraphs
        .iter()
        .filter_map(|p| p.cell.map(|c| c.table))
        .collect();
    assert_eq!(tables.len(), 1);
    // The copy started with "Before", which joins the paragraph at the caret.
    assert_eq!(target.document().paragraphs[0].text(), "IntroBefore");
    assert!(target.document().paragraphs[0].cell.is_none());
}
