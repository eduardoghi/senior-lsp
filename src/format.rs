//! Formatador da indentação. Muda só os espaços do começo de cada linha, pelo aninhamento dos
//! comandos, e não junta, quebra nem mexe dentro das linhas.
//!
//! - O conteúdo de um bloco, com chaves ou com `inicio` e `fim`, entra um nível. A chave ou o
//!   `inicio` sozinho na linha fica na coluna do comando, como o fechamento.
//! - O `senao` fica na coluna do seu `se`, e o comando sem bloco na linha de baixo de um `se`,
//!   `senao`, `enquanto` ou `para` entra um nível.
//! - A linha que continua um comando começado numa linha de cima anda junto com ela, mantendo o
//!   alinhamento que a pessoa fez, e nunca fica à esquerda dela.
//! - O comentário sozinho na linha segue o nível do bloco em que está.
//!
//! Não mudam a linha de dentro de um texto que continua na linha de baixo, em que os espaços fazem
//! parte do texto, a de dentro de um comentário `/* */` de várias linhas, as linhas entre
//! `@ senior-lsp-format-off @` e `@ senior-lsp-format-on @` e a regra com erro de sintaxe.

use rustc_hash::FxHashMap;
use text_size::{TextRange, TextSize};

use crate::{Element, NodeId, SyntaxKind, Tree};

/// Como escrever um nível de indentação, como o editor manda no pedido de formatação.
#[derive(Debug, Clone, Copy)]
pub struct Indent {
    pub tab_size: u32,
    pub spaces: bool,
}

impl Indent {
    /// Os espaços do começo da linha para a coluna `columns`.
    fn whitespace(self, columns: u32) -> String {
        let tab = self.tab_size.max(1);
        if self.spaces {
            " ".repeat(columns as usize)
        } else {
            let mut out = "\t".repeat((columns / tab) as usize);
            out.push_str(&" ".repeat((columns % tab) as usize));
            out
        }
    }

    /// Coluna em que termina o recuo `whitespace`, com a tabulação do tamanho escolhido.
    fn columns(self, whitespace: &str) -> u32 {
        let tab = self.tab_size.max(1);
        whitespace.chars().fold(0, |col, c| match c {
            '\t' => (col / tab + 1) * tab,
            _ => col + 1,
        })
    }
}

/// Troca do recuo de uma linha, com o trecho dos espaços de agora e os novos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: TextRange,
    pub new_text: String,
}

/// As trocas de recuo do arquivo, nas linhas em que algum ponto cai em `only` quando ele vem.
/// `None` quando a regra tem erro de sintaxe ou quando o resultado mudaria algo além dos espaços,
/// que é a última conferência antes de devolver.
///
/// Uma linha longa pode passar do limite do compilador (`linha para o compilador é muito grande`)
/// com o recuo maior. Essa linha fica com o recuo de antes, e o cálculo é refeito para as
/// continuações dela andarem junto.
pub fn indentation(
    text: &str,
    tree: &Tree,
    indent: Indent,
    only: Option<TextRange>,
) -> Option<Vec<Edit>> {
    if !tree.errors().is_empty() || !tree.tokens().errors().is_empty() {
        return None;
    }
    let plan = Plan::build(tree);
    let skipped = format_off_lines(text, tree);
    let mut frozen = rustc_hash::FxHashSet::default();
    // Cada volta congela pelo menos uma linha nova, então o laço termina. O limite só protege de
    // um engano no cálculo.
    for _ in 0..64 {
        let edits = plan.edits(text, tree.tokens(), indent, only, &skipped, &frozen);
        match check(text, &edits) {
            Outcome::Same => return Some(edits),
            Outcome::TooLong(lines) if !lines.iter().all(|l| frozen.contains(l)) => {
                frozen.extend(lines);
            }
            _ => return None,
        }
    }
    None
}

