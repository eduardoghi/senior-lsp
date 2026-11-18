//! Formatador. Acerta o recuo do começo de cada linha, pelo aninhamento dos comandos, e o espaço
//! entre os tokens da mesma linha, e não junta nem quebra linhas.
//!
//! O recuo:
//!
//! - O conteúdo de um bloco, com chaves ou com `inicio` e `fim`, entra um nível. A chave ou o
//!   `inicio` sozinho na linha fica na coluna do comando, como o fechamento.
//! - O `senao` fica na coluna do seu `se`, e o comando sem bloco na linha de baixo de um `se`,
//!   `senao`, `enquanto` ou `para` entra um nível.
//! - A linha que continua um comando começado numa linha de cima anda junto com ela, mantendo o
//!   alinhamento que a pessoa fez, e nunca fica à esquerda dela.
//! - O comentário sozinho na linha segue o nível do bloco em que está.
//!
//! O espaço dentro da linha, só entre dois tokens com nada além de espaço entre eles:
//!
//! - Um espaço em volta dos operadores de dois lados (`=`, `<>`, `<`, `>`, `<=`, `>=`, `+`, `-`,
//!   `*`, `/`, `e` e `ou`), depois da vírgula e do `;` seguidos de outro token na linha e entre
//!   `se`, `enquanto` ou `para` e o `(`.
//! - Nenhum antes da vírgula e do `;`, depois do `(`, antes do `)` e entre o nome de uma função e o
//!   `(` dela, na chamada e na declaração.
//! - O resto, como o menos de um número negativo, o `++`, o ponto de `cur.Campo` e os dois-pontos de
//!   um rótulo, fica como está.
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

    /// O recuo `whitespace` já é o que [`Indent::whitespace`] escreveria para `columns`, sem
    /// montar o texto para comparar.
    fn is_whitespace(self, whitespace: &str, columns: u32) -> bool {
        let tab = self.tab_size.max(1);
        let (tabs, spaces) = if self.spaces {
            (0, columns as usize)
        } else {
            ((columns / tab) as usize, (columns % tab) as usize)
        };
        let bytes = whitespace.as_bytes();
        bytes.len() == tabs + spaces
            && bytes[..tabs].iter().all(|&b| b == b'\t')
            && bytes[tabs..].iter().all(|&b| b == b' ')
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
    run(text, tree, indent, only, false)
}

/// O recuo e o espaço dentro das linhas, como no [`indentation`].
pub fn format(
    text: &str,
    tree: &Tree,
    indent: Indent,
    only: Option<TextRange>,
) -> Option<Vec<Edit>> {
    run(text, tree, indent, only, true)
}

fn run(
    text: &str,
    tree: &Tree,
    indent: Indent,
    only: Option<TextRange>,
    spaces: bool,
) -> Option<Vec<Edit>> {
    if !tree.errors().is_empty() || !tree.tokens().errors().is_empty() {
        return None;
    }
    let plan = Plan::build(tree);
    let skipped = format_off_lines(text, tree);
    let spacing = spaces.then(|| Spacing::build(tree));
    let mut frozen = rustc_hash::FxHashSet::default();
    // Cada volta congela pelo menos uma linha nova, então o laço termina. O limite só protege de
    // um engano no cálculo.
    for _ in 0..64 {
        let mut edits = plan.edits(
            text,
            tree.tokens(),
            indent,
            only,
            &skipped,
            &frozen,
            all_lines(text),
        );
        if let Some(spacing) = &spacing {
            edits.extend(spacing.edits(text, tree.tokens(), only, &skipped, &frozen));
            edits.sort_by_key(|e| e.range.start());
        }
        match check(text, tree.tokens(), &edits) {
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
    #[allow(clippy::too_many_arguments)]
    fn edits(
        &self,
        text: &str,
        tokens: &crate::Tokens,
        indent: Indent,
        only: Option<TextRange>,
        skipped: &rustc_hash::FxHashSet<usize>,
        frozen: &rustc_hash::FxHashSet<usize>,
        lines: impl Iterator<Item = (usize, usize)>,
    ) -> Vec<Edit> {
        let mut edits = Vec::new();
        // Deslocamento e coluna nova de cada linha já calculada, pelo começo dela, para as
        // continuações.
        let mut shifts: FxHashMap<TextSize, (i64, u32)> = FxHashMap::default();
        for (start, line_index) in lines {
            // Depois do trecho pedido nenhuma linha muda, e as continuações só olham para cima.
            if only.is_some_and(|r| start > usize::from(r.end())) {
                break;
            }
            let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
            let line = &text[start..end];
            let ws_len = line.len() - line.trim_start_matches([' ', '\t']).len();
            let first = start + ws_len;
            let line_start = TextSize::new(start as u32);
            let rest = line[ws_len..].trim_end_matches('\r');
            let wanted = touches(only, TextRange::new(line_start, TextSize::new(end as u32)));
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
                        if !indent.is_whitespace(old_ws, columns) {
                            edits.push(Edit {
                                range: TextRange::new(line_start, TextSize::new(first as u32)),
                                new_text: indent.whitespace(columns),
                            });
                        }
                    }
                }
            }
        }
        edits
    }
}

