//! Conferências do SQL das regras.
//!
//! O SQL do cursor (`cur.SQL "..."`) é lido pelo gerador de SQL da Senior já na compilação, que
//! recusa constante entre as colunas, campo do cursor fora do SELECT e campo de tipo diferente da
//! variável. O `SQL_UsarSQLSenior2`, que liga o SQL nativo, e o `SQL_Retornar*` de coluna que não
//! está no SELECT só falham na execução. Todos os casos foram conferidos no ERP.
//!
//! As colunas de um cursor ou de um objeto de SQL juntam todos os SQL literais que ele recebe no
//! arquivo, sem olhar a ordem, então um nome só é cobrado quando não está em nenhum. Um SQL que não
//! é literal, que o `sqlparser` não lê ou que tem coluna sem nome conhecido deixa o cursor ou o
//! objeto sem conferência.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use sqlparser::ast::{
    Expr, Select, SelectItem, SetExpr, Statement, TableFactor, TableWithJoins, UnaryOperator,
};
use sqlparser::dialect::OracleDialect;
use sqlparser::parser::Parser;

use super::{Check, CheckKind};
use crate::dictionary::Dictionaries;
use crate::symbols::{DefKind, Symbols, VarType};
use crate::{NodeId, SyntaxKind, Tree};

/// Operações do cursor, que não são colunas. São as únicas que aparecem nas regras.
const CURSOR_OPERATIONS: [&str; 6] = [
    "SQL",
    "ABRIRCURSOR",
    "FECHARCURSOR",
    "PROXIMO",
    "ACHOU",
    "NAOACHOU",
];

/// Texto de um literal de string, com aspas, como o compilador o monta. `\"` e `\\` são escape,
/// e qualquer outra barra descarta o resto da linha e emenda a seguinte, cujo primeiro caractere
/// entra sem ser examinado. No lugar do trecho descartado fica um espaço, para o SQL não juntar
/// duas palavras.
pub(super) fn string_value(literal: &str) -> String {
    let mut value = String::new();
    let mut chars = literal.chars().skip(1).peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' if matches!(chars.peek(), Some('"' | '\\')) => value.extend(chars.next()),
            '\\' => {
                chars.by_ref().find(|&c| c == '\n');
                value.push(' ');
                value.extend(chars.next());
            }
            _ => value.push(c),
        }
    }
    value
}

/// Colunas constantes do SELECT, como `123` ou `'ABC'`. O cursor recusa todas com `O campo
/// precisa ser informado com a tabela`, e um decimal como `123.45` é lido como tabela e campo
/// (`RowSet inexistente: 123`). Coluna sem tabela e `*` compilam. SQL que o `sqlparser` não lê
/// não é conferido.
fn constants_of(statements: &[Statement]) -> Vec<String> {
    let mut constants = Vec::new();
    for statement in statements {
        let Statement::Query(query) = statement else {
            continue;
        };
        let SetExpr::Select(select) = query.body.as_ref() else {
            continue;
        };
        for item in &select.projection {
            let expr = match item {
                SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => expr,
                _ => continue,
            };
            if is_constant(expr) {
                constants.push(expr.to_string());
            }
        }
    }
    constants
}

/// O que as conferências tiram de um SQL, depois de lê-lo uma única vez.
pub(super) struct Analysis {
    /// As colunas constantes do SELECT, que o cursor recusa.
    pub(super) constants: Vec<String>,
    /// As colunas que o SELECT devolve, ou `None` quando não dá para saber todas.
    pub(super) columns: Option<Vec<Column>>,
}

/// Quanto SQL, em bytes de texto, o cache guarda antes de recomeçar vazio.
const CACHE_LIMIT: usize = 512 * 1024;

type SharedDictionary = Option<Arc<crate::tbs::Dictionary>>;

/// Resultado das conferências de cada SQL já lido, para a próxima análise da regra não ler de novo
/// o que não mudou. Cada tecla analisa o arquivo inteiro, e quase todo SQL fica igual de uma
/// análise para a outra. As colunas dependem do dicionário de dados, então o cache vale para os
/// dicionários que guarda e recomeça vazio quando eles trocam. Guardar os `Arc` também impede que
/// o endereço de um dicionário solto seja reaproveitado por outro.
#[derive(Default)]
struct Cache {
    dictionaries: (SharedDictionary, SharedDictionary),
    entries: HashMap<String, Rc<Analysis>>,
    bytes: usize,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::default();
}

