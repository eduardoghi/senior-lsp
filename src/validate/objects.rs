//! Uso de tabela, cursor e lista, e a definição da tabela.
//!
//! Cada caso foi conferido compilando ou executando a regra no ERP. O que o ERP aceita não pode
//! sair aqui, como o índice que é variável ou expressão (só o literal se confere), a coluna `Alfa
//! Nome[20]` (o `[n]` do alfa é o tamanho e não pede segundo índice) e a tabela de tamanho 0. Só
//! vale para o que a regra define, porque a tabela, o cursor e a lista de outra regra da tela só
//! entram quando o nome diz de que tipo eles são, e a lista não diz os campos dela.

use rustc_hash::{FxHashMap, FxHashSet};
use text_size::TextRange;

use super::types::{self, Scope};
use super::{Check, CheckKind, operator_token};
use crate::SyntaxKind::*;
use crate::symbols::{DefKind, VarType, var_type};
use crate::{Element, NodeId, Tree};

/// O tamanho da tabela é guardado em 16 bits no ERP.
const TABLE_SIZE_LIMIT: u64 = 65_536;

/// Funções da lista, que precisam dos parênteses mesmo sem valor.
const LIST_FUNCTIONS: [&str; 5] = ["Primeiro", "Ultimo", "Anterior", "Proximo", "VaiParaChave"];
/// Propriedades da lista, que não levam parênteses.
const LIST_PROPERTIES: [&str; 4] = ["NumReg", "IDA", "FDA", "QtdRegistros"];
/// Procedimentos da lista.
const LIST_PROCEDURES: [&str; 14] = [
    "DefinirCampos",
    "AdicionarCampo",
    "EfetivarCampos",
    "Adicionar",
    "Inserir",
    "Editar",
    "Gravar",
    "Cancelar",
    "Excluir",
    "SetaNumReg",
    "Limpar",
    "Chave",
    "EditarChave",
    "SetarChave",
];

fn is_one_of(name: &str, names: &[&str]) -> bool {
    names.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// O que o nome na frente de um ponto ou colchete é, quando o arquivo ou a regra da tela diz.
#[derive(Debug, Clone, Copy)]
enum Base {
    /// Tabela, com o `TableDef` quando é desta regra.
    Table(Option<NodeId>),
    Cursor,
    /// Lista, com o índice da definição quando é desta regra e definida uma vez só.
    List(Option<usize>),
    /// Variável numérica com `[n]`, com o `VarDef`.
    Array(NodeId),
}

/// Campos que a regra cria na lista com `AdicionarCampo`.
#[derive(Default)]
struct ListFields {
    names: FxHashSet<std::string::String>,
    /// Todos os `AdicionarCampo` dão o nome por texto literal. Sem isso a lista pode ter campos
    /// que o arquivo não mostra.
    complete: bool,
}

struct Column {
    name: NodeId,
    ty: Option<VarType>,
    /// Tem `[...]` depois do nome.
    sized: bool,
    /// Tamanho, quando é um número literal.
    size: Option<u64>,
}

struct TableInfo {
    /// Número de linhas já com o corte de 16 bits, quando o tamanho é um número literal.
    rows: Option<u64>,
    columns: Vec<Column>,
}

pub(super) struct Objects<'a> {
    tree: &'a Tree,
    scope: &'a Scope<'a>,
    lists: FxHashMap<usize, ListFields>,
}

fn is_ident(tree: &Tree, node: NodeId) -> bool {
    tree.kind(node) == NameRef && tree.tokens().kind(tree.token_range(node).start) == Ident
}

/// Número inteiro escrito direto, com ou sem sinal de menos.
fn literal_int(tree: &Tree, text: &str, node: NodeId) -> Option<i64> {
    match tree.kind(node) {
        Literal if tree.tokens().kind(tree.token_range(node).start) == Number => {
            text[tree.text_range(node)].parse().ok()
        }
        PrefixExpr if operator_token(tree, node).map(|t| tree.tokens().kind(t)) == Some(Minus) => {
            let inner = tree.children(node).next()?;
            literal_int(tree, text, inner).map(|n| -n)
        }
        _ => None,
    }
}

/// Algum erro de sintaxe começa dentro do trecho, e então o resto das conferências dele só
/// repetiria o que o parser já disse.
fn has_error_in(tree: &Tree, range: TextRange) -> bool {
    tree.errors()
        .iter()
        .any(|e| e.range.start() >= range.start() && e.range.start() <= range.end())
}

impl<'a> Objects<'a> {
    pub(super) fn build(scope: &'a Scope<'a>, tree: &'a Tree, checks: &mut Vec<Check>) -> Self {
        let mut objects = Objects {
            tree,
            scope,
            lists: FxHashMap::default(),
        };
        // Só as regras com lista precisam da varredura dos `AdicionarCampo`.
        if scope
            .symbols
            .definitions
            .iter()
            .any(|d| d.kind == DefKind::List)
        {
            let mut stack = vec![tree.root()];
            while let Some(node) = stack.pop() {
                if tree.kind(node) == CallExpr {
                    objects.collect_field(node, checks);
                }
                let first = stack.len();
                stack.extend(tree.children(node));
                stack[first..].reverse();
            }
        }
        objects
    }

