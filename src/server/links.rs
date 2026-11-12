//! Links no JSON de modelo de relatório para as regras separadas, no `"lines": "regras/x.txt"`.

use lsp_types::{DocumentLink, Position, Range, Uri};
use text_size::TextSize;

use super::documents::ReportDocument;
use crate::line_index::ColumnEncoding;

const PREFIX: &str = "\"lines\": \"";

pub(crate) fn links(
    report: &ReportDocument,
    uri: &Uri,
    encoding: ColumnEncoding,
) -> Vec<DocumentLink> {
    let Some(dir) =
        crate::form::path_from_uri(uri.as_str()).and_then(|p| p.parent().map(|d| d.to_path_buf()))
    else {
        return Vec::new();
    };
    let json = &report.json;
    let position = |offset: usize| {
        let lc = report
            .index()
            .line_col(json, TextSize::new(offset as u32), encoding);
        Position::new(lc.line, lc.col)
    };
    let mut out = Vec::new();
    for (at, _) in json.match_indices(PREFIX) {
        let start = at + PREFIX.len();
        let Some(len) = json[start..].find('"') else {
            continue;
        };
        let relative = &json[start..start + len];
        if !relative.starts_with("regras/") {
            continue;
        }
        let target = dir.join(relative);
        let Ok(target) = crate::form::uri_from_path(&target).parse::<Uri>() else {
            continue;
        };
        out.push(DocumentLink {
            range: Range::new(position(start), position(start + len)),
            target: Some(target),
            tooltip: Some("Abrir a regra".to_owned()),
            data: None,
        });
    }
    out
}
