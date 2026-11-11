//! Correção rápida que esconde um diagnóstico com a diretiva `senior-lsp-disable-next-line`.

use std::collections::HashMap;

use lsp_types::{CodeAction, CodeActionKind, Diagnostic, NumberOrString, TextEdit, WorkspaceEdit};
use text_size::{TextRange, TextSize};

use super::directives::{self, Kind};
use super::navigation::Context;
use super::quickfix::{indentation, newline};

impl Context<'_> {
    /// Uma ação "Ignorar SENxxx nesta linha" para cada diagnóstico escondível que toca o trecho
    /// pedido. A diretiva entra na linha acima do começo do diagnóstico, com a indentação dela. Se
    /// a linha acima já é um `senior-lsp-disable-next-line` dessa linha, o código entra nele.
    pub(crate) fn ignore_actions(
        &self,
        range: lsp_types::Range,
        shown: &[Diagnostic],
    ) -> Vec<CodeAction> {
        if self.embedding.is_some() {
            return Vec::new();
        }
        let doc = self.doc;
        let scan = doc.scan();
        let lines = doc.code_lines();
        let mut done: Vec<(&str, u32)> = Vec::new();
        let mut actions = Vec::new();
        for d in shown {
            let Some(NumberOrString::String(code)) = &d.code else {
                continue;
            };
            let touches = d.range.start <= range.end && range.start <= d.range.end;
            let line = d.range.start.line;
            if !touches || !directives::is_suppressible(code) || done.contains(&(code, line)) {
                continue;
            }
            done.push((code, line));
            let line = line as usize;
            let above = scan.directives.iter().find(|dir| {
                let end = directives::end_line(&doc.index, dir.comment);
                dir.kind == Kind::NextLine
                    && end + 1 == line
                    && directives::next_code_line(lines, end) == Some(line)
            });
            let edit = match above.and_then(|dir| dir.codes.last()) {
                Some(last) => (TextRange::empty(last.range.end()), format!(", {code}")),
                None => {
                    let start = doc.index.line_start(line, &doc.text);
                    let rest = &doc.text[usize::from(start)..];
                    let blank = rest.len() - rest.trim_start_matches([' ', '\t']).len();
                    let at = start + TextSize::new(blank as u32);
                    let indent = indentation(&doc.text, at);
                    let text = format!(
                        "{indent}@ {} {code} @{}",
                        directives::NEXT_LINE,
                        newline(&doc.text)
                    );
                    (TextRange::empty(start), text)
                }
            };
            let edit = TextEdit::new(self.range(edit.0), edit.1);
            #[allow(clippy::mutable_key_type)]
            let changes = HashMap::from([(self.uri.clone(), vec![edit])]);
            actions.push(CodeAction {
                title: format!("Ignorar {code} nesta linha"),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![d.clone()]),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..WorkspaceEdit::default()
                }),
                ..CodeAction::default()
            });
        }
        actions
    }
}