    fn text(&self, node: NodeId) -> &'a str {
        &self.scope.text[self.tree.text_range(node)]
    }

    /// Callee e lista de argumentos de uma chamada de método, como `lst.Campo(...)`, com a base e
    /// o membro.
    fn method_call(&self, call: NodeId) -> Option<(NodeId, NodeId, NodeId, NodeId)> {
        let tree = self.tree;
        let mut parts = tree.children(call);
        let (callee, args) = (parts.next()?, parts.next()?);
        if tree.kind(callee) != FieldExpr || tree.kind(args) != ArgList {
            return None;
        }
        let mut sides = tree.children(callee);
        let (base, member) = (sides.next()?, sides.next()?);
        Some((base, member, args, callee))
    }

    /// O que `node` é, pela definição na regra ou, sem ela, pelas variáveis das regras da tela.
    fn base(&self, node: NodeId) -> Option<Base> {
        if !is_ident(self.tree, node) {
            return None;
        }
        let symbols = self.scope.symbols;
        let range = self.tree.text_range(node);
        let Some(reference) = symbols.reference_at(range) else {
            return match self.scope.inherited.variable(self.text(node))?.kind {
                DefKind::Table => Some(Base::Table(None)),
                DefKind::Cursor => Some(Base::Cursor),
                DefKind::List => Some(Base::List(None)),
                _ => None,
            };
        };
        let defs: Vec<_> = reference
            .targets
            .iter()
            .map(|&t| (t, &symbols.definitions[t]))
            .collect();
        let (index, first) = *defs.first()?;
        let single = defs.len() == 1;
        match first.kind {
            DefKind::Table if single => Some(Base::Table(Some(first.node))),
            DefKind::Cursor if defs.iter().all(|(_, d)| d.kind == DefKind::Cursor) => {
                Some(Base::Cursor)
            }
            DefKind::List if defs.iter().all(|(_, d)| d.kind == DefKind::List) => {
                Some(Base::List(single.then_some(index)))
            }
            DefKind::Variable(Some(VarType::Numero)) if single => {
                self.array_size(first.node).map(|_| Base::Array(first.node))
            }
            _ => None,
        }
    }

    /// Tamanho de `Definir Numero x[n]`, só com `n` literal.
    fn array_size(&self, var: NodeId) -> Option<u64> {
        let size = self
            .tree
            .children(var)
            .find(|&c| self.tree.kind(c) != Name)?;
        u64::try_from(literal_int(self.tree, self.scope.text, size)?).ok()
    }

    fn table_info(&self, table: NodeId) -> TableInfo {
        let tree = self.tree;
        let size = tree
            .children(table)
            .find(|&c| !matches!(tree.kind(c), Name | VarDef));
        let rows = size
            .and_then(|s| literal_int(tree, self.scope.text, s))
            .and_then(|n| u64::try_from(n).ok())
            .map(|n| n % TABLE_SIZE_LIMIT);
        let columns = tree
            .children(table)
            .filter(|&c| tree.kind(c) == VarDef)
            .filter_map(|col| {
                let name = tree.children(col).find(|&c| tree.kind(c) == Name)?;
                let size = tree.children(col).find(|&c| tree.kind(c) != Name);
                Some(Column {
                    name,
                    ty: var_type(tree, col),
                    sized: size.is_some(),
                    size: size
                        .and_then(|s| literal_int(tree, self.scope.text, s))
                        .and_then(|n| u64::try_from(n).ok()),
                })
            })
            .collect();
        TableInfo { rows, columns }
    }

    fn column<'t>(&self, info: &'t TableInfo, name: &str) -> Option<&'t Column> {
        info.columns
            .iter()
            .find(|c| self.text(c.name).eq_ignore_ascii_case(name))
    }

    /// `AdicionarCampo` de uma lista da regra, para saber os campos que ela tem e pegar o campo
    /// repetido.
    fn collect_field(&mut self, call: NodeId, checks: &mut Vec<Check>) {
        let Some((base, member, args, _)) = self.method_call(call) else {
            return;
        };
        if !self.text(member).eq_ignore_ascii_case("AdicionarCampo") {
            return;
        }
        let Some(Base::List(Some(list))) = self.base(base) else {
            return;
        };
        let first = self.tree.children(args).next();
        let name = first
            .filter(|&a| {
                self.tree.kind(a) == Literal
                    && self.tree.tokens().kind(self.tree.token_range(a).start) == String
            })
            .map(|a| self.text(a).trim_matches('"').to_ascii_uppercase());
        let list_name = self.text_of_definition(list);
        let fields = self.lists.entry(list).or_insert_with(|| ListFields {
            complete: true,
            ..ListFields::default()
        });
        match (first, name) {
            (Some(arg), Some(name)) => {
                if !fields.names.insert(name.clone()) {
                    checks.push(Check {
                        kind: CheckKind::ListRepeatedField,
                        range: self.tree.text_range(arg),
                        name: None,
                        label: Some(format!("{list_name}|{name}")),
                    });
                }
            }
            _ => fields.complete = false,
        }
    }

    fn text_of_definition(&self, index: usize) -> std::string::String {
        self.scope.text[self.scope.symbols.definitions[index].name].to_ascii_uppercase()
    }

    pub(super) fn table_def(&self, node: NodeId, checks: &mut Vec<Check>) {
        let tree = self.tree;
        let Some(name) = tree.children(node).find(|&c| tree.kind(c) == Name) else {
            return;
        };
        let mut push = |kind, range, label: Option<std::string::String>| {
            checks.push(Check {
                kind,
                range,
                name: None,
                label,
            });
        };
        let table = self.text(name).to_owned();
        let info = self.table_info(node);
        let clean = !has_error_in(tree, tree.text_range(node));
        let sized = tree
            .elements(node)
            .any(|e| matches!(e, Element::Token(t) if tree.tokens().kind(t) == LBracket));
        if clean && !sized {
            push(
                CheckKind::TableWithoutSize,
                tree.text_range(name),
                Some(table.clone()),
            );
        } else if clean && info.columns.is_empty() {
            push(
                CheckKind::TableWithoutColumns,
                tree.text_range(name),
                Some(table.clone()),
            );
        }
        let size = tree
            .children(node)
            .find(|&c| !matches!(tree.kind(c), Name | VarDef));
        if let Some(size) = size
            && let Some(n) = literal_int(tree, self.scope.text, size)
            && let Ok(n) = u64::try_from(n)
            && n >= TABLE_SIZE_LIMIT
        {
            push(
                CheckKind::TableSizeOverflow,
                tree.text_range(size),
                Some(format!("{n}|{}", n % TABLE_SIZE_LIMIT)),
            );
        }
        let mut seen = FxHashSet::default();
        for column in &info.columns {
            let column_name = self.text(column.name);
            if !seen.insert(column_name.to_ascii_uppercase()) {
                push(
                    CheckKind::RepeatedColumn,
                    tree.text_range(column.name),
                    Some(column_name.to_owned()),
                );
            }
        }
    }

    pub(super) fn field_expr(&self, node: NodeId, checks: &mut Vec<Check>) {
        let tree = self.tree;
        let mut parts = tree.children(node);
        let (Some(base), Some(member)) = (parts.next(), parts.next()) else {
            return;
        };
        if tree.kind(member) != NameRef {
            return;
        }
        match tree.kind(base) {
            NameRef => match self.base(base) {
                Some(Base::Table(_)) => checks.push(Check {
                    kind: CheckKind::TableWithoutIndex,
                    range: tree.text_range(node),
                    name: None,
                    label: Some(format!("{}|{}", self.text(base), self.text(member))),
                }),
                Some(Base::List(list)) => self.list_member(node, member, list, checks),
                _ => {}
            },
            IndexExpr => self.table_column(node, base, member, checks),
            _ => {}
        }
    }

    fn table_column(&self, node: NodeId, indexed: NodeId, member: NodeId, checks: &mut Vec<Check>) {
        let tree = self.tree;
        let Some(table) = tree.children(indexed).next() else {
            return;
        };
        let Some(Base::Table(Some(def))) = self.base(table) else {
            return;
        };
        let info = self.table_info(def);
        // Sem colunas a definição já está errada, e toda coluna pareceria inexistente.
        if info.columns.is_empty() {
            return;
        }
        let label = format!("{}|{}", self.text(table), self.text(member));
        let check = |kind, range| Check {
            kind,
            range,
            name: None,
            label: Some(label.clone()),
        };
        match self.column(&info, self.text(member)) {
            None => checks.push(check(
                CheckKind::TableUnknownColumn,
                tree.text_range(member),
            )),
            Some(column) if column.ty == Some(VarType::Numero) && column.sized => {
                let has_second_index = tree.parent(node).is_some_and(|p| {
                    tree.kind(p) == IndexExpr && tree.children(p).next() == Some(node)
                });
                if !has_second_index {
                    checks.push(check(
                        CheckKind::TableColumnWithoutIndex,
                        tree.text_range(node),
                    ));
                }
            }
            Some(_) => {}
        }
    }

    fn list_member(
        &self,
        node: NodeId,
        member: NodeId,
        list: Option<usize>,
        checks: &mut Vec<Check>,
    ) {
        let tree = self.tree;
        let name = self.text(member);
        let is_callee = tree
            .parent(node)
            .is_some_and(|p| tree.kind(p) == CallExpr && tree.children(p).next() == Some(node));
        let report = |kind| Check {
            kind,
            range: tree.text_range(member),
            name: None,
            label: Some(name.to_owned()),
        };
        if is_one_of(name, &LIST_FUNCTIONS) {
            // Como comando, `lst.Primeiro;` o parser já recusa e deixa um nó de erro logo depois.
            let parsed_as_error = tree.parent(node).is_some_and(|p| {
                let mut siblings = tree.children(p).skip_while(|&c| c != node);
                siblings.next();
                siblings.next().is_some_and(|c| tree.kind(c) == ErrorNode)
            });
            if !is_callee && !parsed_as_error {
                checks.push(report(CheckKind::ListFunctionWithoutParentheses));
            }
        } else if !is_one_of(name, &LIST_PROPERTIES) && !is_one_of(name, &LIST_PROCEDURES) {
            // Sem nenhum `AdicionarCampo` no arquivo a lista não tem campo nenhum, como no ERP. Só
            // na regra comum, porque num modelo de relatório ou numa tela outra regra pode criar os
            // campos da lista definida aqui.
            let unknown = self.scope.context == crate::builtins::Context::Rule
                && list.is_some_and(|l| match self.lists.get(&l) {
                    Some(fields) => {
                        fields.complete && !fields.names.contains(&name.to_ascii_uppercase())
                    }
                    None => true,
                });
            if unknown {
                let mut check = report(CheckKind::ListNotMember);
                check.label = Some(format!(
                    "{}|{name}",
                    self.text_of_definition(list.unwrap_or_default())
                ));
                checks.push(check);
            }
        }
    }

    pub(super) fn call(&self, node: NodeId, checks: &mut Vec<Check>) {
        let tree = self.tree;
        let Some((base, member, args, _)) = self.method_call(node) else {
            return;
        };
        let name = self.text(member);
        let arguments: Vec<NodeId> = tree.children(args).collect();
        let report = |kind, range| Check {
            kind,
            range,
            name: None,
            label: Some(name.to_owned()),
        };
        match self.base(base) {
            Some(Base::Cursor) => {
                if is_one_of(name, &["AbrirCursor", "FecharCursor", "Proximo"])
                    && let Some(&first) = arguments.first()
                {
                    checks.push(report(
                        CheckKind::CursorCommandArgument,
                        tree.text_range(first),
                    ));
                } else if is_one_of(name, &["Achou", "NaoAchou"]) {
                    checks.push(report(CheckKind::CursorFlagCalled, tree.text_range(args)));
                } else if name.eq_ignore_ascii_case("UsaAbrangencia") && arguments.is_empty() {
                    checks.push(report(
                        CheckKind::CursorScopeWithoutArgument,
                        tree.text_range(args),
                    ));
                } else if name.eq_ignore_ascii_case("UsaTraducao")
                    && self.scope.inherited.senior_sql2 == Some(false)
                {
                    checks.push(report(
                        CheckKind::CursorTranslationWithoutSql2,
                        tree.text_range(member),
                    ));
                }
            }
            Some(Base::List(_)) => self.list_call(name, member, args, &arguments, checks),
            _ => {}
        }
    }

    fn list_call(
        &self,
        name: &str,
        member: NodeId,
        args: NodeId,
        arguments: &[NodeId],
        checks: &mut Vec<Check>,
    ) {
        let tree = self.tree;
        let first_token = |node: NodeId| tree.tokens().kind(tree.token_range(node).start);
        let is_number = |node: NodeId| tree.kind(node) == Literal && first_token(node) == Number;
        let report = |kind, range| Check {
            kind,
            range,
            name: None,
            label: Some(name.to_owned()),
        };
        if is_one_of(name, &LIST_PROPERTIES) {
            checks.push(report(
                CheckKind::ListPropertyWithParentheses,
                tree.text_range(member),
            ));
        } else if name.eq_ignore_ascii_case("AdicionarCampo") {
            // O nome do campo é um texto. Variável fica sem conferência, que pode ser alfa.
            let without_name = match arguments.first() {
                None => true,
                Some(&first) => {
                    is_number(first)
                        || (tree.kind(first) == NameRef
                            && matches!(first_token(first), AlfaKw | NumeroKw | DataKw))
                }
            };
            if without_name {
                checks.push(report(
                    CheckKind::ListFieldWithoutName,
                    tree.text_range(args),
                ));
            }
            if let (Some(&ty), Some(&size)) = (arguments.get(1), arguments.get(2))
                && tree.kind(ty) == NameRef
                && matches!(first_token(ty), NumeroKw | DataKw)
            {
                checks.push(report(CheckKind::ListSizeOnNonAlfa, tree.text_range(size)));
            }
        } else if name.eq_ignore_ascii_case("Chave")
            && arguments.first().is_none_or(|&first| is_number(first))
        {
            checks.push(report(
                CheckKind::ListKeyWithoutFields,
                tree.text_range(args),
            ));
        }
    }

    /// `cur.CAMPO = valor`, que o compilador recusa, o cursor só se lê.
    pub(super) fn assign(&self, node: NodeId, checks: &mut Vec<Check>) {
        let tree = self.tree;
        let Some(lhs) = tree.children(node).next() else {
            return;
        };
        if tree.kind(lhs) != FieldExpr {
            return;
        }
        let mut parts = tree.children(lhs);
        let (Some(base), Some(member)) = (parts.next(), parts.next()) else {
            return;
        };
        if matches!(self.base(base), Some(Base::Cursor)) {
            checks.push(Check {
                kind: CheckKind::CursorMemberAssigned,
                range: tree.text_range(lhs),
                name: None,
                label: Some(self.text(member).to_owned()),
            });
        }
    }

    pub(super) fn index_expr(&self, node: NodeId, checks: &mut Vec<Check>) {
        let tree = self.tree;
        let text = self.scope.text;
        let mut parts = tree.children(node);
        let (Some(base), Some(index)) = (parts.next(), parts.next()) else {
            return;
        };
        let literal = literal_int(tree, text, index);
        let out_of_range = |what: &str, owner: &str, limit: u64, n: i64| Check {
            kind: CheckKind::IndexOutOfRange,
            range: tree.text_range(index),
            name: None,
            label: Some(format!("{n}|{what}|{owner}|{limit}")),
        };
        match tree.kind(base) {
            NameRef => match self.base(base) {
                Some(Base::Table(def)) => {
                    let rows = def.and_then(|d| self.table_info(d).rows);
                    if let (Some(n), Some(rows)) = (literal, rows)
                        && (n < 1 || n as u64 > rows)
                    {
                        checks.push(out_of_range("tabela", self.text(base), rows, n));
                    }
                    if types::is_alfa_leaf(tree, self.scope, index) {
                        checks.push(Check {
                            kind: CheckKind::AlfaTableIndex,
                            range: tree.text_range(index),
                            name: None,
                            label: Some(self.text(base).to_owned()),
                        });
                    }
                }
                Some(Base::Array(var)) => {
                    if let (Some(n), Some(size)) = (literal, self.array_size(var))
                        && (n < 1 || n as u64 > size)
                    {
                        checks.push(out_of_range("variável", self.text(base), size, n));
                    }
                }
                _ => {}
            },
            FieldExpr => self.second_index(base, index, literal, checks),
            _ => {}
        }
    }

    /// Segundo índice de `t[i].coluna[k]`. O compilador confere o literal contra o número de
    /// linhas da tabela e não contra o tamanho da coluna, e o que passa do tamanho da coluna sem
    /// passar da tabela grava na linha seguinte.
    fn second_index(
        &self,
        field: NodeId,
        index: NodeId,
        literal: Option<i64>,
        checks: &mut Vec<Check>,
    ) {
        let tree = self.tree;
        let mut parts = tree.children(field);
        let (Some(row), Some(member)) = (parts.next(), parts.next()) else {
            return;
        };
        if tree.kind(row) != IndexExpr {
            return;
        }
        let Some(table) = tree.children(row).next() else {
            return;
        };
        let Some(Base::Table(Some(def))) = self.base(table) else {
            return;
        };
        let info = self.table_info(def);
        let (Some(n), Some(rows)) = (literal, info.rows) else {
            return;
        };
        let Some(column) = self
            .column(&info, self.text(member))
            .filter(|c| c.ty == Some(VarType::Numero) && c.sized)
        else {
            return;
        };
        let owner = format!("{}.{}", self.text(table), self.text(member));
        if n < 1 || n as u64 > rows {
            checks.push(Check {
                kind: CheckKind::IndexOutOfRange,
                range: tree.text_range(index),
                name: None,
                label: Some(format!("{n}|coluna|{owner}|{rows}")),
            });
        } else if let Some(size) = column.size
            && n as u64 > size
        {
            checks.push(Check {
                kind: CheckKind::ColumnIndexBeyondSize,
                range: tree.text_range(index),
                name: None,
                label: Some(format!("{n}|{}|{size}", self.text(member))),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::validate::CheckKind::{self, *};

    /// Só as conferências novas, porque os exemplos definem mais do que usam.
    fn found(text: &str) -> Vec<(CheckKind, &str)> {
        crate::validate::tests::checks(text)
            .into_iter()
            .filter(|(kind, _)| *kind != Unused)
            .collect()
    }

    #[test]
    fn campo_de_lista_desconhecido_so_na_regra_comum() {
        // No modelo de relatório e na tela, outra regra pode criar o campo com AdicionarCampo.
        let text = "Definir Lista lst;\nDefinir Numero n;\nn = lst.Conta;\n";
        let tree = crate::parse(text).unwrap();
        let symbols = crate::symbols::Symbols::build(&tree, text);
        let unknown = |context| {
            crate::validate::validate(
                &tree,
                &symbols,
                text,
                context,
                &crate::form::Inherited::default(),
            )
            .iter()
            .any(|c| c.kind == ListNotMember)
        };
        assert!(unknown(crate::builtins::Context::Rule));
        assert!(!unknown(crate::builtins::Context::Report));
        assert!(!unknown(crate::builtins::Context::Sgi));
    }

    fn com_sql2(text: &str, senior_sql2: Option<bool>) -> Vec<(CheckKind, &str)> {
        let tree = crate::parse(text).unwrap();
        let mut inherited = crate::form::Inherited::default();
        inherited.senior_sql2 = senior_sql2;
        crate::validate::validate(
            &tree,
            &crate::symbols::Symbols::build(&tree, text),
            text,
            crate::builtins::Context::Rule,
            &inherited,
        )
        .into_iter()
        .filter(|c| c.kind == CursorTranslationWithoutSql2)
        .map(|c| (c.kind, &text[c.range]))
        .collect()
    }

    #[test]
    fn tabela_sem_tamanho_ou_sem_coluna() {
        assert_eq!(
            found("Definir Tabela t = { Numero a; };"),
            [(TableWithoutSize, "t")]
        );
        assert_eq!(
            found("Definir Tabela t[3] = { };"),
            [(TableWithoutColumns, "t")]
        );
        // As formas que o compilador aceita, inclusive o tamanho 0.
        assert!(found("Definir Tabela t[3] = { Numero a; };").is_empty());
        assert!(found("Definir Tabela t[0] = { Alfa Nome[0]; };").is_empty());
        assert!(found("Definir Tabela t[3] = inicio Numero a; fim;").is_empty());
        // A coluna sem nome o parser já recusa.
        let tree = crate::parse("Definir Tabela t[10] = { Numero ; };").unwrap();
        assert!(!tree.errors().is_empty());
    }

    #[test]
    fn coluna_sem_indice_ou_inexistente() {
        let tabela = "Definir Tabela t[3] = { Numero a; Numero v[2]; Alfa Nome[20]; };\n\
                      Definir Numero n;\nDefinir Alfa aTxt;\n";
        let regra = |corpo: &str| format!("{tabela}{corpo}");
        let casos = [
            ("n = t.a;", vec![(TableWithoutIndex, "t.a")]),
            ("n = t[1].v;", vec![(TableColumnWithoutIndex, "t[1].v")]),
            ("n = t[1].zz;", vec![(TableUnknownColumn, "zz")]),
            ("t[1].zz = 1;", vec![(TableUnknownColumn, "zz")]),
            // O que compila.
            ("n = t[1].a; n = t[1].v[2]; aTxt = t[2].nome;", vec![]),
            ("t[n].v[n] = 1; n = t[n + 1].a;", vec![]),
            ("aTxt = t[1].Nome; aTxt = t[1].NOME;", vec![]),
        ];
        for (corpo, esperado) in casos {
            let text = regra(corpo);
            let found: Vec<_> = found(&text);
            assert_eq!(found, esperado, "{corpo}");
        }
    }

    #[test]
    fn tabela_de_outra_regra_nao_e_conferida() {
        // Sem o `Definir Tabela` no arquivo não se sabe as colunas nem o tamanho.
        assert!(found("n = tOutra[9].zz; n = tOutra[0].a;").is_empty());
    }

    #[test]
    fn indice_literal_fora_da_faixa() {
        let regra = |definir: &str, corpo: &str| format!("{definir}\nDefinir Numero n;\n{corpo}");
        let tabela = "Definir Tabela t[3] = { Numero a; };";
        assert_eq!(
            found(&regra(
                tabela,
                "n = t[0].a; n = t[4].a; n = t[3].a; n = t[1].a;"
            )),
            [(IndexOutOfRange, "0"), (IndexOutOfRange, "4")]
        );
        assert_eq!(
            found(&regra(tabela, "n = t[-1].a;")),
            [(IndexOutOfRange, "-1")]
        );
        // Expressão e variável não se conferem na compilação.
        assert!(found(&regra(tabela, "n = t[n].a; n = t[1 + 5].a; n = t[(7)].a;")).is_empty());
        // Tabela de tamanho 0 recusa qualquer índice literal.
        assert_eq!(
            found(&regra(
                "Definir Tabela t[0] = { Numero a; };",
                "n = t[1].a;"
            )),
            [(IndexOutOfRange, "1")]
        );
        // O tamanho é guardado em 16 bits.
        assert_eq!(
            found(&regra(
                "Definir Tabela t[65536] = { Numero a; };",
                "n = t[1].a;"
            )),
            [(TableSizeOverflow, "65536"), (IndexOutOfRange, "1")]
        );
        let grande = "Definir Tabela t[70000] = { Numero a; };";
        assert_eq!(
            found(&regra(grande, "n = t[4464].a; n = t[4465].a;")),
            [(TableSizeOverflow, "70000"), (IndexOutOfRange, "4465")]
        );
        assert_eq!(
            found(&regra(
                "Definir Tabela t[65535] = { Numero a; };",
                "n = t[65535].a;"
            )),
            []
        );
        // Variável numérica com tamanho. O alfa com tamanho é o tamanho do texto.
        let variavel = "Definir Numero x[3]; Definir Alfa aTxt[20];";
        assert_eq!(
            found(&regra(
                variavel,
                "n = x[4]; n = x[0]; n = x[3]; n = x[1]; aTxt[25] = \"a\";"
            )),
            [(IndexOutOfRange, "4"), (IndexOutOfRange, "0")]
        );
    }

    #[test]
    fn segundo_indice_da_coluna() {
        let regra = |linhas: u32, tamanho: u32, corpo: &str| {
            format!(
                "Definir Tabela t[{linhas}] = {{ Numero v[{tamanho}]; }};\nDefinir Numero n;\n{corpo}"
            )
        };
        // Confere contra as linhas da tabela, não contra o tamanho da coluna.
        assert_eq!(
            found(&regra(
                3,
                2,
                "t[1].v[0] = 1; t[1].v[4] = 1; t[1].v[3] = 1; t[1].v[2] = 1;"
            )),
            [
                (IndexOutOfRange, "0"),
                (IndexOutOfRange, "4"),
                (ColumnIndexBeyondSize, "3")
            ]
        );
        assert_eq!(
            found(&regra(2, 4, "t[1].v[3] = 1; t[1].v[2] = 1;")),
            [(IndexOutOfRange, "3")]
        );
        assert_eq!(
            found(&regra(5, 2, "t[1].v[5] = 1; t[1].v[6] = 1;")),
            [(ColumnIndexBeyondSize, "5"), (IndexOutOfRange, "6")]
        );
        assert!(found(&regra(3, 2, "t[1].v[n] = 1; n = t[n].v[n + 9];")).is_empty());
    }

    #[test]
    fn indice_alfa_em_tabela() {
        let tabela =
            "Definir Tabela t[3] = { Numero a; };\nDefinir Alfa aTxt;\nDefinir Numero n;\n";
        let regra = |corpo: &str| format!("{tabela}{corpo}");
        assert_eq!(found(&regra("n = t[aTxt].a;")), [(AlfaTableIndex, "aTxt")]);
        assert_eq!(
            found(&regra("n = t[\"1\"].a;")),
            [(AlfaTableIndex, "\"1\"")]
        );
        assert!(found(&regra("n = t[n].a; n = t[n + 1].a;")).is_empty());
    }

    #[test]
    fn comandos_do_cursor_que_o_compilador_recusa() {
        let cursor =
            |corpo: &str| format!("Definir Cursor c;\nc.SQL \"SELECT A FROM T\";\n{corpo}");
        let casos = [
            ("c.AbrirCursor(1);", vec![(CursorCommandArgument, "1")]),
            ("c.FecharCursor(1);", vec![(CursorCommandArgument, "1")]),
            ("c.Proximo(1);", vec![(CursorCommandArgument, "1")]),
            ("se (c.Achou()) { n = 1; }", vec![(CursorFlagCalled, "()")]),
            (
                "se (c.NaoAchou()) { n = 1; }",
                vec![(CursorFlagCalled, "()")],
            ),
            (
                "c.UsaAbrangencia();",
                vec![(CursorScopeWithoutArgument, "()")],
            ),
            ("c.A = 1;", vec![(CursorMemberAssigned, "c.A")]),
            // O que compila.
            ("c.AbrirCursor(); c.Proximo(); c.FecharCursor();", vec![]),
            (
                "se (c.Achou) { n = c.A; } se (c.NaoAchou) { n = 1; }",
                vec![],
            ),
            ("c.UsaAbrangencia(1); c.UsaAbrangencia(0);", vec![]),
            ("n = c.A;", vec![]),
        ];
        for (corpo, esperado) in casos {
            let text = cursor(corpo);
            assert_eq!(found(&text), esperado, "{corpo}");
        }
    }

    #[test]
    fn usatraducao_so_no_senior_sql_2() {
        let text = "Definir Cursor c;\nc.UsaTraducao();";
        // Sem a exportação dizendo o modo, não se sabe, e fica sem aviso.
        assert!(com_sql2(text, None).is_empty());
        assert!(com_sql2(text, Some(true)).is_empty());
        assert_eq!(
            com_sql2(text, Some(false)),
            [(CursorTranslationWithoutSql2, "UsaTraducao")]
        );
    }

    #[test]
    fn membro_de_lista() {
        let lista = "Definir Lista l;\nDefinir Numero n;\nl.AdicionarCampo(\"Cod\", Numero);\n";
        let regra = |corpo: &str| format!("{lista}{corpo}");
        let casos = [
            ("l.Foo();", vec![(ListNotMember, "Foo")]),
            ("l.Bar = 1;", vec![(ListNotMember, "Bar")]),
            (
                "n = l.Primeiro;",
                vec![(ListFunctionWithoutParentheses, "Primeiro")],
            ),
            (
                "se (l.Ultimo) { n = 1; }",
                vec![(ListFunctionWithoutParentheses, "Ultimo")],
            ),
            (
                "n = l.NumReg();",
                vec![(ListPropertyWithParentheses, "NumReg")],
            ),
            (
                "n = l.QtdRegistros();",
                vec![(ListPropertyWithParentheses, "QtdRegistros")],
            ),
            ("n = l.IDA();", vec![(ListPropertyWithParentheses, "IDA")]),
            // O que compila, com os procedimentos escritos de qualquer forma e o campo criado.
            (
                "l.Primeiro(); l.Ultimo(); l.Anterior(); l.Proximo(); l.VaiParaChave();",
                vec![],
            ),
            (
                "n = l.NumReg; n = l.IDA; n = l.FDA; n = l.QtdRegistros;",
                vec![],
            ),
            (
                "l.DefinirCampos(); l.EfetivarCampos(); l.Adicionar(); l.Inserir();",
                vec![],
            ),
            (
                "l.Editar(); l.Gravar(); l.Cancelar(); l.Excluir(); l.Limpar();",
                vec![],
            ),
            (
                "l.SetaNumReg(1); l.Chave(\"Cod\"); l.EditarChave(); l.SetarChave();",
                vec![],
            ),
            ("n = l.Cod; l.COD = 2;", vec![]),
        ];
        for (corpo, esperado) in casos {
            let text = regra(corpo);
            assert_eq!(found(&text), esperado, "{corpo}");
        }
        // O comando `l.Primeiro;` é erro de sintaxe, que o parser já diz.
        let tree = crate::parse("Definir Lista l;\nl.Primeiro;").unwrap();
        assert!(!tree.errors().is_empty());
        let text = "Definir Lista l;\nl.Primeiro;";
        let checks = crate::validate::validate(
            &tree,
            &crate::symbols::Symbols::build(&tree, text),
            text,
            crate::builtins::Context::Rule,
            &crate::form::Inherited::default(),
        );
        assert!(
            checks
                .iter()
                .all(|c| c.kind != ListFunctionWithoutParentheses)
        );
    }

    #[test]
    fn campo_de_lista_com_nome_que_nao_e_literal_nao_e_conferido() {
        let text = "Definir Lista l;\nDefinir Alfa aNome;\nl.AdicionarCampo(aNome, Numero);\n\
                    n = l.Qualquer;";
        assert!(found(text).is_empty());
        // A lista de outra regra da tela não diz os campos.
        assert!(found("n = lOutra.Qualquer;").is_empty());
    }

    #[test]
    fn adicionar_campo_e_chave() {
        let lista = |corpo: &str| format!("Definir Lista l;\n{corpo}");
        let casos = [
            ("l.AdicionarCampo();", vec![(ListFieldWithoutName, "()")]),
            (
                "l.AdicionarCampo(1, Numero);",
                vec![(ListFieldWithoutName, "(1, Numero)")],
            ),
            (
                "l.AdicionarCampo(\"a\", Numero, 5);",
                vec![(ListSizeOnNonAlfa, "5")],
            ),
            (
                "l.AdicionarCampo(\"a\", Data, 5);",
                vec![(ListSizeOnNonAlfa, "5")],
            ),
            (
                "l.AdicionarCampo(\"a\", Numero);\nl.AdicionarCampo(\"A\", Alfa, 3);",
                vec![(ListRepeatedField, "\"A\"")],
            ),
            ("l.Chave(1);", vec![(ListKeyWithoutFields, "(1)")]),
            ("l.Chave();", vec![(ListKeyWithoutFields, "()")]),
            // O que compila.
            ("l.AdicionarCampo(\"a\", Alfa, 5);", vec![]),
            (
                "l.AdicionarCampo(\"a\", Numero);\nl.AdicionarCampo(\"b\", Data);",
                vec![],
            ),
            ("l.Chave(\"a;b\");", vec![]),
        ];
        for (corpo, esperado) in casos {
            let text = lista(corpo);
            assert_eq!(found(&text), esperado, "{corpo}");
        }
        // Duas listas podem ter o mesmo nome de campo.
        assert!(
            found(
                "Definir Lista l; Definir Lista m;\nl.AdicionarCampo(\"a\", Numero);\n\
                 m.AdicionarCampo(\"a\", Numero);"
            )
            .is_empty()
        );
    }

    #[test]
    fn coluna_repetida_na_tabela() {
        assert_eq!(
            found("Definir Tabela t[3] = { Alfa Nome[20]; Numero NOME; Numero b; };"),
            [(RepeatedColumn, "NOME")]
        );
        assert!(found("Definir Tabela t[3] = { Numero a; Numero b; };").is_empty());
    }

    /// Pedaços de tabela, cursor, lista, índice e rótulo em qualquer ordem, para o validador não
    /// quebrar com índice negativo, enorme ou fora de lugar.
    #[test]
    fn texto_aleatorio_nao_quebra_as_conferencias() {
        let pecas = [
            "Definir Tabela t[3] = { Numero v[2]; Alfa a[5]; };",
            "Definir Tabela u[70000] = { Numero a; };",
            "Definir Tabela w = { };",
            "Definir Numero x[3];",
            "Definir Cursor c;",
            "Definir Lista l;",
            "Definir Alfa aTxt;",
            "t[1].v[2] = 1;",
            "t[-1].v[99999999999999999999] = 1;",
            "n = t[aTxt].zz;",
            "n = t.v; n = t[1].v;",
            "x[0] = x[4];",
            "c.A = 1; c.AbrirCursor(1); c.UsaAbrangencia(); c.Achou();",
            "l.AdicionarCampo(); l.AdicionarCampo(\"a\", Numero, 3);",
            "l.Foo; l.Primeiro; l.NumReg(); l.Chave(1);",
            "VaPara r; r: r:",
            "Definir Funcao f(Alfa p, Alfa p);",
            "f(\"a\" + aTxt); f(\"x\");",
            "Funcao f(Alfa p, Alfa p); Inicio Fim;",
            "{ }",
            "(",
            ")",
            "[",
            "]",
            ".",
            ";",
        ];
        let mut estado: u64 = 0x2545_f491_4f6c_dd1d;
        let mut proximo = move || {
            estado ^= estado << 13;
            estado ^= estado >> 7;
            estado ^= estado << 17;
            estado
        };
        for _ in 0..3_000 {
            let n = (proximo() % 8) as usize;
            let text: std::string::String = (0..n)
                .map(|_| pecas[(proximo() % pecas.len() as u64) as usize])
                .collect::<Vec<_>>()
                .join("\n");
            let tree = crate::parse(&text).unwrap();
            crate::validate::validate(
                &tree,
                &crate::symbols::Symbols::build(&tree, &text),
                &text,
                crate::builtins::Context::Rule,
                &crate::form::Inherited::default(),
            );
        }
    }
}
