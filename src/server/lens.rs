//! Contagem de referências em cima de cada função, como "3 referências".
//!
//! O clique chama o comando `senior-lsp.mostrarReferencias` com o URI, a posição do nome e os
//! locais. O comando não é do servidor. A extensão do VS Code registra ele e abre o painel de
//! referências do próprio editor, que precisa dos tipos do VS Code e não do JSON do protocolo.

use lsp_types::{CodeLens, Command};
use rustc_hash::FxHashSet;
use text_size::TextRange;

use super::navigation::Context;
use crate::SyntaxKind;
use crate::symbols::DefKind;

pub(crate) const SHOW_REFERENCES: &str = "senior-lsp.mostrarReferencias";

impl Context<'_> {
    pub(crate) fn code_lenses(&self) -> Vec<CodeLens> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let symbols = &self.doc.symbols;
        let definitions = &symbols.definitions;
        // O editor pede as lentes depois de cada mudança, então as chamadas de todas as funções
        // saem de uma passada só pelas referências. Toda chamada alcança as mesmas definições, o
        // `definir funcao` e a `funcao` de mesmo nome, que ficam num grupo com os trechos das
        // chamadas.
        let mut group_of = vec![usize::MAX; definitions.len()];
        let mut groups: Vec<Vec<TextRange>> = Vec::new();
        for reference in &symbols.references {
            let Some(&first) = reference.targets.first() else {
                continue;
            };
            if definitions[first].kind != DefKind::Function {
                continue;
            }
            if group_of[first] == usize::MAX {
                for &target in &reference.targets {
                    group_of[target] = groups.len();
                }
                groups.push(Vec::new());
            }
            groups[group_of[first]].push(reference.range);
        }
        let implemented: FxHashSet<String> = definitions
            .iter()
            .filter(|d| d.kind == DefKind::Function && tree.kind(d.node) == SyntaxKind::FuncImpl)
            .map(|d| text[d.name].to_ascii_uppercase())
            .collect();
        // Função deste arquivo num formulário SGI também é usada nas regras seguintes, que são
        // consultadas uma vez para todas as funções.
        let following = match crate::form::path_from_uri(self.uri.as_str()) {
            Some(path) if self.doc.context == crate::builtins::Context::Sgi => {
                crate::form::following_uses(&path, None)
            }
            _ => Vec::new(),
        };

        let mut lenses = Vec::new();
        for (i, def) in definitions.iter().enumerate() {
            if def.kind != DefKind::Function {
                continue;
            }
            // A contagem fica na `funcao`. Só a função que tem apenas `definir funcao` mostra na
            // declaração.
            let name = &text[def.name];
            if tree.kind(def.node) == SyntaxKind::FuncDecl
                && implemented.contains(&name.to_ascii_uppercase())
            {
                continue;
            }
            let range = self.range(def.name);
            let mut locations: Vec<_> = groups
                .get(group_of[i])
                .into_iter()
                .flatten()
                .map(|&r| self.location(r))
                .collect();
            locations.extend(self.uses_in(&following, name));
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
