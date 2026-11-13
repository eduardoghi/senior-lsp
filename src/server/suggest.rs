//! Sugestão para o diagnóstico de um nome que não existe. Acha o campo, a tabela ou a função mais
//! parecidos com o que foi escrito, para o "Você quis dizer?" da mensagem e para a correção
//! rápida que troca o nome.

use super::documents::Document;
use crate::builtins;
use crate::symbols::DefKind;
use crate::tbs::Field;
use crate::validate::{Check, CheckKind};

/// Distância entre dois nomes, sem diferença de caixa, contando letra trocada, a mais, a menos e
/// duas vizinhas invertidas. `None` quando passa de `limit`.
fn distance(a: &[u8], b: &[u8], limit: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > limit {
        return None;
    }
    let same = |x: u8, y: u8| x.eq_ignore_ascii_case(&y);
    // Três linhas da tabela bastam: a atual, a anterior e a de antes dela, para a inversão.
    let mut before: Vec<usize> = Vec::new();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut current = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(!same(a[i - 1], b[j - 1]));
            let mut best = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
            if i > 1 && j > 1 && same(a[i - 1], b[j - 2]) && same(a[i - 2], b[j - 1]) {
                best = best.min(before[j - 2] + 1);
            }
            current[j] = best;
        }
        before = std::mem::replace(&mut previous, current);
    }
    Some(previous[b.len()]).filter(|&d| d <= limit)
}

/// O candidato mais parecido com `name`, quando há um só a essa distância. Nome de até duas
/// letras não tem sugestão, de três a sete aceita uma diferença, e daí em diante duas. O limite é
/// curto porque os campos do ERP têm seis letras e muitos diferem em duas, como `NUMEND` e
/// `NUMANX`, sem que um seja o outro escrito errado.
pub(super) fn closest<'a>(
    name: &str,
    candidates: impl Iterator<Item = &'a str>,
) -> Option<&'a str> {
    let limit = match name.len() {
        0..=2 => return None,
        3..=7 => 1,
        _ => 2,
    };
    let mut best: Option<(usize, &str)> = None;
    let mut tied = false;
    for candidate in candidates {
        let Some(d) = distance(name.as_bytes(), candidate.as_bytes(), limit) else {
            continue;
        };
        match best {
            Some((least, found)) if d == least => tied |= !found.eq_ignore_ascii_case(candidate),
            Some((least, _)) if d > least => {}
            _ => {
                best = Some((d, candidate));
                tied = false;
            }
        }
    }
    best.filter(|(d, _)| *d > 0 && !tied)
        .map(|(_, found)| found)
}

/// Campos da tabela ou da visão no dicionário de produção, ou no de teste quando produção não a
/// tem.
fn fields<'a>(doc: &'a Document, table: &str) -> Option<&'a [Field]> {
    let dictionaries = &doc.inherited.dictionaries;
    [&dictionaries.production, &dictionaries.test]
        .into_iter()
        .flatten()
        .find_map(|d| match d.table(table) {
            Some(table) => Some(table.fields.as_slice()),
            None => d.view(table).map(|v| v.fields.as_slice()),
        })
}

