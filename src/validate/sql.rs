//! Conferências do SQL das regras.
//!
//! O SQL do cursor (`cur.SQL "..."`) é lido pelo gerador de SQL da Senior já na compilação, que
//! recusa constante entre as colunas, campo do cursor fora do SELECT e campo de tipo diferente da
//! variável. O `SQL_UsarSQLSenior2`, que liga o SQL nativo, e o `SQL_Retornar*` de coluna que não
//! está no SELECT só falham na execução. A tabela e o campo de cada coluna do cursor, que o
//! gerador procura no dicionário de dados, seguem a leitura descrita no [`compiler_columns`].
//! Todos os casos foram conferidos no ERP.
//!
//! As colunas de um cursor ou de um objeto de SQL juntam todos os SQL literais que ele recebe no
//! arquivo, sem olhar a ordem, então um nome só é cobrado quando não está em nenhum. Um SQL que não
//! é literal, que o `sqlparser` não lê ou que tem coluna sem nome conhecido deixa o cursor ou o
//! objeto sem conferência.

use std::cell::RefCell;
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
use crate::tbs::Field;
use crate::{NodeId, SyntaxKind, Tree};
use rustc_hash::{FxHashMap, FxHashSet};

/// Operações do cursor, que não são colunas. São as únicas que aparecem nas regras.
const CURSOR_OPERATIONS: [&str; 6] = [
    "SQL",
    "ABRIRCURSOR",
    "FECHARCURSOR",
    "PROXIMO",
    "ACHOU",
    "NAOACHOU",
];

/// Percorre um literal de string, com aspas, como o compilador o monta, e entrega cada caractere
/// do texto com a posição que ele ocupa no literal. `\"` e `\\` são escape, e qualquer outra barra
/// descarta o resto da linha e emenda a seguinte, cujo primeiro caractere entra sem ser examinado.
/// No lugar do trecho descartado fica um espaço, para o SQL não juntar duas palavras.
fn walk_string(literal: &str, mut push: impl FnMut(char, usize)) {
    let mut chars = literal.char_indices().skip(1).peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => break,
            '\\' if matches!(chars.peek(), Some((_, '"' | '\\'))) => {
                if let Some((at, c)) = chars.next() {
                    push(c, at);
                }
            }
            '\\' => {
                chars.by_ref().find(|&(_, c)| c == '\n');
                push(' ', at);
                if let Some((at, c)) = chars.next() {
                    push(c, at);
                }
            }
            _ => push(c, at),
        }
    }
}

/// Texto de um literal de string, com aspas, como o compilador o monta.
pub(super) fn string_value(literal: &str) -> String {
    let mut value = String::new();
    walk_string(literal, |c, _| value.push(c));
    value
}

/// Posição no literal de cada byte do texto que o [`string_value`] monta.
fn value_offsets(literal: &str) -> Vec<usize> {
    let mut offsets = Vec::new();
    walk_string(literal, |c, at| {
        offsets.extend(at..at + c.len_utf8());
    });
    offsets
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
    /// As colunas do SELECT com tabela ou campo que o dicionário de dados não tem.
    pub(super) unknown: Vec<Unknown>,
    /// As colunas que viram campo do cursor, quando o texto é um SELECT com FROM.
    pub(super) selection: Option<Selection>,
}

/// Campos que o SELECT dá ao cursor, como o compilador os monta.
pub(super) struct Selection {
    pub(super) columns: Vec<Selected>,
    pub(super) star: Option<Star>,
}

/// Coluna do SELECT que vira campo do cursor.
pub(super) struct Selected {
    /// Nome do campo em maiúsculas, sem a tabela.
    pub(super) name: String,
    /// Trecho dentro do literal de string, em bytes a partir da aspa que o abre.
    pub(super) range: std::ops::Range<usize>,
    /// O campo aparece no ORDER BY, que com o Senior SQL 2 só aceita o que está no SELECT.
    pub(super) ordered: bool,
}

