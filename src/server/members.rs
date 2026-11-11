//! Completar depois do ponto, com as operações e as colunas do cursor, as operações e os campos
//! da lista, os campos de uma `definir tabela` e os campos da tabela do dicionário.
//!
//! As operações são as da tabela de palavras do compilador, onde as do cursor e as
//! da lista ficam em grupos separados. O uso nas regras confirma os dois grupos.

use lsp_types::{CompletionItem, CompletionItemKind};

use super::navigation::Context;
use crate::symbols::{DefKind, VarType};
use crate::{NodeId, SyntaxKind, Tree};

const CURSOR_OPERATIONS: [&str; 7] = [
    "SQL",
    "AbrirCursor",
    "FecharCursor",
    "Proximo",
    "Achou",
    "NaoAchou",
    "UsaAbrangencia",
];

const LIST_OPERATIONS: [&str; 23] = [
    "DefinirCampos",
    "AdicionarCampo",
    "EfetivarCampos",
    "Adicionar",
    "Inserir",
    "Editar",
    "Gravar",
    "Cancelar",
    "Excluir",
    "Primeiro",
    "Ultimo",
    "Anterior",
    "Proximo",
    "EditarChave",
    "SetarChave",
    "VaiParaChave",
    "NumReg",
    "SetaNumReg",
    "IDA",
    "FDA",
    "QtdRegistros",
    "Limpar",
    "Chave",
];

impl Context<'_> {
    /// Itens depois de `base.`, que começam com `prefix`. Vazio quando a base não é um nome
    /// simples ou não é cursor, lista, tabela da regra nem tabela do dicionário.
    pub(crate) fn member_completion(&self, head: &str, prefix: &str) -> Vec<CompletionItem> {
        let Some(head) = head.trim_end().strip_suffix('.') else {
            return Vec::new();
        };
        let head = head.trim_end();
        let start = head
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map_or(0, |i| i + 1);
        let base = &head[start..];
        // `a.b.` e nomes qualificados de web service ficam de fora.
        if base.is_empty() || head[..start].ends_with('.') {
            return Vec::new();
        }

        let text = &self.doc.text;
        let tree = &self.doc.tree;
        let definition = self
            .doc
            .symbols
            .definitions
            .iter()
            .find(|d| text[d.name].eq_ignore_ascii_case(base));
        let mut items = Vec::new();
        match definition.map(|d| (d.kind, d.node)) {
            Some((DefKind::Cursor, _)) => {
                operations(&mut items, &CURSOR_OPERATIONS, "operação do cursor");
                let columns = crate::validate::cursor_columns(
                    tree,
                    &self.doc.symbols,
                    text,
                    &self.doc.inherited.dictionaries,
                    base,
                );
                for (name, ty) in columns.into_iter().flatten() {
                    items.push(field(&name, ty, "coluna do SELECT"));
                }
            }
            Some((DefKind::List, _)) => {
                operations(&mut items, &LIST_OPERATIONS, "operação da lista");
                for (name, ty) in list_fields(tree, text, base) {
                    items.push(field(&name, ty, "campo da lista"));
                }
            }
            Some((DefKind::Table, node)) => {
                for (name, ty) in table_fields(tree, text, node) {
                    items.push(field(&name, ty, "campo da tabela"));
                }
            }
            Some(_) => {}
            None => {
                if let Some(table) = self.doc.inherited.dictionaries.table(base) {
                    for f in &table.fields {
                        items.push(field(&f.name, f.ty.var_type(), &f.description));
                    }
                }
            }
        }
        items.retain(|i| {
            i.label.len() >= prefix.len()
                && i.label.is_char_boundary(prefix.len())
                && i.label[..prefix.len()].eq_ignore_ascii_case(prefix)
        });
        let mut seen = std::collections::HashSet::new();
        items.retain(|i| seen.insert(i.label.to_uppercase()));
        items
    }
}

