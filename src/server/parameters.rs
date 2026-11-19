//! Ações que mudam os parâmetros de uma função (tirar, trocar de lugar com o vizinho ou
//! acrescentar), mudando junto o `Definir Funcao`, a `funcao` e o argumento de cada chamada, nesta
//! regra e, num formulário SGI, nas regras seguintes que chamam a função.
//!
//! A ação só aparece quando todas as chamadas são achadas e têm um argumento por parâmetro. Tirar
//! também exige que o corpo não use o parâmetro, nem num texto, e que o argumento não tenha
//! chamada dentro, que deixaria de rodar. Mover exige que nenhum dos dois argumentos tenha
//! chamada dentro, que mudaria de ordem. Acrescentar precisa do tipo, do nome e do valor que as chamadas passam a
//! receber, então só aparece no editor que sabe perguntar (a extensão do VS Code).

use std::collections::HashMap;

use lsp_types::{
    CodeAction, CodeActionKind, Command, Position, Range, TextEdit, Uri, WorkspaceEdit,
};
use text_size::{TextRange, TextSize};

use super::navigation::{Context, name_at};
use crate::symbols::DefKind;
use crate::{NodeId, SyntaxKind, Tree};

/// Comando da extensão que pergunta o tipo, o nome e o valor do parâmetro novo e depois manda o
/// [`ADD_PARAMETER_REQUEST`].
pub(crate) const ADD_PARAMETER_COMMAND: &str = "senior-lsp.acrescentarParametro";

/// Pedido da extensão com as respostas, que devolve as edições.
pub(crate) const ADD_PARAMETER_REQUEST: &str = "senior-lsp/acrescentarParametro";

/// O que a extensão manda no [`ADD_PARAMETER_REQUEST`].
#[derive(serde::Deserialize)]
pub(crate) struct AddParameter {
    pub(crate) uri: Uri,
    pub(crate) position: Position,
    /// Posição do parâmetro novo na lista, de 0 até a quantidade de parâmetros.
    pub(crate) at: usize,
    pub(crate) tipo: String,
    pub(crate) nome: String,
    pub(crate) valor: String,
}

/// Uma lista de parâmetros ou de argumentos, com os parênteses, e o trecho de cada item.
struct List {
    range: TextRange,
    items: Vec<TextRange>,
}

/// O que muda em cada lista.
#[derive(Clone)]
enum Change {
    Remove(usize),
    Swap(usize, usize),
    /// O parâmetro novo, com o texto dele e o do argumento das chamadas, na posição `at`.
    Insert {
        at: usize,
        param: String,
        arg: String,
    },
}

impl List {
    fn of(tree: &Tree, node: NodeId) -> List {
        List {
            range: tree.text_range(node),
            items: tree.children(node).map(|c| tree.text_range(c)).collect(),
        }
    }