/// O `*` do SELECT, que dá ao cursor todos os campos da primeira tabela do FROM.
pub(super) struct Star {
    pub(super) range: std::ops::Range<usize>,
    pub(super) table: String,
    /// Os campos da tabela em maiúsculas, ou `None` quando o dicionário não tem a tabela.
    pub(super) fields: Option<Vec<String>>,
}

/// Coluna do SELECT do cursor que o dicionário de dados não confirma.
pub(super) struct Unknown {
    pub(super) kind: CheckKind,
    /// Trecho dentro do literal de string, em bytes a partir da aspa que o abre.
    pub(super) range: std::ops::Range<usize>,
    /// A tabela, ou `TABELA.CAMPO` quando é o campo que falta.
    pub(super) label: Option<String>,
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
    entries: FxHashMap<String, Rc<Analysis>>,
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

/// Resultado das conferências do SQL de um literal de string (com as aspas, como está no texto da
/// regra), calculado na primeira vez e lembrado depois. A chave é o literal, e não o SQL que sai
/// dele, para a consulta ao cache não montar o SQL de novo.
pub(super) fn analysis(literal: &str, dictionaries: &Dictionaries) -> Rc<Analysis> {
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
        cache.entries.get(literal).cloned()
    });
    if let Some(hit) = cached {
        return hit;
    }
    let sql = string_value(literal);
    let statements = Parser::parse_sql(&OracleDialect {}, &sql).ok();
    let reading = compiler_columns(&sql);
    let mut unknown = match &reading {
        Some(reading) => unknown_columns(reading, dictionaries, statements.is_some()),
        None => Vec::new(),
    };
    let mut selection = reading.as_ref().map(|r| selection(r, dictionaries));
    // As posições saem no SQL montado e vão para o literal, que pode ter escape e emenda.
    let offsets = value_offsets(literal);
    let in_literal = |range: &mut std::ops::Range<usize>| {
        let at = |i: usize| offsets.get(i).copied().unwrap_or(literal.len());
        let start = at(range.start);
        let end = match range.end.checked_sub(1) {
            Some(last) if last >= range.start => at(last) + 1,
            _ => start,
        };
        *range = start..end.clamp(start, literal.len());
    };
    unknown.iter_mut().for_each(|u| in_literal(&mut u.range));
    if let Some(selection) = &mut selection {
        selection
            .columns
            .iter_mut()
            .for_each(|c| in_literal(&mut c.range));
        if let Some(star) = &mut selection.star {
            in_literal(&mut star.range);
        }
    }
    let analysis = Rc::new(Analysis {
        constants: statements.as_deref().map(constants_of).unwrap_or_default(),
        columns: statements.and_then(|s| select_columns(&s, dictionaries)),
        unknown,
        selection,
    });
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.bytes + literal.len() > CACHE_LIMIT {
            cache.entries.clear();
            cache.bytes = 0;
        }
        cache.bytes += literal.len();
        cache.entries.insert(literal.to_owned(), analysis.clone());
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

/// Parte do SQL com o trecho que ela ocupa nele. Uma parte em várias linhas perde as quebras, e
/// então o texto fica menor que o trecho.
struct Piece {
    text: String,
    start: usize,
    end: usize,
}

impl Piece {
    /// O texto é o trecho sem tirar nem pôr, e uma posição dentro de um vale para o outro.
    fn exact(&self) -> bool {
        self.end - self.start == self.text.len()
    }
}

/// O SELECT de um cursor separado como o compilador separa.
struct Reading {
    /// A primeira tabela do FROM.
    table: Piece,
    /// O que fica entre as vírgulas do SELECT ao FROM, em maiúsculas fora de aspas.
    columns: Vec<Piece>,
    /// O que vem depois do `ORDER BY`, em maiúsculas, ou vazio.
    order_by: String,
}

/// Tira das pontas o espaço e os caracteres de controle, que é o que o compilador tira.
fn trimmed(bytes: &[u8], mut start: usize, mut end: usize) -> (usize, usize) {
    while start < end && bytes[start] <= b' ' {
        start += 1;
    }
    while start < end && bytes[end - 1] <= b' ' {
        end -= 1;
    }
    (start, end)
}

fn is_blank(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | b'\r' | b' ')
}

