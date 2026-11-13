//! Diretivas em comentário que escondem diagnósticos por código.
//!
//! Funcionam nos comentários `@ ... @` e `/* ... */`, como o `eslint-disable` do ESLint e o
//! `#pragma warning` do C#.
//!
//! ```text
//! @ senior-lsp-disable-next-line SEN071 @
//! @ senior-lsp-disable SEN048, SEN061 @
//! @ senior-lsp-enable SEN048, SEN061 @
//! ```
//!
//! Os erros de sintaxe (SEN001 a SEN028) não se escondem. Uma diretiva que não escondeu nada ou
//! que não se entende vira aviso no próprio comentário (SEN073 e SEN072), e esses dois avisos
//! também não se escondem. A supressão é feita uma vez por arquivo, depois que todos os
//! diagnósticos estão prontos, em [`filter`].

use std::collections::HashMap;

use text_size::{TextRange, TextSize};

use crate::line_index::LineIndex;
use crate::{SyntaxKind, Tree};

pub(crate) const NEXT_LINE: &str = "senior-lsp-disable-next-line";
pub(crate) const DISABLE: &str = "senior-lsp-disable";
pub(crate) const ENABLE: &str = "senior-lsp-enable";

/// Diretiva mal escrita, ou `enable` sem `disable`.
pub(crate) const BAD_DIRECTIVE: &str = "SEN072";
/// Diretiva que não escondeu nenhum diagnóstico do código citado.
pub(crate) const UNUSED_DIRECTIVE: &str = "SEN073";

/// Erros de sintaxe, que nenhuma diretiva nem configuração esconde ou muda.
pub(crate) const SYNTAX: [&str; 17] = [
    "SEN001", "SEN002", "SEN003", "SEN004", "SEN005", "SEN006", "SEN007", "SEN008", "SEN020",
    "SEN021", "SEN022", "SEN023", "SEN024", "SEN025", "SEN026", "SEN027", "SEN028",
];

/// Códigos que uma diretiva pode esconder, os do validador e os do servidor.
pub(crate) const SUPPRESSIBLE: [&str; 30] = [
    "SEN040", "SEN041", "SEN042", "SEN043", "SEN044", "SEN045", "SEN046", "SEN047", "SEN048",
    "SEN049", "SEN050", "SEN051", "SEN052", "SEN053", "SEN054", "SEN055", "SEN060", "SEN061",
    "SEN062", "SEN063", "SEN064", "SEN065", "SEN066", "SEN067", "SEN068", "SEN069", "SEN070",
    "SEN071", "SEN074", "SEN075",
];

/// O código existe e pode ser escondido por uma diretiva. Espera o código em maiúsculas.
pub(crate) fn is_suppressible(code: &str) -> bool {
    SUPPRESSIBLE.contains(&code)
}

/// O código é de erro de sintaxe. Espera o código em maiúsculas.
pub(crate) fn is_syntax(code: &str) -> bool {
    SYNTAX.contains(&code)
}

/// O código existe, escondível ou não. Espera o código em maiúsculas.
pub(crate) fn is_known(code: &str) -> bool {
    is_suppressible(code) || is_syntax(code) || code == BAD_DIRECTIVE || code == UNUSED_DIRECTIVE
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    NextLine,
    Disable,
    Enable,
}

/// Código citado numa diretiva, em maiúsculas, com o lugar dele no comentário.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Code {
    pub(crate) name: String,
    pub(crate) range: TextRange,
}

/// Diretiva bem formada. Os códigos inválidos ficam de fora e viram [`Scan::problems`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Directive {
    pub(crate) kind: Kind,
    /// O comentário inteiro, com os delimitadores.
    pub(crate) comment: TextRange,
    pub(crate) codes: Vec<Code>,
}

/// Algo que a diretiva tem de errado, publicado como SEN072 no trecho do erro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) range: TextRange,
    pub(crate) message: String,
}

#[derive(Debug, Default)]
pub(crate) struct Scan {
    pub(crate) directives: Vec<Directive>,
    pub(crate) problems: Vec<Problem>,
}

