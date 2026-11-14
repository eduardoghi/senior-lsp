//! Formatação do documento e do trecho selecionado, pelo [`crate::format`]. Por enquanto só a
//! indentação, com o tamanho e o caractere que o editor manda no pedido.

use lsp_types::{FormattingOptions, Range, TextEdit};
use text_size::TextRange;

use super::documents::Document;
use crate::line_index::{ColumnEncoding, LineCol};

/// As trocas de recuo, ou `None` quando a regra não pode ser formatada, como com erro de
/// sintaxe. Com `range`, só as linhas do trecho mudam.
pub(super) fn edits(
    doc: &Document,
    encoding: ColumnEncoding,
    options: &FormattingOptions,
    range: Option<Range>,
) -> Option<Vec<TextEdit>> {
    let indent = crate::format::Indent {
        tab_size: options.tab_size,
        spaces: options.insert_spaces,
    };
    let offset = |p: lsp_types::Position| {
        let lc = LineCol {
            line: p.line,
            col: p.character,
        };
        doc.index.offset(&doc.text, lc, encoding)
    };
    let only = range.map(|r| {
        let (a, b) = (offset(r.start), offset(r.end));
        TextRange::new(a.min(b), a.max(b))
    });
    let edits = crate::format::indentation(&doc.text, &doc.tree, indent, only)?;
    let to_range = super::diagnostics::lsp_range(doc, encoding);
    Some(
        edits
            .into_iter()
            .map(|e| TextEdit::new(to_range(e.range), e.new_text))
            .collect(),
    )
}
