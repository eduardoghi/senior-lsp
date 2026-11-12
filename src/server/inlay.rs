//! Nome do parâmetro antes de cada argumento da chamada, como `IntParaAlfa(Origem: n, end
//! Retorno: aTxt)`, sem mudar o texto do arquivo.

use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Range};

use super::navigation::Context;
use super::signature::params_of;
use crate::SyntaxKind;

impl Context<'_> {
    pub(crate) fn inlay_hints(&self, range: Range) -> Vec<InlayHint> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let mut hints = Vec::new();
        // O editor pede só o trecho na tela. Um nó que acaba antes dele ou começa depois não tem
        // argumento dentro, então nem ele nem os filhos são visitados. Numa regra dentro do JSON
        // de modelo, a posição fora das linhas da regra vira o offset 0, e aí a árvore é toda
        // visitada, como antes.
        let (from, to) = match self.embedding {
            None => (self.offset(range.start), self.offset(range.end)),
            Some(_) => (
                text_size::TextSize::new(0),
                text_size::TextSize::of(text.as_str()),
            ),
        };
        let mut stack = vec![tree.root()];
        while let Some(node) = stack.pop() {
            let node_range = tree.text_range(node);
            if node_range.end() < from || node_range.start() > to {
                continue;
            }
            stack.extend(tree.children(node));
            if tree.kind(node) != SyntaxKind::CallExpr {
                continue;
            }
            let mut children = tree.children(node);
            let (Some(callee), Some(args)) = (children.next(), children.next()) else {
                continue;
            };
            if tree.kind(callee) != SyntaxKind::NameRef || tree.kind(args) != SyntaxKind::ArgList {
                continue;
            }
            let Some(decl) = self.declaration(&text[tree.text_range(callee)]) else {
                continue;
            };
            let params: Vec<(&str, bool)> = params_of(&decl).into_iter().map(param).collect();
            // Com um parâmetro só, o nome não diz nada que a função já não diga.
            if params.len() < 2 {
                continue;
            }
            for (arg, (name, by_ref)) in tree.children(args).zip(params) {
                let arg_range = tree.text_range(arg);
                let arg_text = &text[arg_range];
                if tree.kind(arg) == SyntaxKind::ErrorNode || arg_text.eq_ignore_ascii_case(name) {
                    continue;
                }
                let position = self.range(arg_range).start;
                if position < range.start || position > range.end {
                    continue;
                }
                let label = if by_ref {
                    format!("end {name}:")
                } else {
                    format!("{name}:")
                };
                hints.push(InlayHint {
                    position,
                    label: InlayHintLabel::String(label),
                    kind: Some(InlayHintKind::PARAMETER),
                    text_edits: None,
                    tooltip: None,
                    padding_left: None,
                    padding_right: Some(true),
                    data: None,
                });
            }
        }
        hints.sort_by_key(|h| (h.position.line, h.position.character));
        hints
    }
}

/// Nome do parâmetro e se é `end`, de um trecho como `Alfa end Retorno` ou `Numero Valor`.
fn param(text: &str) -> (&str, bool) {
    let mut words = text.split_whitespace();
    let name = words.next_back().unwrap_or(text);
    let by_ref = text
        .split_whitespace()
        .any(|w| w.eq_ignore_ascii_case("end"));
    (name, by_ref)
}
