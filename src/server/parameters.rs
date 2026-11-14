//! Ações que tiram um parâmetro de uma função ou o trocam de lugar com o vizinho, mudando junto o
//! `Definir Funcao`, a `funcao` e o argumento de cada chamada, nesta regra e, num formulário SGI,
//! nas regras seguintes que chamam a função.
//!
//! A ação só aparece quando todas as chamadas são achadas e têm um argumento por parâmetro. Tirar
//! também exige que o corpo não use o parâmetro e que o argumento não tenha chamada dentro, que
//! deixaria de rodar.

use std::collections::HashMap;

use lsp_types::{CodeAction, CodeActionKind, Position, Range, TextEdit, Uri, WorkspaceEdit};
use text_size::TextRange;

use super::navigation::{Context, name_at};
use crate::symbols::DefKind;
use crate::{NodeId, SyntaxKind, Tree};

/// Uma lista de parâmetros ou de argumentos num texto, com o trecho de cada item.
struct List {
    items: Vec<TextRange>,
}

/// O que muda em cada lista.
#[derive(Clone, Copy)]
enum Change {
    Remove(usize),
    Swap(usize, usize),
}

impl List {
    fn of(tree: &Tree, node: NodeId) -> List {
        List {
            items: tree.children(node).map(|c| tree.text_range(c)).collect(),
        }
    }

    /// As trocas de texto que fazem a mudança. Tirar leva junto a vírgula de um dos lados.
    fn edits(&self, text: &str, change: Change) -> Vec<(TextRange, String)> {
        let items = &self.items;
        match change {
            Change::Remove(i) if items.len() == 1 => vec![(items[i], String::new())],
            Change::Remove(i) if i + 1 < items.len() => {
                vec![(
                    TextRange::new(items[i].start(), items[i + 1].start()),
                    String::new(),
                )]
            }
            Change::Remove(i) => {
                vec![(
                    TextRange::new(items[i - 1].end(), items[i].end()),
                    String::new(),
                )]
            }
            Change::Swap(a, b) => vec![
                (items[a], text[items[b]].to_owned()),
                (items[b], text[items[a]].to_owned()),
            ],
        }
    }
}

/// Lista de argumentos da chamada, quando o nome em `name` é o que é chamado.
fn call_arguments(tree: &Tree, name: NodeId) -> Option<NodeId> {
    let call = tree.parent(name)?;
    let mut parts = tree.children(call);
    if tree.kind(call) != SyntaxKind::CallExpr || parts.next() != Some(name) {
        return None;
    }
    parts
        .next()
        .filter(|&args| tree.kind(args) == SyntaxKind::ArgList)
}

fn has_call(tree: &Tree, node: NodeId) -> bool {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if tree.kind(n) == SyntaxKind::CallExpr {
            return true;
        }
        stack.extend(tree.children(n));
    }
    false
}