/// Lê as diretivas dos comentários do arquivo, na ordem em que aparecem. Comentário sem fechamento
/// é ignorado, porque o lexer já o aponta.
pub(crate) fn scan(text: &str, tree: &Tree) -> Scan {
    let tokens = tree.tokens();
    let mut scan = Scan::default();
    for i in 0..tokens.len() {
        let kind = tokens.kind(i);
        if !matches!(kind, SyntaxKind::AtComment | SyntaxKind::BlockComment) {
            continue;
        }
        let range = tokens.range(i);
        let raw = &text[range];
        let (open, close) = match kind {
            SyntaxKind::AtComment if raw.len() >= 2 && raw.ends_with('@') => (1, 1),
            SyntaxKind::BlockComment if raw.len() >= 4 && raw.ends_with("*/") => (2, 2),
            _ => continue,
        };
        let inner = &raw[open..raw.len() - close];
        let base = range.start() + TextSize::new(open as u32);
        // Quase nenhum comentário é diretiva. A primeira palavra é conferida antes de separar o
        // comentário inteiro em palavras.
        let first = inner.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if !has_prefix(first, "senior-lsp-") {
            continue;
        }
        let mut words = words(inner, base).into_iter();
        let Some((name_range, name)) = words.next() else {
            continue;
        };
        if !has_prefix(name, "senior-lsp-") {
            continue;
        }
        let kind = [
            (NEXT_LINE, Kind::NextLine),
            (DISABLE, Kind::Disable),
            (ENABLE, Kind::Enable),
        ]
        .into_iter()
        .find(|(n, _)| name.eq_ignore_ascii_case(n))
        .map(|(_, k)| k);
        let Some(kind) = kind else {
            scan.problems.push(Problem {
                range: name_range,
                message: format!(
                    "Diretiva {name} desconhecida. Use {NEXT_LINE}, {DISABLE} ou {ENABLE}."
                ),
            });
            continue;
        };
        let mut codes: Vec<Code> = Vec::new();
        let mut written = false;
        for (code_range, item) in words {
            written = true;
            let upper = item.to_ascii_uppercase();
            let problem = if is_suppressible(&upper) {
                if !codes.iter().any(|c| c.name == upper) {
                    codes.push(Code {
                        name: upper,
                        range: code_range,
                    });
                }
                continue;
            } else if is_syntax(&upper) {
                format!("{upper} é erro de sintaxe e não pode ser ignorado.")
            } else if upper == BAD_DIRECTIVE || upper == UNUSED_DIRECTIVE {
                format!("{upper} avisa sobre as diretivas e não pode ser ignorado.")
            } else {
                format!("{item} não é um código de diagnóstico do senior-lsp.")
            };
            scan.problems.push(Problem {
                range: code_range,
                message: problem,
            });
        }
        if !written {
            scan.problems.push(Problem {
                range: name_range,
                message: format!("A diretiva {name} precisa de pelo menos um código, como SEN071."),
            });
        }
        if !codes.is_empty() {
            scan.directives.push(Directive {
                kind,
                comment: range,
                codes,
            });
        }
    }
    scan
}

fn has_prefix(word: &str, prefix: &str) -> bool {
    word.as_bytes()
        .get(..prefix.len())
        .is_some_and(|w| w.eq_ignore_ascii_case(prefix.as_bytes()))
}