impl Plan {
    /// As trocas de recuo, deixando como estão as linhas de `skipped` e as de `frozen`.
    fn edits(
        &self,
        text: &str,
        tokens: &crate::Tokens,
        indent: Indent,
        only: Option<TextRange>,
        skipped: &rustc_hash::FxHashSet<usize>,
        frozen: &rustc_hash::FxHashSet<usize>,
    ) -> Vec<Edit> {
        let mut edits = Vec::new();
        // Deslocamento e coluna nova de cada linha já calculada, pelo começo dela, para as
        // continuações.
        let mut shifts: FxHashMap<TextSize, (i64, u32)> = FxHashMap::default();
        let mut start = 0usize;
        let mut line_index = 0usize;
        while start <= text.len() {
            let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
            let line = &text[start..end];
            let ws_len = line.len() - line.trim_start_matches([' ', '\t']).len();
            let first = start + ws_len;
            let line_start = TextSize::new(start as u32);
            let rest = line[ws_len..].trim_end_matches('\r');
            let wanted = only.is_none_or(|r| {
                let l = TextRange::new(line_start, TextSize::new(end as u32));
                r.intersect(l).is_some() || r.contains_inclusive(line_start)
            });
            if !rest.is_empty() && !skipped.contains(&line_index) {
                let at = TextSize::new(first as u32);
                let old_ws = &line[..ws_len];
                let old = indent.columns(old_ws);
                // A linha que começa no meio de um token, como um texto ou um comentário que vem da
                // linha de cima, fica de fora.
                let source = Source {
                    text,
                    tokens,
                    indent,
                };
                let target = tokens_at(tokens, at)
                    .and_then(|token| self.target(at, token, &source, &shifts, old));
                if let Some(columns) = target {
                    // A linha que não muda, porque está fora do trecho pedido ou passaria do limite,
                    // leva as continuações com o recuo que tem.
                    if !wanted || frozen.contains(&line_index) {
                        shifts.insert(line_start, (0, old));
                    } else {
                        shifts.insert(line_start, (columns as i64 - old as i64, columns));
                        let new_ws = indent.whitespace(columns);
                        if new_ws != old_ws {
                            edits.push(Edit {
                                range: TextRange::new(line_start, TextSize::new(first as u32)),
                                new_text: new_ws,
                            });
                        }
                    }
                }
            }
            if end == text.len() {
                break;
            }
            start = end + 1;
            line_index += 1;
        }
        edits
    }
}

