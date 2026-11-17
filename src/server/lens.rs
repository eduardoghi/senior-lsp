//! Contagem de referências em cima de cada função, como "3 referências".
//!
//! O clique chama o comando `senior-lsp.mostrarReferencias` com o URI, a posição do nome e os
//! locais. O comando não é do servidor. A extensão do VS Code registra ele e abre o painel de
//! referências do próprio editor, que precisa dos tipos do VS Code e não do JSON do protocolo.

use lsp_types::{CodeLens, Command};
use rustc_hash::FxHashSet;

use super::navigation::Context;
use crate::SyntaxKind;
use crate::symbols::DefKind;

pub(crate) const SHOW_REFERENCES: &str = "senior-lsp.mostrarReferencias";

impl Context<'_> {
    /// Lentes das funções, só com o trecho. O comando, com a contagem e os locais, sai no
    /// `codeLens/resolve`, que o editor pede só para as lentes na tela. Montar todas a cada tecla
    /// mandava a lista de referências de cada função do arquivo, uns 150 KB na maior regra.
    pub(crate) fn code_lenses(&self) -> Vec<CodeLens> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let definitions = &self.doc.symbols.definitions;
        let implemented: FxHashSet<String> = definitions
            .iter()
            .filter(|d| d.kind == DefKind::Function && tree.kind(d.node) == SyntaxKind::FuncImpl)
            .map(|d| text[d.name].to_ascii_uppercase())
            .collect();
        let mut lenses = Vec::new();
        for def in definitions {
            if def.kind != DefKind::Function {
                continue;
            }
            // A contagem fica na `funcao`. Só a função que tem apenas `definir funcao` mostra na
            // declaração.
            if tree.kind(def.node) == SyntaxKind::FuncDecl
                && implemented.contains(&text[def.name].to_ascii_uppercase())
            {
                continue;
            }
            lenses.push(CodeLens {
                range: self.range(def.name),
                command: None,
                data: Some(serde_json::json!({
                    "uri": self.uri.as_str(),
                    "offset": u32::from(def.name.start()),
                })),
            });
        }
        lenses
    }

    /// Comando da lente da função cujo nome começa em `offset`. `None` quando o texto mudou e o
    /// nome não está mais ali, e o editor pede as lentes de novo.
    pub(crate) fn resolve_lens(&self, offset: u32) -> Option<Command> {
        let text = &self.doc.text;
        let symbols = &self.doc.symbols;
        let definitions = &symbols.definitions;
        let i = definitions
            .iter()
            .position(|d| d.kind == DefKind::Function && u32::from(d.name.start()) == offset)?;
        let def = &definitions[i];
        // Toda chamada alcança as mesmas definições, o `definir funcao` e a `funcao` de mesmo
        // nome, então as chamadas desta função são as referências que alcançam esta definição.
        let mut locations: Vec<_> = symbols
            .references
            .iter()
            .filter(|r| {
                r.targets
                    .first()
                    .is_some_and(|&t| definitions[t].kind == DefKind::Function)
                    && r.targets.contains(&i)
            })
            .map(|r| self.location(r.range))
            .collect();
        // Função deste arquivo num formulário SGI também é usada nas regras seguintes.
        if self.doc.context == crate::builtins::Context::Sgi
            && let Some(path) = crate::form::path_from_uri(self.uri.as_str())
        {
            let following = crate::form::following_uses(&path, None);
            locations.extend(self.uses_in(&following, &text[def.name]));
        }
        let range = self.range(def.name);
        let title = match locations.len() {
            1 => "1 referência".to_owned(),
            n => format!("{n} referências"),
        };
        Some(Command {
            title,
            command: SHOW_REFERENCES.to_owned(),
            arguments: Some(vec![
                serde_json::to_value(self.uri).unwrap_or_default(),
                serde_json::to_value(range.start).unwrap_or_default(),
                serde_json::to_value(&locations).unwrap_or_default(),
            ]),
        })
    }
}