fn same_dictionary(a: &SharedDictionary, b: &SharedDictionary) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

/// Resultado das conferências do SQL, calculado na primeira vez e lembrado depois.
pub(super) fn analysis(sql: &str, dictionaries: &Dictionaries) -> Rc<Analysis> {
    let cached = CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let (production, test) = &cache.dictionaries;
        if !same_dictionary(production, &dictionaries.production)
            || !same_dictionary(test, &dictionaries.test)
        {
            *cache = Cache {
                dictionaries: (dictionaries.production.clone(), dictionaries.test.clone()),
                ..Cache::default()
            };
        }
        cache.entries.get(sql).cloned()
    });
    if let Some(hit) = cached {
        return hit;
    }
    let statements = Parser::parse_sql(&OracleDialect {}, sql).ok();
    let analysis = Rc::new(Analysis {
        constants: statements.as_deref().map(constants_of).unwrap_or_default(),
        columns: statements.and_then(|s| select_columns(&s, dictionaries)),
    });
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.bytes + sql.len() > CACHE_LIMIT {
            cache.entries.clear();
            cache.bytes = 0;
        }
        cache.bytes += sql.len();
        cache.entries.insert(sql.to_owned(), analysis.clone());
    });
    analysis
}

fn is_constant(expr: &Expr) -> bool {
    match expr {
        Expr::Value(_) => true,
        Expr::UnaryOp {
            op: UnaryOperator::Minus | UnaryOperator::Plus,
            expr,
        } => is_constant(expr),
        _ => false,
    }
}

/// Coluna que um SELECT devolve, pelo nome em maiúsculas, com o tipo do campo no dicionário quando
/// dá para saber de qual tabela ela vem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Column {
    pub(super) name: String,
    pub(super) ty: Option<VarType>,
}

/// Colunas do SELECT, ou `None` quando não dá para saber todas. O nome é o apelido, ou o do campo
/// com ou sem a tabela. `*` e `T.*` precisam do dicionário para virar campos.
fn select_columns(statements: &[Statement], dictionaries: &Dictionaries) -> Option<Vec<Column>> {
    let [Statement::Query(query)] = statements else {
        return None;
    };
    let SetExpr::Select(select) = query.body.as_ref() else {
        return None;
    };
    let from = From::read(select);
    let mut columns = Vec::new();
    for item in &select.projection {
        match item {
            SelectItem::UnnamedExpr(expr) => {
                let (qualifier, field) = column_ref(expr)?;
                columns.push(Column {
                    name: field.to_uppercase(),
                    ty: from.field_type(dictionaries, qualifier, field),
                });
            }
            SelectItem::ExprWithAlias { expr, alias } => columns.push(Column {
                name: alias.value.to_uppercase(),
                ty: column_ref(expr).and_then(|(q, f)| from.field_type(dictionaries, q, f)),
            }),
            SelectItem::Wildcard(_) => {
                if from.opaque || from.tables.is_empty() {
                    return None;
                }
                for (table, _) in &from.tables {
                    columns.extend(table_columns(dictionaries, table)?);
                }
            }
            SelectItem::QualifiedWildcard(..) => {
                let text = item.to_string();
                let qualifier = text.strip_suffix(".*")?;
                columns.extend(table_columns(dictionaries, &from.table(qualifier)?)?);
            }
            // Vários apelidos para uma expressão, de outros dialetos.
            _ => return None,
        }
    }
    Some(columns)
}

/// `CAMPO` ou `TABELA.CAMPO`.
fn column_ref(expr: &Expr) -> Option<(Option<&str>, &str)> {
    match expr {
        Expr::Identifier(field) => Some((None, &field.value)),
        Expr::CompoundIdentifier(parts) => match parts.as_slice() {
            [table, field] => Some((Some(&table.value), &field.value)),
            _ => None,
        },
        _ => None,
    }
}

fn table_columns(dictionaries: &Dictionaries, table: &str) -> Option<Vec<Column>> {
    let table = dictionaries.table(table)?;
    Some(
        table
            .fields
            .iter()
            .map(|f| Column {
                name: f.name.to_uppercase(),
                ty: f.ty.var_type(),
            })
            .collect(),
    )
}

