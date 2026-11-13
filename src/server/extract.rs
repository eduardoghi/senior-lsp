//! Ação de código "Extrair função", que leva comandos inteiros para uma `funcao` nova.
//!
//! Toda variável definida fora de função é global e a função enxerga, então o trecho vira uma
//! função sem parâmetros e sem retorno. A seleção precisa cobrir comandos inteiros e contíguos do
//! mesmo bloco, e o trecho não pode depender do lugar onde está (laço, rótulo, escopo da função em
//! volta), senão a ação não é oferecida.

use std::collections::HashMap;

use lsp_types::{CodeAction, CodeActionKind, TextEdit, WorkspaceEdit};
use text_size::{TextRange, TextSize};

use super::navigation::Context;
use super::quickfix::newline;
use crate::builtins;
use crate::symbols::DefKind;
use crate::{NodeId, SyntaxKind};

const BASE_NAME: &str = "NovaFuncao";

/// Quem pode ser pai de comandos. O corpo de uma `funcao` é o bloco dela mais os comandos que
/// vêm depois dele, por isso a `FuncImpl` também conta.
pub(super) const CONTAINERS: [SyntaxKind; 3] = [
    SyntaxKind::SourceFile,
    SyntaxKind::Block,
    SyntaxKind::FuncImpl,
];