/// Primeira tabela do FROM e colunas do SELECT, separadas como o compilador separa ao compilar o
/// `cur.SQL`. Ele não lê o SQL como o banco lê. Passa o texto fora de aspas para maiúsculas, acha
/// `FROM`, `WHERE` e `ORDER BY` pela primeira posição em que aparecem, fica com a tabela que vem
/// depois do `FROM` até a primeira vírgula ou espaço e corta as colunas em toda vírgula entre o
/// `SELECT` e o `FROM`, mesmo dentro de parênteses. `None` quando o texto não começa com `SELECT`
/// e um espaço ou não tem `FROM`, casos em que as colunas não são conferidas.
fn compiler_columns(sql: &str) -> Option<Reading> {
    let bytes = sql.as_bytes();
    let (lead, end) = trimmed(bytes, 0, bytes.len());
    let bytes = &bytes[lead..end];
    let first = &bytes[..memchr::memchr(b' ', bytes)?];
    if !first.eq_ignore_ascii_case(b"SELECT") {
        return None;
    }
    let mut quoted = false;
    let upper: Vec<u8> = bytes
        .iter()
        .map(|&b| {
            quoted ^= matches!(b, b'"' | b'\'');
            if quoted { b } else { b.to_ascii_uppercase() }
        })
        .collect();
    let find = |needle: &[u8]| memchr::memmem::find(&upper, needle);
    let piece = |start: usize, end: usize| Piece {
        text: String::from_utf8_lossy(&upper[start..end]).into_owned(),
        start: lead + start,
        end: lead + end,
    };

    let from = find(b"FROM")?;
    // A tabela começa um caractere depois do FROM e vai até o WHERE, ou o ORDER BY, ou o fim.
    let after = from + 5;
    let limit = find(b"WHERE")
        .or_else(|| find(b"ORDER BY"))
        .unwrap_or(upper.len());
    let table = if after < limit {
        let stop = memchr::memchr(b',', &upper[after..limit]).map_or(limit, |i| after + i);
        let (start, end) = trimmed(&upper, after, stop);
        let end = memchr::memchr(b' ', &upper[start..end]).map_or(end, |i| start + i);
        piece(start, end)
    } else {
        let at = after.min(upper.len());
        piece(at, at)
    };

    // As colunas acabam no INTO que tem espaço dos dois lados, ou no FROM.
    let into = find(b"INTO").filter(|&at| {
        at > 0 && is_blank(upper[at - 1]) && upper.len() > at + 5 && is_blank(upper[at + 4])
    });
    let last = into.unwrap_or(from);
    let mut columns = Vec::new();
    // O texto de cada coluna sem as quebras de linha, com a posição de cada byte dele.
    let (mut text, mut offsets) = (Vec::new(), Vec::new());
    for (at, &byte) in upper
        .iter()
        .enumerate()
        .take(last + 1)
        .skip("SELECT ".len())
    {
        match byte {
            _ if at == last => {}
            b',' => {}
            b'\n' | b'\r' => continue,
            byte => {
                text.push(byte);
                offsets.push(at);
                continue;
            }
        }
        let (start, end) = trimmed(&text, 0, text.len());
        columns.push(Piece {
            text: String::from_utf8_lossy(&text[start..end]).into_owned(),
            start: lead + offsets.get(start).copied().unwrap_or(at),
            end: lead + offsets[start..end].last().map_or(at, |&last| last + 1),
        });
        text.clear();
        offsets.clear();
    }
    let order_by = find(b"ORDER BY").map_or(upper.len(), |at| at + "ORDER BY".len());
    Some(Reading {
        table,
        columns,
        order_by: String::from_utf8_lossy(&upper[order_by..]).into_owned(),
    })
}

/// Campos da tabela ou da visão `name` num dicionário.
fn rowset<'a>(dictionary: &'a SharedDictionary, name: &str) -> Option<&'a [Field]> {
    let dictionary = dictionary.as_deref()?;
    match dictionary.table(name) {
        Some(table) => Some(&table.fields),
        None => dictionary.view(name).map(|view| view.fields.as_slice()),
    }
}

/// O que o dicionário diz de `TABELA.CAMPO`.
enum Lookup {
    Found,
    OnlyInTest,
    NoField,
    NoTable,
}