/// As trocas de recuo da linha em que o editor acabou de digitar `typed`, para a formatação ao
/// digitar. Com `}` ou com o `m` que fecha um `fim`, a linha vai para o nível dela. Com a quebra de
/// linha, a linha nova, ainda vazia ou com o que veio depois do cursor, vai para o nível do bloco em
/// que está. Como o cálculo usa a árvore, nada muda enquanto a regra tem erro de sintaxe.
pub fn on_type(
    text: &str,
    tree: &Tree,
    indent: Indent,
    offset: TextSize,
    typed: &str,
) -> Option<Vec<Edit>> {
    let at = usize::from(offset).min(text.len());
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let line = &text[start..end];
    let content = line.trim_start_matches([' ', '\t']).trim_end_matches('\r');
    let closes = |word: &str| {
        content.len() >= word.len()
            && content[..word.len()].eq_ignore_ascii_case(word)
            && !content[word.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    let range = TextRange::new(TextSize::new(start as u32), TextSize::new(end as u32));
    match typed {
        "}" if content.starts_with('}') => indentation(text, tree, indent, Some(range)),
        "m" | "M" if closes("fim") => indentation(text, tree, indent, Some(range)),
        "\n" if !content.is_empty() => indentation(text, tree, indent, Some(range)),
        "\n" => {
            let columns = if tree.errors().is_empty() && tree.tokens().errors().is_empty() {
                let plan = Plan::build(tree);
                let level = plan
                    .blocks
                    .iter()
                    .filter(|(r, _)| r.contains(offset))
                    .max_by_key(|(r, _)| r.start())
                    .map_or(0, |(_, level)| *level);
                level * indent.tab_size.max(1)
            } else {
                // Ao digitar um bloco, a regra fica com erro até o fechamento. O nível vem da
                // linha de cima, com um a mais quando ela abre um bloco.
                previous_line_columns(&text[..start], indent)
            };
            let ws_len = line.len() - line.trim_start_matches([' ', '\t']).len();
            let new_text = indent.whitespace(columns);
            if new_text == line[..ws_len] {
                return Some(Vec::new());
            }
            let ws = TextRange::new(range.start(), range.start() + TextSize::new(ws_len as u32));
            Some(vec![Edit {
                range: ws,
                new_text,
            }])
        }
        _ => None,
    }
}

/// Coluna para a linha nova depois de `before`, pela última linha com algo escrito. Ela soma um nível
/// quando termina em `{` ou em `inicio`, fora um comentário `@ ... @` no fim.
fn previous_line_columns(before: &str, indent: Indent) -> u32 {
    let Some(line) = before.lines().rev().find(|l| !l.trim().is_empty()) else {
        return 0;
    };
    let ws_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let mut code = line.trim_end();
    if code.ends_with('@')
        && let Some(open) = code[..code.len() - 1].rfind('@')
    {
        code = code[..open].trim_end();
    }
    let lower = code.to_ascii_lowercase();
    let opens = lower.ends_with('{')
        || (lower.ends_with("inicio")
            && !lower[..lower.len() - 6]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_'));
    let columns = indent.columns(&line[..ws_len]);
    if opens {
        columns + indent.tab_size.max(1)
    } else {
        columns
    }
}

/// Índice do token que começa em `at`, quando há um.
fn tokens_at(tokens: &crate::Tokens, at: TextSize) -> Option<usize> {
    let (mut lo, mut hi) = (0, tokens.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if tokens.range(mid).start() < at {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    (lo < tokens.len() && tokens.range(lo).start() == at).then_some(lo)
}

/// Linhas entre `@ senior-lsp-format-off @` e `@ senior-lsp-format-on @`, as dos dois
/// comentários inclusive. Sem o `on`, vai até o fim do arquivo.
fn format_off_lines(text: &str, tree: &Tree) -> rustc_hash::FxHashSet<usize> {
    let tokens = tree.tokens();
    let line_of = |offset: TextSize| text[..usize::from(offset)].matches('\n').count();
    let directive = |i: usize| {
        let raw = tokens.text(i, text);
        let inner = match tokens.kind(i) {
            SyntaxKind::AtComment => raw.trim_matches('@'),
            SyntaxKind::BlockComment => raw.trim_start_matches("/*").trim_end_matches("*/"),
            _ => return None,
        };
        let inner = inner.trim();
        if inner.eq_ignore_ascii_case("senior-lsp-format-off") {
            Some(false)
        } else if inner.eq_ignore_ascii_case("senior-lsp-format-on") {
            Some(true)
        } else {
            None
        }
    };
    let mut lines = rustc_hash::FxHashSet::default();
    let mut off: Option<usize> = None;
    for i in 0..tokens.len() {
        match (directive(i), off) {
            (Some(false), None) => off = Some(line_of(tokens.range(i).start())),
            (Some(true), Some(from)) => {
                lines.extend(from..=line_of(tokens.range(i).start()));
                off = None;
            }
            _ => {}
        }
    }
    if let Some(from) = off {
        lines.extend(from..=text.matches('\n').count());
    }
    lines
}

/// O nível de cada ponto que começa um comando, um bloco ou um `senao`, os trechos dos comandos,
/// para achar as continuações, e o conteúdo dos blocos, para os comentários.
#[derive(Default)]
struct Plan {
    /// Nível pelo começo do token.
    anchors: FxHashMap<TextSize, u32>,
    /// Trecho de cada comando, para a continuação achar o comando de que faz parte.
    statements: Vec<TextRange>,
    /// O conteúdo de cada bloco, entre a abertura e o fechamento, com o nível dele.
    blocks: Vec<(TextRange, u32)>,
}

impl Plan {
    fn build(tree: &Tree) -> Plan {
        use SyntaxKind::*;
        let mut plan = Plan::default();
        let mut stack: Vec<(NodeId, u32)> = tree.children(tree.root()).map(|c| (c, 0)).collect();
        while let Some((node, level)) = stack.pop() {
            // O bloco não conta como comando de que uma linha seja continuação. As linhas dele são
            // conteúdo, com o nível que ele dá.
            if tree.kind(node) != Block {
                plan.statements.push(tree.text_range(node));
            }
            let tokens = significant_tokens(tree, node);
            // O primeiro token pode estar num nó filho, como o nome no começo de `x = 1;`.
            let first = tree
                .token_range(node)
                .find(|&t| !tree.tokens().kind(t).is_trivia());
            if let Some(first) = first {
                plan.anchors
                    .entry(tree.tokens().range(first).start())
                    .or_insert(level);
            }
            let children: Vec<NodeId> = tree.children(node).collect();
            // O corpo de um `se`, `senao`, `enquanto` ou `para`. O bloco fica na coluna do comando
            // e o comando sozinho entra um nível.
            let body = |child: NodeId| {
                if tree.kind(child) == Block {
                    (child, level)
                } else {
                    (child, level + 1)
                }
            };
            match tree.kind(node) {
                Block => {
                    if let Some(&close) = tokens
                        .iter()
                        .rev()
                        .find(|&&t| matches!(tree.tokens().kind(t), RBrace | FimKw))
                    {
                        let close_range = tree.tokens().range(close);
                        plan.anchors.insert(close_range.start(), level);
                        if let Some(&open) = tokens.first() {
                            let open_end = tree.tokens().range(open).end();
                            if open_end <= close_range.start() {
                                let inside = TextRange::new(open_end, close_range.start());
                                plan.blocks.push((inside, level + 1));
                            }
                        }
                    }
                    stack.extend(children.into_iter().map(|c| (c, level + 1)));
                }
                If => {
                    let else_branch = children
                        .iter()
                        .copied()
                        .find(|&c| tree.kind(c) == ElseBranch);
                    let body_child = children.iter().copied().rev().find(|&c| {
                        tree.kind(c) != ElseBranch && Some(c) != children.first().copied()
                    });
                    if let Some(b) = body_child {
                        stack.push(body(b));
                    }
                    if let Some(e) = else_branch {
                        stack.push((e, level));
                    }
                }
                ElseBranch => {
                    if let Some(&b) = children.last() {
                        // O `senao se` continua na mesma coluna, como uma corrente.
                        if tree.kind(b) == If {
                            stack.push((b, level));
                        } else {
                            stack.push(body(b));
                        }
                    }
                }
                While | For => {
                    if let Some(&b) = children.last() {
                        if matches!(tree.kind(b), ForInit | ForStep) {
                            continue;
                        }
                        stack.push(body(b));
                    }
                }
                FuncImpl => {
                    for child in children {
                        if !matches!(tree.kind(child), Name | ParamList) {
                            stack.push((child, level));
                        }
                    }
                }
                _ => {}
            }
        }
        plan.statements
            .sort_by_key(|r| (r.start(), std::cmp::Reverse(r.end())));
        plan
    }

    /// Coluna que a linha que começa no token `token`, em `at`, passa a ter. `old` é a coluna de
    /// agora. `None` deixa a linha como está.
    fn target(
        &self,
        at: TextSize,
        token: usize,
        source: &Source,
        shifts: &FxHashMap<TextSize, (i64, u32)>,
        old: u32,
    ) -> Option<u32> {
        let (text, tokens) = (source.text, source.tokens);
        let unit = |level: u32| level * source.indent.tab_size.max(1);
        if let Some(&level) = self.anchors.get(&at) {
            return Some(unit(level));
        }
        // Continuação do comando mais de dentro que começou numa linha de cima.
        let line_start = |offset: TextSize| {
            TextSize::new(text[..usize::from(offset)].rfind('\n').map_or(0, |i| i + 1) as u32)
        };
        let here = line_start(at);
        let owner = self
            .statements
            .iter()
            .filter(|r| r.start() < here && r.contains(at))
            .max_by_key(|r| r.start());
        // O bloco mais de dentro em volta da linha. Quando ele começa depois do comando, a linha é
        // conteúdo do bloco, como um comentário dentro do bloco de um `se`, e não continuação.
        let block = self
            .blocks
            .iter()
            .filter(|(r, _)| r.contains(at))
            .max_by_key(|(r, _)| r.start());
        let inside_block = match (owner, block) {
            (Some(owner), Some((b, _))) => b.start() > owner.start(),
            (None, _) => true,
            (Some(_), None) => false,
        };
        if !inside_block && let Some(owner) = owner {
            let (shift, first) = shifts.get(&line_start(owner.start())).copied()?;
            return Some(((old as i64 + shift).max(0) as u32).max(first));
        }
        if tokens.kind(token).is_trivia() {
            return Some(unit(block.map_or(0, |(_, level)| *level)));
        }
        None
    }
}

/// O texto da regra, os tokens dele e o recuo pedido.
struct Source<'a> {
    text: &'a str,
    tokens: &'a crate::Tokens,
    indent: Indent,
}

/// Tokens do nó que não são espaço nem comentário, na ordem.
fn significant_tokens(tree: &Tree, node: NodeId) -> Vec<usize> {
    tree.elements(node)
        .filter_map(|e| match e {
            Element::Token(t) if !tree.tokens().kind(t).is_trivia() => Some(t),
            _ => None,
        })
        .collect()
}

/// Resultado da conferência do texto com as trocas.
enum Outcome {
    /// Os mesmos tokens, fora os espaços, e nenhum erro.
    Same,
    /// Os mesmos tokens, com estas linhas passando do limite do compilador.
    TooLong(Vec<usize>),
    /// Mudou algum token ou apareceu outro erro.
    Changed,
}

/// Confere que o texto com as trocas tem os mesmos tokens, fora os espaços, que o de antes.
fn check(text: &str, edits: &[Edit]) -> Outcome {
    if edits.is_empty() {
        return Outcome::Same;
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for edit in edits {
        out.push_str(&text[at..usize::from(edit.range.start())]);
        out.push_str(&edit.new_text);
        at = usize::from(edit.range.end());
    }
    out.push_str(&text[at..]);
    let (Ok(before), Ok(after)) = (crate::lex(text), crate::lex(&out)) else {
        return Outcome::Changed;
    };
    let significant = |tokens: &crate::Tokens, src: &str| -> Vec<(SyntaxKind, String)> {
        (0..tokens.len())
            .filter(|&i| !matches!(tokens.kind(i), SyntaxKind::Whitespace | SyntaxKind::Newline))
            .map(|i| (tokens.kind(i), tokens.text(i, src).to_owned()))
            .collect()
    };
    if significant(&before, text) != significant(&after, &out) {
        return Outcome::Changed;
    }
    if after.errors().is_empty() {
        return Outcome::Same;
    }
    let mut lines = Vec::new();
    for error in after.errors() {
        if error.kind != crate::LexErrorKind::LineTooLong {
            return Outcome::Changed;
        }
        lines.push(
            out[..usize::from(error.range.start())]
                .matches('\n')
                .count(),
        );
    }
    Outcome::TooLong(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format_with(text: &str, indent: Indent, only: Option<TextRange>) -> Option<String> {
        let tree = crate::parse(text).unwrap();
        let edits = indentation(text, &tree, indent, only)?;
        let mut out = text.to_owned();
        for edit in edits.iter().rev() {
            let range = usize::from(edit.range.start())..usize::from(edit.range.end());
            out.replace_range(range, &edit.new_text);
        }
        Some(out)
    }

    fn format(text: &str) -> String {
        let spaces = Indent {
            tab_size: 2,
            spaces: true,
        };
        let once = format_with(text, spaces, None).unwrap();
        // Formatar de novo não muda mais nada.
        assert_eq!(format_with(&once, spaces, None).unwrap(), once, "{once}");
        once
    }

    #[test]
    fn blocos_se_senao_e_comando_sem_bloco() {
        let text = "se (a = 1) {\nx = 1;\n      se (b = 2)\n{\ny = 2;\n    }\nsenao\nz = 3;\n}\n";
        assert_eq!(
            format(text),
            "se (a = 1) {\n  x = 1;\n  se (b = 2)\n  {\n    y = 2;\n  }\n  senao\n    z = 3;\n}\n"
        );
        // O `inicio` sozinho fica na coluna do comando, e o `senao se` continua na mesma coluna.
        let text = "enquanto (a < 1)\n    Inicio\n        a = a + 1;\n    Fim;\nse (a = 1) x = 1;\nsenao se (a = 2)\nx = 2;\n";
        assert_eq!(
            format(text),
            "enquanto (a < 1)\nInicio\n  a = a + 1;\nFim;\nse (a = 1) x = 1;\nsenao se (a = 2)\n  x = 2;\n"
        );
    }

    #[test]
    fn funcao_e_para() {
        let text =
            "Definir Funcao F();\nF();\nfuncao F(); {\npara (n = 1; n < 3; n++) {\nx = n;\n}\n}\n";
        assert_eq!(
            format(text),
            "Definir Funcao F();\nF();\nfuncao F(); {\n  para (n = 1; n < 3; n++) {\n    x = n;\n  }\n}\n"
        );
    }

    #[test]
    fn continuacao_anda_junto_com_a_primeira_linha() {
        let text = "enquanto (a < 1) {\nse ((a = 1) e\n    (b = 2)) {\nx = 1;\n}\n}\n";
        assert_eq!(
            format(text),
            "enquanto (a < 1) {\n  se ((a = 1) e\n      (b = 2)) {\n    x = 1;\n  }\n}\n"
        );
        // A continuação nunca fica à esquerda da primeira linha.
        let text = "        x = Soma(1,\n  2);\n";
        assert_eq!(format(text), "x = Soma(1,\n2);\n");
    }

    #[test]
    fn texto_e_comentario_de_varias_linhas_ficam_como_estao() {
        let text = "definir cursor c;\nse (a = 1) {\nc.SQL \"SELECT A \\\n       FROM T\";\n/* comentário\n      que continua */\n}\n";
        assert_eq!(
            format(text),
            "definir cursor c;\nse (a = 1) {\n  c.SQL \"SELECT A \\\n       FROM T\";\n  /* comentário\n      que continua */\n}\n"
        );
    }

    #[test]
    fn comentario_segue_o_nivel_do_bloco() {
        let text = "se (a = 1) {\n@ dentro @\nx = 1;\n      @ antes do fim @\n}\n    @ fora @\n";
        assert_eq!(
            format(text),
            "se (a = 1) {\n  @ dentro @\n  x = 1;\n  @ antes do fim @\n}\n@ fora @\n"
        );
    }

    #[test]
    fn trecho_desligado_fica_como_esta() {
        let text = "se (a = 1) {\n@ senior-lsp-format-off @\n      x  =  1;\n          y = 2;\n@ senior-lsp-format-on @\nz = 3;\n}\n";
        assert_eq!(
            format(text),
            "se (a = 1) {\n@ senior-lsp-format-off @\n      x  =  1;\n          y = 2;\n@ senior-lsp-format-on @\n  z = 3;\n}\n"
        );
    }

    #[test]
    fn tabulacao_crlf_trecho_e_erro_de_sintaxe() {
        let tabs = Indent {
            tab_size: 4,
            spaces: false,
        };
        let text = "se (a = 1) {\r\nse (b = 1) {\r\nx = 1;\r\n}\r\n}\r\n";
        assert_eq!(
            format_with(text, tabs, None).unwrap(),
            "se (a = 1) {\r\n\tse (b = 1) {\r\n\t\tx = 1;\r\n\t}\r\n}\r\n"
        );
        // Só as linhas do trecho mudam, com o nível calculado pelo arquivo inteiro.
        let spaces = Indent {
            tab_size: 2,
            spaces: true,
        };
        let text = "se (a = 1) {\nx = 1;\ny = 2;\n}\n";
        let second = TextRange::new(TextSize::new(13), TextSize::new(14));
        assert_eq!(
            format_with(text, spaces, Some(second)).unwrap(),
            "se (a = 1) {\n  x = 1;\ny = 2;\n}\n"
        );
        assert!(format_with("se (a = 1) {\nx = ;\n}\n", spaces, None).is_none());
    }

    #[test]
    fn linha_que_passaria_do_limite_fica_com_o_recuo_de_antes() {
        // A linha tem 253 caracteres, e com o recuo de um nível passaria do limite do compilador.
        let longa = format!("a = \"{}\";", "x".repeat(246));
        let text = format!("se (b = 1) {{\n{longa}\nc = 1;\n}}\n");
        assert_eq!(
            format(&text),
            format!("se (b = 1) {{\n{longa}\n  c = 1;\n}}\n")
        );
    }

    #[test]
    fn ao_digitar_o_fechamento_e_a_quebra_de_linha() {
        let spaces = Indent {
            tab_size: 2,
            spaces: true,
        };
        let typed = |text: &str, at: usize, typed: &str| {
            let tree = crate::parse(text).unwrap();
            let edits = on_type(text, &tree, spaces, TextSize::new(at as u32), typed)?;
            let mut out = text.to_owned();
            for edit in edits.iter().rev() {
                let range = usize::from(edit.range.start())..usize::from(edit.range.end());
                out.replace_range(range, &edit.new_text);
            }
            Some(out)
        };
        // O `}` digitado com o recuo do conteúdo volta para a coluna do `se`.
        let text = "se (a = 1) {\n  x = 1;\n  }\n";
        assert_eq!(typed(text, 24, "}").unwrap(), "se (a = 1) {\n  x = 1;\n}\n");
        // O `fim` digitado também.
        let text = "se (a = 1)\ninicio\n  x = 1;\n  fim\n";
        assert_eq!(
            typed(text, 29, "m").unwrap(),
            "se (a = 1)\ninicio\n  x = 1;\nfim\n"
        );
        // O `m` de outra palavra não mexe na linha.
        assert!(typed("se (a = 1) {\n  nome = 1;\n}\n", 17, "m").is_none());
        // A linha nova dentro do bloco entra um nível.
        let text = "se (a = 1) {\n\n}\n";
        assert_eq!(typed(text, 13, "\n").unwrap(), "se (a = 1) {\n  \n}\n");
    }

    #[test]
    fn quebra_de_linha_com_o_bloco_ainda_aberto() {
        let spaces = Indent {
            tab_size: 4,
            spaces: true,
        };
        let typed = |text: &str| {
            let tree = crate::parse(text).unwrap();
            let at = TextSize::new(text.len() as u32);
            let edits = on_type(text, &tree, spaces, at, "\n").unwrap();
            let mut out = text.to_owned();
            for edit in edits.iter().rev() {
                let range = usize::from(edit.range.start())..usize::from(edit.range.end());
                out.replace_range(range, &edit.new_text);
            }
            out
        };
        // Sem o `fim` a regra tem erro, e o nível vem da linha de cima.
        assert_eq!(
            typed("se (n = 1)\nInicio @ x @\n"),
            "se (n = 1)\nInicio @ x @\n    "
        );
        assert_eq!(
            typed("se (n = 1)\ninicio\n    n = 2;\n"),
            "se (n = 1)\ninicio\n    n = 2;\n    "
        );
        assert_eq!(
            typed("se (n = 1)\nmeuinicio = 1;\n"),
            "se (n = 1)\nmeuinicio = 1;\n"
        );
    }
}
