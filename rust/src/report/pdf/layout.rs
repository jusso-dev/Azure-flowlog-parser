//! Top-down flow layout over PdfDocument, with automatic page breaks. Port of PdfLayout.cs.

use super::document::{PAGE_HEIGHT, PAGE_WIDTH, PdfColor, PdfDocument};
use super::text;

pub const MARGIN: f64 = 40.0;
pub const CONTENT_WIDTH: f64 = PAGE_WIDTH - 2.0 * MARGIN;
const BOTTOM_LIMIT: f64 = PAGE_HEIGHT - 56.0;
const ROW_HEIGHT: f64 = 14.0;
const HEADER_HEIGHT: f64 = 16.0;

const INK: PdfColor = PdfColor::hex(0x111827);
const MUTED: PdfColor = PdfColor::hex(0x4B5563);
const HEADER_FILL: PdfColor = PdfColor::hex(0x1F2937);
const ZEBRA: PdfColor = PdfColor::hex(0xF3F4F6);
const BORDER: PdfColor = PdfColor::hex(0xD1D5DB);
const BAR_FILL: PdfColor = PdfColor::hex(0x2563EB);
const ACCENT: PdfColor = PdfColor::hex(0x1D4ED8);

/// A table cell: text, or a bar drawn at this share (0..=1) of the usable cell width.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Text(String),
    Bar(f64),
}

pub struct PdfLayout {
    document: PdfDocument,
    page: usize,
    top: f64,
}

impl PdfLayout {
    pub fn new() -> Self {
        let mut document = PdfDocument::default();
        let page = document.add_page();
        Self { document, page, top: MARGIN }
    }

    pub fn title(&mut self, text: &str) {
        self.ensure(30.0);
        self.document.page_mut(self.page).text(text, MARGIN, self.top, 20.0, INK, true);
        self.top += 30.0;
    }

    pub fn heading(&mut self, text: &str) {
        self.ensure(40.0);
        self.top += 8.0;
        self.document.page_mut(self.page).text(text, MARGIN, self.top, 13.0, ACCENT, true);
        self.top += 18.0;
        self.document.page_mut(self.page).line(MARGIN, self.top, MARGIN + CONTENT_WIDTH, self.top, BORDER, 0.8);
        self.top += 6.0;
    }

    /// Wrapped text. Muted uses the grey ink colour.
    pub fn paragraph(&mut self, text: &str, size: f64, muted: bool, bold: bool) {
        let color = if muted { MUTED } else { INK };
        for line in text::wrap(text, CONTENT_WIDTH, size, bold) {
            self.ensure(size + 4.0);
            self.document.page_mut(self.page).text(&line, MARGIN, self.top, size, color, bold);
            self.top += size + 4.0;
        }
    }

    /// A dash in the margin, then the text indented 12pt. Wrapped lines are indented without a dash.
    pub fn bullet(&mut self, text: &str) {
        let lines = text::wrap(text, CONTENT_WIDTH - 12.0, 9.0, false);
        for (i, line) in lines.iter().enumerate() {
            self.ensure(13.0);
            if i == 0 {
                self.document.page_mut(self.page).text("-", MARGIN, self.top, 9.0, INK, false);
            }
            self.document.page_mut(self.page).text(line, MARGIN + 12.0, self.top, 9.0, INK, false);
            self.top += 13.0;
        }
    }

    pub fn space(&mut self, points: f64) {
        self.top += points;
    }

    /// Draws a table. The widths should add up to at most CONTENT_WIDTH.
    pub fn table(&mut self, headers: &[&str], widths: &[f64], rows: &[Vec<Cell>], font_size: f64) {
        self.ensure(HEADER_HEIGHT + ROW_HEIGHT);
        self.draw_header(headers, widths, font_size);

        for (r, row) in rows.iter().enumerate() {
            if self.top + ROW_HEIGHT > BOTTOM_LIMIT {
                self.new_page();
                self.draw_header(headers, widths, font_size);
            }

            if r % 2 == 1 {
                self.document.page_mut(self.page).fill_rect(MARGIN, self.top, CONTENT_WIDTH, ROW_HEIGHT, ZEBRA);
            }

            let mut x = MARGIN;
            for (c, width) in widths.iter().enumerate() {
                match row.get(c) {
                    Some(Cell::Text(value)) => self.draw_cell_text(value, x, *width, font_size),
                    Some(Cell::Bar(share)) => {
                        let bar_width = ((*width - 8.0) * share.min(1.0)).max(0.0);
                        if bar_width > 0.0 {
                            self.document.page_mut(self.page).fill_rect(
                                x + 4.0,
                                self.top + 3.0,
                                bar_width,
                                ROW_HEIGHT - 6.0,
                                BAR_FILL,
                            );
                        }
                    }
                    None => {}
                }
                x += *width;
            }

            self.top += ROW_HEIGHT;
            self.document.page_mut(self.page).line(MARGIN, self.top, MARGIN + CONTENT_WIDTH, self.top, BORDER, 0.4);
        }

        self.top += 6.0;
    }

