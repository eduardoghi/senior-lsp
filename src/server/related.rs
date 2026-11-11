//! Outro lugar envolvido num diagnóstico, que o editor mostra junto da mensagem e deixa abrir.

use lsp_types::{Diagnostic, DiagnosticRelatedInformation, Location, NumberOrString};

use super::navigation::Context;
use crate::SyntaxKind;
use crate::symbols::DefKind;

impl Context<'_> {
    /// Acrescenta a informação relacionada aos diagnósticos que têm um outro lugar a mostrar. O
    /// comando depois do bloco (SEN061) aponta a `funcao` a que ele ainda pertence, e o parâmetro
    /// usado fora da função (SEN063) aponta o parâmetro na função dele.
    pub(crate) fn add_related(&self, diagnostics: &mut [Diagnostic]) {
        let symbols = &self.doc.symbols;
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        for d in diagnostics.iter_mut() {
            let Some(NumberOrString::String(code)) = &d.code else {
                continue;
            };
            let offset = self.offset(d.range.start);
            let related = match code.as_str() {
                "SEN061" => super::navigation::enclosing_function_at(tree, offset).and_then(|f| {
                    let name = tree
                        .children(f)
                        .find(|&c| tree.kind(c) == SyntaxKind::Name)?;
                    let range = tree.text_range(name);
                    Some((range, format!("Função {}", &text[range])))
                }),
                "SEN063" => symbols
                    .references
                    .iter()
                    .find(|r| r.range.contains_inclusive(offset))
                    .and_then(|r| r.targets.first())
                    .map(|&t| &symbols.definitions[t])
                    .filter(|def| matches!(def.kind, DefKind::Parameter(_)))
                    .map(|def| (def.name, format!("Parâmetro {}", &text[def.name]))),
                _ => None,
            };
            if let Some((range, message)) = related {
                d.related_information = Some(vec![DiagnosticRelatedInformation {
                    location: Location::new(self.uri.clone(), self.range(range)),
                    message,
                }]);
            }
        }
    }
}
