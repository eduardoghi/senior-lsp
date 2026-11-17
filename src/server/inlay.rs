//! Nome do parâmetro antes de cada argumento da chamada, como `IntParaAlfa(Origem: n, end
//! Retorno: aTxt)`, e o tipo depois do primeiro uso de um nome sem `definir`, sem mudar o texto
//! do arquivo.

use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Range};

use super::navigation::Context;
use super::signature::params_of;
use crate::SyntaxKind;

impl Context<'_> {
    /// Dicas do trecho na tela. `types` liga a dica de tipo no nome sem `definir`.
    pub(crate) fn inlay_hints(&self, range: Range, types: bool) -> Vec<InlayHint> {
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
        let implicit = if types {
            self.doc.implicit_names()
        } else {
            &[]
        };
        for &name in implicit {
            let position = self.range(name).end;
            if position < range.start || position > range.end {
                continue;
            }
            hints.push(InlayHint {
                position,
                label: InlayHintLabel::String(": numero".to_owned()),
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: Some(lsp_types::InlayHintTooltip::String(
                    "Sem definir. O compilador cria o nome como número.".to_owned(),
                )),
                padding_left: None,
                padding_right: None,
                data: None,
            });
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

/// Primeiro uso de cada nome que não tem `definir` no arquivo nem vem de outra regra, do ERP ou
/// do dicionário, e que o compilador cria como número.
pub(crate) fn implicit_names(doc: &super::documents::Document) -> Vec<text_size::TextRange> {
    let tree = &doc.tree;
    let text = &doc.text;
    let mut seen = std::collections::HashSet::new();
    let mut found = Vec::new();
    // Em ordem de texto, para o primeiro uso sair antes dos outros.
    for node in tree.nodes() {
        if tree.kind(node) != SyntaxKind::NameRef {
            continue;
        }
        let range = tree.text_range(node);
        let name = &text[range];
        let parent = tree.parent(node);
        let parent_kind = parent.map(|p| tree.kind(p));
        let first = parent.is_some_and(|p| tree.children(p).next() == Some(node));
        let member_or_call = match parent_kind {
            Some(SyntaxKind::FieldExpr) => {
                !first || doc.inherited.dictionaries.table(name).is_some()
            }
            Some(SyntaxKind::CallExpr) => first,
            Some(SyntaxKind::Goto | SyntaxKind::Label) => true,
            _ => false,
        };
        // As variáveis que cada identificador acrescenta começam com VS ou VE, como `VSCodEmp`, e
        // o servidor não tem a lista delas. Compara bytes, porque um nome pode começar com letra
        // acentuada.
        let identifier_variable = doc.context == crate::builtins::Context::Rule
            && name.len() > 2
            && name
                .as_bytes()
                .get(..2)
                .is_some_and(|p| p.eq_ignore_ascii_case(b"VS") || p.eq_ignore_ascii_case(b"VE"));
        let ident = tree.tokens().kind(tree.token_range(node).start) == SyntaxKind::Ident;
        if member_or_call
            || !ident
            || identifier_variable
            // Tipo no `AdicionarCampo`, modo no `Abrir` e tipo do `Mensagem`, que são palavras
            // do ERP e não variáveis.
            || SyntaxKind::keyword(name).is_some()
            || ["RETORNA", "ERRO", "REFAZ"]
                .iter()
                .any(|c| c.eq_ignore_ascii_case(name))
            || doc.symbols.reference_at(range).is_some()
            || crate::builtins::system_variable(name, doc.context).is_some()
            || crate::builtins::lookup(name, doc.context).is_some()
            || doc.inherited.variable(name).is_some()
            || doc.inherited.is_entry(name)
            || doc.inherited.contains(name)
        {
            continue;
        }
        if seen.insert(name.to_uppercase()) {
            found.push(range);
        }
    }
    found
}