/// Procura o campo na tabela ou na visão, que é o que o cursor aceita antes do ponto. Vale o
/// dicionário de produção, e o de teste quando produção não tem o campo.
fn lookup(dictionaries: &Dictionaries, table: &str, field: &str) -> Lookup {
    let has = |fields: Option<&[Field]>| {
        fields.is_some_and(|f| f.iter().any(|f| f.name.eq_ignore_ascii_case(field)))
    };
    let (production, test) = (
        rowset(&dictionaries.production, table),
        rowset(&dictionaries.test, table),
    );
    if production.is_none() && test.is_none() {
        Lookup::NoTable
    } else if has(production) {
        Lookup::Found
    } else if !has(test) {
        Lookup::NoField
    } else if dictionaries.production.is_some() {
        Lookup::OnlyInTest
    } else {
        Lookup::Found
    }
}

/// Começo de um número ou de um texto entre aspas.
fn looks_constant(column: &str) -> bool {
    let column = column.trim_start_matches(['+', '-']);
    column.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '\'' | '"' | '.'))
}

/// Colunas do SELECT do cursor com tabela ou campo que não está no dicionário de dados, que o
/// compilador recusa com `RowSet inexistente` e `Campo inexistente na tabela/view`. Cada coluna é
/// conferida como o compilador a lê, pelo [`compiler_columns`]. O que vem antes do primeiro ponto
/// é a tabela, e por isso um apelido de tabela ou uma função com `TABELA.CAMPO` dentro são lidos
/// como uma tabela que não existe. A coluna sem ponto é procurada na primeira tabela do FROM. Com
/// algo depois de um espaço, como um apelido, só vale o que vem antes dele. O `*`, o WHERE e o
/// ORDER BY ficam de fora, porque o compilador só os confere quando a regra usa o Senior SQL 2.
///
/// A coluna com espaço e sem ponto e a que fica com a tabela ou o campo vazio dão outros erros,
/// e as constantes ficam com o [`constants_of`] quando o `sqlparser` leu o SQL (`skip_constants`).
fn unknown_columns(
    reading: &Reading,
    dictionaries: &Dictionaries,
    skip_constants: bool,
) -> Vec<Unknown> {
    let mut unknown = Vec::new();
    if dictionaries.is_empty() {
        return unknown;
    }
    let (main, columns) = (&reading.table, &reading.columns);
    let ctl = |c: char| c <= ' ';
    let mut missing_tables: Vec<String> = Vec::new();
    for item in columns {
        if item.text == "*" {
            continue;
        }
        let (column, range) = match item.text.split_once(' ') {
            Some((left, _)) => {
                let left = left.trim_matches(ctl);
                if !left.contains('.') {
                    continue;
                }
                let end = if item.exact() {
                    item.start + left.len()
                } else {
                    item.end
                };
                (left, item.start..end)
            }
            None => (item.text.as_str(), item.start..item.end),
        };
        if skip_constants && looks_constant(column) {
            continue;
        }
        let qualified = column.contains('.');
        let name = if qualified {
            std::borrow::Cow::Borrowed(column)
        } else {
            std::borrow::Cow::Owned(format!("{}.{column}", main.text))
        };
        let Some((table, field)) = name.split_once('.') else {
            continue;
        };
        let (table, field) = (table.trim_matches(ctl), field.trim_matches(ctl));
        if table.is_empty() || field.is_empty() {
            continue;
        }
        match lookup(dictionaries, table, field) {
            Lookup::Found => {}
            Lookup::OnlyInTest => unknown.push(Unknown {
                kind: CheckKind::OnlyInTest,
                range,
                label: None,
            }),
            Lookup::NoField => unknown.push(Unknown {
                kind: CheckKind::CursorUnknownField,
                range,
                label: Some(format!("{table}.{field}")),
            }),
            // Uma vez por tabela, na primeira coluna que a usa.
            Lookup::NoTable if missing_tables.iter().any(|t| t == table) => {}
            Lookup::NoTable => {
                missing_tables.push(table.to_owned());
                let range = if !qualified {
                    main.start..main.end
                } else if item.exact() {
                    range.start..range.start + column.find('.').unwrap_or(column.len())
                } else {
                    range
                };
                unknown.push(Unknown {
                    kind: CheckKind::CursorUnknownTable,
                    range,
                    label: Some(table.split_whitespace().collect::<Vec<_>>().join(" ")),
                });
            }
        }
    }
    unknown
}

