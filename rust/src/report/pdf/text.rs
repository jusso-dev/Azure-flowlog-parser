//! Text helpers for the PDF writer: printable-ASCII sanitizing, string escaping and Helvetica width estimates.
//! Port of the PdfText class in PdfDocument.cs. Widths are estimates: 0.52em per character, 0.6em in bold.

/// Replaces every character outside printable ASCII with '?'.
pub fn sanitize(text: &str) -> String {
    text.chars().map(|c| if (' '..='~').contains(&c) { c } else { '?' }).collect()
}

/// Sanitizes, then escapes the characters that are special inside a PDF literal string.
pub fn escape(text: &str) -> String {
    sanitize(text).replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)")
}

fn advance(bold: bool) -> f64 {
    if bold { 0.6 } else { 0.52 }
}

/// Estimated width in points.
pub fn width(text: &str, size: f64, bold: bool) -> f64 {
    text.chars().count() as f64 * size * advance(bold)
}

/// Truncates to fit `max_width`, ending in ".." when it has to cut.
pub fn fit(text: &str, max_width: f64, size: f64, bold: bool) -> String {
    let text = sanitize(text);
    if width(&text, size, bold) <= max_width {
        return text;
    }

    let max_chars = ((max_width / (size * advance(bold))).floor() as i64 - 2).max(0) as usize;
    let kept: String = text.chars().take(max_chars).collect();
    format!("{kept}..")
}

/// Splits on spaces into lines no wider than `max_width`. A word wider than the line keeps a line of its own.
pub fn wrap(text: &str, max_width: f64, size: f64, bold: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in sanitize(text).split(' ').filter(|word| !word.is_empty()) {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if width(&candidate, size, bold) > max_width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            line.push_str(word);
        } else {
            line = candidate;
        }
    }

    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_non_printable_characters() {
        assert_eq!(sanitize("Café\tok~"), "Caf??ok~");
    }

    #[test]
    fn escape_protects_parentheses_and_backslashes() {
        assert_eq!(escape(r"a(b)c\d"), r"a\(b\)c\\d");
    }

    #[test]
    fn fit_truncates_with_two_dots() {
        // 0.52 * 8 = 4.16pt per character: 40pt holds 9 characters, and the ".." takes 2 of them.
        assert_eq!(fit("abcdefghijklmnop", 40.0, 8.0, false), "abcdefg..");
        assert_eq!(fit("short", 40.0, 8.0, false), "short");
    }

    #[test]
    fn wrap_breaks_on_spaces_within_width() {
        // 0.52 * 10 = 5.2pt per character: "aa bb" (26pt) fits in 27pt, "aa bb cc" does not.
        assert_eq!(wrap("aa bb cc dd", 27.0, 10.0, false), vec!["aa bb", "cc dd"]);
    }
}