/// Tabelas do FROM, com o apelido, em maiúsculas. `opaque` diz que há alguma relação que não é
/// tabela, como uma subconsulta.
struct From {
    tables: Vec<(String, Option<String>)>,
    opaque: bool,
}

impl From {
    fn read(select: &Select) -> From {
        let mut from = From {
            tables: Vec::new(),
            opaque: false,
        };
        let mut add = |factor: &TableFactor| match factor {
            TableFactor::Table { name, alias, .. } => {
                let name = name.to_string().to_uppercase();
                let name = name.rsplit('.').next().unwrap_or(&name).to_owned();
                from.tables
                    .push((name, alias.as_ref().map(|a| a.name.value.to_uppercase())));
            }
            _ => from.opaque = true,
        };
        for TableWithJoins { relation, joins } in &select.from {
            add(relation);
            for join in joins {
                add(&join.relation);
            }
        }
        from
    }

    /// Tabela de um apelido ou nome do FROM.
    fn table(&self, qualifier: &str) -> Option<String> {
        let qualifier = qualifier.to_uppercase();
        self.tables
            .iter()
            .find(|(name, alias)| alias.as_deref() == Some(&qualifier) || *name == qualifier)
            .map(|(name, _)| name.clone())
    }

    /// Tipo de um campo pela tabela dele. Sem a tabela na frente, vale a única tabela do FROM que
    /// tem o campo no dicionário.
    fn field_type(
        &self,
        dictionaries: &Dictionaries,
        qualifier: Option<&str>,
        field: &str,
    ) -> Option<VarType> {
        let table = match qualifier {
            Some(q) => self.table(q)?,
            None if self.opaque => return None,
            None => {
                let mut with_field = self
                    .tables
                    .iter()
                    .filter(|(t, _)| dictionaries.field(t, field).is_some());
                let (table, _) = with_field.next()?;
                if with_field.next().is_some() {
                    return None;
                }
                table.clone()
            }
        };
        dictionaries.field(&table, field)?.field.ty.var_type()
    }
}

/// Colunas do SELECT do cursor `cursor`, com o tipo quando o dicionário diz, para o completar
/// depois de `cur.`. `None` quando o cursor não tem SQL conhecido.
pub fn cursor_columns(
    tree: &Tree,
    symbols: &Symbols,
    text: &str,
    dictionaries: &Dictionaries,
    cursor: &str,
) -> Option<Vec<(String, Option<VarType>)>> {
    let sources = Sources::build(tree, symbols, text, dictionaries);
    let columns = sources.cursors.get(&cursor.to_uppercase())?.as_ref()?;
    Some(columns.iter().map(|c| (c.name.clone(), c.ty)).collect())
}

/// Campo de `cur.CAMPO` e os literais de todos os `cur.SQL "..."` do mesmo cursor no arquivo, para
/// achar a coluna no texto do SELECT. `None` quando o nó não é o campo de um cursor definido.
pub(crate) fn cursor_sql_literals<'t>(
    tree: &Tree,
    symbols: &Symbols,
    text: &'t str,
    node: NodeId,
) -> Option<(&'t str, Vec<text_size::TextRange>)> {
    let (cursor, field) = cursor_field(tree, symbols, text, node)?;
    let mut literals = Vec::new();
    let mut stack = vec![tree.root()];
    while let Some(node) = stack.pop() {
        stack.extend(tree.children(node));
        if tree.kind(node) != SyntaxKind::FieldCommand {
            continue;
        }
        let is_sql = tree
            .children(node)
            .next()
            .and_then(|f| cursor_field(tree, symbols, text, f))
            .is_some_and(|(c, f)| c == cursor && f.eq_ignore_ascii_case("SQL"));
        if let Some(literal) = tree
            .children(node)
            .nth(1)
            .filter(|_| is_sql)
            .and_then(|value| string_literal(tree, value))
        {
            literals.push(literal);
        }
    }
    literals.sort_by_key(|r| r.start());
    Some((field, literals))
}

/// Colunas de cada cursor e de cada objeto de `SQL_DefinirComando`, pelo nome em maiúsculas.
/// `None` é cursor ou objeto sem conferência.
#[derive(Default)]
pub(super) struct Sources {
    cursors: HashMap<String, Option<Vec<Column>>>,
    objects: HashMap<String, Option<Vec<Column>>>,
}

