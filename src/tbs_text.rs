//! Dicionário de dados em arquivos de texto, para versionar e comparar.
//!
//! Os arquivos `.txt` têm uma linha por item, com as colunas separadas por tabulação. A primeira
//! coluna identifica o item (`TABELA` ou `TABELA.Campo`, por exemplo), e a comparação entre dois
//! dicionários casa as linhas por ela. O código das triggers, das stored procedures e das views fica
//! num arquivo por item, com uma versão por banco quando o dicionário tem mais de uma.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::tbs::{Dictionary, Field, Link, Table};

/// Arquivos do dicionário, pelo caminho relativo, em ordem. Arquivo que ficaria vazio não entra.
pub fn export(dictionary: &Dictionary) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut add = |path: String, text: String| {
        if !text.is_empty() {
            files.insert(path, text);
        }
    };
    add("tabelas.txt".to_owned(), tables(dictionary));
    add("enumeracoes.txt".to_owned(), enumerations(dictionary));
    add("indices.txt".to_owned(), indexes(dictionary));
    add("relacionamentos.txt".to_owned(), relations(dictionary));
    add("check-constraints.txt".to_owned(), checks(dictionary));
    add("ligacoes.txt".to_owned(), links(dictionary));
    let mut views = String::new();
    for view in &dictionary.views {
        rowset(&mut views, &view.name, &view.description, &view.fields);
        add(
            format!("views/{}.sql", file_name(&view.name)),
            code(&view.sql),
        );
    }
    add("views.txt".to_owned(), views);
    for table in &dictionary.tables {
        for trigger in &table.triggers {
            for (key, text) in &trigger.code {
                let path = format!(
                    "triggers/{}.{}.{}.sql",
                    file_name(&table.name),
                    file_name(&trigger.name),
                    database(key)
                );
                add(path, code(text));
            }
        }
    }
    for procedure in &dictionary.procedures {
        for (key, text) in &procedure.code {
            let path = format!(
                "stored-procedures/{}.{}.sql",
                file_name(&procedure.name),
                database(key)
            );
            add(path, code(text));
        }
    }
    files
}

/// Tabelas e campos, uma linha por tabela (nome e descrição) e uma por campo (`TABELA.Campo`, tipo
/// e descrição).
pub fn tables(dictionary: &Dictionary) -> String {
    let mut out = String::new();
    for table in &dictionary.tables {
        rowset(&mut out, &table.name, &table.description, &table.fields);
    }
    out
}

fn rowset(out: &mut String, name: &str, description: &str, fields: &[Field]) {
    let _ = writeln!(out, "{name}\t{}", clean(description));
    for field in fields {
        let _ = writeln!(
            out,
            "{name}.{}\t{}\t{}",
            field.name,
            field.ty,
            clean(&field.description)
        );
    }
}

/// Uma linha por enumeração (nome e tipo dos valores) e uma por valor (`ENUMERAÇÃO.valor` e a
/// descrição).
fn enumerations(dictionary: &Dictionary) -> String {
    let mut list: Vec<_> = dictionary.enumerations.values().collect();
    list.sort_by_cached_key(|e| e.name.to_ascii_uppercase());
    let mut out = String::new();
    for e in list {
        let _ = writeln!(out, "{}\t{}", e.name, e.base);
        for (value, description) in &e.values {
            let _ = writeln!(out, "{}.{value}\t{}", e.name, clean(description));
        }
    }
    out
}

/// Chave primária, colunas únicas e índices, como `TABELA.NOME`, o tipo e os campos.
fn indexes(dictionary: &Dictionary) -> String {
    let mut out = String::new();
    for table in &dictionary.tables {
        if !table.primary_key.is_empty() {
            let _ = writeln!(
                out,
                "{}.PK\tchave primária\t{}",
                table.name,
                field_names(table, &table.primary_key)
            );
        }
        for key in &table.uniques {
            let _ = writeln!(
                out,
                "{}.{}\tcolunas únicas\t{}",
                table.name,
                key.name,
                field_names(table, &key.fields)
            );
        }
        for key in &table.indexes {
            let kind = if key.unique {
                "índice único"
            } else {
                "índice"
            };
            let _ = writeln!(
                out,
                "{}.{}\t{kind}\t{}",
                table.name,
                key.name,
                field_names(table, &key.fields)
            );
        }
    }
    out
}