impl Context<'_> {
    /// A ação que leva os comandos selecionados para uma função nova.
    pub(crate) fn extract_actions(&self, range: lsp_types::Range) -> Vec<CodeAction> {
        // Dentro do JSON de modelo, as edições teriam que ir para dentro do texto escapado.
        if self.embedding.is_some() {
            return Vec::new();
        }
        self.extraction(range).into_iter().collect()
    }

    fn extraction(&self, range: lsp_types::Range) -> Option<CodeAction> {
        let (a, b) = (self.offset(range.start), self.offset(range.end));
        let selection = TextRange::new(a.min(b), a.max(b));
        if selection.is_empty() {
            return None;
        }
        let (container, statements) = self.selected_statements(selection)?;
        let first = *statements.first()?;
        let last = *statements.last()?;
        let tree = &self.doc.tree;
        let span = TextRange::new(tree.text_range(first).start(), tree.text_range(last).end());
        if !self.movable(container, &statements, span) {
            return None;
        }

        let text = &self.doc.text;
        let nl = newline(text);
        let name = self.new_name(BASE_NAME);
        let declaration = format!("{} {name}();", self.keyword_case("Definir Funcao"));
        let (at, insert) = self.insert_after(self.declaration_point(span.start()), &declaration);
        // A declaração entra na linha de baixo do último `definir`, que não pode estar na linha
        // do trecho nem depois dela.
        if at > span.start() {
            return None;
        }

        let call = format!("{name}();");
        let end = TextSize::of(text.as_str());
        let tail = self.function_text(&name, span, nl);
        let mut edits = vec![(TextRange::empty(at), insert)];
        if span.end() == end {
            edits.push((span, format!("{call}{tail}")));
        } else {
            edits.push((span, call));
            edits.push((TextRange::empty(end), tail));
        }
        let edits = edits
            .into_iter()
            .map(|(range, new_text)| TextEdit::new(self.range(range), new_text))
            .collect();
        #[allow(clippy::mutable_key_type)]
        let changes = HashMap::from([(self.uri.clone(), edits)]);
        Some(CodeAction {
            title: "Extrair função".to_owned(),
            kind: Some(CodeActionKind::REFACTOR_EXTRACT),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..WorkspaceEdit::default()
            }),
            ..CodeAction::default()
        })
    }

    /// O bloco que contém a seleção e os comandos dele que ela cobre. `None` quando ela corta um
    /// comando, pega algo que não é comando (nome de função, chave solta) ou tem erro de sintaxe.
    pub(super) fn selected_statements(
        &self,
        selection: TextRange,
    ) -> Option<(NodeId, Vec<NodeId>)> {
        let tree = &self.doc.tree;
        if tree.errors().iter().any(|e| {
            let (start, end) = (e.range.start(), e.range.end());
            if start == end {
                selection.start() < start && start <= selection.end()
            } else {
                start < selection.end() && end > selection.start()
            }
        }) {
            return None;
        }
        let container = self.container(selection);
        let statements: Vec<NodeId> = tree
            .children(container)
            .filter(|&c| {
                let r = tree.text_range(c);
                r.start() < selection.end() && r.end() > selection.start()
            })
            .collect();
        if statements.is_empty() {
            return None;
        }
        for &s in &statements {
            if !is_statement(tree.kind(s)) || !selection.contains_range(tree.text_range(s)) {
                return None;
            }
        }
        // Fora dos comandos só pode haver comentário e espaço, senão a seleção pega pedaço de
        // outra coisa, como a chave do bloco.
        let tokens = tree.tokens();
        let first_token = first_token_ending_after(tree, selection.start());
        let mut next = 0;
        for t in first_token..tokens.len() {
            let range = tokens.range(t);
            if range.start() >= selection.end() {
                break;
            }
            if tokens.kind(t).is_trivia() {
                continue;
            }
            while next < statements.len()
                && tree.text_range(statements[next]).end() <= range.start()
            {
                next += 1;
            }
            let covered = next < statements.len()
                && tree.text_range(statements[next]).start() <= range.start();
            if !covered {
                return None;
            }
        }
        Some((container, statements))
    }

    /// O bloco mais fundo (arquivo, `{ }` ou `funcao`) que contém a seleção inteira.
    fn container(&self, selection: TextRange) -> NodeId {
        let tree = &self.doc.tree;
        let mut node = tree.root();
        let mut best = node;
        while let Some(child) = tree
            .children(node)
            .find(|&c| tree.text_range(c).contains_range(selection))
        {
            node = child;
            if CONTAINERS.contains(&tree.kind(child)) {
                best = child;
            }
        }
        best
    }

    /// Diz se o trecho funciona igual dentro de uma função nova, no fim do arquivo.
    fn movable(&self, container: NodeId, statements: &[NodeId], span: TextRange) -> bool {
        let tree = &self.doc.tree;
        let symbols = &self.doc.symbols;
        let mut has_label = false;
        let mut stack: Vec<NodeId> = statements.to_vec();
        while let Some(node) = stack.pop() {
            stack.extend(tree.children(node));
            match tree.kind(node) {
                // A variável local passaria a existir só dentro da função nova.
                SyntaxKind::VarDef
                | SyntaxKind::CursorDef
                | SyntaxKind::ListDef
                | SyntaxKind::TableDef
                | SyntaxKind::WsDef
                | SyntaxKind::FuncDecl
                | SyntaxKind::FuncImpl
                | SyntaxKind::ErrorNode => return false,
                SyntaxKind::Break | SyntaxKind::Continue if !self.in_loop(node, container) => {
                    return false;
                }
                SyntaxKind::Goto => {
                    let Some(label) = tree.children(node).next() else {
                        return false;
                    };
                    let Some(reference) = symbols.reference_at(tree.text_range(label)) else {
                        return false;
                    };
                    let inside = |&t: &usize| {
                        span.contains_range(tree.text_range(symbols.definitions[t].node))
                    };
                    if !reference.targets.iter().all(inside) {
                        return false;
                    }
                }
                SyntaxKind::Label => has_label = true,
                _ => {}
            }
        }
        if has_label && self.label_used_outside(span) {
            return false;
        }
        let mut enclosing = None;
        let mut up = Some(container);
        while let Some(n) = up {
            if tree.kind(n) == SyntaxKind::FuncImpl {
                enclosing = Some(n);
            }
            up = tree.parent(n);
        }
        match enclosing {
            Some(function) => self.independent_of_function(function, span),
            None => true,
        }
    }

    /// O `pare` ou `continue` tem um `enquanto` ou `para` entre ele e o bloco do trecho.
    pub(super) fn in_loop(&self, node: NodeId, container: NodeId) -> bool {
        let tree = &self.doc.tree;
        let mut up = tree.parent(node);
        while let Some(n) = up.filter(|&n| n != container) {
            if matches!(tree.kind(n), SyntaxKind::While | SyntaxKind::For) {
                return true;
            }
            up = tree.parent(n);
        }
        false
    }

    /// Algum `vapara` de fora do trecho aponta para um rótulo de dentro dele.
    pub(super) fn label_used_outside(&self, span: TextRange) -> bool {
        let tree = &self.doc.tree;
        let symbols = &self.doc.symbols;
        symbols.references.iter().any(|r| {
            !span.contains_range(r.range)
                && r.targets.iter().any(|&t| {
                    let def = &symbols.definitions[t];
                    def.kind == DefKind::Label && span.contains_range(tree.text_range(def.node))
                })
        })
    }

    /// O trecho dentro de uma `funcao` não usa parâmetro nem variável definida nela. A variável é
    /// global, mas o `definir` fica dentro do corpo que o trecho deixaria.
    fn independent_of_function(&self, function: NodeId, span: TextRange) -> bool {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let symbols = &self.doc.symbols;
        let body = tree.text_range(function);
        let local = |def: &crate::symbols::Definition| match def.kind {
            DefKind::Parameter(_) => true,
            DefKind::Variable(_)
            | DefKind::Cursor
            | DefKind::List
            | DefKind::Table
            | DefKind::WebService => body.contains_range(tree.text_range(def.node)),
            DefKind::Function | DefKind::Label => false,
        };
        let from = symbols
            .references
            .partition_point(|r| r.range.start() < span.start());
        let by_reference = symbols.references[from..]
            .iter()
            .take_while(|r| r.range.start() < span.end())
            .any(|r| r.targets.iter().any(|&t| local(&symbols.definitions[t])));
        if by_reference {
            return false;
        }
        // O `:nome` de dentro de um texto de SQL também lê a variável.
        let tokens = tree.tokens();
        for t in first_token_ending_after(tree, span.start())..tokens.len() {
            let range = tokens.range(t);
            if range.start() >= span.end() {
                break;
            }
            if tokens.kind(t) != SyntaxKind::String {
                continue;
            }
            for name in sql_variables(&text[range]) {
                let targets = symbols.value_targets(tree, text, name, Some(function));
                if targets.iter().any(|&i| local(&symbols.definitions[i])) {
                    return false;
                }
            }
        }
        true
    }

    /// `base`, ou `base2` e seguintes se o nome já é de algo que o arquivo enxerga.
    pub(super) fn new_name(&self, base: &str) -> String {
        let doc = self.doc;
        let taken = |name: &str| {
            doc.symbols
                .definitions
                .iter()
                .any(|d| doc.text[d.name].eq_ignore_ascii_case(name))
                || doc.inherited.contains(name)
                || doc.inherited.variable(name).is_some()
                || doc.inherited.is_entry(name)
                || builtins::lookup(name, doc.context).is_some()
                || builtins::system_variable(name, doc.context).is_some()
        };
        (1..)
            .map(|n| match n {
                1 => base.to_owned(),
                n => format!("{base}{n}"),
            })
            .find(|name| !taken(name))
            .unwrap_or_else(|| base.to_owned())
    }

    /// Fim do último `definir funcao` do nível de cima antes do trecho, ou do último `definir`
    /// se não houver nenhum `definir funcao`.
    fn declaration_point(&self, before: TextSize) -> Option<TextSize> {
        let tree = &self.doc.tree;
        let (mut function, mut any) = (None, None);
        for d in &self.doc.symbols.definitions {
            if tree.parent(d.node) != Some(tree.root()) {
                continue;
            }
            let end = tree.text_range(d.node).end();
            if end > before {
                continue;
            }
            match tree.kind(d.node) {
                SyntaxKind::FuncDecl => function = function.max(Some(end)),
                SyntaxKind::VarDef
                | SyntaxKind::CursorDef
                | SyntaxKind::ListDef
                | SyntaxKind::TableDef
                | SyntaxKind::WsDef => any = any.max(Some(end)),
                _ => {}
            }
        }
        function.or(any)
    }

    /// O texto que vai no fim do arquivo, com uma linha em branco antes e a quebra de linha depois.
    fn function_text(&self, name: &str, span: TextRange, nl: &str) -> String {
        let text = &self.doc.text;
        let blank = if text.ends_with("\n\n") || text.ends_with("\r\n\r\n") {
            ""
        } else if text.ends_with('\n') {
            nl
        } else {
            // O arquivo termina na última linha sem quebra.
            &format!("{nl}{nl}")
        };
        let body = self.reindented(span, "    ", nl);
        let keyword = self.keyword_case("Funcao");
        format!("{blank}{keyword} {name}(); {{{nl}{body}{nl}}}{nl}")
    }

    /// Texto do trecho sem a indentação comum às linhas e com `indent` no lugar, com a quebra de
    /// linha do arquivo e sem espaço no fim das linhas. Linhas que começam dentro de um texto
    /// entre aspas ficam como estão, porque a indentação delas é conteúdo.
    pub(super) fn reindented(&self, span: TextRange, indent: &str, nl: &str) -> String {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let tokens = tree.tokens();
        let mut inside_string = Vec::new();
        for t in first_token_ending_after(tree, span.start())..tokens.len() {
            let range = tokens.range(t);
            if range.start() >= span.end() {
                break;
            }
            if matches!(tokens.kind(t), SyntaxKind::String | SyntaxKind::Char) {
                let literal = &text[range];
                let base = usize::from(range.start());
                inside_string.extend(literal.match_indices('\n').map(|(i, _)| base + i));
            }
        }
        let start = usize::from(span.start());
        let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
        let first_indent: String = text[line_start..start]
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .collect();
        reindent(
            &text[start..usize::from(span.end())],
            start,
            &first_indent,
            &inside_string,
            indent,
            nl,
        )
    }
}

