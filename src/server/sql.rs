//! Hover, ir para a definição e completar dentro do texto do SQL, e ir de `cur.COLUNA` para a
//! coluna no SELECT do cursor.
//!
//! O SQL das regras é um literal de texto. Dentro dele o `:nome` é uma variável da regra, e o
//! `TABELA.Campo` e o nome de tabela são do dicionário de dados. Os nomes são procurados no texto
//! como o compilador o lê. O que vem depois de uma barra que não é escape, até o fim da linha, é
//! descartado, e o conteúdo entre apóstrofos é constante e não tem nomes.
//!
//! O apelido de tabela (`FROM e070emp e`, `JOIN e075pro AS p`) vale no mesmo literal e dentro do
//! par de parênteses em que foi declarado, então uma subconsulta enxerga os apelidos de fora e
//! pode ter os seus.

use std::ops::Range;

use lsp_types::{CompletionItem, CompletionItemKind, Hover, Location};
use text_size::{TextRange, TextSize};

use super::navigation::{Context, enclosing_function_at, field_expr_at};
use crate::SyntaxKind;

/// Primeiras palavras de um texto que é SQL.
const STATEMENTS: [&str; 5] = ["select", "update", "insert", "delete", "with"];

/// Palavras que, depois do nome da tabela, não são apelido.
const RESERVED: &[&str] = &[
    "all",
    "and",
    "as",
    "asc",
    "between",
    "by",
    "case",
    "connect",
    "cross",
    "delete",
    "desc",
    "distinct",
    "else",
    "end",
    "except",
    "exists",
    "fetch",
    "for",
    "from",
    "full",
    "group",
    "having",
    "in",
    "inner",
    "insert",
    "intersect",
    "into",
    "is",
    "join",
    "lateral",
    "left",
    "like",
    "limit",
    "minus",
    "model",
    "natural",
    "nowait",
    "nulls",
    "offset",
    "on",
    "only",
    "or",
    "order",
    "outer",
    "over",
    "partition",
    "pivot",
    "returning",
    "right",
    "sample",
    "select",
    "set",
    "start",
    "then",
    "union",
    "unpivot",
    "update",
    "using",
    "values",
    "when",
    "where",
    "window",
    "with",
];

/// Palavras que encerram a lista de tabelas do `FROM`.
const ENDS_FROM: &[&str] = &[
    "connect",
    "except",
    "fetch",
    "for",
    "group",
    "having",
    "intersect",
    "limit",
    "minus",
    "model",
    "offset",
    "order",
    "returning",
    "start",
    "union",
    "where",
];

/// Apelido de tabela declarado no SQL, em posições do texto do literal.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Alias {
    name: Range<usize>,
    table: Range<usize>,
    /// Parênteses em que foi declarado, ou o literal inteiro.
    scope: Range<usize>,
}

/// Tabela citada no `FROM`, no `JOIN`, no `UPDATE` ou no `INSERT INTO`, com o apelido quando há,
/// em posições do texto do literal.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TableRef {
    table: Range<usize>,
    alias: Option<Range<usize>>,
    /// Parênteses em que foi citada, ou o literal inteiro.
    scope: Range<usize>,
}

/// Palavras que dizem o que cabe depois delas, uma tabela ou um campo.
const CONTEXT_WORDS: [&str; 16] = [
    "select", "from", "join", "update", "into", "where", "on", "and", "or", "set", "by", "having",
    "values", "when", "then", "else",
];

/// O que está sob o cursor, em posições do texto do literal.
#[derive(Debug, PartialEq, Eq)]
enum Spot {
    /// `:nome`, com o trecho do nome.
    Variable(Range<usize>),
    /// `TABELA.Campo`, com o trecho de cada parte.
    Column {
        table: Range<usize>,
        field: Range<usize>,
    },
    /// Um nome sozinho, que pode ser uma tabela ou o apelido dela.
    Word(Range<usize>),
}