/// Campos que o SELECT dá ao cursor. A coluna sem espaço vira um campo com o nome que vem depois
/// do primeiro ponto, e o `*` vira todos os campos da primeira tabela do FROM. A coluna com algo
/// depois de um espaço, como um apelido, não vira campo, e a constante dá erro, então as duas
/// ficam de fora.
fn selection(reading: &Reading, dictionaries: &Dictionaries) -> Selection {
    let mut selection = Selection {
        columns: Vec::new(),
        star: None,
    };
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let ordered = |name: &str| {
        let text = reading.order_by.as_bytes();
        memchr::memmem::find_iter(text, name.as_bytes()).any(|at| {
            let end = at + name.len();
            (at == 0 || !word(text[at - 1])) && (end == text.len() || !word(text[end]))
        })
    };
    for item in &reading.columns {
        let range = item.start..item.end;
        if item.text == "*" {
            let table = reading.table.text.as_str();
            let fields = rowset(&dictionaries.production, table)
                .or_else(|| rowset(&dictionaries.test, table))
                .map(|fields| fields.iter().map(|f| f.name.to_uppercase()).collect());
            selection.star = Some(Star {
                range,
                table: table.to_owned(),
                fields,
            });
            continue;
        }
        if item.text.contains(' ') || looks_constant(&item.text) {
            continue;
        }
        let name = item
            .text
            .split_once('.')
            .map_or(item.text.as_str(), |n| n.1);
        let name = name.trim_matches(|c: char| c <= ' ');
        if !name.is_empty() {
            selection.columns.push(Selected {
                name: name.to_owned(),
                range,
                ordered: ordered(name),
            });
        }
    }
    selection
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
    Some(
        columns
            .list
            .iter()
            .map(|c| (c.name.clone(), c.ty))
            .collect(),
    )
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
    cursors: FxHashMap<String, Option<ColumnSet>>,
    objects: FxHashMap<String, Option<ColumnSet>>,
}

/// Colunas conhecidas de um cursor ou objeto, na ordem em que apareceram, com a posição de cada
/// nome para a consulta não percorrer a lista.
#[derive(Default)]
struct ColumnSet {
    list: Vec<Column>,
    by_name: FxHashMap<String, usize>,
}

impl ColumnSet {
    /// Acrescenta a coluna. O mesmo nome com tipos diferentes fica sem tipo.
    fn add(&mut self, column: &Column) {
        match self.by_name.get(&column.name) {
            Some(&i) if self.list[i].ty != column.ty => self.list[i].ty = None,
            Some(_) => {}
            None => {
                self.by_name.insert(column.name.clone(), self.list.len());
                self.list.push(column.clone());
            }
        }
    }

    fn get(&self, name: &str) -> Option<&Column> {
        self.by_name.get(name).map(|&i| &self.list[i])
    }
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
                        .map(|lit| analysis(&text[lit], dictionaries));
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
                        .map(|lit| analysis(&text[lit], dictionaries));
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
        columns.get(&field)?.ty
    }

    /// `cur.CAMPO` com um campo que não é coluna do SQL do cursor nem operação. Devolve o cursor
    /// e o campo lido, em maiúsculas, quando o nó é o campo de um cursor e não uma operação dele.
    pub(super) fn check_cursor_field(
        &self,
        tree: &Tree,
        symbols: &Symbols,
        text: &str,
        node: NodeId,
        checks: &mut Vec<Check>,
    ) -> Option<(String, String)> {
        let (cursor, field) = cursor_field(tree, symbols, text, node)?;
        let field = field.to_uppercase();
        if CURSOR_OPERATIONS.contains(&field.as_str()) {
            return None;
        }
        if let Some(Some(columns)) = self.cursors.get(&cursor)
            && columns.get(&field).is_none()
            && let Some(name) = tree.children(node).nth(1)
        {
            checks.push(Check {
                kind: CheckKind::CursorFieldNotInSelect,
                range: tree.text_range(name),
                name: None,
                label: Some(cursor.clone()),
            });
        }
        Some((cursor, field))
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
        if columns.get(&name).is_none() {
            checks.push(Check {
                kind: CheckKind::ReturnedColumnNotInSelect,
                range: literal,
                name: None,
                label: Some(object),
            });
        }
    }
}