/// Começo e número de cada linha do texto.
fn all_lines(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .enumerate()
        .map(|(index, start)| (start, index))
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
    // A linha pode começar com letra acentuada, então o corte é conferido na fronteira da letra.
    let closes = |word: &str| {
        content
            .get(..word.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(word))
            && !content[word.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    let range = TextRange::new(TextSize::new(start as u32), TextSize::new(end as u32));
    match typed {
        "}" if content.starts_with('}') => line_indentation(text, tree, indent, range),
        "m" | "M" if closes("fim") => line_indentation(text, tree, indent, range),
        "\n" if !content.is_empty() => line_indentation(text, tree, indent, range),
        "\n" => {
            let columns = if tree.errors().is_empty() && tree.tokens().errors().is_empty() {
                let plan = Plan::build_within(tree, Some(&[TextRange::empty(offset)]));
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

/// O recuo de uma linha só, como o [`indentation`] com o trecho `line`, sem percorrer o arquivo
/// inteiro. Uma linha que começa um comando ou fecha um bloco tem a coluna do nível dela, e uma
/// continuação depende só da linha em que começa o comando de que faz parte, que por sua vez pode
/// ser continuação de outro. O plano e o cálculo cobrem só essa corrente de linhas. Como só o recuo
/// de uma linha que começa num token muda, a conferência lexa só a linha.
fn line_indentation(text: &str, tree: &Tree, indent: Indent, line: TextRange) -> Option<Vec<Edit>> {
    if !tree.errors().is_empty() || !tree.tokens().errors().is_empty() {
        return None;
    }
    let line_start = |at: usize| text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_range = |start: usize| {
        let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
        TextRange::new(TextSize::new(start as u32), TextSize::new(end as u32))
    };
    // Do fim para o começo. O dono de cada linha é achado como no `Plan::target`, e todo comando
    // que contém o começo da linha encosta nela, então o plano da linha sozinha basta.
    let mut chain = vec![usize::from(line.start())];
    loop {
        let here = chain[chain.len() - 1];
        let window = line_range(here);
        let content = &text[here..usize::from(window.end())];
        let first = here + content.len() - content.trim_start_matches([' ', '\t']).len();
        let (here, at) = (TextSize::new(here as u32), TextSize::new(first as u32));
        let plan = Plan::build_within(tree, Some(&[window]));
        let owner = plan
            .statements
            .iter()
            .filter(|r| r.start() < here && r.contains(at))
            .max_by_key(|r| r.start());
        match owner {
            Some(owner) => chain.push(line_start(usize::from(owner.start()))),
            None => break,
        }
    }
    chain.reverse();
    let windows: Vec<TextRange> = chain.iter().map(|&start| line_range(start)).collect();
    let plan = Plan::build_within(tree, Some(&windows));
    let skipped = format_off_lines_until(text, tree, line.end());
    // Os números só servem para as linhas desligadas e a congelada. Sem trecho desligado, a
    // posição na corrente identifica a linha sem contar as quebras do começo do arquivo.
    let numbers: Vec<usize> = if skipped.is_empty() {
        (0..chain.len()).collect()
    } else {
        let mut number = text[..chain[0]].matches('\n').count();
        let mut numbers = vec![number];
        for pair in chain.windows(2) {
            number += text[pair[0]..pair[1]].matches('\n').count();
            numbers.push(number);
        }
        numbers
    };
    let own = numbers[numbers.len() - 1];
    let mut frozen = rustc_hash::FxHashSet::default();
    for _ in 0..2 {
        let lines = chain.iter().copied().zip(numbers.iter().copied());
        let edits = plan.edits(
            text,
            tree.tokens(),
            indent,
            Some(line),
            &skipped,
            &frozen,
            lines,
        );
        let [edit] = edits.as_slice() else {
            if edits.is_empty() {
                return Some(edits);
            }
            return indentation(text, tree, indent, Some(line));
        };
        match check_line(text, tree.tokens(), edit) {
            Outcome::Same => return Some(edits),
            // A linha que passaria do limite do compilador fica com o recuo de antes.
            Outcome::TooLong(_) => {
                frozen.insert(own);
            }
            // Inconclusivo só pela linha, como um texto que começa antes dela.
            Outcome::Changed => return indentation(text, tree, indent, Some(line)),
        }
    }
    indentation(text, tree, indent, Some(line))
}

/// A conferência do [`check`] só na linha da troca, que é o recuo de uma linha que começa num
/// token. O trecho lexado vai até o fim do último token que começa na linha, que pode ser um texto
/// que continua nas linhas de baixo.
fn check_line(text: &str, tokens: &crate::Tokens, edit: &Edit) -> Outcome {
    let start = usize::from(edit.range.start());
    let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    // O último token que começa antes do fim da linha, por busca binária.
    let (mut low, mut high) = (0, tokens.len());
    while low < high {
        let mid = (low + high) / 2;
        if usize::from(tokens.range(mid).start()) < line_end {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    let last = low
        .checked_sub(1)
        .filter(|&i| usize::from(tokens.range(i).start()) >= start);
    let mut end = last.map_or(line_end, |i| {
        usize::from(tokens.range(i).end()).max(line_end)
    });
    end = text[end..].find('\n').map_or(text.len(), |i| end + i);
    let old = &text[start..end];
    let Ok(before) = crate::lex(old) else {
        return Outcome::Changed;
    };
    if !before.errors().is_empty() {
        return Outcome::Changed;
    }
    let local = Edit {
        range: TextRange::new(TextSize::new(0), edit.range.end() - edit.range.start()),
        new_text: edit.new_text.clone(),
    };
    check(old, &before, std::slice::from_ref(&local))
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

/// O papel de cada token nas regras do espaço dentro da linha, pelo índice do token.
struct Spacing {
    roles: Vec<u8>,
}

/// Operador de dois lados, como o `=` da atribuição e o `+` de uma conta.
const BINARY: u8 = 1;
/// O `(` da lista de argumentos de uma chamada ou dos parâmetros de uma declaração.
const CALL_PAREN: u8 = 2;

impl Spacing {
    fn build(tree: &Tree) -> Spacing {
        use SyntaxKind::*;
        let mut spacing = Spacing {
            roles: vec![0; tree.tokens().len()],
        };
        let direct = |node: NodeId| {
            tree.elements(node).filter_map(|e| match e {
                Element::Token(t) => Some(t),
                Element::Node(_) => None,
            })
        };
        let first_token = |node: NodeId| {
            tree.token_range(node)
                .find(|&t| !tree.tokens().kind(t).is_trivia())
        };
        let mut stack = vec![tree.root()];
        while let Some(node) = stack.pop() {
            stack.extend(tree.children(node));
            match tree.kind(node) {
                BinaryExpr => {
                    for t in direct(node).filter(|&t| !tree.tokens().kind(t).is_trivia()) {
                        spacing.roles[t] |= BINARY;
                    }
                }
                Assign => {
                    for t in direct(node).filter(|&t| tree.tokens().kind(t) == Eq) {
                        spacing.roles[t] |= BINARY;
                    }
                }
                ArgList | ParamList => {
                    if let Some(open) = first_token(node)
                        && tree.tokens().kind(open) == LParen
                    {
                        spacing.roles[open] |= CALL_PAREN;
                    }
                }
                _ => {}
            }
        }
        spacing
    }

    /// Quantos espaços vão entre os tokens `a` e `b`, ou `None` para deixar como está.
    fn wanted(&self, tokens: &crate::Tokens, a: usize, b: usize) -> Option<usize> {
        use SyntaxKind::*;
        let (ka, kb) = (tokens.kind(a), tokens.kind(b));
        if ka == LParen || matches!(kb, RParen | Semicolon | Comma) {
            return Some(0);
        }
        if self.roles[b] & CALL_PAREN != 0 {
            return Some(0);
        }
        if matches!(ka, Comma | Semicolon) || (self.roles[a] | self.roles[b]) & BINARY != 0 {
            return Some(1);
        }
        if matches!(ka, SeKw | EnquantoKw | ParaKw) && kb == LParen {
            return Some(1);
        }
        None
    }

    /// As trocas do espaço entre os tokens de cada linha, fora as de `skipped` e de `frozen`.
    fn edits(
        &self,
        text: &str,
        tokens: &crate::Tokens,
        only: Option<TextRange>,
        skipped: &rustc_hash::FxHashSet<usize>,
        frozen: &rustc_hash::FxHashSet<usize>,
    ) -> Vec<Edit> {
        let mut edits = Vec::new();
        let mut line = 0usize;
        let mut previous: Option<usize> = None;
        for i in 0..tokens.len() {
            let kind = tokens.kind(i);
            match kind {
                SyntaxKind::Newline => {
                    line += 1;
                    previous = None;
                    continue;
                }
                SyntaxKind::Whitespace => continue,
                // Um comentário no meio separa os dois lados, que ficam como estão. O de várias
                // linhas também conta as quebras.
                SyntaxKind::AtComment | SyntaxKind::BlockComment => {
                    line += tokens.text(i, text).matches('\n').count();
                    previous = None;
                    continue;
                }
                _ => {}
            }
            // Um texto que continua na linha de baixo também conta as quebras dele. Fora o texto e os
            // comentários, nenhum token tem quebra.
            let breaks = if kind == SyntaxKind::String {
                tokens.text(i, text).matches('\n').count()
            } else {
                0
            };
            if let Some(a) = previous
                && let Some(count) = self.wanted(tokens, a, i)
                && !skipped.contains(&line)
                && !frozen.contains(&line)
            {
                let gap = TextRange::new(tokens.range(a).end(), tokens.range(i).start());
                let current = &text[gap];
                let same = current.len() == count && current.bytes().all(|b| b == b' ');
                // Juntar dois operadores mudaria os tokens, como `- -` virando `--`.
                let joins = count == 0 && !gap.is_empty() && glues(tokens.kind(a), kind);
                if !same && !joins && touches(only, gap) {
                    edits.push(Edit {
                        range: gap,
                        new_text: " ".repeat(count),
                    });
                }
            }
            line += breaks;
            previous = (breaks == 0).then_some(i);
        }
        edits
    }
}

/// Dois tokens que, colados, viram outro, como `-` e `-` virando `--`, ou que viram um nome só.
fn glues(a: SyntaxKind, b: SyntaxKind) -> bool {
    use SyntaxKind::*;
    let word = |k: SyntaxKind| k == Ident || k == Number || k.is_keyword();
    matches!(
        (a, b),
        (Minus, Minus) | (Plus, Plus) | (Lt, Gt) | (Lt, Eq) | (Gt, Eq)
    ) || (word(a) && word(b))
}

/// O trecho `range` está no trecho pedido `only`, ou não há trecho pedido. Os dois precisam se
/// sobrepor, e não só se encostar, porque a seleção de linhas inteiras termina no começo da linha
/// de baixo, que não entra. A seleção vazia, só com o cursor, vale para o que está em volta dele.
fn touches(only: Option<TextRange>, range: TextRange) -> bool {
    let Some(r) = only else {
        return true;
    };
    if r.is_empty() {
        range.contains_inclusive(r.start())
    } else if range.is_empty() {
        r.start() <= range.start() && range.start() < r.end()
    } else {
        r.start() < range.end() && range.start() < r.end()
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
    format_off_lines_until(text, tree, TextSize::of(text))
}

/// Como o [`format_off_lines`], só com as linhas até `until`.
fn format_off_lines_until(
    text: &str,
    tree: &Tree,
    until: TextSize,
) -> rustc_hash::FxHashSet<usize> {
    // Quase nenhuma regra tem a diretiva, e procurar no texto custa bem menos que olhar cada token.
    const DIRECTIVE: &[u8] = b"senior-lsp-format-o";
    let until_text = &text.as_bytes()[..usize::from(until).min(text.len())];
    let mentioned = memchr::memchr_iter(b'-', until_text).any(|i| {
        i >= 10
            && until_text
                .get(i - 10..i - 10 + DIRECTIVE.len())
                .is_some_and(|w| w.eq_ignore_ascii_case(DIRECTIVE))
    });
    if !mentioned {
        return rustc_hash::FxHashSet::default();
    }
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
        if tokens.range(i).start() > until {
            break;
        }
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
        lines.extend(from..=line_of(until.min(TextSize::of(text))));
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
        Plan::build_within(tree, None)
    }

    /// O plano só dos nós que encostam em alguma das `windows`, ou de todos sem elas. O nível de um
    /// nó depende só dos que estão em volta dele, que também encostam na janela, então o que o
    /// plano diz dos pontos das janelas sai igual ao do arquivo inteiro.
    fn build_within(tree: &Tree, windows: Option<&[TextRange]>) -> Plan {
        use SyntaxKind::*;
        let mut plan = Plan::default();
        let inside = |node: NodeId| {
            let range = tree.text_range(node);
            windows.is_none_or(|ws| ws.iter().any(|w| range.intersect(*w).is_some()))
        };
        let mut stack: Vec<(NodeId, u32)> = tree.children(tree.root()).map(|c| (c, 0)).collect();
        while let Some((node, level)) = stack.pop() {
            if !inside(node) {
                continue;
            }
            // O bloco não conta como comando de que uma linha seja continuação. As linhas dele são
            // conteúdo, com o nível que ele dá.
            if tree.kind(node) != Block {
                plan.statements.push(tree.text_range(node));
            }
            // O primeiro token pode estar num nó filho, como o nome no começo de `x = 1;`.
            let first = tree
                .token_range(node)
                .find(|&t| !tree.tokens().kind(t).is_trivia());
            if let Some(first) = first {
                plan.anchors
                    .entry(tree.tokens().range(first).start())
                    .or_insert(level);
            }
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
                    let tokens = significant_tokens(tree, node);
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
                    stack.extend(tree.children(node).map(|c| (c, level + 1)));
                }
                If => {
                    let children: Vec<NodeId> = tree.children(node).collect();
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
                    if let Some(b) = tree.children(node).last() {
                        // O `senao se` continua na mesma coluna, como uma corrente.
                        if tree.kind(b) == If {
                            stack.push((b, level));
                        } else {
                            stack.push(body(b));
                        }
                    }
                }
                While | For => {
                    if let Some(b) = tree.children(node).last() {
                        if matches!(tree.kind(b), ForInit | ForStep) {
                            continue;
                        }
                        stack.push(body(b));
                    }
                }
                FuncImpl => {
                    for child in tree.children(node) {
                        if !matches!(tree.kind(child), Name | ParamList) {
                            stack.push((child, level));
                        }
                    }
                }
                _ => {}
            }
        }
        plan.statements
            .sort_unstable_by_key(|r| (r.start(), std::cmp::Reverse(r.end())));
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

/// Confere que o texto com as trocas tem os mesmos tokens, fora os espaços, que o de antes. `before`
/// são os tokens de `text`, que a árvore já tem.
fn check(text: &str, before: &crate::Tokens, edits: &[Edit]) -> Outcome {
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
    let Ok(after) = crate::lex(&out) else {
        return Outcome::Changed;
    };
    fn significant<'a>(
        tokens: &'a crate::Tokens,
        src: &'a str,
    ) -> impl Iterator<Item = (SyntaxKind, &'a str)> + 'a {
        (0..tokens.len())
            .filter(|&i| !matches!(tokens.kind(i), SyntaxKind::Whitespace | SyntaxKind::Newline))
            .map(move |i| (tokens.kind(i), tokens.text(i, src)))
    }
    if !significant(before, text).eq(significant(&after, &out)) {
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

    /// Textos para as conferências de propriedade do recuo, com continuação de comando, SQL de
    /// várias linhas, trecho desligado, tabulação misturada, CRLF e erro de sintaxe.
    fn textos_de_recuo() -> Vec<std::string::String> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/regras");
        let mut textos: Vec<std::string::String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
            .collect();
        textos.extend(
            [
                "enquanto (a < 1) {\nse ((a = 1) e\n    (b = 2)) {\nx = Soma(1,\n  2,\n\t3);\n}\n}\n",
                "se (a = 1) {\n@ senior-lsp-format-off @\n      x  =  1;\n@ senior-lsp-format-on @\n\t  z = 3;\n}\n",
                "definir cursor c;\r\nse (a = 1) {\r\n\tc.SQL \"SELECT A \\\r\n       FROM T\";\r\n  }\r\n",
                "se (a = 1)\ninicio\n\t\tx = 1;\n   fim\nsenao\n y = 2;\n",
                "se (a = 1) {\n  x = ;\n}\n",
                "funcao f(); {\n\t\t\tx = 1;\n se (x) { y = 1;\n}\n}\n",
            ]
            .map(str::to_owned),
        );
        textos
    }

    fn estilos() -> [Indent; 4] {
        [
            Indent {
                tab_size: 2,
                spaces: true,
            },
            Indent {
                tab_size: 4,
                spaces: true,
            },
            Indent {
                tab_size: 4,
                spaces: false,
            },
            Indent {
                tab_size: 3,
                spaces: false,
            },
        ]
    }

    #[test]
    fn recuo_de_uma_linha_igual_ao_do_arquivo() {
        // O recuo de uma linha só, usado ao digitar, tem de dar o mesmo que o recuo do arquivo
        // inteiro restrito àquela linha, em todo estilo de recuo.
        for text in textos_de_recuo() {
            let tree = crate::parse(&text).unwrap();
            for indent in estilos() {
                let mut start = 0;
                for line in text.split('\n') {
                    let end = start + line.len();
                    let range =
                        TextRange::new(TextSize::new(start as u32), TextSize::new(end as u32));
                    let one = line_indentation(&text, &tree, indent, range);
                    let all = indentation(&text, &tree, indent, Some(range));
                    assert_eq!(one, all, "{indent:?} na linha {line:?} de {text:?}");
                    start = end + 1;
                }
            }
        }
    }

    #[test]
    fn recuo_estavel_em_todo_estilo() {
        // Aplicar o recuo e calcular de novo não muda nada, e o texto só muda em espaço.
        for text in textos_de_recuo() {
            for indent in estilos() {
                let tree = crate::parse(&text).unwrap();
                let Some(edits) = indentation(&text, &tree, indent, None) else {
                    continue;
                };
                let mut out = text.clone();
                for edit in edits.iter().rev() {
                    let range = usize::from(edit.range.start())..usize::from(edit.range.end());
                    out.replace_range(range, &edit.new_text);
                }
                let sem_espaco = |t: &str| t.split_whitespace().collect::<std::string::String>();
                assert_eq!(sem_espaco(&out), sem_espaco(&text), "{indent:?}");
                let tree = crate::parse(&out).unwrap();
                assert_eq!(
                    indentation(&out, &tree, indent, None).map(|e| e.len()),
                    Some(0),
                    "{indent:?} em {out:?}"
                );
            }
        }
    }

    #[test]
    fn ao_digitar_nos_limites() {
        let run = |text: &str, at: usize, typed: &str, indent: Indent| {
            let tree = crate::parse(text).unwrap();
            let edits = on_type(text, &tree, indent, TextSize::new(at as u32), typed);
            edits.map(|edits| {
                let mut out = text.to_owned();
                for edit in edits.iter().rev() {
                    let range = usize::from(edit.range.start())..usize::from(edit.range.end());
                    out.replace_range(range, &edit.new_text);
                }
                out
            })
        };
        let spaces = Indent {
            tab_size: 4,
            spaces: true,
        };
        let tabs = Indent {
            tab_size: 4,
            spaces: false,
        };
        // Com a regra ainda com erro, a linha depois de um `{` entra um nível, com espaço ou
        // tabulação, a partir do recuo da linha de cima.
        assert_eq!(
            run("se (n = 1) {\n", 13, "\n", spaces).as_deref(),
            Some("se (n = 1) {\n    ")
        );
        assert_eq!(
            run("se (n = 1) {\n", 13, "\n", tabs).as_deref(),
            Some("se (n = 1) {\n\t")
        );
        assert_eq!(
            run("\tse (n = 1) {\n", 14, "\n", tabs).as_deref(),
            Some("\tse (n = 1) {\n\t\t")
        );
        // Só o `fim` inteiro fecha o bloco.
        assert_eq!(
            run("se (n = 1)\ninicio\n  fim\n", 21, "m", spaces).as_deref(),
            Some("se (n = 1)\ninicio\nfim\n")
        );
        assert_eq!(run("se (n = 1)\ninicio\n  fim_\n", 22, "m", spaces), None);
        assert_eq!(run("se (n = 1)\ninicio\n  fimx\n", 22, "m", spaces), None);
        // O `}` no meio da linha não mexe nela.
        assert_eq!(run("se (a = 1) {\n  x = {};\n}\n", 20, "}", spaces), None);
        // A quebra com o resto da linha depois do cursor leva o resto para o nível do bloco, e a
        // linha nova que já tem recuo fica com o do bloco.
        assert_eq!(
            run("se (a = 1) {\nx = 1;\n}\n", 13, "\n", spaces).as_deref(),
            Some("se (a = 1) {\n    x = 1;\n}\n")
        );
        assert_eq!(
            run("se (a = 1) {\n      \n}\n", 13, "\n", spaces).as_deref(),
            Some("se (a = 1) {\n    \n}\n")
        );
        assert_eq!(
            run("se (a = 1) {\n    \n}\n", 13, "\n", spaces).as_deref(),
            Some("se (a = 1) {\n    \n}\n")
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

    fn format_all(text: &str) -> String {
        let spaces = Indent {
            tab_size: 2,
            spaces: true,
        };
        let tree = crate::parse(text).unwrap();
        let edits = super::format(text, &tree, spaces, None).unwrap();
        let mut out = text.to_owned();
        for edit in edits.iter().rev() {
            let range = usize::from(edit.range.start())..usize::from(edit.range.end());
            out.replace_range(range, &edit.new_text);
        }
        let tree = crate::parse(&out).unwrap();
        assert!(
            super::format(&out, &tree, spaces, None).unwrap().is_empty(),
            "{out}"
        );
        out
    }

    #[test]
    fn espaco_dentro_da_linha() {
        assert_eq!(
            format_all("se((a=1) e(b<>2)){\nx=Soma(a,b ,c) ;\ny = - 1;\n}\n"),
            "se ((a = 1) e (b <> 2)){\n  x = Soma(a, b, c);\n  y = - 1;\n}\n"
        );
        // O menos de número negativo, o `++` e o ponto ficam como estão, e um operador nunca
        // cola no outro.
        assert_eq!(
            format_all("para(n=1;n<=3;n++) x=a- -b;\nc.SQL \"A\";\nEnviar( 1 );\n"),
            "para (n = 1; n <= 3; n++) x = a - -b;\nc.SQL \"A\";\nEnviar(1);\n"
        );
        // A declaração e a implementação da função também perdem o espaço antes do `(`.
        assert_eq!(
            format_all(
                "Definir Funcao F (Numero pA,Alfa pB);\nfuncao F (Numero pA,Alfa pB); { }\n"
            ),
            "Definir Funcao F(Numero pA, Alfa pB);\nfuncao F(Numero pA, Alfa pB); { }\n"
        );
        // Texto e comentário ficam como estão, e um comentário no meio separa os dois lados.
        assert_eq!(
            format_all("a=\"x=1,2\" @ y=2 @;\n"),
            "a = \"x=1,2\" @ y=2 @;\n"
        );
    }

    #[test]
    fn selecao_que_termina_no_comeco_da_linha_de_baixo() {
        let spaces = Indent {
            tab_size: 2,
            spaces: true,
        };
        let text = "se (a = 1) {\nx=1;\ny=2;\n}\n";
        // A seleção da segunda linha inteira termina no começo da terceira, que fica como está.
        let second = TextRange::new(TextSize::new(13), TextSize::new(18));
        let tree = crate::parse(text).unwrap();
        let edits = super::format(text, &tree, spaces, Some(second)).unwrap();
        let mut out = text.to_owned();
        for edit in edits.iter().rev() {
            let range = usize::from(edit.range.start())..usize::from(edit.range.end());
            out.replace_range(range, &edit.new_text);
        }
        assert_eq!(out, "se (a = 1) {\n  x = 1;\ny=2;\n}\n");
    }
}