/// Nome do relacionamento, a tabela e os campos de origem, a tabela e os campos de destino.
fn relations(dictionary: &Dictionary) -> String {
    let mut list: Vec<_> = dictionary.relations.iter().collect();
    list.sort_by_cached_key(|r| (r.name.to_ascii_uppercase(), r.from_table.clone()));
    let mut out = String::new();
    for r in list {
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}",
            r.name,
            r.from_table,
            r.from_fields.join(", "),
            r.to_table,
            r.to_fields.join(", ")
        );
    }
    out
}

/// `TABELA.NOME` e a condição de cada check constraint.
fn checks(dictionary: &Dictionary) -> String {
    let mut out = String::new();
    for table in &dictionary.tables {
        for check in &table.checks {
            let _ = writeln!(
                out,
                "{}.{}\t{}",
                table.name,
                check.name,
                clean(&check.condition)
            );
        }
    }
    out
}

/// Busca de cada campo de tabela e de view, com `TABELA.Campo`, a tabela ou view da busca, os
/// campos de origem, os campos da busca e o campo mostrado, que fica de fora quando não há.
fn links(dictionary: &Dictionary) -> String {
    let mut out = String::new();
    let rowsets = dictionary
        .tables
        .iter()
        .map(|t| (&t.name, &t.fields))
        .chain(dictionary.views.iter().map(|v| (&v.name, &v.fields)));
    for (name, fields) in rowsets {
        for field in fields {
            let Some(Link {
                from_fields,
                target,
                target_fields,
                result,
            }) = &field.link
            else {
                continue;
            };
            let _ = write!(
                out,
                "{name}.{}\t{target}\t{}\t{}",
                field.name,
                from_fields.join(", "),
                target_fields.join(", "),
            );
            match result {
                Some(result) => {
                    let _ = writeln!(out, "\t{result}");
                }
                None => out.push('\n'),
            }
        }
    }
    out
}

fn field_names(table: &Table, positions: &[usize]) -> String {
    positions
        .iter()
        .filter_map(|&i| table.fields.get(i).map(|f| f.name.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Diferenças de `a` para `b`, agrupadas por arquivo. A linha começa com `-` para o que só existe
/// em `a`, `+` para o que só existe em `b` e `~` para o item que mudou, com o valor de antes e o
/// de depois. O código muda no arquivo inteiro, então sai só o caminho.
pub fn diff(a: &Dictionary, b: &Dictionary) -> String {
    let (a, b) = (export(a), export(b));
    let mut out = String::new();
    let mut code = String::new();
    let paths: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).collect();
    for path in paths {
        let (x, y) = (a.get(path), b.get(path));
        if path.ends_with(".txt") {
            let lines = diff_lines(x.map_or("", String::as_str), y.map_or("", String::as_str));
            if !lines.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                let _ = writeln!(out, "{path}");
                out.push_str(&lines);
            }
            continue;
        }
        let sign = match (x, y) {
            (Some(_), None) => '-',
            (None, Some(_)) => '+',
            (Some(x), Some(y)) if x != y => '~',
            _ => continue,
        };
        let _ = writeln!(code, "{sign} {path}");
    }
    if !code.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("código\n");
        out.push_str(&code);
    }
    out
}