/// Trecho a trocar e o nome sugerido para o diagnóstico de um nome que não existe. No campo
/// escrito com a tabela na frente, o trecho é só o campo. `None` quando o diagnóstico não é de
/// nome, quando nada parecido existe ou quando o trecho não é um nome simples.
pub(super) fn suggestion(check: &Check, doc: &Document) -> Option<(text_size::TextRange, String)> {
    let text = doc.text.as_str();
    let written = &text[check.range];
    let is_name =
        |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    // O nome escrito depois do último ponto, com o trecho dele.
    let last = || {
        let start = written.rfind('.').map_or(0, |dot| dot + 1);
        let range = text_size::TextRange::new(
            check.range.start() + text_size::TextSize::new(start as u32),
            check.range.end(),
        );
        (&written[start..], range)
    };
    let (name, range, found) = match check.kind {
        CheckKind::CursorUnknownField | CheckKind::Sql2UnknownField => {
            let (table, _) = check.label.as_deref()?.split_once('.')?;
            let (name, range) = last();
            let fields = fields(doc, table)?;
            let found = closest(name, fields.iter().map(|f| f.name.as_str()))?;
            (name, range, found.to_owned())
        }
        CheckKind::CursorUnknownTable | CheckKind::Sql2UnknownTable => {
            let dictionaries = &doc.inherited.dictionaries;
            let names = [&dictionaries.production, &dictionaries.test]
                .into_iter()
                .flatten()
                .flat_map(|d| {
                    let tables = d.tables.iter().map(|t| t.name.as_str());
                    tables.chain(d.views.iter().map(|v| v.name.as_str()))
                });
            (written, check.range, closest(written, names)?.to_owned())
        }
        CheckKind::UnknownFunction => {
            let defined = doc
                .symbols
                .definitions
                .iter()
                .filter(|d| d.kind == DefKind::Function)
                .map(|d| &text[d.name]);
            let inherited = doc.inherited.functions().map(|d| &d.text[d.name]);
            let erp = builtins::all(doc.context).map(|b| b.name);
            let found = closest(written, defined.chain(inherited).chain(erp))?;
            (written, check.range, found.to_owned())
        }
        // O nome solto do WHERE, do SET ou dos valores, procurado nos campos das tabelas que o
        // comando cita nessa posição.
        CheckKind::Sql2UnknownSymbol if !written.contains('.') => {
            let tables = super::sql::cited_tables(text, &doc.tree, check.range.start());
            let names = tables
                .iter()
                .filter_map(|table| fields(doc, table))
                .flat_map(|fields| fields.iter().map(|f| f.name.as_str()));
            (written, check.range, closest(written, names)?.to_owned())
        }
        CheckKind::CursorFieldNotInSelect => {
            let columns = crate::validate::cursor_columns(
                &doc.tree,
                &doc.symbols,
                text,
                &doc.inherited.dictionaries,
                check.label.as_deref()?,
            )?;
            let found = closest(written, columns.iter().map(|(name, _)| name.as_str()))?;
            (written, check.range, found.to_owned())
        }
        _ => return None,
    };
    is_name(name).then_some((range, found))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nome_mais_parecido() {
        let nomes = ["CODEMP", "NOMEMP", "CODFIL", "SIGEMP"];
        let sugere = |name: &str| closest(name, nomes.into_iter());
        // Letra trocada, a mais, a menos e duas invertidas, sem diferença de caixa.
        assert_eq!(sugere("NOMENP"), Some("NOMEMP"));
        assert_eq!(sugere("nomeemp"), Some("NOMEMP"));
        assert_eq!(sugere("NOMEM"), Some("NOMEMP"));
        assert_eq!(sugere("NOMEPM"), Some("NOMEMP"));
        // O nome igual não é sugestão, e o que fica longe demais também não.
        assert_eq!(sugere("codemp"), None);
        assert_eq!(sugere("DESCRICAO"), None);
        assert_eq!(sugere("AB"), None);
        // Dois candidatos à mesma distância deixam sem sugestão.
        assert_eq!(sugere("XODEMP"), Some("CODEMP"));
        assert_eq!(sugere("COMEMP"), None);
        // Nome curto só aceita uma diferença.
        assert_eq!(closest("ABCD", ["ABXY", "ABCE"].into_iter()), Some("ABCE"));
        assert_eq!(closest("ABCD", ["AXYD"].into_iter()), None);
        assert_eq!(closest("NUMEND", ["NUMANX"].into_iter()), None);
        assert_eq!(
            closest(
                "ConverteMascra",
                ["ConverteMascara", "ConverteData"].into_iter()
            ),
            Some("ConverteMascara")
        );
    }
}