    /// As trocas de texto que fazem a mudança. Tirar leva junto a vírgula de um dos lados.
    /// `params` diz se a lista é de parâmetros ou de argumentos.
    fn edits(&self, text: &str, change: &Change, params: bool) -> Vec<(TextRange, String)> {
        let items = &self.items;
        match change {
            Change::Remove(i) if items.len() == 1 => vec![(items[*i], String::new())],
            Change::Remove(i) if i + 1 < items.len() => vec![(
                TextRange::new(items[*i].start(), items[i + 1].start()),
                String::new(),
            )],
            Change::Remove(i) => vec![(
                TextRange::new(items[i - 1].end(), items[*i].end()),
                String::new(),
            )],
            Change::Swap(a, b) => vec![
                (items[*a], text[items[*b]].to_owned()),
                (items[*b], text[items[*a]].to_owned()),
            ],
            Change::Insert { at, param, arg } => {
                let new = if params { param } else { arg };
                match items.get(*at) {
                    _ if items.is_empty() => {
                        let open = self.range.start() + TextSize::new(1);
                        vec![(TextRange::empty(open), new.clone())]
                    }
                    Some(item) => vec![(TextRange::empty(item.start()), format!("{new}, "))],
                    None => {
                        let last = items[items.len() - 1].end();
                        vec![(TextRange::empty(last), format!(", {new}"))]
                    }
                }
            }
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

/// A mesma caixa de `model` em `word`, que vem como `Numero`, `Alfa` ou `Data`.
fn same_case(word: &str, model: &str) -> String {
    if model
        .chars()
        .all(|c| !c.is_alphabetic() || c.is_lowercase())
    {
        word.to_lowercase()
    } else if model
        .chars()
        .all(|c| !c.is_alphabetic() || c.is_uppercase())
    {
        word.to_uppercase()
    } else {
        word.to_owned()
    }
}

impl Context<'_> {
    /// As ações para o parâmetro no cursor ou, com o cursor no nome da função, a de acrescentar
    /// no fim. `ask` diz se o editor sabe perguntar o que o parâmetro novo precisa.
    pub(crate) fn parameter_actions(&self, range: Range, ask: bool) -> Vec<CodeAction> {
        let Some((name, index)) = self.signature_target(self.offset(range.start)) else {
            return Vec::new();
        };
        let mut actions = Vec::new();
        if let Some((index, param)) = &index {
            let index = *index;
            if index > 0 {
                let title = format!("Mover {param} para a esquerda");
                actions.extend(self.signature_action(
                    &name,
                    Change::Swap(index - 1, index),
                    &title,
                ));
            }
            let title = format!("Mover {param} para a direita");
            actions.extend(self.signature_action(&name, Change::Swap(index, index + 1), &title));
            let title = format!("Tirar o parâmetro {param}");
            actions.extend(self.signature_action(&name, Change::Remove(index), &title));
        }
        if ask {
            let (at, title) = match &index {
                Some((index, param)) => (
                    index + 1,
                    format!("Acrescentar parâmetro depois de {param}"),
                ),
                None => (
                    self.parameter_count(&name).unwrap_or(0),
                    "Acrescentar parâmetro no fim".to_owned(),
                ),
            };
            let probe = Change::Insert {
                at,
                param: String::new(),
                arg: String::new(),
            };
            if self.signature_edit(&name, &probe).is_some() {
                let arguments = vec![
                    serde_json::json!(self.uri.as_str()),
                    serde_json::json!(range.start),
                    serde_json::json!(at),
                ];
                actions.push(CodeAction {
                    title: title.clone(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    command: Some(Command::new(
                        title,
                        ADD_PARAMETER_COMMAND.to_owned(),
                        Some(arguments),
                    )),
                    ..CodeAction::default()
                });
            }
        }
        actions
    }

    /// As edições que acrescentam o parâmetro com as respostas da extensão. O erro vai para a
    /// pessoa, que pode tentar de novo.
    pub(crate) fn add_parameter(&self, request: &AddParameter) -> Result<WorkspaceEdit, String> {
        let (name, _) = self
            .signature_target(self.offset(request.position))
            .ok_or("o cursor não está mais num parâmetro nem no nome da função")?;
        let tipo = ["Numero", "Alfa", "Data"]
            .into_iter()
            .find(|t| t.eq_ignore_ascii_case(request.tipo.trim()))
            .ok_or_else(|| format!("\"{}\" não é Numero, Alfa nem Data", request.tipo))?;
        let nome = request.nome.trim();
        let valid = nome
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && nome.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && nome.len() <= 100;
        if !valid {
            return Err(format!(
                "\"{nome}\" não é um nome válido. Use letras, dígitos e _, começando por letra ou _."
            ));
        }
        if SyntaxKind::keyword(nome).is_some() {
            return Err(format!("\"{nome}\" é palavra reservada da linguagem."));
        }
        // Um nome que já existe no arquivo mudaria o que o corpo da função lê.
        let text = self.doc.text.as_str();
        if self
            .doc
            .symbols
            .definitions
            .iter()
            .any(|d| text[d.name].eq_ignore_ascii_case(nome))
        {
            return Err(format!("Já existe \"{nome}\" definido neste arquivo."));
        }
        let valor = request.valor.trim();
        if valor.is_empty() {
            return Err("falta o valor que as chamadas passam a receber".to_owned());
        }
        let param = format!("{} {nome}", self.type_keyword(&name, tipo));
        let change = Change::Insert {
            at: request.at,
            param,
            arg: valor.to_owned(),
        };
        self.signature_edit(&name, &change).ok_or_else(|| {
            "a função ou as chamadas dela mudaram desde que a ação foi oferecida".to_owned()
        })
    }

    /// A função e, com o cursor num parâmetro, a posição e o nome dele. Só na regra comum e na de
    /// formulário SGI. Num modelo de relatório a função vale em todas as regras dele, e a edição
    /// teria que ir para dentro do texto escapado do JSON.
    fn signature_target(&self, offset: TextSize) -> Option<(String, Option<(usize, String)>)> {
        if self.embedding.is_some()
            || !matches!(
                self.doc.context,
                crate::builtins::Context::Rule | crate::builtins::Context::Sgi
            )
        {
            return None;
        }
        let (tree, text, symbols) = (&self.doc.tree, self.doc.text.as_str(), &self.doc.symbols);
        let header_name = |header: NodeId| {
            symbols
                .definitions
                .iter()
                .find(|d| d.kind == DefKind::Function && d.node == header)
                .map(|d| text[d.name].to_owned())
        };
        if let Some(param) = symbols
            .definitions
            .iter()
            .find(|d| matches!(d.kind, DefKind::Parameter(_)) && d.name.contains_inclusive(offset))
        {
            let list = tree.parent(param.node)?;
            let index = tree.children(list).position(|c| c == param.node)?;
            let name = header_name(tree.parent(list)?)?;
            return Some((name, Some((index, text[param.name].to_owned()))));
        }
        let function = symbols
            .definitions
            .iter()
            .find(|d| d.kind == DefKind::Function && d.name.contains_inclusive(offset))?;
        Some((text[function.name].to_owned(), None))
    }

    /// Quantos parâmetros a primeira lista da função tem.
    fn parameter_count(&self, name: &str) -> Option<usize> {
        let lists = self.parameter_lists(name)?;
        Some(self.doc.tree.children(*lists.first()?).count())
    }

    /// O tipo com a caixa do primeiro parâmetro da função ou, sem parâmetro, a dos `definir`.
    fn type_keyword(&self, name: &str, tipo: &str) -> String {
        let (tree, text) = (&self.doc.tree, self.doc.text.as_str());
        let model = self
            .parameter_lists(name)
            .and_then(|lists| tree.children(*lists.first()?).next())
            .map(|param| &text[tree.tokens().range(tree.token_range(param).start)]);
        match model {
            Some(model) => same_case(tipo, model),
            None => self.keyword_case(tipo),
        }
    }

    /// As listas de parâmetros do `Definir Funcao` e da `funcao`.
    fn parameter_lists(&self, name: &str) -> Option<Vec<NodeId>> {
        let (tree, text, symbols) = (&self.doc.tree, self.doc.text.as_str(), &self.doc.symbols);
        symbols
            .definitions
            .iter()
            .filter(|d| d.kind == DefKind::Function && text[d.name].eq_ignore_ascii_case(name))
            .map(|d| {
                tree.children(d.node)
                    .find(|&c| tree.kind(c) == SyntaxKind::ParamList)
            })
            .collect()
    }

    fn signature_action(&self, name: &str, change: Change, title: &str) -> Option<CodeAction> {
        Some(CodeAction {
            title: title.to_owned(),
            kind: Some(CodeActionKind::REFACTOR_REWRITE),
            edit: Some(self.signature_edit(name, &change)?),
            ..CodeAction::default()
        })
    }

    /// As edições que fazem `change` em todas as listas de parâmetros da função `name` e nos
    /// argumentos de todas as chamadas. `None` quando alguma não pode ser mudada com segurança.
    fn signature_edit(&self, name: &str, change: &Change) -> Option<WorkspaceEdit> {
        let (tree, text, symbols) = (&self.doc.tree, self.doc.text.as_str(), &self.doc.symbols);
        let functions: Vec<usize> = symbols
            .definitions
            .iter()
            .enumerate()
            .filter(|(_, d)| d.kind == DefKind::Function && text[d.name].eq_ignore_ascii_case(name))
            .map(|(i, _)| i)
            .collect();
        let lists = self.parameter_lists(name)?;
        let count = tree.children(*lists.first()?).count();
        let valid = match change {
            Change::Remove(i) => *i < count,
            Change::Swap(a, b) => *a < count && *b < count,
            Change::Insert { at, .. } => *at <= count,
        };
        if !valid || lists.iter().any(|&l| tree.children(l).count() != count) {
            return None;
        }
        // O parâmetro que sai não pode ser lido nem escrito no corpo, nem citado num texto, porque
        // o SQL lê o parâmetro como `:nome`.
        if let Change::Remove(i) = change {
            for &list in &lists {
                let param = tree.children(list).nth(*i)?;
                let mut defs = symbols
                    .definitions
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| d.node == param);
                let used = defs
                    .clone()
                    .any(|(d, _)| symbols.references.iter().any(|r| r.targets.contains(&d)));
                let in_strings = defs.next().is_some_and(|(_, def)| {
                    let wanted = std::iter::once(text[def.name].to_ascii_uppercase()).collect();
                    !crate::validate::names_in_strings(tree, text, &wanted).is_empty()
                });
                if used || in_strings {
                    return None;
                }
            }
        }
        let mut edits: Vec<(TextRange, String)> = lists
            .iter()
            .flat_map(|&l| List::of(tree, l).edits(text, change, true))
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
        Some(WorkspaceEdit {
            changes: Some(changes),
            ..WorkspaceEdit::default()
        })
    }
}

/// As trocas nos argumentos da chamada cujo nome é `node`. `None` quando o nome não é chamado,
/// quando a quantidade de argumentos não é a de parâmetros ou quando o argumento que sai, ou um
/// dos que trocam de lugar, tem uma chamada dentro.
fn arguments_edits(
    tree: &Tree,
    text: &str,
    node: NodeId,
    change: &Change,
    count: usize,
) -> Option<Vec<(TextRange, String)>> {
    let args = call_arguments(tree, node)?;
    if tree.children(args).count() != count {
        return None;
    }
    if let Change::Remove(i) = change
        && has_call(tree, tree.children(args).nth(*i)?)
    {
        return None;
    }
    // Trocar de lugar um argumento com chamada muda a ordem em que as chamadas rodam e, com uma
    // chamada da mesma função dentro, faria duas edições no mesmo trecho.
    if let Change::Swap(a, b) = change {
        let (first, second) = (tree.children(args).nth(*a)?, tree.children(args).nth(*b)?);
        if has_call(tree, first) || has_call(tree, second) {
            return None;
        }
    }
    Some(List::of(tree, args).edits(text, change, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Aplica as trocas de `change` na lista `text`, de itens de uma letra separados por `, `.
    fn apply(text: &str, change: &Change) -> String {
        let at = |i: usize| TextSize::new(i as u32);
        let items = text
            .char_indices()
            .filter(|(_, c)| c.is_ascii_alphabetic())
            .map(|(i, _)| TextRange::new(at(i), at(i + 1)))
            .collect();
        let list = List {
            range: TextRange::new(at(0), at(text.len())),
            items,
        };
        let mut out = text.to_owned();
        let mut edits = list.edits(text, change, true);
        edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start()));
        for (range, new) in edits {
            out.replace_range(usize::from(range.start())..usize::from(range.end()), &new);
        }
        out
    }

    #[test]
    fn trocas_da_lista_nas_pontas_e_no_meio() {
        let insert = |at| Change::Insert {
            at,
            param: "x".to_owned(),
            arg: "y".to_owned(),
        };
        assert_eq!(apply("(a, b, c)", &Change::Remove(0)), "(b, c)");
        assert_eq!(apply("(a, b, c)", &Change::Remove(1)), "(a, c)");
        assert_eq!(apply("(a, b, c)", &Change::Remove(2)), "(a, b)");
        assert_eq!(apply("(a)", &Change::Remove(0)), "()");
        assert_eq!(apply("(a, b, c)", &Change::Swap(0, 2)), "(c, b, a)");
        assert_eq!(apply("(a, b)", &insert(0)), "(x, a, b)");
        assert_eq!(apply("(a, b)", &insert(1)), "(a, x, b)");
        assert_eq!(apply("(a, b)", &insert(2)), "(a, b, x)");
        assert_eq!(apply("()", &insert(0)), "(x)");
    }

    #[test]
    fn tipo_na_caixa_do_modelo() {
        assert_eq!(same_case("Numero", "alfa"), "numero");
        assert_eq!(same_case("Numero", "ALFA_1"), "NUMERO");
        assert_eq!(same_case("Numero", "Alfa"), "Numero");
        assert_eq!(same_case("Numero", "aLFA"), "Numero");
    }
}
