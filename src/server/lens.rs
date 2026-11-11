//! Contagem de referências em cima de cada função, como "3 referências".
//!
//! O clique chama o comando `senior-lsp.mostrarReferencias` com o URI, a posição do nome e os
//! locais. O comando não é do servidor. A extensão do VS Code registra ele e abre o painel de
//! referências do próprio editor, que precisa dos tipos do VS Code e não do JSON do protocolo.

use lsp_types::{CodeLens, Command};

use super::navigation::Context;
use crate::SyntaxKind;
use crate::symbols::DefKind;

pub(crate) const SHOW_REFERENCES: &str = "senior-lsp.mostrarReferencias";

impl Context<'_> {
    pub(crate) fn code_lenses(&self) -> Vec<CodeLens> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let definitions = &self.doc.symbols.definitions;
        let mut lenses = Vec::new();
        for def in definitions {
            if def.kind != DefKind::Function {
                continue;
            }
            // A contagem fica na `funcao`. Só a função que tem apenas `definir funcao` mostra na
            // declaração.
            let has_implementation = definitions.iter().any(|d| {
                d.kind == DefKind::Function
                    && tree.kind(d.node) == SyntaxKind::FuncImpl
                    && text[d.name].eq_ignore_ascii_case(&text[def.name])
            });
            if tree.kind(def.node) == SyntaxKind::FuncDecl && has_implementation {
                continue;
            }
            let range = self.range(def.name);
            let locations = self.references(range.start, false);
            let title = match locations.len() {
                1 => "1 referência".to_owned(),
                n => format!("{n} referências"),
            };
            lenses.push(CodeLens {
                range,
                command: Some(Command {
                    title,
                    command: SHOW_REFERENCES.to_owned(),
                    arguments: Some(vec![
                        serde_json::to_value(self.uri).unwrap_or_default(),
                        serde_json::to_value(range.start).unwrap_or_default(),
                        serde_json::to_value(&locations).unwrap_or_default(),
                    ]),
                }),
                data: None,
            });
        }
        lenses
    }
}
