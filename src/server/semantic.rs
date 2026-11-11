//! Destaque semântico: a cor de cada nome pelo que ele é, e não só pelo formato do texto.
//!
//! A gramática do editor colore as palavras reservadas, os textos e os comentários. Só a análise
//! sabe se `Soma` é uma função da regra, se `IntParaAlfa` é do ERP ou se `nValor` é variável ou
//! parâmetro, e é isso que vai aqui.

use std::collections::HashMap;

use lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensLegend,
};
use text_size::TextRange;

use super::documents::Document;
use crate::builtins;
use crate::line_index::ColumnEncoding;
use crate::symbols::DefKind;
use crate::{NodeId, SyntaxKind};

/// Tipos na ordem da legenda. O índice é o que vai em cada token.
const TYPES: [SemanticTokenType; 6] = [
    SemanticTokenType::FUNCTION,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::STRUCT,
    SemanticTokenType::CLASS,
];

#[derive(Clone, Copy)]
enum Type {
    Function,
    Variable,
    Parameter,
    /// Campo depois do ponto, de tabela, cursor ou web service.
    Property,
    /// Tabela definida com `definir tabela`.
    Struct,
    /// Web service definido na regra.
    Class,
}

/// Bits dos modificadores, na ordem da legenda.
const DECLARATION: u32 = 1;
const DEFAULT_LIBRARY: u32 = 2;

pub(crate) fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TYPES.to_vec(),
        token_modifiers: vec![
            SemanticTokenModifier::DECLARATION,
            SemanticTokenModifier::DEFAULT_LIBRARY,
        ],
    }
}

fn of_definition(kind: DefKind) -> Option<Type> {
    Some(match kind {
        DefKind::Function => Type::Function,
        DefKind::Variable(_) | DefKind::Cursor | DefKind::List => Type::Variable,
        DefKind::Parameter(_) => Type::Parameter,
        DefKind::Table => Type::Struct,
        DefKind::WebService => Type::Class,
        DefKind::Label => return None,
    })
}

/// Tokens do documento inteiro, em ordem de texto, no formato relativo do protocolo.
pub(crate) fn tokens(doc: &Document, encoding: ColumnEncoding) -> SemanticTokens {
    let tree = &doc.tree;
    let text = &doc.text;
    let symbols = &doc.symbols;
    let references: HashMap<TextRange, &[usize]> = symbols
        .references
        .iter()
        .map(|r| (r.range, r.targets.as_slice()))
        .collect();
    let mut found: Vec<(TextRange, Type, u32)> = symbols
        .definitions
        .iter()
        .filter_map(|d| Some((d.name, of_definition(d.kind)?, DECLARATION)))
        .collect();

    let mut stack = vec![tree.root()];
    while let Some(node) = stack.pop() {
        stack.extend(tree.children(node));
        if tree.kind(node) != SyntaxKind::NameRef {
            continue;
        }
        let range = tree.text_range(node);
        if let Some(classified) = classify(doc, &references, node, range) {
            found.push((range, classified.0, classified.1));
        }
    }

    found.sort_by_key(|(range, _, _)| range.start());
    found.dedup_by_key(|(range, _, _)| range.start());
    let mut data = Vec::with_capacity(found.len());
    let (mut line, mut col) = (0, 0);
    for (range, ty, modifiers) in found {
        let start = doc.index.line_col(text, range.start(), encoding);
        let end = doc.index.line_col(text, range.end(), encoding);
        // Um nome nunca quebra linha. Fica de fora o que não estiver numa linha só.
        if end.line != start.line {
            continue;
        }
        let delta_start = if start.line == line {
            start.col - col
        } else {
            start.col
        };
        data.push(SemanticToken {
            delta_line: start.line - line,
            delta_start,
            length: end.col - start.col,
            token_type: ty as u32,
            token_modifiers_bitset: modifiers,
        });
        (line, col) = (start.line, start.col);
    }
    SemanticTokens {
        result_id: None,
        data,
    }
}

fn classify(
    doc: &Document,
    references: &HashMap<TextRange, &[usize]>,
    node: NodeId,
    range: TextRange,
) -> Option<(Type, u32)> {
    let tree = &doc.tree;
    let name = &doc.text[range];
    let parent = tree.parent(node);
    let first = parent.is_some_and(|p| tree.children(p).next() == Some(node));
    let parent_kind = parent.map(|p| tree.kind(p));

    if parent_kind == Some(SyntaxKind::FieldExpr) && !first {
        return Some((Type::Property, 0));
    }
    if let Some(targets) = references.get(&range) {
        let def = &doc.symbols.definitions[*targets.first()?];
        return Some((of_definition(def.kind)?, 0));
    }
    if parent_kind == Some(SyntaxKind::CallExpr) && first {
        if builtins::lookup(name, doc.context).is_some() {
            return Some((Type::Function, DEFAULT_LIBRARY));
        }
        return doc.inherited.get(name).map(|_| (Type::Function, 0));
    }
    if builtins::system_variable(name, doc.context).is_some() {
        return Some((Type::Variable, DEFAULT_LIBRARY));
    }
    if doc.inherited.variable(name).is_some() || doc.inherited.is_entry(name) {
        return Some((Type::Variable, 0));
    }
    None
}