impl Context<'_> {
    /// Hover do `:nome`, do `TABELA.Campo` ou da tabela dentro de um literal de texto.
    pub(super) fn sql_hover(&self, offset: TextSize) -> Option<Hover> {
        let (literal, mask, spot) = self.spot_at(offset)?;
        let text = &self.doc.text;
        let at = |r: &Range<usize>| shift(literal, r);
        match spot {
            Spot::Variable(name) => {
                let range = at(&name);
                let targets = self.variable_targets(offset, &text[range]);
                if !targets.is_empty() {
                    return Some(self.symbol_hover(range, &targets));
                }
                let def = self.doc.inherited.variable(&text[range])?;
                Some(self.inherited_hover(range, def))
            }
            Spot::Column { table, field } => {
                let alias = resolve_alias(&mask, &table);
                let real = alias.as_ref().map_or(&table, |a| &a.table);
                let (real, table, field) = (at(real), at(&table), at(&field));
                let whole = TextRange::new(table.start(), field.end());
                // Sobre o apelido, o hover é o da tabela dele.
                if alias.is_some() && offset <= table.end() {
                    return self.table_hover(table, &text[real]);
                }
                let dictionaries = &self.doc.inherited.dictionaries;
                match dictionaries.field(&text[real], &text[field]) {
                    Some(found) if alias.is_some() => {
                        // O apelido não diz de que tabela é o campo, então o rótulo traz a real.
                        let label = format!("{}.{}", &text[real], &text[field]);
                        Some(self.field_hover(&found, &label, whole))
                    }
                    Some(found) => Some(self.field_hover(&found, &text[whole], whole)),
                    // Com o cursor na tabela, vale a descrição dela mesmo sem o campo.
                    None if offset <= table.end() => self.table_hover(table, &text[real]),
                    None => None,
                }
            }
            // Um nome solto só vira tabela num texto de SQL. Numa mensagem, qualquer palavra
            // pode coincidir com uma tabela.
            Spot::Word(name) => {
                if !starts_statement(&mask) {
                    return None;
                }
                let range = at(&name);
                match resolve_alias(&mask, &name) {
                    Some(alias) => self.table_hover(range, &text[at(&alias.table)]),
                    None => self.table_hover(range, &text[range]),
                }
            }
        }
    }

    /// Ir para o `definir` do `:nome` dentro de um literal de texto.
    pub(super) fn sql_definition(&self, offset: TextSize) -> Option<Vec<Location>> {
        let (literal, mask, spot) = self.spot_at(offset)?;
        let name = match spot {
            Spot::Variable(name) => name,
            // O apelido vai para onde foi declarado, na lista de tabelas.
            Spot::Column { table, .. } if offset <= shift(literal, &table).end() => {
                return self.alias_location(literal, resolve_alias(&mask, &table)?);
            }
            Spot::Word(word) if starts_statement(&mask) => {
                return self.alias_location(literal, resolve_alias(&mask, &word)?);
            }
            _ => return None,
        };
        let range = shift(literal, &name);
        let name = &self.doc.text[range];
        let symbols = &self.doc.symbols;
        let targets = self.variable_targets(offset, name);
        if !targets.is_empty() {
            return Some(
                targets
                    .into_iter()
                    .map(|t| self.location(symbols.definitions[t].name))
                    .collect(),
            );
        }
        let def = self.doc.inherited.variable(name)?;
        Some(self.def_location(def, def.name).into_iter().collect())
    }

    fn alias_location(&self, literal: TextRange, alias: Alias) -> Option<Vec<Location>> {
        Some(vec![self.location(shift(literal, &alias.name))])
    }

    /// Ir de `cur.COLUNA` para a coluna na lista do SELECT de cada `cur.SQL "..."` do cursor.
    pub(super) fn cursor_column_definition(&self, offset: TextSize) -> Vec<Location> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let Some(node) = field_expr_at(tree, offset) else {
            return Vec::new();
        };
        if !tree
            .children(node)
            .nth(1)
            .is_some_and(|name| tree.text_range(name).contains_inclusive(offset))
        {
            return Vec::new();
        }
        let Some((column, literals)) =
            crate::validate::cursor_sql_literals(tree, &self.doc.symbols, text, node)
        else {
            return Vec::new();
        };
        literals
            .into_iter()
            .filter_map(|literal| {
                let found = select_column(&mask(&text[literal]), column)?;
                Some(self.location(shift(literal, &found)))
            })
            .collect()
    }

    /// Definições do `:nome` usado em `offset`, vistas da `funcao` em volta.
    fn variable_targets(&self, offset: TextSize, name: &str) -> Vec<usize> {
        let tree = &self.doc.tree;
        let function = enclosing_function_at(tree, offset);
        self.doc
            .symbols
            .value_targets(tree, &self.doc.text, name, function)
    }

    /// Hover da tabela `name` do dicionário, mostrado em `range`, que pode ser o apelido dela.
    fn table_hover(&self, range: TextRange, name: &str) -> Option<Hover> {
        let table = self.doc.inherited.dictionaries.table(name)?;
        let mut value = format!("```lsp\nTabela {}\n```\n", table.name);
        if !table.description.is_empty() {
            value.push_str(&table.description);
            value.push_str("\n\n");
        }
        value.push_str("Tabela do dicionário de dados");
        if !table.primary_key.is_empty() {
            let names: Vec<_> = table
                .primary_key
                .iter()
                .filter_map(|&i| table.fields.get(i).map(|f| f.name.as_str()))
                .collect();
            value.push_str(&format!("\n\nChave primária ({})", names.join(", ")));
        }
        Some(self.markup(range, value))
    }

    /// Literal de texto que contém `offset`, sem contar as aspas, com o texto dele mascarado e o
    /// que está sob o cursor.
    /// Tabelas e campos do dicionário de dados que cabem na posição, quando ela está dentro de
    /// um literal de texto. `None` fora de um literal. Depois de `:` saem as variáveis da regra.
    /// O texto que não começa como um SQL e o conteúdo entre apóstrofos não têm sugestão.
    ///
    /// Depois de `TABELA.` ou de `APELIDO.` saem os campos da tabela. Depois de `FROM`, `JOIN`,
    /// `UPDATE` e `INSERT INTO` saem as tabelas e as visões. No resto do comando saem os campos
    /// das tabelas que ele cita, as do parêntese da posição e as de fora dele, junto com os nomes
    /// e os apelidos delas. Enquanto o comando não cita nenhuma, saem as tabelas.
    pub(super) fn sql_completion(&self, offset: TextSize) -> Option<Vec<CompletionItem>> {
        let tokens = self.doc.tree.tokens();
        let (mut i, mut end) = (0, tokens.len());
        while i < end {
            let mid = (i + end) / 2;
            if tokens.range(mid).end() < offset {
                i = mid + 1;
            } else {
                end = mid;
            }
        }
        if i >= tokens.len() || tokens.kind(i) != SyntaxKind::String {
            return None;
        }
        let literal = tokens.range(i);
        let text = &self.doc.text[literal];
        // A aspa que fecha fica de fora, e o texto ainda sem ela vai até o fim.
        let closed = text.len() > 1 && text.ends_with('"');
        if offset <= literal.start() || (closed && offset >= literal.end()) {
            return None;
        }
        let mask = mask(text);
        let at = usize::from(offset - literal.start());
        let bytes = text.as_bytes();
        let quoted = bytes[1..at].iter().filter(|&&b| b == b'\'').count() % 2 == 1;
        if !starts_statement(&mask) || quoted {
            return Some(Vec::new());
        }
        let word_start = |end: usize| {
            end - mask[..end]
                .iter()
                .rev()
                .take_while(|&&b| is_word(b))
                .count()
        };
        let start = word_start(at);
        let prefix = &bytes[start..at];
        if start > 0 && mask[start - 1] == b':' {
            return Some(self.parameter_items(literal.start(), prefix));
        }
        let matches = |name: &str| {
            name.as_bytes()
                .get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        };
        let dictionaries = &self.doc.inherited.dictionaries;
        let mut items = Vec::new();

        if start > 0 && mask[start - 1] == b'.' {
            let qualifier = word_start(start - 1)..start - 1;
            let table = match resolve_alias(&mask, &qualifier) {
                Some(alias) => &text[alias.table],
                None => &text[qualifier],
            };
            if let Some((name, fields)) = rowset(dictionaries, table) {
                items.extend(
                    fields
                        .iter()
                        .filter(|f| matches(&f.name))
                        .map(|f| field_item(name, f)),
                );
            }
            return Some(items);
        }

        let keyword = pieces(&mask[..start])
            .iter()
            .rev()
            .find_map(|piece| match piece {
                Piece::Word(r) => CONTEXT_WORDS
                    .iter()
                    .find(|k| mask[r.clone()].eq_ignore_ascii_case(k.as_bytes()))
                    .copied(),
                _ => None,
            });
        let cited: Vec<TableRef> = from_tables(&mask)
            .into_iter()
            .filter(|t| t.scope.start <= at && at <= t.scope.end)
            .collect();
        let wants_table = matches!(keyword, Some("from" | "join" | "update" | "into"));
        if wants_table || cited.is_empty() {
            let mut seen = std::collections::HashSet::new();
            for dictionary in [&dictionaries.production, &dictionaries.test]
                .into_iter()
                .flatten()
            {
                let tables = dictionary.tables.iter().map(|t| (&t.name, &t.description));
                let views = dictionary.views.iter().map(|v| (&v.name, &v.description));
                for (name, description) in tables.chain(views) {
                    if matches(name) && seen.insert(name.to_uppercase()) {
                        items.push(table_item(name, description.clone(), '2'));
                    }
                }
            }
            return Some(items);
        }
        let mut seen = std::collections::HashSet::new();
        for table in &cited {
            let Some((name, fields)) = rowset(dictionaries, &text[table.table.clone()]) else {
                continue;
            };
            for field in fields {
                if matches(&field.name) && seen.insert(field.name.to_uppercase()) {
                    items.push(field_item(name, field));
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        for table in &cited {
            let name = &text[table.table.clone()];
            if let Some(alias) = &table.alias {
                let alias = &text[alias.clone()];
                if matches(alias) && seen.insert(alias.to_uppercase()) {
                    items.push(table_item(alias, format!("apelido de {name}"), '1'));
                }
            } else if matches(name) && seen.insert(name.to_uppercase()) {
                let detail = self
                    .doc
                    .inherited
                    .dictionaries
                    .table(name)
                    .map(|t| t.description.clone())
                    .unwrap_or_default();
                items.push(table_item(name, detail, '1'));
            }
        }
        Some(items)
    }

    /// Variáveis que cabem depois de `:` num SQL que começa em `literal`. O compilador só aceita
    /// ali a variável que já apareceu na regra, então entram as definidas antes do texto, os
    /// parâmetros da função em que ele está e as variáveis das regras anteriores da tela.
    fn parameter_items(&self, literal: TextSize, prefix: &[u8]) -> Vec<CompletionItem> {
        let text = &self.doc.text;
        let tree = &self.doc.tree;
        let matches = |name: &str| {
            name.as_bytes()
                .get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        };
        let function = enclosing_function_at(tree, literal);
        let mut seen = std::collections::HashSet::new();
        let mut items = Vec::new();
        for def in &self.doc.symbols.definitions {
            let visible = match def.kind {
                crate::symbols::DefKind::Variable(_) => def.name.start() < literal,
                crate::symbols::DefKind::Parameter(_) => {
                    let owner = tree.parent(def.node).and_then(|list| tree.parent(list));
                    owner.is_some() && owner == function
                }
                _ => false,
            };
            let name = &text[def.name];
            if visible && matches(name) && seen.insert(name.to_uppercase()) {
                items.push(CompletionItem {
                    label: name.to_owned(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(self.header(def.node)),
                    sort_text: Some(format!("0{}", name.to_lowercase())),
                    ..CompletionItem::default()
                });
            }
        }
        for def in self.doc.inherited.variables() {
            let name = &def.text[def.name];
            if matches(name) && seen.insert(name.to_uppercase()) {
                items.push(CompletionItem {
                    label: name.to_owned(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(def.decl.clone()),
                    sort_text: Some(format!("1{}", name.to_lowercase())),
                    ..CompletionItem::default()
                });
            }
        }
        items
    }

    fn spot_at(&self, offset: TextSize) -> Option<(TextRange, Vec<u8>, Spot)> {
        let tokens = self.doc.tree.tokens();
        let (mut lo, mut hi) = (0, tokens.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if tokens.range(mid).end() < offset {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo >= tokens.len() || tokens.kind(lo) != SyntaxKind::String {
            return None;
        }
        let literal = tokens.range(lo);
        if offset <= literal.start() || offset >= literal.end() {
            return None;
        }
        let mask = mask(&self.doc.text[literal]);
        let at = usize::from(offset - literal.start());
        let spot = spot_at(&mask, at)?;
        Some((literal, mask, spot))
    }
}

/// Trecho do literal de texto, dado em posições dentro dele, no texto da regra.
fn shift(literal: TextRange, inside: &Range<usize>) -> TextRange {
    let start = literal.start();
    TextRange::new(
        start + TextSize::new(inside.start as u32),
        start + TextSize::new(inside.end as u32),
    )
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Texto do literal com espaços no lugar do que o compilador não lê como SQL, que são os
/// escapes, o que uma barra descarta até o fim da linha e o conteúdo entre apóstrofos. Tem o
/// tamanho do original, para as posições valerem nos dois. A aspa de abertura fica, e a de fechamento
/// encerra a leitura.
fn mask(literal: &str) -> Vec<u8> {
    let bytes = literal.as_bytes();
    let mut out = bytes.to_vec();
    let mut quoted = false;
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => break,
            b'\\' if matches!(bytes.get(i + 1), Some(b'"' | b'\\')) => {
                out[i] = b' ';
                out[i + 1] = b' ';
                i += 2;
            }
            b'\\' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            b'\'' => {
                quoted = !quoted;
                out[i] = b' ';
                i += 1;
            }
            b'\n' => i += 1,
            _ => {
                if quoted {
                    out[i] = b' ';
                }
                i += 1;
            }
        }
    }
    out
}

/// O que está em `at`, ou logo depois do nome que termina ali.
fn spot_at(mask: &[u8], at: usize) -> Option<Spot> {
    let at = at.min(mask.len());
    let mut start = at;
    while start > 0 && is_word(mask[start - 1]) {
        start -= 1;
    }
    let mut end = at;
    while end < mask.len() && is_word(mask[end]) {
        end += 1;
    }
    if start == end || mask[start].is_ascii_digit() {
        return None;
    }
    if start > 0 && mask[start - 1] == b':' {
        return Some(Spot::Variable(start..end));
    }
    if start > 0 && mask[start - 1] == b'.' {
        let mut table = start - 1;
        while table > 0 && is_word(mask[table - 1]) {
            table -= 1;
        }
        if table < start - 1 && !mask[table].is_ascii_digit() {
            return Some(Spot::Column {
                table: table..start - 1,
                field: start..end,
            });
        }
    }
    if mask.get(end) == Some(&b'.') {
        let mut field = end + 1;
        while field < mask.len() && is_word(mask[field]) {
            field += 1;
        }
        if field > end + 1 && !mask[end + 1].is_ascii_digit() {
            return Some(Spot::Column {
                table: start..end,
                field: end + 1..field,
            });
        }
    }
    Some(Spot::Word(start..end))
}

/// Peça do SQL para ler o `FROM`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Word(Range<usize>),
    Open(usize),
    Close(usize),
    Comma,
    Dot,
    Other,
}

fn pieces(mask: &[u8]) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < mask.len() {
        let b = mask[i];
        if is_word(b) {
            let end = i + mask[i..].iter().take_while(|&&b| is_word(b)).count();
            out.push(Piece::Word(i..end));
            i = end;
            continue;
        }
        match b {
            b'(' => out.push(Piece::Open(i)),
            b')' => out.push(Piece::Close(i)),
            b',' => out.push(Piece::Comma),
            b'.' => out.push(Piece::Dot),
            b if b.is_ascii_whitespace() => {}
            _ => out.push(Piece::Other),
        }
        i += 1;
    }
    out
}

fn is_reserved(word: &[u8]) -> bool {
    RESERVED
        .iter()
        .any(|r| word.eq_ignore_ascii_case(r.as_bytes()))
}

/// Apelidos de tabela declarados no texto, tirados das tabelas do [`from_tables`].
fn aliases(mask: &[u8]) -> Vec<Alias> {
    from_tables(mask)
        .into_iter()
        .filter_map(|t| {
            Some(Alias {
                name: t.alias?,
                table: t.table,
                scope: t.scope,
            })
        })
        .collect()
}

/// Tabelas citadas no texto, com o apelido de cada uma. A tabela vem depois do `FROM`, da vírgula
/// da lista do `FROM`, do `JOIN`, do `UPDATE` e do `INSERT INTO`, e o apelido depois dela, com ou
/// sem `AS`. O que vem depois de `(` é uma subconsulta, que cita as tabelas dela mesma, e o
/// apelido dela não é de tabela.
fn from_tables(mask: &[u8]) -> Vec<TableRef> {
    let pieces = pieces(mask);
    // Cada par de parênteses é um escopo. O que não fecha vai até o fim do texto.
    let mut groups = Vec::new();
    let mut open = Vec::new();
    for piece in &pieces {
        match piece {
            Piece::Open(at) => open.push(*at),
            Piece::Close(at) => {
                if let Some(start) = open.pop() {
                    groups.push(start..at + 1);
                }
            }
            _ => {}
        }
    }
    groups.extend(open.into_iter().map(|start| start..mask.len()));
    let scope_of = |at: usize| {
        groups
            .iter()
            .filter(|g| g.contains(&at))
            .min_by_key(|g| g.len())
            .cloned()
            .unwrap_or(0..mask.len())
    };
    let word = |piece: Option<&Piece>| match piece {
        Some(Piece::Word(r)) if !mask[r.start].is_ascii_digit() => Some(r.clone()),
        _ => None,
    };
    let is =
        |r: &Range<usize>, keyword: &str| mask[r.clone()].eq_ignore_ascii_case(keyword.as_bytes());

    let mut found = Vec::new();
    // Nível dos parênteses em que o `FROM` está aberto, para a vírgula separar tabelas.
    let mut from_depth: Option<i32> = None;
    let mut depth = 0;
    for (i, piece) in pieces.iter().enumerate() {
        let mut reference = None;
        match piece {
            Piece::Open(_) => depth += 1,
            Piece::Close(_) => {
                depth -= 1;
                if from_depth.is_some_and(|d| depth < d) {
                    from_depth = None;
                }
            }
            Piece::Comma if from_depth == Some(depth) => reference = Some(i + 1),
            Piece::Word(r) => {
                let text = &mask[r.clone()];
                let after = |keyword: &str| {
                    i.checked_sub(1)
                        .and_then(|p| word(pieces.get(p)))
                        .is_some_and(|w| is(&w, keyword))
                };
                let after_for = after("for");
                if text.eq_ignore_ascii_case(b"from") {
                    from_depth = Some(depth);
                    reference = Some(i + 1);
                } else if text.eq_ignore_ascii_case(b"join")
                    || (text.eq_ignore_ascii_case(b"update") && !after_for)
                    || (text.eq_ignore_ascii_case(b"into") && after("insert"))
                {
                    reference = Some(i + 1);
                } else if from_depth == Some(depth)
                    && ENDS_FROM
                        .iter()
                        .any(|k| text.eq_ignore_ascii_case(k.as_bytes()))
                {
                    from_depth = None;
                }
            }
            _ => {}
        }
        let Some(mut j) = reference else {
            continue;
        };
        let Some(mut table) = word(pieces.get(j)).filter(|t| !is_reserved(&mask[t.clone()])) else {
            continue;
        };
        j += 1;
        // `DONO.TABELA`: a tabela é a última parte.
        if pieces.get(j) == Some(&Piece::Dot)
            && let Some(name) = word(pieces.get(j + 1))
        {
            table = name;
            j += 2;
        }
        if let Some(r) = word(pieces.get(j))
            && is(&r, "as")
        {
            j += 1;
        }
        let alias = word(pieces.get(j)).filter(|a| !is_reserved(&mask[a.clone()]));
        found.push(TableRef {
            scope: scope_of(table.start),
            alias,
            table,
        });
    }
    found
}

/// Campos da tabela ou da visão `name`, com o nome como o dicionário o escreve. Vale o dicionário
/// de produção, e o de teste quando produção não tem.
fn rowset<'a>(
    dictionaries: &'a crate::dictionary::Dictionaries,
    name: &str,
) -> Option<(&'a str, &'a [crate::tbs::Field])> {
    [&dictionaries.production, &dictionaries.test]
        .into_iter()
        .flatten()
        .find_map(|d| match d.table(name) {
            Some(table) => Some((table.name.as_str(), table.fields.as_slice())),
            None => d.view(name).map(|v| (v.name.as_str(), v.fields.as_slice())),
        })
}

fn field_item(table: &str, field: &crate::tbs::Field) -> CompletionItem {
    let ty = match field.ty.var_type() {
        Some(crate::symbols::VarType::Alfa) => "alfa ",
        Some(crate::symbols::VarType::Numero) => "numero ",
        Some(crate::symbols::VarType::Data) => "data ",
        None => "",
    };
    CompletionItem {
        label: field.name.clone(),
        kind: Some(CompletionItemKind::FIELD),
        detail: Some(format!("{table} · {ty}{}", field.description)),
        sort_text: Some(format!("0{}", field.name.to_lowercase())),
        ..CompletionItem::default()
    }
}

fn table_item(name: &str, detail: String, rank: char) -> CompletionItem {
    CompletionItem {
        label: name.to_owned(),
        kind: Some(CompletionItemKind::STRUCT),
        detail: Some(detail),
        sort_text: Some(format!("{rank}{}", name.to_lowercase())),
        ..CompletionItem::default()
    }
}

/// A declaração do apelido `word`, que é a mais próxima entre os escopos que contêm o nome. Sem
/// declaração, `word` não é apelido.
fn resolve_alias(mask: &[u8], word: &Range<usize>) -> Option<Alias> {
    let name = &mask[word.clone()];
    aliases(mask)
        .into_iter()
        .filter(|a| {
            mask[a.name.clone()].eq_ignore_ascii_case(name) && a.scope.contains(&word.start)
        })
        .min_by_key(|a| (a.scope.len(), a.name.start.abs_diff(word.start)))
}

/// Diz se o texto começa com `SELECT`, `UPDATE`, `INSERT`, `DELETE` ou `WITH`.
fn starts_statement(mask: &[u8]) -> bool {
    let rest = mask.get(1..).unwrap_or_default();
    let rest = rest.trim_ascii_start();
    let end = rest.iter().position(|&b| !is_word(b)).unwrap_or(rest.len());
    let word = &rest[..end];
    STATEMENTS
        .iter()
        .any(|s| word.eq_ignore_ascii_case(s.as_bytes()))
}

/// Trecho do nome da coluna `column` na lista do primeiro SELECT. O nome de cada item é a última
/// palavra dele, que é o apelido quando há (com ou sem `AS`) e o campo em `TABELA.CAMPO`. Item
/// que termina em `)` ou em `*` não tem nome.
fn select_column(mask: &[u8], column: &str) -> Option<Range<usize>> {
    let mut depth = 0_i32;
    let mut list = false;
    let mut item_start = 0;
    let mut items = Vec::new();
    let mut i = 0;
    while i < mask.len() {
        match mask[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b',' if list && depth == 0 => {
                items.push(item_start..i);
                item_start = i + 1;
            }
            b if is_word(b) && (i == 0 || !is_word(mask[i - 1])) => {
                let end = i + mask[i..].iter().take_while(|&&b| is_word(b)).count();
                let word = &mask[i..end];
                if depth == 0 && !list && word.eq_ignore_ascii_case(b"select") {
                    list = true;
                    item_start = end;
                } else if depth == 0 && list && word.eq_ignore_ascii_case(b"from") {
                    items.push(item_start..i);
                    list = false;
                    break;
                }
                i = end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    if list {
        items.push(item_start..mask.len());
    }
    items.into_iter().find_map(|item| {
        let end = item.start + mask[item.clone()].trim_ascii_end().len();
        let start = mask[item.start..end]
            .iter()
            .rposition(|&b| !is_word(b))
            .map_or(item.start, |p| item.start + p + 1);
        (start < end && mask[start..end].eq_ignore_ascii_case(column.as_bytes()))
            .then_some(start..end)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(literal: &str, marker: char) -> Option<Spot> {
        let at = literal.find(marker).unwrap();
        let literal = literal.replacen(marker, "", 1);
        spot_at(&mask(&literal), at)
    }

    #[test]
    fn mascara_escape_descartado_e_constante() {
        let literal = "\"A 'B C' \\\" \\\\ D \\ resto\n E\"";
        let masked = String::from_utf8(mask(literal)).unwrap();
        assert_eq!(masked.len(), literal.len());
        assert_eq!(masked.replace(' ', ""), "\"AD\nE\"");
    }

    #[test]
    fn o_que_esta_sob_o_cursor() {
        let sql = "\"SELECT * FROM E070EMP WHERE CODEMP = :^nCodEmp AND E070EMP.CodEmp = 1\"";
        assert_eq!(spot(sql, '^'), Some(Spot::Variable(39..46)));
        let sql = "\"SELECT * FROM E070EMP WHERE CODEMP = :nCodEmp AND E070EMP.Cod^Emp = 1\"";
        assert_eq!(
            spot(sql, '^'),
            Some(Spot::Column {
                table: 51..58,
                field: 59..65
            })
        );
        let sql = "\"SELECT * FROM E07^0EMP WHERE X = 1\"";
        assert_eq!(spot(sql, '^'), Some(Spot::Word(15..22)));
        // Número, espaço e nome dentro de apóstrofos não são nomes.
        assert_eq!(spot("\"SELECT 1^23 FROM T\"", '^'), None);
        assert_eq!(spot("\"SELECT 'AB^C' FROM T\"", '^'), None);
        assert_eq!(spot("\"SELECT A ^ FROM T\"", '^'), None);
        assert_eq!(spot("\"SELECT A \\ T^UDO\n FROM T\"", '^'), None);
    }

    #[test]
    fn so_o_texto_de_sql_vira_tabela() {
        assert!(starts_statement(&mask("\"  select a from t\"")));
        assert!(starts_statement(&mask(
            "\"With x as (select 1 from dual) select 1\""
        )));
        assert!(!starts_statement(&mask("\"Selecionar a empresa\"")));
        assert!(!starts_statement(&mask("\"E070EMP\"")));
    }

    #[test]
    fn coluna_na_lista_do_select() {
        let find =
            |sql: &str, column: &str| select_column(&mask(sql), column).map(|r| sql[r].to_owned());
        let sql = "\"SELECT e000anx.numanx, \\\n  e000anx.seqanx AS seq, COUNT(*), \
                   NVL(x, 0) total, a.* \\\n FROM e000anx WHERE numanx = 1\"";
        assert_eq!(find(sql, "NUMANX").as_deref(), Some("numanx"));
        assert_eq!(find(sql, "Seq").as_deref(), Some("seq"));
        assert_eq!(find(sql, "TOTAL").as_deref(), Some("total"));
        // O campo de origem do apelido, a expressão e o que está fora da lista não valem.
        assert_eq!(find(sql, "SEQANX"), None);
        assert_eq!(find(sql, "X"), None);
        assert_eq!(find(sql, "NUMANX2"), None);
        // O WHERE não entra, nem a subconsulta no meio da lista.
        let sql = "\"SELECT (SELECT MAX(z) FROM t) AS m, k FROM u WHERE k = 1\"";
        assert_eq!(find(sql, "M").as_deref(), Some("m"));
        assert_eq!(find(sql, "Z"), None);
        assert_eq!(find(sql, "K").as_deref(), Some("k"));
    }

    #[test]
    fn coluna_do_select_com_tabela_e_antes_do_from() {
        let sql = "\"SELECT DISTINCT E070EMP.NomEmp FROM E070EMP\"";
        let found = select_column(&mask(sql), "nomemp").unwrap();
        assert_eq!(&sql[found], "NomEmp");
        assert_eq!(select_column(&mask("\"SELECT * FROM T\""), "A"), None);
    }

    /// Documento de teste com o dicionário `E070EMP` (`CodEmp` e `NomEmp`, chave em `CodEmp`).
    fn document(text: &str) -> crate::server::documents::Document {
        use crate::tbs::{Dictionary, Field, FieldKind, FieldType, Table};
        let field = |name: &str, description: &str, kind| Field {
            name: name.to_owned(),
            description: description.to_owned(),
            ty: FieldType {
                kind,
                size: 4,
                ..FieldType::default()
            },
            ..Field::default()
        };
        let table = Table {
            name: "E070EMP".to_owned(),
            description: "Empresas".to_owned(),
            fields: vec![
                field("CodEmp", "Código da empresa", FieldKind::Number),
                field("NomEmp", "Nome da empresa", FieldKind::Text),
            ],
            primary_key: vec![0],
            ..Table::default()
        };
        let mut inherited = crate::form::Inherited::default();
        inherited.dictionaries = crate::dictionary::Dictionaries {
            production: Some(std::sync::Arc::new(Dictionary {
                tables: vec![table],
                ..Dictionary::default()
            })),
            test: None,
        };
        let tree = crate::parse(text).unwrap();
        let symbols = crate::symbols::Symbols::build(&tree, text);
        let index = crate::line_index::LineIndex::new(text);
        crate::server::documents::Document::new(
            text.to_owned(),
            1,
            tree,
            symbols,
            index,
            crate::builtins::Context::Rule,
            std::sync::Arc::new(inherited),
        )
    }

    /// Nomes que o completar oferece no ponto marcado com `^`, em ordem.
    fn completions(source: &str) -> Vec<String> {
        let at = source.find('^').unwrap();
        let text = source.replacen('^', "", 1);
        let before = &text[..at];
        let line = before.matches('\n').count() as u32;
        let character = (before.len() - before.rfind('\n').map_or(0, |p| p + 1)) as u32;
        let doc = document(&text);
        let uri: lsp_types::Uri = "file:///regras/EXEMPLO.LSP/Rule.txt".parse().unwrap();
        let context = Context {
            doc: &doc,
            uri: &uri,
            encoding: crate::line_index::ColumnEncoding::Utf8,
            embedding: None,
        };
        let mut items = context.completion(lsp_types::Position::new(line, character));
        items.sort_by(|a, b| a.sort_text.cmp(&b.sort_text));
        items.into_iter().map(|i| i.label).collect()
    }

    #[test]
    fn sugere_o_nome_mais_parecido() {
        let doc = document(
            "definir cursor c; definir cursor c2; definir cursor c3; definir numero n;\n\
             c.SQL \"SELECT E070EMP.NomeEmp, CodEnp FROM E070EMP\";\n\
             c2.SQL \"SELECT CodEmp, NomEmp FROM E070EMP\";\nn = c2.CodEmpp;\n\
             c3.SQL \"SELECT CodEmp FROM E070EMPP\";\nIntParaAlfaa(n, a);\n\
             c3.SQL \"SELECT Descricao FROM E070EMP\";",
        );
        let found: Vec<(&str, String)> = doc
            .checks()
            .iter()
            .filter_map(|c| crate::server::suggest::suggestion(c, &doc))
            .map(|(range, name)| (&doc.text[range], name))
            .collect();
        // No campo com a tabela na frente só o campo é trocado, e o que não parece com nada
        // fica sem sugestão.
        assert_eq!(
            found,
            [
                ("NomeEmp", "NomEmp".to_owned()),
                ("CodEnp", "CodEmp".to_owned()),
                ("CodEmpp", "CODEMP".to_owned()),
                ("E070EMPP", "E070EMP".to_owned()),
                ("IntParaAlfaa", "IntParaAlfa".to_owned())
            ]
        );
    }

    #[test]
    fn completar_dentro_do_sql() {
        let cursor = |sql: &str| completions(&format!("definir cursor c;\nc.SQL \"{sql}\";"));
        // Depois do ponto, os campos da tabela ou da tabela do apelido.
        assert_eq!(
            cursor("SELECT e070emp.^ FROM E070EMP"),
            ["CodEmp", "NomEmp"]
        );
        assert_eq!(cursor("SELECT E.Nom^ FROM E070EMP E"), ["NomEmp"]);
        assert!(cursor("SELECT X.^ FROM E070EMP").is_empty());
        // Depois do FROM, do JOIN, do UPDATE e do INSERT INTO, as tabelas.
        assert_eq!(cursor("SELECT CodEmp FROM ^"), ["E070EMP"]);
        assert_eq!(cursor("SELECT CodEmp FROM T, e0^"), ["E070EMP"]);
        assert_eq!(cursor("SELECT CodEmp FROM T INNER JOIN ^"), ["E070EMP"]);
        assert!(cursor("SELECT CodEmp FROM X^").is_empty());
        let exec = |sql: &str| completions(&format!("ExecSql \"{sql}\";"));
        assert_eq!(exec("UPDATE ^"), ["E070EMP"]);
        assert_eq!(exec("INSERT INTO E0^"), ["E070EMP"]);
        // No resto do comando, os campos das tabelas citadas e depois o nome delas.
        assert_eq!(
            cursor("SELECT ^ FROM E070EMP"),
            ["CodEmp", "NomEmp", "E070EMP"]
        );
        assert_eq!(cursor("SELECT CodEmp FROM E070EMP WHERE co^"), ["CodEmp"]);
        assert_eq!(
            exec("UPDATE E070EMP SET ^"),
            ["CodEmp", "NomEmp", "E070EMP"]
        );
        assert_eq!(
            cursor("SELECT CodEmp FROM E070EMP E WHERE ^"),
            ["CodEmp", "NomEmp", "E"]
        );
        // A subconsulta enxerga as tabelas de fora.
        assert_eq!(
            cursor("SELECT CodEmp FROM E070EMP WHERE EXISTS (SELECT 1 FROM T WHERE Nom^)"),
            ["NomEmp"]
        );
        // Sem tabela citada ainda, saem as tabelas.
        assert_eq!(cursor("SELECT ^"), ["E070EMP"]);
        // O texto sem a aspa que fecha também vale.
        assert_eq!(
            completions("definir cursor c;\nc.SQL \"SELECT CodEmp FROM e0^"),
            ["E070EMP"]
        );
        // Depois de `:`, as variáveis que já apareceram, os parâmetros da função e mais nada.
        assert_eq!(
            completions(
                "definir numero nAntes; definir alfa aAntes; definir cursor c;\n\
                 c.SQL \"SELECT CodEmp FROM E070EMP WHERE CodEmp = :^\";\ndefinir numero nDepois;"
            ),
            ["aAntes", "nAntes"]
        );
        assert_eq!(
            completions(
                "definir cursor c;\ndefinir funcao f(numero pCod);\nfuncao f(numero pCod); {\n\
                 c.SQL \"SELECT CodEmp FROM E070EMP WHERE CodEmp = :p^\";\n}"
            ),
            ["pCod"]
        );
        // Constante e texto que não é SQL não têm sugestão.
        assert!(cursor("SELECT CodEmp FROM E070EMP WHERE NomEmp = 'Co^'").is_empty());
        assert!(completions("definir alfa a;\na = \"Cod^\";").is_empty());
        // Fora do texto o completar da regra continua.
        assert_eq!(completions("definir numero nValor;\nnVa^"), ["nValor"]);
    }

    /// Texto do hover no ponto marcado com `^`, ou `None` quando não há hover.
    fn hover_text(source: &str) -> Option<String> {
        let at = source.find('^').unwrap();
        let text = source.replacen('^', "", 1);
        let before = &text[..at];
        let line = before.matches('\n').count() as u32;
        let character = (before.len() - before.rfind('\n').map_or(0, |p| p + 1)) as u32;
        let doc = document(&text);
        let uri: lsp_types::Uri = "file:///regras/EXEMPLO.LSP/Rule.txt".parse().unwrap();
        let context = Context {
            doc: &doc,
            uri: &uri,
            encoding: crate::line_index::ColumnEncoding::Utf8,
            embedding: None,
        };
        let hover = context.hover(lsp_types::Position::new(line, character))?;
        match hover.contents {
            lsp_types::HoverContents::Markup(m) => Some(m.value),
            _ => None,
        }
    }

    #[test]
    fn hover_de_campo_e_tabela_no_sql() {
        let campo = hover_text("cur.SQL \"SELECT E070EMP.Cod^Emp FROM E070EMP\";").unwrap();
        assert!(campo.contains("E070EMP.CodEmp"), "{campo}");
        assert!(campo.contains("Código da empresa"), "{campo}");
        assert!(campo.contains("Faz parte da chave primária"), "{campo}");
        // O mesmo hover vale com o cursor na parte da tabela, e a caixa não importa.
        let campo = hover_text("cur.SQL \"SELECT e070^emp.codemp FROM E070EMP\";").unwrap();
        assert!(campo.contains("e070emp.codemp"), "{campo}");

        let tabela = hover_text("cur.SQL \"SELECT 1 FROM E07^0EMP\";").unwrap();
        assert!(tabela.contains("Tabela E070EMP"), "{tabela}");
        assert!(tabela.contains("Empresas"), "{tabela}");
        assert!(tabela.contains("Chave primária (CodEmp)"), "{tabela}");

        // Com o cursor na tabela, o campo que não existe ainda dá a descrição dela.
        let tabela = hover_text("cur.SQL \"SELECT E07^0EMP.Nada FROM E070EMP\";").unwrap();
        assert!(tabela.contains("Tabela E070EMP"), "{tabela}");
        assert!(hover_text("cur.SQL \"SELECT E070EMP.Na^da FROM E070EMP\";").is_none());
        // Tabela que não existe, nome fora de SQL e constante entre apóstrofos não têm hover.
        assert!(hover_text("cur.SQL \"SELECT 1 FROM E9^99XYZ\";").is_none());
        assert!(hover_text("Mensagem(Retorna, \"E07^0EMP\");").is_none());
        assert!(hover_text("cur.SQL \"SELECT 'E07^0EMP' FROM DUAL\";").is_none());
        // Fora do texto vale o hover de antes.
        assert!(
            hover_text("x = E070EMP.Cod^Emp;")
                .unwrap()
                .contains("Código da empresa")
        );
    }

    /// Apelido sob o `^` e a tabela a que ele leva, ou `None` quando não é apelido.
    fn alias_table(sql: &str) -> Option<String> {
        let at = sql.find('^').unwrap();
        let sql = sql.replacen('^', "", 1);
        let mask = mask(&sql);
        let mut end = at;
        while end < mask.len() && is_word(mask[end]) {
            end += 1;
        }
        let mut start = at;
        while start > 0 && is_word(mask[start - 1]) {
            start -= 1;
        }
        resolve_alias(&mask, &(start..end)).map(|a| sql[a.table].to_owned())
    }

    #[test]
    fn apelido_no_from_e_nos_joins() {
        let sql = "\"SELECT ^e.codemp, p.codpro FROM e070emp e INNER JOIN e075pro p ON p.codemp = e.codemp\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e070emp"));
        let sql = "\"SELECT e.codemp, ^p.codpro FROM e070emp e INNER JOIN e075pro p ON p.codemp = e.codemp\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e075pro"));
        // Com AS, na declaração e em outro ponto do texto.
        let sql = "\"SELECT x.codemp FROM e070emp AS ^x\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e070emp"));
        let sql = "\"SELECT * FROM e070emp AS x WHERE ^X.codemp = 1\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e070emp"));
        // Lista separada por vírgula, com a mesma tabela duas vezes.
        let sql = "\"SELECT a.codpro, ^b.codpro FROM e075pro a, e075pro b\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e075pro"));
        let sql = "\"SELECT a.x FROM e070emp a, e075pro b, e085cli ^c WHERE c.x = 1\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e085cli"));
        // Dono na frente da tabela, UPDATE e DELETE.
        let sql = "\"SELECT ^e.x FROM sapiens.e070emp e\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e070emp"));
        let sql = "\"UPDATE e070emp u SET u.nomemp = 'X' WHERE ^u.codemp = 1\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e070emp"));
        let sql = "\"DELETE FROM e070emp d WHERE ^d.codemp = 1\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e070emp"));
        // Linhas continuadas com barra.
        let sql = "\"SELECT e.codemp \\\n FROM e070emp \\\n e WHERE ^e.codemp = 1\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e070emp"));
    }

    #[test]
    fn palavra_reservada_nao_e_apelido() {
        for rest in [
            "WHERE x = 1",
            "INNER JOIN e075pro p ON p.a = 1",
            "LEFT JOIN e075pro p ON p.a = 1",
            "LEFT OUTER JOIN e075pro p ON p.a = 1",
            "CROSS JOIN e075pro p",
            "GROUP BY x",
            "ORDER BY x",
            "UNION SELECT 1 FROM dual",
            "",
        ] {
            let sql = format!("\"SELECT 1 FROM e070emp {rest}\"");
            let mask = mask(&sql);
            let all = aliases(&mask);
            for alias in &all {
                let name = &sql[alias.name.clone()];
                assert!(
                    name == "p" || name == "x",
                    "apelido falso {name:?} em {sql}"
                );
            }
            assert!(
                all.iter().all(|a| &sql[a.table.clone()] != "e070emp"),
                "{sql}"
            );
        }
        let sql = "\"SELECT 1 FROM e070emp WHERE ^where = 1\"";
        assert_eq!(alias_table(sql), None);
        // Não é apelido o nome que só aparece no texto.
        assert_eq!(alias_table("\"SELECT ^z.a FROM e070emp e\""), None);
        assert_eq!(
            alias_table("\"SELECT 1 FROM e070emp e WHERE ^f.a = 1\""),
            None
        );
    }

    #[test]
    fn apelido_de_subconsulta() {
        // A subconsulta enxerga os apelidos de fora e tem os seus.
        let sql = "\"SELECT 1 FROM e070emp e WHERE EXISTS (SELECT 1 FROM e075pro p WHERE p.codemp = ^e.codemp)\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e070emp"));
        let sql = "\"SELECT 1 FROM e070emp e WHERE EXISTS (SELECT 1 FROM e075pro p WHERE ^p.codemp = e.codemp)\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e075pro"));
        // O apelido de dentro não vale fora da subconsulta.
        let sql = "\"SELECT ^p.x FROM e070emp e WHERE EXISTS (SELECT 1 FROM e075pro p)\"";
        assert_eq!(alias_table(sql), None);
        // O de dentro esconde o de fora de mesmo nome.
        let sql =
            "\"SELECT 1 FROM e070emp e WHERE EXISTS (SELECT 1 FROM e075pro e WHERE ^e.x = 1)\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e075pro"));
        let sql = "\"SELECT ^e.x FROM e070emp e WHERE EXISTS (SELECT 1 FROM e075pro e)\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e070emp"));
        // A tabela derivada não é tabela do dicionário, mas o que há dentro dela tem apelido.
        let sql = "\"SELECT t.x FROM (SELECT ^e.x FROM e070emp e) t\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e070emp"));
        let sql = "\"SELECT ^t.x FROM (SELECT e.x FROM e070emp e) t\"";
        assert_eq!(alias_table(sql), None);
        // A vírgula depois da subconsulta no SELECT não abre outra tabela, e o FROM de uma função
        // também não deixa apelido.
        let sql = "\"SELECT (SELECT 1 FROM e075pro p), ^k.x FROM e070emp e, e085cli k\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e085cli"));
        let sql = "\"SELECT EXTRACT(YEAR FROM e.dat) y, ^e.x FROM e070emp e\"";
        assert_eq!(alias_table(sql).as_deref(), Some("e070emp"));
    }

    #[test]
    fn hover_de_campo_e_tabela_pelo_apelido() {
        let direto = hover_text("cur.SQL \"SELECT E070EMP.Cod^Emp FROM E070EMP\";").unwrap();
        let campo = hover_text("cur.SQL \"SELECT e.Cod^Emp FROM E070EMP e\";").unwrap();
        assert!(campo.contains("Código da empresa"), "{campo}");
        assert!(campo.contains("Faz parte da chave primária"), "{campo}");
        // O rótulo traz a tabela real, e o resto é o hover do nome da tabela.
        assert!(campo.contains("E070EMP.CodEmp"), "{campo}");
        assert_eq!(campo, direto);
        let campo = hover_text("cur.SQL \"SELECT x.Nom^Emp FROM E070EMP AS x\";").unwrap();
        assert!(campo.contains("Nome da empresa"), "{campo}");
        let campo = hover_text(
            "cur.SQL \"SELECT 1 FROM E075PRO p INNER JOIN E070EMP e ON e.CodEmp = p.CodEmp \
             WHERE e.Nom^Emp = 'A'\";",
        )
        .unwrap();
        assert!(campo.contains("Nome da empresa"), "{campo}");
        // Apelido de subconsulta.
        let campo = hover_text(
            "cur.SQL \"SELECT 1 FROM E075PRO p WHERE EXISTS (SELECT 1 FROM E070EMP e WHERE e.Cod^Emp = p.CodEmp)\";",
        )
        .unwrap();
        assert!(campo.contains("Código da empresa"), "{campo}");
        // Campo que não existe na tabela do apelido não tem hover.
        assert!(hover_text("cur.SQL \"SELECT e.Na^da FROM E070EMP e\";").is_none());
        assert!(hover_text("cur.SQL \"SELECT z.Cod^Emp FROM E070EMP e\";").is_none());

        // O cursor no apelido, antes do ponto ou na declaração, mostra a tabela.
        for sql in [
            "cur.SQL \"SELECT ^e.CodEmp FROM E070EMP e\";",
            "cur.SQL \"SELECT e.CodEmp FROM E070EMP ^e\";",
            "cur.SQL \"SELECT 1 FROM E070EMP e WHERE ^e.Nada = 1\";",
        ] {
            let tabela = hover_text(sql).unwrap();
            assert!(tabela.contains("Tabela E070EMP"), "{sql}\n{tabela}");
            assert!(tabela.contains("Empresas"), "{sql}\n{tabela}");
        }
        // Fora de um texto de SQL, o nome solto continua sem hover.
        assert!(hover_text("Mensagem(Retorna, \"E070EMP e ^e\");").is_none());
    }

    #[test]
    fn definicao_do_apelido() {
        let definition = |source: &str| {
            let at = source.find('^').unwrap();
            let text = source.replacen('^', "", 1);
            let before = &text[..at];
            let line = before.matches('\n').count() as u32;
            let character = (before.len() - before.rfind('\n').map_or(0, |p| p + 1)) as u32;
            let doc = document(&text);
            let uri: lsp_types::Uri = "file:///regras/EXEMPLO.LSP/Rule.txt".parse().unwrap();
            let context = Context {
                doc: &doc,
                uri: &uri,
                encoding: crate::line_index::ColumnEncoding::Utf8,
                embedding: None,
            };
            context.definition(lsp_types::Position::new(line, character))
        };
        let sql = "cur.SQL \"SELECT ^e.CodEmp FROM E070EMP e\";";
        let found = definition(sql);
        assert_eq!(found.len(), 1);
        let declared = sql.replacen('^', "", 1).find("EMP e\"").unwrap() as u32 + 4;
        assert_eq!(found[0].range.start.character, declared);
        // No campo, na tabela real e fora de apelido não há para onde ir.
        assert!(definition("cur.SQL \"SELECT e.Cod^Emp FROM E070EMP e\";").is_empty());
        assert!(definition("cur.SQL \"SELECT ^E070EMP.CodEmp FROM E070EMP\";").is_empty());
    }

    #[test]
    fn hover_de_variavel_no_sql() {
        let hover = hover_text(
            "definir numero nCodEmp;\ncur.SQL \"SELECT 1 FROM E070EMP WHERE CODEMP = :ncod^emp\";",
        )
        .unwrap();
        assert!(hover.contains("definir numero nCodEmp;"), "{hover}");
        assert!(hover.contains("Variável definida na linha 1"), "{hover}");
        assert!(
            hover_text("cur.SQL \"SELECT 1 FROM E070EMP WHERE CODEMP = :nCod^Emp\";").is_none()
        );
    }
}