impl Sources {
    pub(super) fn build(
        tree: &Tree,
        symbols: &Symbols,
        text: &str,
        dictionaries: &Dictionaries,
    ) -> Sources {
        let mut sources = Sources::default();
        let mut stack = vec![tree.root()];
        while let Some(node) = stack.pop() {
            stack.extend(tree.children(node));
            match tree.kind(node) {
                SyntaxKind::FieldCommand => {
                    let Some((cursor, field)) = tree
                        .children(node)
                        .next()
                        .and_then(|f| cursor_field(tree, symbols, text, f))
                    else {
                        continue;
                    };
                    if !field.eq_ignore_ascii_case("SQL") {
                        continue;
                    }
                    let analysis = tree
                        .children(node)
                        .nth(1)
                        .and_then(|value| string_literal(tree, value))
                        .map(|lit| analysis(&string_value(&text[lit]), dictionaries));
                    let columns = analysis.as_ref().and_then(|a| a.columns.as_deref());
                    merge(&mut sources.cursors, cursor, columns);
                }
                SyntaxKind::CallStmt => {
                    let Some((callee, args)) = call(tree, text, node) else {
                        continue;
                    };
                    if !callee.eq_ignore_ascii_case("SQL_DefinirComando") {
                        continue;
                    }
                    let (Some(object), Some(command)) = (args.first(), args.get(1)) else {
                        continue;
                    };
                    if tree.kind(*object) != SyntaxKind::NameRef {
                        continue;
                    }
                    let analysis = string_literal(tree, *command)
                        .map(|lit| analysis(&string_value(&text[lit]), dictionaries));
                    let columns = analysis.as_ref().and_then(|a| a.columns.as_deref());
                    let object = text[tree.text_range(*object)].to_uppercase();
                    merge(&mut sources.objects, object, columns);
                }
                _ => {}
            }
        }
        sources
    }

    /// Tipo de `cur.CAMPO`, quando o cursor tem só SQL conhecido e o campo tem tipo.
    pub(super) fn cursor_field_type(
        &self,
        tree: &Tree,
        symbols: &Symbols,
        text: &str,
        node: NodeId,
    ) -> Option<VarType> {
        let (cursor, field) = cursor_field(tree, symbols, text, node)?;
        let columns = self.cursors.get(&cursor)?.as_ref()?;
        let field = field.to_uppercase();
        columns.iter().find(|c| c.name == field)?.ty
    }

    /// `cur.CAMPO` com um campo que não é coluna do SQL do cursor nem operação.
    pub(super) fn check_cursor_field(
        &self,
        tree: &Tree,
        symbols: &Symbols,
        text: &str,
        node: NodeId,
        checks: &mut Vec<Check>,
    ) {
        let Some((cursor, field)) = cursor_field(tree, symbols, text, node) else {
            return;
        };
        let field = field.to_uppercase();
        if CURSOR_OPERATIONS.contains(&field.as_str()) {
            return;
        }
        let Some(Some(columns)) = self.cursors.get(&cursor) else {
            return;
        };
        if columns.iter().any(|c| c.name == field) {
            return;
        }
        if let Some(name) = tree.children(node).nth(1) {
            checks.push(Check {
                kind: CheckKind::CursorFieldNotInSelect,
                range: tree.text_range(name),
                name: None,
                label: Some(cursor),
            });
        }
    }

    /// `SQL_Retornar*(obj, "coluna", ...)` com uma coluna que não está no SQL do objeto.
    pub(super) fn check_returned_column(
        &self,
        tree: &Tree,
        text: &str,
        node: NodeId,
        checks: &mut Vec<Check>,
    ) {
        let Some((callee, args)) = call(tree, text, node) else {
            return;
        };
        if !callee
            .get(..12)
            .is_some_and(|p| p.eq_ignore_ascii_case("SQL_Retornar"))
        {
            return;
        }
        let (Some(object), Some(column)) = (args.first(), args.get(1)) else {
            return;
        };
        if tree.kind(*object) != SyntaxKind::NameRef {
            return;
        }
        let Some(literal) = string_literal(tree, *column) else {
            return;
        };
        let object = text[tree.text_range(*object)].to_uppercase();
        let Some(Some(columns)) = self.objects.get(&object) else {
            return;
        };
        let name = string_value(&text[literal]).trim().to_uppercase();
        if !columns.iter().any(|c| c.name == name) {
            checks.push(Check {
                kind: CheckKind::ReturnedColumnNotInSelect,
                range: literal,
                name: None,
                label: Some(object),
            });
        }
    }
}