/// Campos que a regra lê de cada cursor e os SQL literais de cada um, para avisar no fim as
/// colunas do SELECT que a regra não lê.
#[derive(Default)]
pub(super) struct ColumnUses {
    read: FxHashMap<String, FxHashSet<String>>,
    literals: Vec<(String, text_size::TextRange)>,
}

impl ColumnUses {
    /// Anota a leitura de `cursor.CAMPO`, os dois em maiúsculas.
    pub(super) fn read(&mut self, cursor: String, field: String) {
        self.read.entry(cursor).or_default().insert(field);
    }

    /// Anota o literal de um `cur.SQL "..."`.
    pub(super) fn command(&mut self, tree: &Tree, symbols: &Symbols, text: &str, node: NodeId) {
        let mut children = tree.children(node);
        let Some((cursor, field)) = children
            .next()
            .and_then(|f| cursor_field(tree, symbols, text, f))
        else {
            return;
        };
        if field.eq_ignore_ascii_case("SQL")
            && let Some(literal) = children.next().and_then(|v| string_literal(tree, v))
        {
            self.literals.push((cursor, literal));
        }
    }

    /// Colunas do SELECT que a regra não lê pelo cursor. O campo que aparece no ORDER BY conta
    /// como lido, porque com o Senior SQL 2 o ORDER BY só aceita o que está no SELECT. Também
    /// conta o `cursor.CAMPO` escrito em qualquer lugar do arquivo, mesmo dentro de um texto.
    /// Quando a regra não lê nenhuma coluna de um SELECT, como no cursor que só serve ao `Achou`,
    /// a primeira fica sem aviso, porque o SELECT precisa de pelo menos uma. O `*` só é
    /// conferido com a tabela no dicionário de dados.
    pub(super) fn finish(self, text: &str, dictionaries: &Dictionaries, checks: &mut Vec<Check>) {
        let upper = std::cell::OnceCell::new();
        for (cursor, literal) in &self.literals {
            let analysis = analysis(&text[*literal], dictionaries);
            let Some(selection) = &analysis.selection else {
                continue;
            };
            let read = self.read.get(cursor);
            let is_read = |name: &str| {
                read.is_some_and(|read| read.contains(name))
                    || upper
                        .get_or_init(|| text.to_ascii_uppercase())
                        .contains(&format!("{cursor}.{name}"))
            };
            let at = |range: &std::ops::Range<usize>| {
                let at = |offset: usize| literal.start() + text_size::TextSize::new(offset as u32);
                text_size::TextRange::new(at(range.start), at(range.end))
            };
            if let Some(star) = &selection.star
                && let Some(fields) = &star.fields
            {
                let used: Vec<&str> = fields
                    .iter()
                    .map(String::as_str)
                    .filter(|f| is_read(f))
                    .collect();
                if used.len() < fields.len() {
                    checks.push(Check {
                        kind: CheckKind::CursorStarUnused,
                        range: at(&star.range),
                        name: None,
                        label: Some(format!(
                            "{cursor}|{}|{}|{}",
                            star.table,
                            fields.len(),
                            used.join(", ")
                        )),
                    });
                }
            }
            let mut unused: Vec<&Selected> = selection
                .columns
                .iter()
                .filter(|c| !c.ordered && !is_read(&c.name))
                .collect();
            if selection.star.is_none() && unused.len() == selection.columns.len() {
                unused.drain(..1.min(unused.len()));
            }
            checks.extend(unused.into_iter().map(|column| Check {
                kind: CheckKind::CursorColumnUnused,
                range: at(&column.range),
                name: None,
                label: Some(cursor.clone()),
            }));
        }
    }
}

