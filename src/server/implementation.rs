//! Ir para a implementação: do `definir funcao` ou de uma chamada para a `funcao`, e da `funcao`
//! de volta para o `definir funcao`.

use lsp_types::{Location, Position};

use super::navigation::Context;
use crate::SyntaxKind;
use crate::symbols::DefKind;

impl Context<'_> {
    pub(crate) fn implementation(&self, position: Position) -> Vec<Location> {
        let offset = self.offset(position);
        let symbols = &self.doc.symbols;
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let Some(targets) = symbols.at(offset) else {
            // Função de outra regra da tela, que só tem a implementação lá.
            return self.definition(position);
        };
        let Some(&first) = targets
            .iter()
            .find(|&&t| symbols.definitions[t].kind == DefKind::Function)
        else {
            return Vec::new();
        };
        // O `definir funcao` e a `funcao` são definições separadas, ligadas pelo nome.
        let name = &text[symbols.definitions[first].name];
        let here = symbols
            .definitions
            .iter()
            .find(|d| d.name.contains_inclusive(offset));
        let on_implementation = here.is_some_and(|d| {
            d.kind == DefKind::Function && tree.kind(d.node) == SyntaxKind::FuncImpl
        });
        // Na própria `funcao`, o caminho é de volta para a declaração.
        let wanted = if on_implementation {
            SyntaxKind::FuncDecl
        } else {
            SyntaxKind::FuncImpl
        };
        symbols
            .definitions
            .iter()
            .filter(|d| {
                d.kind == DefKind::Function
                    && tree.kind(d.node) == wanted
                    && text[d.name].eq_ignore_ascii_case(name)
            })
            .map(|d| self.location(d.name))
            .collect()
    }
}