/// Junta as colunas de mais um SQL às que o cursor ou objeto já tem. Um SQL desconhecido deixa
/// tudo desconhecido, e o mesmo nome com tipos diferentes fica sem tipo.
fn merge(map: &mut HashMap<String, Option<Vec<Column>>>, key: String, columns: Option<&[Column]>) {
    let entry = map.entry(key).or_insert_with(|| Some(Vec::new()));
    let (Some(known), Some(columns)) = (entry.as_mut(), columns) else {
        *entry = None;
        return;
    };
    for column in columns {
        match known.iter_mut().find(|c| c.name == column.name) {
            Some(existing) if existing.ty != column.ty => existing.ty = None,
            Some(_) => {}
            None => known.push(column.clone()),
        }
    }
}

/// Cursor e campo de `cur.CAMPO`, com o cursor em maiúsculas. A base tem que ser um cursor
/// definido no arquivo.
fn cursor_field<'t>(
    tree: &Tree,
    symbols: &Symbols,
    text: &'t str,
    node: NodeId,
) -> Option<(String, &'t str)> {
    if tree.kind(node) != SyntaxKind::FieldExpr {
        return None;
    }
    let mut children = tree.children(node);
    let (base, field) = (children.next()?, children.next()?);
    if tree.kind(base) != SyntaxKind::NameRef {
        return None;
    }
    let range = tree.text_range(base);
    let reference = symbols.reference_at(range)?;
    let is_cursor = !reference.targets.is_empty()
        && reference
            .targets
            .iter()
            .all(|&t| symbols.definitions[t].kind == DefKind::Cursor);
    is_cursor.then(|| (text[range].to_uppercase(), &text[tree.text_range(field)]))
}

/// Nome e argumentos de um comando `Funcao(...)`.
fn call<'t>(tree: &Tree, text: &'t str, node: NodeId) -> Option<(&'t str, Vec<NodeId>)> {
    let call = tree
        .children(node)
        .next()
        .filter(|&c| tree.kind(c) == SyntaxKind::CallExpr)?;
    let mut parts = tree.children(call);
    let (callee, args) = (parts.next()?, parts.next()?);
    if tree.kind(callee) != SyntaxKind::NameRef {
        return None;
    }
    Some((
        &text[tree.text_range(callee)],
        tree.children(args).collect(),
    ))
}

/// Trecho do literal de string, quando o nó é só ele. Uma concatenação como `"..." + aVar` fica de
/// fora, porque o texto final só existe na execução.
fn string_literal(tree: &Tree, node: NodeId) -> Option<text_size::TextRange> {
    if tree.kind(node) != SyntaxKind::Literal {
        return None;
    }
    let token = tree.token_range(node).start;
    (tree.tokens().kind(token) == SyntaxKind::String).then(|| tree.tokens().range(token))
}

/// `cur.SQL "SELECT ..."` com uma constante entre as colunas.
pub(super) fn cursor_sql(
    tree: &Tree,
    text: &str,
    node: NodeId,
    dictionaries: &Dictionaries,
    checks: &mut Vec<Check>,
) {
    let Some(field) = tree.children(node).next() else {
        return;
    };
    let is_sql = text[tree.text_range(field)]
        .rsplit('.')
        .next()
        .is_some_and(|name| name.trim().eq_ignore_ascii_case("SQL"));
    if !is_sql {
        return;
    }
    let Some(range) = tree
        .children(node)
        .nth(1)
        .and_then(|value| string_literal(tree, value))
    else {
        return;
    };
    for constant in &analysis(&string_value(&text[range]), dictionaries).constants {
        checks.push(Check {
            kind: CheckKind::CursorConstantColumn,
            range,
            name: None,
            label: Some(constant.clone()),
        });
    }
}

