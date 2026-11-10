//! Conferências do SQL das regras.
//!
//! O SQL do cursor (`cur.SQL "..."`) é lido pelo gerador de SQL da Senior já na compilação, e o
//! `SQL_UsarSQLSenior2`, que liga o SQL nativo, só falha na execução. Os dois casos foram
//! conferidos no ERP.

use std::collections::HashSet;

use sqlparser::ast::{Expr, SelectItem, SetExpr, Statement, UnaryOperator};
use sqlparser::dialect::OracleDialect;
use sqlparser::parser::Parser;

use super::{Check, CheckKind};
use crate::{NodeId, SyntaxKind, Tree};

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
pub(super) fn constant_columns(sql: &str) -> Vec<String> {
    let Ok(statements) = Parser::parse_sql(&OracleDialect {}, sql) else {
        return Vec::new();
    };
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

/// `cur.SQL "SELECT ..."` com uma constante entre as colunas.
pub(super) fn cursor_sql(tree: &Tree, text: &str, node: NodeId, checks: &mut Vec<Check>) {
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
    let Some(literal) = tree
        .token_range(node)
        .find(|&t| tree.tokens().kind(t) == SyntaxKind::String)
    else {
        return;
    };
    let range = tree.tokens().range(literal);
    for constant in constant_columns(&string_value(&text[range])) {
        checks.push(Check {
            kind: CheckKind::CursorConstantColumn,
            range,
            name: None,
            label: Some(constant),
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