fn operations(items: &mut Vec<CompletionItem>, names: &[&str], detail: &str) {
    for name in names {
        items.push(CompletionItem {
            label: (*name).to_owned(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some(detail.to_owned()),
            // As operações vêm depois dos campos.
            sort_text: Some(format!("1{}", name.to_lowercase())),
            ..CompletionItem::default()
        });
    }
}

fn field(name: &str, ty: Option<VarType>, detail: &str) -> CompletionItem {
    CompletionItem {
        label: name.to_owned(),
        kind: Some(CompletionItemKind::FIELD),
        detail: Some(match ty {
            Some(_) => format!("{} {detail}", type_name(ty)),
            None => detail.to_owned(),
        }),
        sort_text: Some(format!("0{}", name.to_lowercase())),
        ..CompletionItem::default()
    }
}

fn type_name(ty: Option<VarType>) -> &'static str {
    match ty {
        Some(VarType::Alfa) => "alfa",
        Some(VarType::Numero) => "numero",
        Some(VarType::Data) => "data",
        None => "",
    }
}

/// Campos criados com `lista.AdicionarCampo("Nome", tipo, ...)` na regra.
fn list_fields(tree: &Tree, text: &str, list: &str) -> Vec<(String, Option<VarType>)> {
    let mut fields = Vec::new();
    let mut stack = vec![tree.root()];
    while let Some(node) = stack.pop() {
        stack.extend(tree.children(node));
        if tree.kind(node) != SyntaxKind::CallExpr {
            continue;
        }
        let mut children = tree.children(node);
        let (Some(callee), Some(args)) = (children.next(), children.next()) else {
            continue;
        };
        if tree.kind(callee) != SyntaxKind::FieldExpr {
            continue;
        }
        let mut parts = tree.children(callee);
        let (Some(object), Some(method)) = (parts.next(), parts.next()) else {
            continue;
        };
        if !text[tree.text_range(object)].eq_ignore_ascii_case(list)
            || !text[tree.text_range(method)].eq_ignore_ascii_case("AdicionarCampo")
        {
            continue;
        }
        let mut args = tree.children(args);
        let Some(name) = args.next().and_then(|a| string(tree, text, a)) else {
            continue;
        };
        let ty = args.next().and_then(|a| {
            match text[tree.text_range(a)].to_ascii_lowercase().as_str() {
                "alfa" => Some(VarType::Alfa),
                "numero" => Some(VarType::Numero),
                "data" => Some(VarType::Data),
                _ => None,
            }
        });
        fields.push((name, ty));
    }
    fields
}

/// Texto de um literal de string, sem as aspas.
fn string(tree: &Tree, text: &str, node: NodeId) -> Option<String> {
    let is_string = tree.kind(node) == SyntaxKind::Literal
        && tree.tokens().kind(tree.token_range(node).start) == SyntaxKind::String;
    let literal = &text[tree.text_range(node)];
    is_string.then(|| literal.trim_matches('"').to_owned())
}

/// Campos de uma `definir tabela`, que são os `VarDef` dentro dela.
fn table_fields(tree: &Tree, text: &str, table: NodeId) -> Vec<(String, Option<VarType>)> {
    let mut fields = Vec::new();
    let mut stack: Vec<NodeId> = tree.children(table).collect();
    while let Some(node) = stack.pop() {
        stack.extend(tree.children(node));
        if tree.kind(node) != SyntaxKind::VarDef {
            continue;
        }
        let Some(name) = tree
            .children(node)
            .find(|&c| tree.kind(c) == SyntaxKind::Name)
        else {
            continue;
        };
        let words = text[tree.text_range(node)].to_ascii_lowercase();
        let ty = if words.starts_with("alfa") {
            Some(VarType::Alfa)
        } else if words.starts_with("numero") {
            Some(VarType::Numero)
        } else if words.starts_with("data") {
            Some(VarType::Data)
        } else {
            None
        };
        fields.push((text[tree.text_range(name)].to_owned(), ty));
    }
    fields
}
