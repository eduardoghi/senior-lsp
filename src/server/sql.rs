//! Hover e ir para a definição dentro do texto do SQL.
//!
//! O SQL das regras é um literal de texto. Dentro dele o `:nome` é uma variável da regra, e o
//! `TABELA.Campo` e o nome de tabela são do dicionário de dados. Os nomes são procurados no texto
//! como o compilador o lê. O que vem depois de uma barra que não é escape, até o fim da linha, é
//! descartado, e o conteúdo entre apóstrofos é constante e não tem nomes.

use std::ops::Range;

use lsp_types::{Hover, Location};
use text_size::{TextRange, TextSize};

use super::navigation::{Context, enclosing_function_at};
use crate::SyntaxKind;

/// Primeiras palavras de um texto que é SQL.
const STATEMENTS: [&str; 5] = ["select", "update", "insert", "delete", "with"];

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
    /// Um nome sozinho, que pode ser uma tabela.
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
                let (table, field) = (at(&table), at(&field));
                let whole = TextRange::new(table.start(), field.end());
                let dictionaries = &self.doc.inherited.dictionaries;
                match dictionaries.field(&text[table], &text[field]) {
                    Some(found) => Some(self.field_hover(&found, &text[whole], whole)),
                    // Com o cursor na tabela, vale a descrição dela mesmo sem o campo.
                    None if offset <= table.end() => self.table_hover(table),
                    None => None,
                }
            }
            // Um nome solto só vira tabela num texto de SQL. Numa mensagem, qualquer palavra
            // pode coincidir com uma tabela.
            Spot::Word(name) => starts_statement(&mask)
                .then(|| self.table_hover(at(&name)))
                .flatten(),
        }
    }

    /// Ir para o `definir` do `:nome` dentro de um literal de texto.
    pub(super) fn sql_definition(&self, offset: TextSize) -> Option<Vec<Location>> {
        let (literal, _, spot) = self.spot_at(offset)?;
        let Spot::Variable(name) = spot else {
            return None;
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

    /// Definições do `:nome` usado em `offset`, vistas da `funcao` em volta.
    fn variable_targets(&self, offset: TextSize, name: &str) -> Vec<usize> {
        let tree = &self.doc.tree;
        let function = enclosing_function_at(tree, offset);
        self.doc
            .symbols
            .value_targets(tree, &self.doc.text, name, function)
    }

    fn table_hover(&self, range: TextRange) -> Option<Hover> {
        let table = self
            .doc
            .inherited
            .dictionaries
            .table(&self.doc.text[range])?;
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
        crate::server::documents::Document {
            text: text.to_owned(),
            version: 1,
            symbols: crate::symbols::Symbols::build(&tree, text),
            index: crate::line_index::LineIndex::new(text),
            tree,
            context: crate::builtins::Context::Rule,
            inherited: std::sync::Arc::new(inherited),
        }
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
