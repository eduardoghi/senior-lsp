//! Destaque das ocorrências do nome que está no cursor, no arquivo aberto.

use std::collections::HashSet;

use lsp_types::{DocumentHighlight, DocumentHighlightKind, Position};

use super::navigation::{Context, name_at};
use crate::SyntaxKind;

impl Context<'_> {
    /// Definições e usos do nome no cursor. A definição vem marcada como escrita e os usos como
    /// leitura. Um nome que o arquivo não define, como uma função do ERP, marca as outras
    /// ocorrências do mesmo nome no mesmo papel, chamada ou valor.
    pub(crate) fn document_highlight(&self, position: Position) -> Vec<DocumentHighlight> {
        let offset = self.offset(position);
        let symbols = &self.doc.symbols;
        let mut highlights = Vec::new();
        if let Some(targets) = symbols.at(offset) {
            // `definir funcao` e `funcao` são duas definições do mesmo nome, e uma referência
            // alcança as duas. Juntar os alvos das referências faz a declaração marcar a
            // implementação e o contrário.
            let mut targets: HashSet<usize> = targets.into_iter().collect();
            for r in &symbols.references {
                if r.targets.iter().any(|t| targets.contains(t)) {
                    targets.extend(&r.targets);
                }
            }
            for &t in &targets {
                highlights.push((symbols.definitions[t].name, DocumentHighlightKind::WRITE));
            }
            for r in &symbols.references {
                if r.targets.iter().any(|t| targets.contains(t)) {
                    highlights.push((r.range, DocumentHighlightKind::READ));
                }
            }
        } else if let Some((node, call)) = name_at(&self.doc.tree, offset) {
            let tree = &self.doc.tree;
            let text = &self.doc.text;
            let name = &text[tree.text_range(node)];
            let mut stack = vec![tree.root()];
            while let Some(n) = stack.pop() {
                stack.extend(tree.children(n));
                if tree.kind(n) != SyntaxKind::NameRef {
                    continue;
                }
                let range = tree.text_range(n);
                // O mesmo papel que o `name_at` dá, chamada ou valor. O campo depois do ponto fica
                // de fora.
                let parent = tree.parent(n);
                let first = parent.is_some_and(|p| tree.children(p).next() == Some(n));
                let role = match parent.map(|p| tree.kind(p)) {
                    Some(SyntaxKind::FieldExpr) if !first => None,
                    Some(SyntaxKind::CallExpr) if first => Some(true),
                    _ => Some(false),
                };
                let same_role = role == Some(call);
                if same_role && text[range].eq_ignore_ascii_case(name) {
                    highlights.push((range, DocumentHighlightKind::TEXT));
                }
            }
        }
        highlights.sort_by_key(|(range, _)| range.start());
        highlights.dedup_by_key(|(range, _)| range.start());
        highlights
            .into_iter()
            .map(|(range, kind)| DocumentHighlight {
                range: self.range(range),
                kind: Some(kind),
            })
            .collect()
    }
}
