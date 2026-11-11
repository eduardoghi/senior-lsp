//! Hierarquia de chamadas: quem chama uma função e o que ela chama.
//!
//! As chamadas recebidas são as referências da função, agrupadas pela `funcao` onde cada uma
//! está. O que fica fora de qualquer `funcao` é o corpo principal da regra, que aparece como um
//! item com o nome do arquivo. Chamadas em outras regras da tela entram assim também.

use std::collections::BTreeMap;

use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Position, Range,
    SymbolKind,
};
use text_size::TextRange;

use super::navigation::{Context, enclosing_function_at};
use crate::SyntaxKind;
use crate::symbols::DefKind;

impl Context<'_> {
    /// Item da função no cursor, na `funcao` quando ela existe, senão no `definir funcao`.
    pub(crate) fn prepare_call_hierarchy(&self, position: Position) -> Vec<CallHierarchyItem> {
        let offset = self.offset(position);
        let symbols = &self.doc.symbols;
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let Some(targets) = symbols.at(offset) else {
            return Vec::new();
        };
        let Some(&first) = targets
            .iter()
            .find(|&&t| symbols.definitions[t].kind == DefKind::Function)
        else {
            return Vec::new();
        };
        let name = &text[symbols.definitions[first].name];
        let functions: Vec<_> = symbols
            .definitions
            .iter()
            .filter(|d| d.kind == DefKind::Function && text[d.name].eq_ignore_ascii_case(name))
            .collect();
        let def = functions
            .iter()
            .find(|d| tree.kind(d.node) == SyntaxKind::FuncImpl)
            .or(functions.first())
            .copied();
        def.map(|d| self.function_item(d.node, d.name))
            .into_iter()
            .collect()
    }

    /// Chamadas à função do item, agrupadas por quem chama.
    pub(crate) fn incoming_calls(
        &self,
        item: &CallHierarchyItem,
    ) -> Vec<CallHierarchyIncomingCall> {
        let tree = &self.doc.tree;
        // Chave por arquivo e começo do chamador, para juntar as chamadas de cada um.
        let mut callers: BTreeMap<(String, u32, u32), (CallHierarchyItem, Vec<Range>)> =
            BTreeMap::new();
        for location in self.references(item.selection_range.start, false) {
            let caller = if location.uri == *self.uri {
                let offset = self.offset(location.range.start);
                match enclosing_function_at(tree, offset) {
                    Some(function) => {
                        let name = tree
                            .children(function)
                            .find(|&c| tree.kind(c) == SyntaxKind::Name)
                            .map(|n| tree.text_range(n))
                            .unwrap_or_else(|| tree.text_range(function));
                        self.function_item(function, name)
                    }
                    None => self.rule_item(&self.uri.clone(), None),
                }
            } else {
                self.rule_item(&location.uri, Some(location.range))
            };
            let key = (
                caller.uri.as_str().to_owned(),
                caller.range.start.line,
                caller.range.start.character,
            );
            callers
                .entry(key)
                .or_insert_with(|| (caller, Vec::new()))
                .1
                .push(location.range);
        }
        callers
            .into_values()
            .map(|(from, from_ranges)| CallHierarchyIncomingCall { from, from_ranges })
            .collect()
    }

    /// Funções do arquivo chamadas dentro da função do item, cada uma com os lugares da chamada.
    pub(crate) fn outgoing_calls(
        &self,
        item: &CallHierarchyItem,
    ) -> Vec<CallHierarchyOutgoingCall> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let symbols = &self.doc.symbols;
        let offset = self.offset(item.selection_range.start);
        let Some(function) = enclosing_function_at(tree, offset) else {
            return Vec::new();
        };
        let mut called: BTreeMap<String, (CallHierarchyItem, Vec<Range>)> = BTreeMap::new();
        let mut stack = vec![function];
        while let Some(node) = stack.pop() {
            stack.extend(tree.children(node));
            if tree.kind(node) != SyntaxKind::CallExpr {
                continue;
            }
            let Some(callee) = tree.children(node).next() else {
                continue;
            };
            if tree.kind(callee) != SyntaxKind::NameRef {
                continue;
            }
            let range = tree.text_range(callee);
            let name = &text[range];
            // Função da regra, com a `funcao` quando existe. Funções do ERP não têm onde abrir.
            let defs: Vec<_> = symbols
                .definitions
                .iter()
                .filter(|d| d.kind == DefKind::Function && text[d.name].eq_ignore_ascii_case(name))
                .collect();
            let Some(def) = defs
                .iter()
                .find(|d| tree.kind(d.node) == SyntaxKind::FuncImpl)
                .or(defs.first())
            else {
                continue;
            };
            called
                .entry(name.to_ascii_uppercase())
                .or_insert_with(|| (self.function_item(def.node, def.name), Vec::new()))
                .1
                .push(self.range(range));
        }
        called
            .into_values()
            .map(|(to, from_ranges)| CallHierarchyOutgoingCall { to, from_ranges })
            .collect()
    }

    fn function_item(&self, node: crate::NodeId, name: TextRange) -> CallHierarchyItem {
        CallHierarchyItem {
            name: self.doc.text[name].to_owned(),
            kind: SymbolKind::FUNCTION,
            tags: None,
            detail: Some(self.header(node)),
            uri: self.uri.clone(),
            range: self.range(self.doc.tree.text_range(node)),
            selection_range: self.range(name),
            data: None,
        }
    }

    /// O corpo principal de uma regra, como quem chama, com o nome da pasta da regra.
    fn rule_item(&self, uri: &lsp_types::Uri, at: Option<Range>) -> CallHierarchyItem {
        let path = crate::form::path_from_uri(uri.as_str());
        let name = path
            .as_ref()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .map_or_else(|| "regra".to_owned(), |n| n.to_string_lossy().into_owned());
        let range = at.unwrap_or_else(|| Range::new(Position::new(0, 0), Position::new(0, 0)));
        CallHierarchyItem {
            name,
            kind: SymbolKind::FILE,
            tags: None,
            detail: Some("corpo da regra".to_owned()),
            uri: uri.clone(),
            range,
            selection_range: range,
            data: None,
        }
    }
}