/// Reindenta o `trecho`, que começa em `offset` no arquivo. `first_indent` é a indentação da linha
/// onde ele começa, `inside_string` os deslocamentos das quebras de linha que estão dentro de um
/// texto entre aspas e `indent` a indentação que cada linha ganha.
fn reindent(
    snippet: &str,
    offset: usize,
    first_indent: &str,
    inside_string: &[usize],
    indent: &str,
    nl: &str,
) -> String {
    struct Line<'a> {
        content: &'a str,
        /// A linha começa dentro de um texto entre aspas.
        verbatim: bool,
        /// A quebra que a termina está dentro de um texto entre aspas.
        raw_break: bool,
        /// Tem quebra depois.
        broken: bool,
    }
    let mut lines = Vec::new();
    let mut pos = 0;
    let mut verbatim = false;
    for segment in snippet.split('\n') {
        let end = pos + segment.len();
        let broken = end < snippet.len();
        let raw_break = broken && inside_string.contains(&(offset + end));
        lines.push(Line {
            content: segment,
            verbatim,
            raw_break,
            broken,
        });
        verbatim = raw_break;
        pos = end + 1;
    }
    // Indentação comum, a da linha onde o trecho começa para a primeira.
    let mut common: Option<String> = None;
    for (i, line) in lines.iter().enumerate() {
        if line.verbatim || line.content.trim().is_empty() {
            continue;
        }
        let indent: String = if i == 0 {
            first_indent.to_owned()
        } else {
            line.content
                .chars()
                .take_while(|c| matches!(c, ' ' | '\t'))
                .collect()
        };
        common = Some(match common {
            None => indent,
            Some(c) => c
                .chars()
                .zip(indent.chars())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a)
                .collect(),
        });
    }
    let common = common.unwrap_or_default();
    let mut out = String::with_capacity(snippet.len() + lines.len() * 4);
    for (i, line) in lines.iter().enumerate() {
        if line.verbatim {
            out.push_str(line.content);
        } else {
            let content = if i == 0 {
                line.content
            } else {
                line.content
                    .strip_prefix(common.as_str())
                    .unwrap_or_else(|| line.content.trim_start())
            };
            // Numa linha cuja quebra é do texto entre aspas, o fim é conteúdo.
            let content = if line.raw_break {
                content
            } else {
                content.trim_end()
            };
            if !content.trim().is_empty() {
                out.push_str(indent);
                out.push_str(content);
            }
        }
        if line.broken {
            out.push_str(if line.raw_break { "\n" } else { nl });
        }
    }
    out
}