    /// Writes "Page N of M" centred in the footer of every page. Call once, after all content.
    pub fn footers(&mut self) {
        let count = self.document.page_count();
        for index in 0..count {
            let label = format!("Page {} of {count}", index + 1);
            let x = PAGE_WIDTH / 2.0 - text::width(&label, 8.0, false) / 2.0;
            self.document.page_mut(index).text(&label, x, PAGE_HEIGHT - 34.0, 8.0, MUTED, false);
        }
    }

    /// The finished PDF bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.document.build()
    }

    fn draw_header(&mut self, headers: &[&str], widths: &[f64], font_size: f64) {
        self.document.page_mut(self.page).fill_rect(MARGIN, self.top, CONTENT_WIDTH, HEADER_HEIGHT, HEADER_FILL);
        let mut x = MARGIN;
        for (header, width) in headers.iter().zip(widths) {
            let label = text::fit(header, *width - 8.0, font_size, true);
            self.document.page_mut(self.page).text(&label, x + 4.0, self.top + 4.0, font_size, PdfColor::WHITE, true);
            x += *width;
        }
        self.top += HEADER_HEIGHT;
    }

    fn draw_cell_text(&mut self, value: &str, x: f64, width: f64, font_size: f64) {
        let fitted = text::fit(value, width - 8.0, font_size, false);
        self.document.page_mut(self.page).text(&fitted, x + 4.0, self.top + 3.0, font_size, INK, false);
    }

    fn ensure(&mut self, height: f64) {
        if self.top + height > BOTTOM_LIMIT {
            self.new_page();
        }
    }

    fn new_page(&mut self) {
        self.page = self.document.add_page();
        self.top = MARGIN;
    }
}

#[cfg(test)]
mod tests {
    use super::super::document::test_support::{assert_qpdf_clean, assert_well_formed};
    use super::*;

    #[test]
    fn long_table_paginates_and_repeats_its_header() {
        let mut layout = PdfLayout::new();
        let rows: Vec<Vec<Cell>> =
            (0..200).map(|i| vec![Cell::Text(format!("row {i}")), Cell::Bar(i as f64 / 199.0)]).collect();

        layout.table(&["Name", "Share"], &[300.0, 215.0], &rows, 8.0);

        let contents: Vec<&str> = layout.document.pages.iter().map(|page| page.content.as_str()).collect();
        assert!(contents.len() > 1, "200 rows must span several pages");
        for content in &contents {
            assert!(content.contains("(Name) Tj"), "header missing on a page");
            assert!(content.contains("(Share) Tj"), "header missing on a page");
        }
        let rows_drawn: usize = contents.iter().map(|content| content.matches("(row ").count()).sum();
        assert_eq!(rows_drawn, 200);
    }

    #[test]
    fn footers_number_every_page() {
        let mut layout = PdfLayout::new();
        let rows: Vec<Vec<Cell>> = (0..200).map(|i| vec![Cell::Text(format!("row {i}"))]).collect();
        layout.table(&["Name"], &[515.0], &rows, 8.0);

        layout.footers();

        let count = layout.document.page_count();
        assert_eq!(count, 4, "52 rows fit per page: 200 rows need four pages");
        for (i, page) in layout.document.pages.iter().enumerate() {
            assert!(page.content.contains(&format!("(Page {} of {count}) Tj", i + 1)));
        }
        let pdf = layout.into_bytes();
        assert_well_formed(&pdf);
        assert_qpdf_clean("layout-paginated-table", &pdf);
    }

    #[test]
    fn bullet_puts_the_dash_on_the_first_line_only() {
        let mut layout = PdfLayout::new();
        let note = "word ".repeat(60);

        layout.bullet(note.trim());

        let content = &layout.document.pages[0].content;
        assert_eq!(content.matches("(-) Tj").count(), 1);
        assert!(content.matches("(word").count() > 1, "wrapped lines continue without a dash");
    }
}
