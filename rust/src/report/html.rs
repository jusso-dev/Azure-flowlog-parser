use std::path::Path;

use crate::model::ReportModel;

const TEMPLATE: &str = include_str!("../../../shared/report-template.html");
const DATA_PLACEHOLDER: &str = "__FLOWLOG_DATA__";

/// Embeds the model into the shared HTML template. The template comes from ../shared/report-template.html
/// at compile time (include_str!), so the binary is self-contained. "</" in the JSON becomes "<\/".
pub fn write(model: &ReportModel, path: &Path) -> anyhow::Result<()> {
    let json = serde_json::to_string(model)?.replace("</", "<\\/");
    std::fs::write(path, TEMPLATE.replace(DATA_PLACEHOLDER, &json))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_has_exactly_one_data_placeholder() {
        assert_eq!(TEMPLATE.matches(DATA_PLACEHOLDER).count(), 1);
    }

    #[test]
    fn embedded_json_never_contains_a_closing_tag_sequence() {
        let model = ReportModel { notes: vec!["</script><b>x</b>".to_string()], ..Default::default() };
        let json = serde_json::to_string(&model).unwrap().replace("</", "<\\/");
        assert!(!json.contains("</"));
        assert!(json.contains("<\\/script>"));
    }
}