/// Comandos que uma seleção pode levar. Os `definir` e as `funcao` ficam de fora.
pub(super) fn is_statement(kind: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        kind,
        Block
            | If
            | While
            | For
            | Goto
            | Label
            | Break
            | Continue
            | Assign
            | CallStmt
            | FieldCommand
            | KeywordStmt
    )
}

/// Índice do primeiro token que termina depois de `offset`.
pub(super) fn first_token_ending_after(tree: &crate::Tree, offset: TextSize) -> usize {
    let tokens = tree.tokens();
    let (mut lo, mut hi) = (0, tokens.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if tokens.range(mid).end() <= offset {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// Os nomes depois de `:` dentro de um literal de texto, como o `nCod` de `"... = :nCod"`.
fn sql_variables(literal: &str) -> impl Iterator<Item = &str> {
    literal.match_indices(':').filter_map(|(i, _)| {
        let rest = &literal[i + 1..];
        let len = rest
            .char_indices()
            .find(|&(_, c)| !(c.is_alphanumeric() || c == '_'))
            .map_or(rest.len(), |(n, _)| n);
        let name = &rest[..len];
        name.starts_with(|c: char| c.is_alphabetic() || c == '_')
            .then_some(name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tira_a_indentacao_comum_e_poe_quatro_espacos() {
        let snippet = "se (a) {\n        b = 1;\n    }";
        assert_eq!(
            reindent(snippet, 0, "    ", &[], "    ", "\n"),
            "    se (a) {\n        b = 1;\n    }"
        );
    }

    #[test]
    fn linha_em_branco_fica_vazia() {
        let snippet = "a = 1;\n\t\n\nb = 2;";
        assert_eq!(
            reindent(snippet, 0, "", &[], "    ", "\n"),
            "    a = 1;\n\n\n    b = 2;"
        );
    }

    #[test]
    fn quebra_de_linha_do_arquivo_e_sem_espaco_no_fim() {
        let snippet = "a = 1;   \r\n  b = 2;  \r\n  c = 3;";
        assert_eq!(
            reindent(snippet, 0, "  ", &[], "    ", "\r\n"),
            "    a = 1;\r\n    b = 2;\r\n    c = 3;"
        );
    }

    #[test]
    fn texto_entre_aspas_em_varias_linhas_fica_como_esta() {
        let snippet = "a = \"x  \n   y\";\nb = 1;";
        // A primeira quebra, no deslocamento 8, está dentro do texto.
        assert_eq!(
            reindent(snippet, 0, "", &[8], "    ", "\n"),
            "    a = \"x  \n   y\";\n    b = 1;"
        );
    }

    #[test]
    fn mistura_de_tabulacao_e_espacos_usa_so_o_prefixo_comum() {
        let snippet = "a = 1;\n\tb = 2;\n\t    c = 3;\n\td = 4;";
        // Sai só o `\t` que todas as linhas têm, e os espaços depois dele ficam.
        assert_eq!(
            reindent(snippet, 0, "\t", &[], "    ", "\n"),
            "    a = 1;\n    b = 2;\n        c = 3;\n    d = 4;"
        );
    }

    #[test]
    fn variaveis_do_sql() {
        let found: Vec<_> = sql_variables("WHERE A = :nCod AND H = '12:30' OR B = :_x1").collect();
        assert_eq!(found, ["nCod", "_x1"]);
    }
}
