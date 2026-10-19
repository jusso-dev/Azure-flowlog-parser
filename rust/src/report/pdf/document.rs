//! Minimal PDF 1.4 writer: base-14 Helvetica fonts, filled rectangles, lines and text.
//! Port of PdfDocument.cs. The page API takes top-down points on an A4 portrait page.
//! Objects: 1 catalog, 2 page tree, 3 Helvetica, 4 Helvetica-Bold, then a page object and a content stream per page.

use super::text;

pub const PAGE_WIDTH: f64 = 595.0;
pub const PAGE_HEIGHT: f64 = 842.0;

#[derive(Debug, Default)]
pub struct PdfDocument {
    pub(super) pages: Vec<PdfPage>,
}

impl PdfDocument {
    /// Appends a blank page and returns its index.
    pub fn add_page(&mut self) -> usize {
        self.pages.push(PdfPage::default());
        self.pages.len() - 1
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn page_mut(&mut self, index: usize) -> &mut PdfPage {
        &mut self.pages[index]
    }

    /// Serialises the document. Everything is built in memory so each xref offset is an exact byte position.
    pub fn build(&self) -> Vec<u8> {
        let blank = [PdfPage::default()];
        let pages: &[PdfPage] = if self.pages.is_empty() { &blank } else { &self.pages };
        let page_ids: Vec<usize> = (0..pages.len()).map(|i| 5 + 2 * i).collect();

        let mut out = ObjectWriter::new(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
        out.object("<< /Type /Catalog /Pages 2 0 R >>\n");
        let kids: Vec<String> = page_ids.iter().map(|id| format!("{id} 0 R")).collect();
        out.object(&format!("<< /Type /Pages /Kids [{}] /Count {} >>\n", kids.join(" "), pages.len()));
        out.object("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\n");
        out.object("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>\n");

        for (page, page_id) in pages.iter().zip(&page_ids) {
            let content_id = page_id + 1;
            out.object(&format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {content_id} 0 R >>\n"
            ));
            out.object(&format!("<< /Length {} >>\nstream\n{}\nendstream\n", page.content.len(), page.content));
        }

        let xref_offset = out.bytes.len();
        let size = out.offsets.len() + 1;
        // Each in-use entry is exactly 20 bytes, as the xref format requires.
        let entries: String = out.offsets.iter().map(|offset| format!("{offset:010} 00000 n \n")).collect();
        out.text(&format!(
            "xref\n0 {size}\n0000000000 65535 f \n{entries}trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
        ));
        out.bytes
    }
}

/// Appends objects and records the byte offset of each one for the xref table.
struct ObjectWriter {
    bytes: Vec<u8>,
    offsets: Vec<usize>,
}

impl ObjectWriter {
    fn new(header: &[u8]) -> Self {
        Self { bytes: header.to_vec(), offsets: Vec::new() }
    }

    /// Writes the next object. Objects are numbered in write order, so the offset list has no gaps.
    fn object(&mut self, body: &str) {
        self.offsets.push(self.bytes.len());
        let number = self.offsets.len();
        self.text(&format!("{number} 0 obj\n{body}endobj\n"));
    }

    fn text(&mut self, text: &str) {
        self.bytes.extend_from_slice(text.as_bytes());
    }
}

/// One page's content stream.
#[derive(Debug, Default)]
pub struct PdfPage {
    pub(super) content: String,
}

impl PdfPage {
    /// Draws text with its top edge at `top`. The text is sanitized and escaped here.
    pub fn text(&mut self, text: &str, x: f64, top: f64, size: f64, color: PdfColor, bold: bool) {
        let baseline = PAGE_HEIGHT - top - size;
        let font = if bold { "F2" } else { "F1" };
        self.content.push_str(&format!(
            "BT /{font} {} Tf {} rg 1 0 0 1 {} {} Tm ({}) Tj ET\n",
            num(size),
            color.components(),
            num(x),
            num(baseline),
            text::escape(text)
        ));
    }

    pub fn fill_rect(&mut self, x: f64, top: f64, width: f64, height: f64, color: PdfColor) {
        let bottom = PAGE_HEIGHT - top - height;
        self.content.push_str(&format!(
            "q {} rg {} {} {} {} re f Q\n",
            color.components(),
            num(x),
            num(bottom),
            num(width),
            num(height)
        ));
    }

    pub fn line(&mut self, x1: f64, top1: f64, x2: f64, top2: f64, color: PdfColor, width: f64) {
        let y1 = PAGE_HEIGHT - top1;
        let y2 = PAGE_HEIGHT - top2;
        self.content.push_str(&format!(
            "q {} RG {} w {} {} m {} {} l S Q\n",
            color.components(),
            num(width),
            num(x1),
            num(y1),
            num(x2),
            num(y2)
        ));
    }
}

/// RGB colour with components in 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdfColor {
    r: f64,
    g: f64,
    b: f64,
}

impl PdfColor {
    pub const WHITE: PdfColor = PdfColor::hex(0xFFFFFF);

    /// From 0xRRGGBB.
    pub const fn hex(value: u32) -> Self {
        Self {
            r: ((value >> 16) & 0xFF) as f64 / 255.0,
            g: ((value >> 8) & 0xFF) as f64 / 255.0,
            b: (value & 0xFF) as f64 / 255.0,
        }
    }

