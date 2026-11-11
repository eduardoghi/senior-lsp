//! Trechos que o editor pode recolher: cada `funcao`, os blocos `{ }` e `inicio`/`fim`, os
//! comentários `/* */` de várias linhas e cada sequência de `definir` do nível de cima.

use lsp_types::{FoldingRange, FoldingRangeKind};
use text_size::TextRange;

use super::documents::Document;
use crate::SyntaxKind;
use crate::line_index::ColumnEncoding;

pub(crate) fn ranges(doc: &Document, encoding: ColumnEncoding) -> Vec<FoldingRange> {
    let tree = &doc.tree;
    let line = |offset| doc.index.line_col(&doc.text, offset, encoding).line;
    let mut ranges = Vec::new();
    let mut push = |range: TextRange, kind: Option<FoldingRangeKind>, keep_last: bool| {
        let (start, end) = (line(range.start()), line(range.end()));
        // Num bloco a última linha é a do fecho, que fica à mostra. O comentário e a função
        // recolhem até o fim.
        let end = if keep_last {
            end.saturating_sub(1)
        } else {
            end
        };
        if end > start {
            ranges.push(FoldingRange {
                start_line: start,
                end_line: end,
                kind,
                ..FoldingRange::default()
            });
        }
    };

    let mut stack = vec![tree.root()];
    while let Some(node) = stack.pop() {
        stack.extend(tree.children(node));
        match tree.kind(node) {
            SyntaxKind::FuncImpl => push(trimmed(doc, tree.text_range(node)), None, false),
            SyntaxKind::Block => push(tree.text_range(node), None, true),
            _ => {}
        }
    }

    let tokens = tree.tokens();
    for i in 0..tokens.len() {
        if tokens.kind(i) == SyntaxKind::BlockComment {
            push(tokens.range(i), Some(FoldingRangeKind::Comment), false);
        }
    }

    // Os `definir` seguidos do nível de cima, como o cabeçalho de declarações da regra.
    let root = tree.root();
    let mut run: Option<TextRange> = None;
    for child in tree.children(root) {
        let definition = matches!(
            tree.kind(child),
            SyntaxKind::VarDef
                | SyntaxKind::CursorDef
                | SyntaxKind::ListDef
                | SyntaxKind::TableDef
                | SyntaxKind::WsDef
                | SyntaxKind::FuncDecl
        );
        let range = tree.text_range(child);
        run = match (run, definition) {
            (Some(r), true) => Some(r.cover(range)),
            (None, true) => Some(range),
            (Some(r), false) => {
                push(r, Some(FoldingRangeKind::Imports), false);
                None
            }
            (None, false) => None,
        };
    }
    if let Some(r) = run {
        push(r, Some(FoldingRangeKind::Imports), false);
    }

    ranges.sort_by_key(|r| (r.start_line, std::cmp::Reverse(r.end_line)));
    ranges
}

/// Trecho sem as linhas em branco do fim. A `funcao` vai até a próxima, e as linhas vazias entre
/// as duas não devem sumir junto.
fn trimmed(doc: &Document, range: TextRange) -> TextRange {
    let text = &doc.text[range];
    let len = text.trim_end().len() as u32;
    TextRange::at(range.start(), len.into())
}