impl Context<'_> {
    /// As ações para o parâmetro no cursor, na declaração ou na implementação da função.
    pub(crate) fn parameter_actions(&self, range: Range) -> Vec<CodeAction> {
        // Num modelo de relatório a função vale em todas as regras dele, e a edição teria que ir
        // para dentro do texto escapado do JSON.
        if self.embedding.is_some()
            || !matches!(
                self.doc.context,
                crate::builtins::Context::Rule | crate::builtins::Context::Sgi
            )
        {
            return Vec::new();
        }
        let offset = self.offset(range.start);
        let (tree, text, symbols) = (&self.doc.tree, self.doc.text.as_str(), &self.doc.symbols);
        let Some(param) = symbols
            .definitions
            .iter()
            .find(|d| matches!(d.kind, DefKind::Parameter(_)) && d.name.contains_inclusive(offset))
        else {
            return Vec::new();
        };
        let Some(header) = tree.parent(param.node).and_then(|list| tree.parent(list)) else {
            return Vec::new();
        };
        let Some(function) = symbols
            .definitions
            .iter()
            .find(|d| d.kind == DefKind::Function && d.node == header)
        else {
            return Vec::new();
        };
        let name = &text[function.name];
        let index = tree
            .parent(param.node)
            .and_then(|list| tree.children(list).position(|c| c == param.node));
        let Some(index) = index else {
            return Vec::new();
        };
        let mut actions = Vec::new();
        let param_name = &text[param.name];
        if index > 0 {
            let title = format!("Mover {param_name} para a esquerda");
            actions.extend(self.signature_action(name, Change::Swap(index - 1, index), &title));
        }
        let title = format!("Mover {param_name} para a direita");
        actions.extend(self.signature_action(name, Change::Swap(index, index + 1), &title));
        let title = format!("Tirar o parâmetro {param_name}");
        actions.extend(self.signature_action(name, Change::Remove(index), &title));
        actions
    }

    /// A ação que faz `change` em todas as listas de parâmetros da função `name` e nos argumentos
    /// de todas as chamadas. `None` quando alguma não pode ser mudada com segurança.
    fn signature_action(&self, name: &str, change: Change, title: &str) -> Option<CodeAction> {
        let (tree, text, symbols) = (&self.doc.tree, self.doc.text.as_str(), &self.doc.symbols);
        let functions: Vec<usize> = symbols
            .definitions
            .iter()
            .enumerate()
            .filter(|(_, d)| d.kind == DefKind::Function && text[d.name].eq_ignore_ascii_case(name))
            .map(|(i, _)| i)
            .collect();
        let mut lists = Vec::new();
        for &f in &functions {
            let header = symbols.definitions[f].node;
            let list = tree
                .children(header)
                .find(|&c| tree.kind(c) == SyntaxKind::ParamList)?;
            lists.push(list);
        }
        let count = tree.children(*lists.first()?).count();
        let in_range = |i: usize| i < count;
        match change {
            Change::Remove(i) if !in_range(i) => return None,
            Change::Swap(a, b) if !in_range(a) || !in_range(b) => return None,
            _ => {}
        }
        if lists.iter().any(|&l| tree.children(l).count() != count) {
            return None;
        }
        // O parâmetro que sai não pode ser lido nem escrito no corpo.
        if let Change::Remove(i) = change {
            for &list in &lists {
                let param = tree.children(list).nth(i)?;
                let used = symbols.definitions.iter().enumerate().any(|(d, def)| {
                    def.node == param && symbols.references.iter().any(|r| r.targets.contains(&d))
                });
                if used {
                    return None;
                }
            }
        }
        let mut edits: Vec<(TextRange, String)> = lists
            .iter()
            .flat_map(|&l| List::of(tree, l).edits(text, change))
            .collect();
        // As chamadas desta regra.
        for reference in &symbols.references {
            if !reference.targets.iter().any(|t| functions.contains(t)) {
                continue;
            }
            let (node, _) = name_at(tree, reference.range.start())?;
            edits.extend(arguments_edits(tree, text, node, change, count)?);
        }
        #[allow(clippy::mutable_key_type)]
        let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
        changes.insert(
            self.uri.clone(),
            edits
                .into_iter()
                .map(|(range, new_text)| TextEdit::new(self.range(range), new_text))
                .collect(),
        );
        // Num formulário SGI, as regras seguintes que chamam a função.
        if self.doc.context == crate::builtins::Context::Sgi {
            let path = crate::form::path_from_uri(self.uri.as_str())?;
            for (rule, uses) in crate::form::following_uses(&path, Some(&path)) {
                if rule == path {
                    continue;
                }
                // Sem o texto da regra não dá para saber se ela chama a função.
                let uses = uses?;
                let ranges = uses.of_name(name);
                if ranges.is_empty() {
                    continue;
                }
                let other = crate::parse(&uses.text).ok()?;
                let mut other_edits = Vec::new();
                for range in ranges {
                    let (node, _) = name_at(&other, range.start())?;
                    other_edits.extend(arguments_edits(&other, &uses.text, node, change, count)?);
                }
                let uri = crate::form::uri_from_path(&rule).parse::<Uri>().ok()?;
                let position = |offset| {
                    let lc = uses.index.line_col(&uses.text, offset, self.encoding);
                    Position::new(lc.line, lc.col)
                };
                changes.insert(
                    uri,
                    other_edits
                        .into_iter()
                        .map(|(r, new_text)| {
                            TextEdit::new(
                                Range::new(position(r.start()), position(r.end())),
                                new_text,
                            )
                        })
                        .collect(),
                );
            }
        }
        Some(CodeAction {
            title: title.to_owned(),
            kind: Some(CodeActionKind::REFACTOR_REWRITE),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..WorkspaceEdit::default()
            }),
            ..CodeAction::default()
        })
    }
}

/// As trocas nos argumentos da chamada cujo nome é `node`. `None` quando o nome não é chamado,
/// quando a quantidade de argumentos não é a de parâmetros ou quando o argumento que sai tem uma
/// chamada dentro.
fn arguments_edits(
    tree: &Tree,
    text: &str,
    node: NodeId,
    change: Change,
    count: usize,
) -> Option<Vec<(TextRange, String)>> {
    let args = call_arguments(tree, node)?;
    if tree.children(args).count() != count {
        return None;
    }
    if let Change::Remove(i) = change
        && has_call(tree, tree.children(args).nth(i)?)
    {
        return None;
    }
    Some(List::of(tree, args).edits(text, change))
}