/// Objetos de SQL com `SQL_UsarSQLSenior2(x, 0)` e com `SQL_UsarAbrangencia(x, 0)`, para avisar
/// no fim os que ligam o SQL nativo sem desligar a abrangência. Os nomes não diferenciam caixa,
/// e a ordem das chamadas no arquivo não importa.
#[derive(Default)]
pub(super) struct NativeSql {
    native: Vec<(String, text_size::TextRange)>,
    without_scope: HashSet<String>,
}

impl NativeSql {
    pub(super) fn visit(&mut self, tree: &Tree, text: &str, node: NodeId) {
        let Some(call) = tree
            .children(node)
            .next()
            .filter(|&c| tree.kind(c) == SyntaxKind::CallExpr)
        else {
            return;
        };
        let mut parts = tree.children(call);
        let (Some(callee), Some(args)) = (parts.next(), parts.next()) else {
            return;
        };
        let mut args = tree.children(args);
        let (Some(object), Some(value)) = (args.next(), args.next()) else {
            return;
        };
        if tree.kind(object) != SyntaxKind::NameRef || tree.kind(value) != SyntaxKind::Literal {
            return;
        }
        if text[tree.text_range(value)].trim() != "0" {
            return;
        }
        let callee_range = tree.text_range(callee);
        let object = text[tree.text_range(object)].to_ascii_uppercase();
        let name = &text[callee_range];
        if name.eq_ignore_ascii_case("SQL_UsarSQLSenior2") {
            self.native.push((object, callee_range));
        } else if name.eq_ignore_ascii_case("SQL_UsarAbrangencia") {
            self.without_scope.insert(object);
        }
    }

    pub(super) fn finish(self, checks: &mut Vec<Check>) {
        for (object, range) in self.native {
            if !self.without_scope.contains(&object) {
                checks.push(Check {
                    kind: CheckKind::NativeSqlWithScope,
                    range,
                    name: None,
                    label: Some(object),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valor_da_string_com_escape_e_continuacao() {
        assert_eq!(string_value(r#""a\"b\\c""#), r#"a"b\c"#);
        assert_eq!(
            string_value("\"SELECT A \\ resto\n FROM T\""),
            "SELECT A   FROM T"
        );
        assert_eq!(string_value("\"x \\\r\nyz\""), "x  yz");
    }

    fn constant_columns(sql: &str) -> Vec<String> {
        analysis(sql, &Dictionaries::default()).constants.clone()
    }

    #[test]
    fn o_cache_vale_para_os_dicionarios_que_guarda() {
        use crate::tbs::{Dictionary, Field, FieldKind, FieldType, Table};
        let with_field = |name: &str| {
            let field = Field {
                name: name.to_owned(),
                ty: FieldType {
                    kind: FieldKind::Number,
                    ..FieldType::default()
                },
                ..Field::default()
            };
            Dictionaries {
                production: Some(Arc::new(Dictionary {
                    tables: vec![Table {
                        name: "T".to_owned(),
                        fields: vec![field],
                        ..Table::default()
                    }],
                    ..Dictionary::default()
                })),
                test: None,
            }
        };
        let (one, other) = (with_field("A"), with_field("B"));
        let names = |a: Rc<Analysis>| -> Vec<String> {
            a.columns.iter().flatten().map(|c| c.name.clone()).collect()
        };
        let sql = "SELECT * FROM T";
        assert_eq!(names(analysis(sql, &one)), ["A"]);
        assert_eq!(names(analysis(sql, &one)), ["A"]);
        assert_eq!(names(analysis(sql, &other)), ["B"]);
        assert!(names(analysis(sql, &Dictionaries::default())).is_empty());
    }

    #[test]
    fn constantes_do_select() {
        assert_eq!(
            constant_columns("SELECT 123.45 AS NUM, 'ABC' AS TXT FROM DUAL"),
            ["123.45", "'ABC'"]
        );
        assert_eq!(constant_columns("SELECT -1, T.A FROM T"), ["-1"]);
        assert!(
            constant_columns(
                "SELECT E070EMP.CODEMP, NOMEMP, T.* FROM E070EMP, T WHERE CODEMP = :VSCodEmp"
            )
            .is_empty()
        );
        assert!(constant_columns("SELECT * FROM T WHERE A = 1").is_empty());
        assert!(constant_columns("não é SQL").is_empty());
    }
}
