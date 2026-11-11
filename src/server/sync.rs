//! Aplicação das mudanças incrementais de um documento (`TextDocumentSyncKind::Incremental`).

use lsp_types::TextDocumentContentChangeEvent;

use crate::line_index::{ColumnEncoding, LineCol, LineIndex};

/// Aplica as mudanças de um `didChange` ao texto, na ordem em que vieram. Cada range vale para o
/// texto que sobrou da mudança anterior, e uma mudança sem range troca o texto inteiro.
///
/// `index` é o `LineIndex` de `text` antes das mudanças. Coluna além do fim da linha vale o fim
/// dela, e linha além da última vale o fim do texto, como o protocolo pede. A coluna é contada na
/// `encoding` negociada, em UTF-16 por padrão.
pub(super) fn apply(
    mut text: String,
    index: &LineIndex,
    changes: Vec<TextDocumentContentChangeEvent>,
    encoding: ColumnEncoding,
) -> String {
    // Tudo antes da última troca do texto inteiro não faz diferença.
    let from = changes.iter().rposition(|c| c.range.is_none()).unwrap_or(0);
    // O `index` recebido só vale enquanto nenhuma mudança foi aplicada.
    let mut original_valid = from == 0;
    let mut own: Option<LineIndex> = None;
    for change in changes.into_iter().skip(from) {
        let Some(range) = change.range else {
            text = change.text;
            original_valid = false;
            own = None;
            continue;
        };
        let index = if original_valid {
            index
        } else {
            own.get_or_insert_with(|| LineIndex::new(&text))
        };
        let at = |p: lsp_types::Position| {
            let lc = LineCol {
                line: p.line,
                col: p.character,
            };
            usize::from(index.offset(&text, lc, encoding))
        };
        let start = at(range.start);
        let end = at(range.end).max(start);
        text.replace_range(start..end, &change.text);
        original_valid = false;
        own = None;
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use ColumnEncoding::{Utf8, Utf16};
    use lsp_types::{Position, Range};

    fn edit(start: (u32, u32), end: (u32, u32), text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(Range::new(
                Position::new(start.0, start.1),
                Position::new(end.0, end.1),
            )),
            range_length: None,
            text: text.to_owned(),
        }
    }

    fn whole(text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.to_owned(),
        }
    }

    fn run(
        text: &str,
        changes: Vec<TextDocumentContentChangeEvent>,
        encoding: ColumnEncoding,
    ) -> String {
        apply(text.to_owned(), &LineIndex::new(text), changes, encoding)
    }

    #[test]
    fn insere_apaga_e_troca() {
        assert_eq!(
            run("x = 1;", vec![edit((0, 4), (0, 5), "22")], Utf16),
            "x = 22;"
        );
        assert_eq!(
            run("x = 1;", vec![edit((0, 1), (0, 1), "y")], Utf16),
            "xy = 1;"
        );
        assert_eq!(run("x = 1;", vec![edit((0, 0), (0, 4), "")], Utf16), "1;");
    }

    #[test]
    fn quebras_de_linha_crlf_e_cr() {
        let text = "a;\r\nb;\r\nc;";
        // Apagar a quebra da linha 0 junta com a linha 1.
        assert_eq!(
            run(text, vec![edit((0, 2), (1, 0), "")], Utf16),
            "a;b;\r\nc;"
        );
        // Coluna além da linha não passa do \r.
        assert_eq!(
            run(text, vec![edit((1, 99), (1, 99), "!")], Utf16),
            "a;\r\nb;!\r\nc;"
        );
        // O \r sozinho também quebra a linha.
        assert_eq!(run("a\rb", vec![edit((1, 0), (1, 1), "X")], Utf16), "a\rX");
        // Nova linha inserida no meio.
        assert_eq!(
            run(text, vec![edit((1, 2), (1, 2), "\r\nd;")], Utf16),
            "a;\r\nb;\r\nd;\r\nc;"
        );
    }

    #[test]
    fn acento_fora_do_ascii() {
        // `ç` e `ã` ocupam 2 bytes e 1 unidade de UTF-16.
        let text = "x = \"ação\";";
        assert_eq!(
            run(text, vec![edit((0, 6), (0, 7), "c")], Utf16),
            "x = \"acão\";"
        );
        assert_eq!(
            run(text, vec![edit((0, 8), (0, 9), "")], Utf16),
            "x = \"açã\";"
        );
        // Em UTF-8 a coluna conta bytes, e `ç` vai de 6 a 8.
        assert_eq!(
            run(text, vec![edit((0, 6), (0, 8), "c")], Utf8),
            "x = \"acão\";"
        );
    }

    #[test]
    fn emoji_conta_duas_unidades_em_utf16() {
        let text = "s = \"😀x\";";
        // O emoji vai das colunas 5 a 7 em UTF-16 e ocupa 4 bytes em UTF-8.
        assert_eq!(
            run(text, vec![edit((0, 7), (0, 8), "y")], Utf16),
            "s = \"😀y\";"
        );
        assert_eq!(
            run(text, vec![edit((0, 5), (0, 7), "")], Utf16),
            "s = \"x\";"
        );
        assert_eq!(
            run(text, vec![edit((0, 5), (0, 9), "")], Utf8),
            "s = \"x\";"
        );
        assert_eq!(
            run(text, vec![edit((0, 9), (0, 10), "y")], Utf8),
            "s = \"😀y\";"
        );
        // Coluna no meio do par de substitutos cai antes do emoji.
        assert_eq!(
            run(text, vec![edit((0, 6), (0, 6), "!")], Utf16),
            "s = \"!😀x\";"
        );
    }

    #[test]
    fn fim_do_arquivo() {
        let text = "a;\nb;\n";
        // A linha depois da última quebra existe e está vazia.
        assert_eq!(
            run(text, vec![edit((2, 0), (2, 0), "c;")], Utf16),
            "a;\nb;\nc;"
        );
        // Linha além do fim vale o fim do texto.
        assert_eq!(
            run(text, vec![edit((9, 0), (9, 5), "c;")], Utf16),
            "a;\nb;\nc;"
        );
        assert_eq!(run("a;", vec![edit((0, 2), (0, 2), "\n")], Utf16), "a;\n");
        assert_eq!(run("", vec![edit((0, 0), (0, 0), "x")], Utf16), "x");
    }

    #[test]
    fn varias_mudancas_valem_uma_sobre_a_outra() {
        // A segunda mudança usa as linhas do texto que a primeira deixou.
        let text = "a;\nb;\nc;";
        let changes = vec![edit((0, 0), (1, 0), ""), edit((1, 0), (1, 0), "z;\n")];
        assert_eq!(run(text, changes, Utf16), "b;\nz;\nc;");
        let changes = vec![
            edit((0, 1), (0, 1), "é"),
            edit((0, 2), (0, 2), "😀"),
            edit((0, 0), (0, 1), ""),
        ];
        assert_eq!(run("ab", changes, Utf16), "é😀b");
    }

    #[test]
    fn mudanca_sem_range_troca_o_texto_inteiro() {
        let text = "a;\nb;";
        assert_eq!(run(text, vec![whole("novo")], Utf16), "novo");
        // O que veio antes da troca do texto inteiro é descartado, e o resto vale sobre o novo.
        let changes = vec![
            edit((0, 0), (0, 1), "zzz"),
            whole("x;\ny;"),
            edit((1, 0), (1, 1), "w"),
        ];
        assert_eq!(run(text, changes, Utf16), "x;\nw;");
    }

    #[test]
    fn range_invertido_vira_inserção() {
        assert_eq!(run("abc", vec![edit((0, 2), (0, 1), "X")], Utf16), "abXc");
    }
}