    /// Operands only ("r g b"); the caller adds the rg (fill) or RG (stroke) operator.
    fn components(&self) -> String {
        format!("{} {} {}", num(self.r), num(self.g), num(self.b))
    }
}

/// Up to three decimals with trailing zeros removed, as the .NET "0.###" format does.
fn num(value: f64) -> String {
    format!("{value:.3}").trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
pub(super) mod test_support {
    use std::process::Command;

    /// Checks what a reader relies on: the header, the startxref pointer, the 20-byte xref entries,
    /// each in-use offset pointing at "N 0 obj", and the trailer directly after the entries.
    pub fn assert_well_formed(pdf: &[u8]) {
        assert!(pdf.starts_with(b"%PDF-1.4\n"), "missing PDF header");
        assert!(pdf.ends_with(b"%%EOF\n"), "missing %%EOF");

        let startxref = rfind(pdf, b"startxref\n").expect("startxref keyword");
        let xref_offset: usize = line_at(pdf, startxref + b"startxref\n".len()).parse().expect("startxref offset");
        assert!(pdf[xref_offset..].starts_with(b"xref\n"), "startxref does not point at xref");

        let subsection = line_at(pdf, xref_offset + b"xref\n".len());
        assert!(subsection.starts_with("0 "), "xref subsection header: {subsection}");
        let count: usize = subsection[2..].parse().expect("xref entry count");
        let entries_start = xref_offset + b"xref\n".len() + subsection.len() + 1;
        let entries_end = entries_start + 20 * count;
        assert!(pdf.len() >= entries_end, "xref entries run past end of file");

        for (number, entry) in pdf[entries_start..entries_end].chunks(20).enumerate() {
            assert_eq!(entry.len(), 20);
            assert_eq!(entry[19], b'\n');
            if number == 0 {
                assert_eq!(entry, b"0000000000 65535 f \n");
                continue;
            }
            assert_eq!(&entry[10..], b" 00000 n \n", "entry {number}");
            let offset: usize = std::str::from_utf8(&entry[..10]).unwrap().parse().unwrap();
            let expected = format!("{number} 0 obj\n");
            assert!(pdf[offset..].starts_with(expected.as_bytes()), "object {number} offset {offset}");
        }

        let trailer = format!("trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n");
        assert!(pdf[entries_end..].starts_with(trailer.as_bytes()), "trailer after xref entries");
    }

    /// Runs `qpdf --check` on the bytes, with the file written to the temp dir. Skips when qpdf is not installed.
    pub fn assert_qpdf_clean(name: &str, pdf: &[u8]) {
        let Some(qpdf) = qpdf_binary() else {
            eprintln!("skipping qpdf check for {name}: qpdf is not installed");
            return;
        };
        let dir = std::env::temp_dir().join("azure-flowlog-parser-pdf-tests");
        std::fs::create_dir_all(&dir).expect("create test dir");
        let path = dir.join(format!("{name}.pdf"));
        std::fs::write(&path, pdf).expect("write test pdf");

        let output = Command::new(qpdf).arg("--check").arg(&path).output().expect("run qpdf");
        assert!(
            output.status.success(),
            "qpdf --check {} failed:\n{}{}",
            path.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// qpdf on PATH, or the Homebrew location used on this machine.
    fn qpdf_binary() -> Option<&'static str> {
        ["qpdf", "/opt/homebrew/bin/qpdf"].into_iter().find(|bin| Command::new(bin).arg("--version").output().is_ok())
    }

    fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).rposition(|window| window == needle)
    }

    /// The text of the line starting at `start`, without its newline.
    fn line_at(pdf: &[u8], start: usize) -> &str {
        let end = start + pdf[start..].iter().position(|&b| b == b'\n').expect("line end");
        std::str::from_utf8(&pdf[start..end]).expect("ascii line")
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{assert_qpdf_clean, assert_well_formed};
    use super::*;

    #[test]
    fn empty_document_still_writes_one_blank_page() {
        let pdf = PdfDocument::default().build();

        assert_well_formed(&pdf);
        assert!(String::from_utf8_lossy(&pdf).contains("/Count 1 >>"));
        assert_qpdf_clean("document-empty", &pdf);
    }

    #[test]
    fn multi_page_document_has_consistent_xref_and_escaped_text() {
        let mut document = PdfDocument::default();
        for i in 0..3 {
            let index = document.add_page();
            document.page_mut(index).text(&format!("page {i} (a)"), 40.0, 40.0, 9.0, PdfColor::hex(0x111827), i == 1);
        }

        let pdf = document.build();

        assert_well_formed(&pdf);
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("/Count 3 >>"));
        assert!(text.contains("(page 0 \\(a\\)) Tj"));
        assert!(text.contains("/F2 9 Tf"));
        assert_qpdf_clean("document-three-pages", &pdf);
    }

    #[test]
    fn numbers_drop_trailing_zeros_like_the_dotnet_format() {
        assert_eq!(num(40.0), "40");
        assert_eq!(num(0.5), "0.5");
        assert_eq!(num(0.0), "0");
        assert_eq!(num(786.1234), "786.123");
    }
}