/// Diferenças entre dois textos, com as linhas casadas pela primeira coluna e na ordem de `b`,
/// seguidas do que só existe em `a`.
fn diff_lines(a: &str, b: &str) -> String {
    let index = |text: &str| -> BTreeMap<String, Vec<String>> {
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for line in text.lines() {
            let (key, rest) = line.split_once('\t').unwrap_or((line, ""));
            map.entry(key.to_owned()).or_default().push(rest.to_owned());
        }
        map
    };
    let (old, new) = (index(a), index(b));
    let mut out = String::new();
    let mut seen = std::collections::HashSet::new();
    let order = a
        .lines()
        .chain(b.lines())
        .map(|l| l.split_once('\t').map_or(l, |(k, _)| k));
    for key in order {
        if !seen.insert(key) {
            continue;
        }
        match (old.get(key), new.get(key)) {
            (Some(x), None) => {
                for rest in x {
                    let _ = writeln!(out, "- {key}\t{rest}");
                }
            }
            (None, Some(y)) => {
                for rest in y {
                    let _ = writeln!(out, "+ {key}\t{rest}");
                }
            }
            (Some(x), Some(y)) if x != y => {
                if let ([x], [y]) = (x.as_slice(), y.as_slice()) {
                    let _ = writeln!(out, "~ {key}\t{x} -> {y}");
                } else {
                    for rest in x.iter().filter(|r| !y.contains(r)) {
                        let _ = writeln!(out, "- {key}\t{rest}");
                    }
                    for rest in y.iter().filter(|r| !x.contains(r)) {
                        let _ = writeln!(out, "+ {key}\t{rest}");
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Nome de arquivo com só letras, dígitos e `_`.
fn file_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Banco da versão do código, pela chave do dicionário. `ORACLE;0` vira `oracle`, e uma chave com
/// outro número depois do `;` fica com ele, como `oracle-1`.
fn database(key: &str) -> String {
    let (name, number) = key.split_once(';').unwrap_or((key, "0"));
    let name = file_name(&name.to_ascii_lowercase());
    match number.trim() {
        "" | "0" => name,
        n => format!("{name}-{}", file_name(n)),
    }
}

/// Código com fim de linha `\n` e uma quebra no fim.
fn code(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let text = text.trim_end();
    if text.is_empty() {
        String::new()
    } else {
        format!("{text}\n")
    }
}

/// Tira tabulação e quebra de linha, que separam colunas e linhas no texto.
fn clean(text: &str) -> String {
    text.split(['\t', '\r', '\n'])
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tbs::{Enumeration, FieldKind, FieldType, Key, Procedure, Relation, Trigger, View};

    fn field(name: &str, kind: FieldKind, size: u32, description: &str) -> Field {
        Field {
            name: name.to_owned(),
            ty: FieldType {
                kind,
                size,
                ..FieldType::default()
            },
            description: description.to_owned(),
            ..Field::default()
        }
    }

    fn sample() -> Dictionary {
        let mut situacao = field("SitEmp", FieldKind::Text, 1, "Situação");
        situacao.link = Some(Link {
            from_fields: vec!["SitEmp".to_owned()],
            target: "E070EMP".to_owned(),
            target_fields: vec!["SitEmp".to_owned()],
            result: Some("CodEmp".to_owned()),
        });
        let empresa = Table {
            name: "E070EMP".to_owned(),
            description: "Empresas".to_owned(),
            fields: vec![
                field("CodEmp", FieldKind::Number, 4, "Código da\tempresa"),
                situacao,
            ],
            primary_key: vec![0],
            indexes: vec![Key {
                name: "E070EMPIndice2".to_owned(),
                fields: vec![1],
                unique: false,
            }],
            triggers: vec![Trigger {
                name: "T070EMPD".to_owned(),
                code: vec![
                    (
                        "SENIOR;0".to_owned(),
                        "TRIGGER T070EMPD\r\nBEGIN\r\nEND;".to_owned(),
                    ),
                    ("ORACLE;1".to_owned(), "BEGIN NULL; END;".to_owned()),
                ],
            }],
            ..Table::default()
        };
        let mut enumerations = std::collections::HashMap::new();
        enumerations.insert(
            "LSITEMP".to_owned(),
            Enumeration {
                name: "LSitEmp".to_owned(),
                base: FieldType {
                    kind: FieldKind::Text,
                    size: 1,
                    ..FieldType::default()
                },
                values: vec![("A".to_owned(), "Ativa".to_owned())],
            },
        );
        Dictionary {
            tables: vec![empresa],
            relations: vec![Relation {
                name: "IR_E070EMP".to_owned(),
                from_table: "E070EMP".to_owned(),
                from_fields: vec!["SitEmp".to_owned()],
                to_table: "E070EMP".to_owned(),
                to_fields: vec!["CodEmp".to_owned()],
            }],
            enumerations,
            procedures: vec![Procedure {
                name: "GETNEXT".to_owned(),
                code: vec![("ORACLE;0".to_owned(), "BEGIN NULL; END;\n\n".to_owned())],
            }],
            views: vec![View {
                name: "VW_EMP".to_owned(),
                description: "Visão das empresas".to_owned(),
                fields: vec![field("CodEmp", FieldKind::Number, 4, "Código")],
                sql: "SELECT CodEmp FROM E070EMP".to_owned(),
            }],
        }
    }

    #[test]
    fn exporta_um_arquivo_por_assunto() {
        let files = export(&sample());
        let get = |path: &str| files.get(path).map(String::as_str);
        assert_eq!(
            files.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "enumeracoes.txt",
                "indices.txt",
                "ligacoes.txt",
                "relacionamentos.txt",
                "stored-procedures/GETNEXT.oracle.sql",
                "tabelas.txt",
                "triggers/E070EMP.T070EMPD.oracle-1.sql",
                "triggers/E070EMP.T070EMPD.senior.sql",
                "views.txt",
                "views/VW_EMP.sql",
            ]
        );
        assert_eq!(
            get("tabelas.txt"),
            Some(
                "E070EMP\tEmpresas\n\
                 E070EMP.CodEmp\tnumero 4\tCódigo da empresa\n\
                 E070EMP.SitEmp\talfa 1\tSituação\n"
            )
        );
        assert_eq!(
            get("enumeracoes.txt"),
            Some("LSitEmp\talfa 1\nLSitEmp.A\tAtiva\n")
        );
        assert_eq!(
            get("indices.txt"),
            Some(
                "E070EMP.PK\tchave primária\tCodEmp\n\
                 E070EMP.E070EMPIndice2\tíndice\tSitEmp\n"
            )
        );
        assert_eq!(
            get("relacionamentos.txt"),
            Some("IR_E070EMP\tE070EMP\tSitEmp\tE070EMP\tCodEmp\n")
        );
        assert_eq!(
            get("ligacoes.txt"),
            Some("E070EMP.SitEmp\tE070EMP\tSitEmp\tSitEmp\tCodEmp\n")
        );
        assert_eq!(
            get("triggers/E070EMP.T070EMPD.senior.sql"),
            Some("TRIGGER T070EMPD\nBEGIN\nEND;\n")
        );
        assert_eq!(
            get("stored-procedures/GETNEXT.oracle.sql"),
            Some("BEGIN NULL; END;\n")
        );
        assert_eq!(
            get("views.txt"),
            Some("VW_EMP\tVisão das empresas\nVW_EMP.CodEmp\tnumero 4\tCódigo\n")
        );
    }

    #[test]
    fn compara_por_arquivo() {
        let a = sample();
        let mut b = a.clone();
        let table = &mut b.tables[0];
        table.fields.remove(1);
        table.fields[0].ty.size = 5;
        table.indexes.clear();
        table.triggers[0].code[0].1.push_str(" -- alterado");
        b.tables.push(Table {
            name: "USU_TNOVA".to_owned(),
            description: "Nova".to_owned(),
            ..Table::default()
        });
        b.procedures.clear();
        assert_eq!(
            diff(&a, &b),
            "indices.txt\n\
             - E070EMP.E070EMPIndice2\tíndice\tSitEmp\n\
             \n\
             ligacoes.txt\n\
             - E070EMP.SitEmp\tE070EMP\tSitEmp\tSitEmp\tCodEmp\n\
             \n\
             tabelas.txt\n\
             ~ E070EMP.CodEmp\tnumero 4\tCódigo da empresa -> numero 5\tCódigo da empresa\n\
             - E070EMP.SitEmp\talfa 1\tSituação\n\
             + USU_TNOVA\tNova\n\
             \n\
             código\n\
             - stored-procedures/GETNEXT.oracle.sql\n\
             ~ triggers/E070EMP.T070EMPD.senior.sql\n"
        );
        assert_eq!(diff(&a, &a), "");
    }
}