/// Palavras separadas por espaço ou vírgula, com o lugar de cada uma. `base` é a posição do
/// começo de `text` no arquivo.
fn words(text: &str, base: TextSize) -> Vec<(TextRange, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain([(text.len(), ' ')]) {
        let separator = c.is_whitespace() || c == ',';
        match (start, separator) {
            (None, false) => start = Some(i),
            (Some(s), true) => {
                let range = TextRange::new(
                    base + TextSize::new(s as u32),
                    base + TextSize::new(i as u32),
                );
                out.push((range, &text[s..i]));
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// Linhas, a partir de zero, que têm algo além de espaço e comentário.
pub(crate) fn code_lines(tree: &Tree, index: &LineIndex) -> Vec<usize> {
    let tokens = tree.tokens();
    let mut lines: Vec<usize> = Vec::new();
    for i in 0..tokens.len() {
        let range = tokens.range(i);
        if tokens.kind(i).is_trivia() || range.is_empty() {
            continue;
        }
        let first = index.line_of(range.start());
        let last = index.line_of(range.end() - TextSize::new(1));
        for line in first..=last {
            if lines.last() != Some(&line) {
                lines.push(line);
            }
        }
    }
    lines
}

/// Primeira linha com código depois de `after`.
pub(crate) fn next_code_line(code_lines: &[usize], after: usize) -> Option<usize> {
    let i = code_lines.partition_point(|&line| line <= after);
    code_lines.get(i).copied()
}

/// Linha onde o comentário termina.
pub(crate) fn end_line(index: &LineIndex, comment: TextRange) -> usize {
    index.line_of(comment.end() - TextSize::new(1))
}

/// Aviso sobre uma diretiva, para o servidor publicar no comentário.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notice {
    pub(crate) range: TextRange,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

#[derive(PartialEq, Eq)]
enum SpanKind {
    /// Diretiva de próxima linha. O campo diz se há uma linha de código depois dela.
    NextLine(bool),
    Disable,
    /// `disable` de um código que já estava desligado.
    Repeated,
}

/// Trecho do arquivo em que um código está escondido, por uma diretiva.
struct Span {
    code: String,
    /// Onde o código aparece na diretiva, para o aviso de diretiva sem efeito.
    at: TextRange,
    from: TextSize,
    to: TextSize,
    kind: SpanKind,
    used: bool,
}

/// Tira de `items` os diagnósticos que as diretivas do arquivo escondem e devolve também os
/// avisos sobre as próprias diretivas. `key` diz o código e onde o diagnóstico começa.
pub(crate) fn filter<'a, T>(
    text: &str,
    index: &LineIndex,
    scan: &Scan,
    code_lines: impl FnOnce() -> &'a [usize],
    items: Vec<T>,
    key: impl Fn(&T) -> (&str, TextSize),
) -> (Vec<T>, Vec<Notice>) {
    if scan.directives.is_empty() && scan.problems.is_empty() {
        return (items, Vec::new());
    }
    let mut notices: Vec<Notice> = scan
        .problems
        .iter()
        .map(|p| Notice {
            range: p.range,
            code: BAD_DIRECTIVE,
            message: p.message.clone(),
        })
        .collect();
    let lines = code_lines();
    let mut spans: Vec<Span> = Vec::new();
    let mut open: HashMap<&str, usize> = HashMap::new();
    for directive in &scan.directives {
        for code in &directive.codes {
            match directive.kind {
                Kind::NextLine => {
                    let target = next_code_line(lines, end_line(index, directive.comment));
                    let (from, to) = match target {
                        Some(line) => (
                            index.line_start(line, text),
                            index.line_start(line + 1, text),
                        ),
                        None => (TextSize::new(0), TextSize::new(0)),
                    };
                    spans.push(Span {
                        code: code.name.clone(),
                        at: code.range,
                        from,
                        to,
                        kind: SpanKind::NextLine(target.is_some()),
                        used: false,
                    });
                }
                Kind::Disable => {
                    let repeated = open.contains_key(code.name.as_str());
                    let (from, to, kind) = if repeated {
                        (TextSize::new(0), TextSize::new(0), SpanKind::Repeated)
                    } else {
                        (
                            directive.comment.end(),
                            TextSize::new(u32::MAX),
                            SpanKind::Disable,
                        )
                    };
                    spans.push(Span {
                        code: code.name.clone(),
                        at: code.range,
                        from,
                        to,
                        kind,
                        used: false,
                    });
                    if !repeated {
                        open.insert(&code.name, spans.len() - 1);
                    }
                }
                Kind::Enable => match open.remove(code.name.as_str()) {
                    Some(i) => spans[i].to = directive.comment.start(),
                    None => notices.push(Notice {
                        range: code.range,
                        code: BAD_DIRECTIVE,
                        message: format!("Não há {DISABLE} de {} antes desta diretiva.", code.name),
                    }),
                },
            }
        }
    }
    let kept = items
        .into_iter()
        .filter(|item| {
            let (code, start) = key(item);
            let mut hidden = false;
            for span in &mut spans {
                if span.code.eq_ignore_ascii_case(code) && span.from <= start && start < span.to {
                    span.used = true;
                    hidden = true;
                }
            }
            !hidden
        })
        .collect();
    for span in spans.iter().filter(|s| !s.used) {
        let code = &span.code;
        let message = match span.kind {
            SpanKind::NextLine(true) => {
                format!("Nenhum diagnóstico {code} começa na próxima linha de código.")
            }
            SpanKind::NextLine(false) => {
                format!("Não há linha de código depois desta diretiva para esconder {code}.")
            }
            SpanKind::Disable => {
                format!("Nenhum diagnóstico {code} foi escondido por esta diretiva.")
            }
            SpanKind::Repeated => {
                format!("{code} já estava desligado por uma diretiva anterior.")
            }
        };
        notices.push(Notice {
            range: span.at,
            code: UNUSED_DIRECTIVE,
            message,
        });
    }
    notices.sort_by_key(|n| n.range.start());
    (kept, notices)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Código e linha inicial de cada diagnóstico fictício, que o `filter` recebe pela chave.
    type Item = (&'static str, u32);

    fn run(text: &str, items: &[Item]) -> (Vec<Item>, Vec<(String, String)>) {
        let tree = crate::parse(text).unwrap();
        let index = LineIndex::new(text);
        let items: Vec<(Item, TextSize)> = items
            .iter()
            .map(|&(code, line)| ((code, line), index.line_start(line as usize, text)))
            .collect();
        let scan = scan(text, &tree);
        let lines = code_lines(&tree, &index);
        let (kept, notices) = filter(
            text,
            &index,
            &scan,
            || &lines,
            items,
            |(item, start)| (item.0, *start),
        );
        (
            kept.into_iter().map(|(item, _)| item).collect(),
            notices
                .into_iter()
                .map(|n| (n.code.to_owned(), text[n.range].to_owned()))
                .collect(),
        )
    }

    #[test]
    fn proxima_linha_pula_vazias_e_comentarios() {
        let text =
            "@ senior-lsp-disable-next-line SEN071 @\n\n@ outro @\n/* mais\n um */\nx = 1;\ny = 2;";
        let (kept, notices) = run(text, &[("SEN071", 5), ("SEN071", 6), ("SEN048", 5)]);
        assert_eq!(kept, [("SEN071", 6), ("SEN048", 5)]);
        assert!(notices.is_empty());
    }

    #[test]
    fn diretivas_seguidas_valem_para_a_mesma_linha() {
        let text = "@ senior-lsp-disable-next-line SEN071 @\n@ senior-lsp-disable-next-line SEN048 @\nx = 1;";
        let (kept, notices) = run(text, &[("SEN071", 2), ("SEN048", 2), ("SEN052", 2)]);
        assert_eq!(kept, [("SEN052", 2)]);
        assert!(notices.is_empty());
    }

    #[test]
    fn trecho_ate_o_enable() {
        let text = "x = 1;\n@ senior-lsp-disable SEN048, sen061 @\ny = 2;\n@ senior-lsp-enable SEN048 @\nz = 3;\nw = 4;";
        let (kept, notices) = run(
            text,
            &[
                ("SEN048", 0),
                ("SEN048", 2),
                ("SEN061", 2),
                ("SEN048", 4),
                ("SEN061", 5),
            ],
        );
        // O SEN048 volta depois do enable, e o SEN061 vale até o fim do arquivo.
        assert_eq!(kept, [("SEN048", 0), ("SEN048", 4)]);
        assert!(notices.is_empty());
    }

    #[test]
    fn sem_enable_vale_ate_o_fim() {
        let text = "@ senior-lsp-disable SEN071 @\nx = 1;\ny = 2;";
        let (kept, notices) = run(text, &[("SEN071", 1), ("SEN071", 2)]);
        assert!(kept.is_empty());
        assert!(notices.is_empty());
    }

    #[test]
    fn enable_sem_disable_vira_aviso() {
        let text = "@ senior-lsp-enable SEN048 @\nx = 1;";
        let (kept, notices) = run(text, &[("SEN048", 1)]);
        assert_eq!(kept, [("SEN048", 1)]);
        assert_eq!(notices, [("SEN072".to_owned(), "SEN048".to_owned())]);
    }

    #[test]
    fn codigo_ausente_inexistente_e_de_sintaxe() {
        let text = "@ senior-lsp-disable @\n@ senior-lsp-disable-next-line SEN999, SEN020, SEN072 @\n@ senior-lsp-foo SEN048 @\nx = 1;";
        let (_, notices) = run(text, &[]);
        let found: Vec<_> = notices
            .iter()
            .map(|(code, at)| (code.as_str(), at.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                ("SEN072", "senior-lsp-disable"),
                ("SEN072", "SEN999"),
                ("SEN072", "SEN020"),
                ("SEN072", "SEN072"),
                ("SEN072", "senior-lsp-foo"),
            ]
        );
    }

    #[test]
    fn codigo_em_minuscula_vale() {
        let text = "@ senior-lsp-disable-next-line sen071 @\nx = 1;";
        let (kept, notices) = run(text, &[("SEN071", 1)]);
        assert!(kept.is_empty());
        assert!(notices.is_empty());
    }

    #[test]
    fn diretiva_sem_efeito_avisa_so_os_codigos_que_nao_esconderam() {
        let text = "@ senior-lsp-disable-next-line SEN071, SEN048 @\nx = 1;";
        let (kept, notices) = run(text, &[("SEN071", 1)]);
        assert!(kept.is_empty());
        assert_eq!(notices, [("SEN073".to_owned(), "SEN048".to_owned())]);
    }

    #[test]
    fn disable_sem_efeito_e_disable_repetido_avisam() {
        let text = "@ senior-lsp-disable SEN048 @\n@ senior-lsp-disable SEN048 @\nx = 1;";
        let (_, notices) = run(text, &[]);
        assert_eq!(
            notices,
            [
                ("SEN073".to_owned(), "SEN048".to_owned()),
                ("SEN073".to_owned(), "SEN048".to_owned()),
            ]
        );
        let (_, notices) = run(text, &[("SEN048", 2)]);
        // O primeiro esconde, e o segundo não tinha o que fazer.
        assert_eq!(notices, [("SEN073".to_owned(), "SEN048".to_owned())]);
    }

    #[test]
    fn proxima_linha_no_fim_do_arquivo_nao_tem_alvo() {
        let text = "x = 1;\n@ senior-lsp-disable-next-line SEN048 @";
        let (_, notices) = run(text, &[]);
        assert_eq!(notices, [("SEN073".to_owned(), "SEN048".to_owned())]);
    }

    #[test]
    fn comentario_de_bloco_em_varias_linhas() {
        let text = "/* senior-lsp-disable-next-line\n   SEN071,\n   SEN048 */\nx = 1;";
        let (kept, notices) = run(text, &[("SEN071", 3), ("SEN048", 3)]);
        assert!(kept.is_empty());
        assert!(notices.is_empty());
        let text = "/* senior-lsp-disable SEN048 */ x = 1;\ny = 2;";
        let (kept, _) = run(text, &[("SEN048", 1)]);
        assert!(kept.is_empty());
    }

    #[test]
    fn comentario_comum_nao_e_diretiva() {
        let text = "@ veja o senior-lsp-disable na documentação @\n/* nada */\n@ senior-lsp-disable-next-line SEN071\nx = 1;";
        let (kept, notices) = run(text, &[("SEN071", 3)]);
        // O terceiro comentário não fecha na linha, então o lexer o aponta e a diretiva não vale.
        assert_eq!(kept, [("SEN071", 3)]);
        assert!(notices.is_empty());
    }

    #[test]
    fn os_codigos_conhecidos_batem_com_a_tabela_do_readme() {
        let readme = include_str!("../../README.md");
        let mut in_table: Vec<String> = readme
            .lines()
            .filter_map(|l| l.strip_prefix("| SEN"))
            .map(|l| format!("SEN{}", &l[..3]))
            .collect();
        in_table.sort();
        let mut known: Vec<String> = SYNTAX
            .iter()
            .chain(&SUPPRESSIBLE)
            .chain(&[BAD_DIRECTIVE, UNUSED_DIRECTIVE])
            .map(|c| (*c).to_owned())
            .collect();
        known.sort();
        assert_eq!(known, in_table);
    }
}