/// Junta as colunas de mais um SQL às que o cursor ou objeto já tem. Um SQL desconhecido deixa
/// tudo desconhecido, e o mesmo nome com tipos diferentes fica sem tipo.
fn merge(map: &mut FxHashMap<String, Option<ColumnSet>>, key: String, columns: Option<&[Column]>) {
    let entry = map.entry(key).or_insert_with(|| Some(ColumnSet::default()));
    let (Some(known), Some(columns)) = (entry.as_mut(), columns) else {
        *entry = None;
        return;
    };
    for column in columns {
        known.add(column);
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

/// `cur.SQL "SELECT ..."` com uma constante entre as colunas, ou com tabela ou campo que o
/// dicionário de dados não tem.
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
    let analysis = analysis(&text[range], dictionaries);
    for constant in &analysis.constants {
        checks.push(Check {
            kind: CheckKind::CursorConstantColumn,
            range,
            name: None,
            label: Some(constant.clone()),
        });
    }
    let at = |offset: usize| range.start() + text_size::TextSize::new(offset as u32);
    for unknown in &analysis.unknown {
        checks.push(Check {
            kind: unknown.kind,
            range: text_size::TextRange::new(at(unknown.range.start), at(unknown.range.end)),
            name: None,
            label: unknown.label.clone(),
        });
    }
}

/// Objetos de SQL com `SQL_UsarSQLSenior2(x, 0)` e com `SQL_UsarAbrangencia(x, 0)`, para avisar
/// no fim os que ligam o SQL nativo sem desligar a abrangência. Os nomes não diferenciam caixa,
/// e a ordem das chamadas no arquivo não importa.
#[derive(Default)]
pub(super) struct NativeSql {
    native: Vec<(String, text_size::TextRange)>,
    without_scope: FxHashSet<String>,
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

    /// O SQL como literal de string, que é o que o cache recebe.
    fn literal(sql: &str) -> String {
        format!("\"{sql}\"")
    }

    fn constant_columns(sql: &str) -> Vec<String> {
        analysis(&literal(sql), &Dictionaries::default())
            .constants
            .clone()
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
        let sql = literal("SELECT * FROM T");
        assert_eq!(names(analysis(&sql, &one)), ["A"]);
        assert_eq!(names(analysis(&sql, &one)), ["A"]);
        assert_eq!(names(analysis(&sql, &other)), ["B"]);
        assert!(names(analysis(&sql, &Dictionaries::default())).is_empty());
    }

    /// A tabela e as colunas, com o trecho do SQL que cada uma ocupa.
    fn separado(sql: &str) -> Option<(String, Vec<String>)> {
        let Reading { table, columns, .. } = compiler_columns(sql)?;
        let source = |p: &Piece| sql[p.start..p.end].to_owned();
        let texts = columns.iter().map(|p| p.text.clone()).collect();
        assert!(table.text.eq_ignore_ascii_case(&source(&table)), "{sql}");
        for column in columns.iter().filter(|c| c.exact()) {
            assert!(column.text.eq_ignore_ascii_case(&source(column)), "{sql}");
        }
        Some((table.text, texts))
    }

    #[test]
    fn colunas_como_o_compilador_separa() {
        let separado = |sql: &str| {
            let (table, columns) = separado(sql).unwrap_or_default();
            (table, columns.join("|"))
        };
        let esperado = |table: &str, columns: &str| (table.to_owned(), columns.to_owned());
        assert_eq!(
            separado("  select E070EMP.CodEmp, NomEmp from E070EMP, T where X = 1"),
            esperado("E070EMP", "E070EMP.CODEMP|NOMEMP")
        );
        // A vírgula corta mesmo dentro de parênteses e de aspas, e a quebra de linha some.
        assert_eq!(
            separado("SELECT a.x  AS y,\r\n max(b.z, 1), 'p,q' FROM tab a INNER JOIN b ON 1=1"),
            esperado("TAB", "A.X  AS Y|MAX(B.Z|1)|'p|q'")
        );
        assert_eq!(
            separado("SELECT A,\n B\n FROM T\n ORDER BY A"),
            esperado("T", "A|B")
        );
        // Só o INTO com espaço dos dois lados acaba as colunas.
        assert_eq!(separado("SELECT A INTO :X FROM T"), esperado("T", "A"));
        assert_eq!(separado("SELECT PINTO, B FROM T"), esperado("T", "PINTO|B"));
        // Coluna vazia e FROM sem tabela continuam na lista, como o compilador as vê.
        assert_eq!(separado("SELECT A,, FROM"), esperado("", "A||"));
        assert_eq!(separado("SELECT A FROM T WHERE"), esperado("T", "A"));
        assert_eq!(
            separado("SELECT A WHERE X FROM T"),
            esperado("", "A WHERE X")
        );
        // Só é conferido o que começa com SELECT e um espaço e tem FROM.
        for sql in [
            "SELECT\nA FROM T",
            "UPDATE T SET A = 1",
            "SELECT A",
            "",
            "é",
        ] {
            assert!(compiler_columns(sql).is_none(), "{sql}");
        }
    }

    /// Cada pedaço de um SQL com acento, aspas e as palavras fora do lugar passa pela leitura sem
    /// pânico, com os trechos dentro do texto.
    #[test]
    fn sql_cortado_em_qualquer_ponto() {
        let sql = " SELECT é.ç, 'INTO' x, FROMé INTO :á FROM ã, T WHERE ORDER BY SELECT FROM é ";
        let cuts: Vec<usize> = sql.char_indices().map(|(at, _)| at).collect();
        for &start in &cuts {
            for &end in cuts.iter().filter(|&&end| end >= start) {
                let part = &sql[start..end];
                let Some(Reading { table, columns, .. }) = compiler_columns(part) else {
                    continue;
                };
                for piece in columns.iter().chain([&table]) {
                    assert!(
                        piece.start <= piece.end && piece.end <= part.len(),
                        "{part}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_tabela_do_cursor_pode_ser_uma_visao() {
        use crate::tbs::{Dictionary, Table, View};
        let field = |name: &str| Field {
            name: name.to_owned(),
            ..Field::default()
        };
        let dictionaries = Dictionaries {
            production: Some(Arc::new(Dictionary {
                tables: vec![Table {
                    name: "T".to_owned(),
                    fields: vec![field("A")],
                    ..Table::default()
                }],
                views: vec![View {
                    name: "V".to_owned(),
                    fields: vec![field("B")],
                    ..View::default()
                }],
                ..Dictionary::default()
            })),
            test: None,
        };
        let kinds = |sql: &str| -> Vec<(CheckKind, String)> {
            unknown_columns(&compiler_columns(sql).unwrap(), &dictionaries, true)
                .into_iter()
                .map(|u| (u.kind, sql[u.range].to_owned()))
                .collect()
        };
        assert!(kinds("SELECT V.B, T.A, b FROM v, T").is_empty());
        assert_eq!(
            kinds("SELECT V.A, X.A, X.B, 1, 'T.Z' FROM V"),
            [
                (CheckKind::CursorUnknownField, "V.A".to_owned()),
                (CheckKind::CursorUnknownTable, "X".to_owned())
            ]
        );
        // Sem a leitura do `sqlparser`, a constante é conferida como o compilador a lê.
        assert_eq!(
            unknown_columns(
                &compiler_columns("SELECT 1 FROM V").unwrap(),
                &dictionaries,
                false
            )[0]
            .label
            .as_deref(),
            Some("V.1")
        );
    }

    #[test]
    fn posicao_no_literal_de_cada_byte_do_texto() {
        let literal = "\"a\\\"é \\ fora\n  b\"";
        let value = string_value(literal);
        assert_eq!(value, "a\"é    b");
        let offsets = value_offsets(literal);
        assert_eq!(offsets.len(), value.len());
        assert_eq!(&literal[offsets[0]..=offsets[0]], "a");
        assert_eq!(&literal[offsets[1]..=offsets[1]], "\"");
        assert_eq!(&literal[offsets[2]..=offsets[3]], "é");
        assert_eq!(&literal[*offsets.last().unwrap()..], "b\"");
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
