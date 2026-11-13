//! Ações de código de refatoração, fora o "Extrair função".
//!
//! Cada ação só é oferecida quando o resultado compila e faz o mesmo que o código de antes. Onde
//! isso não dá para garantir, como numa conta tirada de dentro da condição de um laço, a ação não
//! aparece.

use std::collections::HashMap;

use lsp_types::{CodeAction, CodeActionKind, TextEdit, WorkspaceEdit};
use text_size::{TextRange, TextSize};

use super::extract::{CONTAINERS, first_token_ending_after, is_statement};
use super::navigation::Context;
use super::quickfix::{indentation, newline};
use crate::symbols::{DefKind, VarType};
use crate::{NodeId, SyntaxKind};

impl Context<'_> {
    /// As refatorações que valem para o cursor ou a seleção. `wants` diz se o editor pediu o
    /// tipo da ação.
    pub(crate) fn refactor_actions(
        &self,
        range: lsp_types::Range,
        wants: impl Fn(&CodeActionKind) -> bool,
    ) -> Vec<CodeAction> {
        // Dentro do JSON de modelo, as edições teriam que ir para dentro do texto escapado.
        if self.embedding.is_some() {
            return Vec::new();
        }
        let (a, b) = (self.offset(range.start), self.offset(range.end));
        let selection = TextRange::new(a.min(b), a.max(b));
        let mut actions = Vec::new();
        if wants(&CodeActionKind::REFACTOR_EXTRACT) {
            actions.extend(self.extract_variable(selection));
        }
        if wants(&CodeActionKind::REFACTOR_INLINE) {
            actions.extend(self.inline_function(selection.start()));
        }
        if wants(&CodeActionKind::REFACTOR_REWRITE) {
            actions.extend(self.invert_if(selection.start()));
            actions.extend(self.wrap(selection, "Se"));
            actions.extend(self.wrap(selection, "Enquanto"));
            actions.extend(self.toggle_block(selection.start()));
        }
        actions
    }

    fn action(
        &self,
        title: &str,
        kind: CodeActionKind,
        edits: Vec<(TextRange, String)>,
    ) -> CodeAction {
        let edits = edits
            .into_iter()
            .map(|(range, new_text)| TextEdit::new(self.range(range), new_text))
            .collect();
        #[allow(clippy::mutable_key_type)]
        let changes = HashMap::from([(self.uri.clone(), edits)]);
        CodeAction {
            title: title.to_owned(),
            kind: Some(kind),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..WorkspaceEdit::default()
            }),
            ..CodeAction::default()
        }
    }

    /// Diz se o trecho não tem erro do lexer nem do parser. Com erro, a árvore do trecho pode não
    /// ser a que o texto parece ter.
    fn clean(&self, range: TextRange) -> bool {
        let tree = &self.doc.tree;
        let touches = |r: TextRange| r.start() <= range.end() && r.end() >= range.start();
        !tree.errors().iter().any(|e| touches(e.range))
            && !tree.tokens().errors().iter().any(|e| touches(e.range))
    }

    /// Nós que contêm `offset`, da raiz para o mais fundo.
    fn path_to(&self, offset: TextSize) -> Vec<NodeId> {
        let tree = &self.doc.tree;
        let mut path = vec![tree.root()];
        while let Some(child) = tree
            .children(*path.last().unwrap_or(&tree.root()))
            .find(|&c| tree.text_range(c).contains_inclusive(offset))
        {
            path.push(child);
        }
        path
    }

    /// Ação "Extrair variável". A expressão selecionada vai para uma variável nova, atribuída logo
    /// antes do comando.
    ///
    /// Só vale para conta e texto de tipo conhecido, que podem ir sozinhos numa atribuição. O
    /// comando tem que estar direto num bloco, porque a atribuição entra na linha de cima. Numa
    /// condição de `enquanto` ou de `para` a conta é refeita a cada volta, e fora dela seria feita
    /// uma vez só, então ali a ação não aparece.
    fn extract_variable(&self, selection: TextRange) -> Option<CodeAction> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let selected = &text[selection];
        let start =
            selection.start() + TextSize::of(selected) - TextSize::of(selected.trim_start());
        let end = start + TextSize::of(selected.trim());
        let selection = TextRange::new(start, end);
        if selection.is_empty() {
            return None;
        }
        // O nó mais fundo que cobre a seleção tem que ser exatamente ela.
        let mut node = tree.root();
        while let Some(child) = tree
            .children(node)
            .find(|&c| tree.text_range(c).contains_range(selection))
        {
            node = child;
        }
        if tree.text_range(node) != selection {
            return None;
        }
        let first = tree.tokens().kind(tree.token_range(node).start);
        let extractable = match tree.kind(node) {
            SyntaxKind::BinaryExpr | SyntaxKind::ParenExpr | SyntaxKind::PrefixExpr => true,
            SyntaxKind::Literal => matches!(first, SyntaxKind::String | SyntaxKind::Number),
            _ => false,
        };
        if !extractable {
            return None;
        }
        let mut statement = tree.parent(node)?;
        while !is_statement(tree.kind(statement)) {
            statement = tree.parent(statement)?;
        }
        let direct = tree.parent(node) == Some(statement);
        let allowed = match tree.kind(statement) {
            SyntaxKind::Assign | SyntaxKind::CallStmt | SyntaxKind::KeywordStmt => true,
            // O texto do `cur.SQL "..."` é conferido como SQL, e numa variável não seria.
            SyntaxKind::FieldCommand => !(direct && tree.kind(node) == SyntaxKind::Literal),
            // A condição do `se` é o primeiro filho. O corpo tem os comandos dele.
            SyntaxKind::If => true,
            _ => false,
        };
        let in_block = tree
            .parent(statement)
            .is_some_and(|p| CONTAINERS.contains(&tree.kind(p)));
        let statement_range = tree.text_range(statement);
        if !allowed || !in_block || !self.clean(statement_range) {
            return None;
        }
        let ty = crate::validate::expression_type(
            tree,
            &self.doc.symbols,
            text,
            self.doc.context,
            &self.doc.inherited,
            node,
        )?;
        let (base, ty) = match ty {
            VarType::Alfa => ("aNovaVariavel", "Alfa"),
            VarType::Numero | VarType::Data => ("nNovaVariavel", "Numero"),
        };
        let name = self.new_name(base);
        let nl = newline(text);
        let definition = format!("{} {name};", self.keyword_case(&format!("Definir {ty}")));
        let (at, insert) = self.insertion(statement_range.start(), &definition);
        let indent = indentation(text, statement_range.start());
        let assignment = format!("{name} = {};{nl}{indent}", &text[selection]);
        Some(self.action(
            "Extrair variável",
            CodeActionKind::REFACTOR_EXTRACT,
            vec![
                (TextRange::empty(at), insert),
                (TextRange::empty(statement_range.start()), assignment),
                (selection, name),
            ],
        ))
    }

    /// Ação "Embutir função". A chamada no cursor dá lugar aos comandos da função.
    ///
    /// É o inverso do "Extrair função", então vale para a função sem parâmetros, com o corpo num
    /// bloco só, e que não depende de estar numa função. O corpo não pode ter `definir`, rótulo,
    /// `vapara`, nem `pare` ou `continue` que passariam a valer para um laço de fora.
    fn inline_function(&self, offset: TextSize) -> Option<CodeAction> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let symbols = &self.doc.symbols;
        let path = self.path_to(offset);
        let (&callee, &call, &statement) = match path.as_slice() {
            [.., statement, call, callee] => (callee, call, statement),
            _ => return None,
        };
        if tree.kind(callee) != SyntaxKind::NameRef
            || tree.kind(call) != SyntaxKind::CallExpr
            || tree.kind(statement) != SyntaxKind::CallStmt
            || tree.children(call).next() != Some(callee)
        {
            return None;
        }
        let args = tree.children(call).nth(1)?;
        if tree.kind(args) != SyntaxKind::ArgList || tree.children(args).next().is_some() {
            return None;
        }
        let reference = symbols.reference_at(tree.text_range(callee))?;
        let mut implementations = reference
            .targets
            .iter()
            .map(|&t| symbols.definitions[t])
            .filter(|d| d.kind == DefKind::Function && tree.kind(d.node) == SyntaxKind::FuncImpl);
        let function = implementations.next()?.node;
        if implementations.next().is_some() {
            return None;
        }
        // O corpo é o bloco. Com comandos depois dele, que ainda são da função, não é só ele.
        let parts: Vec<NodeId> = tree.children(function).collect();
        let [name, params, block] = parts.as_slice() else {
            return None;
        };
        if tree.kind(*name) != SyntaxKind::Name
            || tree.kind(*params) != SyntaxKind::ParamList
            || tree.children(*params).next().is_some()
            || tree.kind(*block) != SyntaxKind::Block
        {
            return None;
        }
        let statement_range = tree.text_range(statement);
        if !self.clean(tree.text_range(function)) || !self.clean(statement_range) {
            return None;
        }
        let mut stack: Vec<NodeId> = tree.children(*block).collect();
        while let Some(node) = stack.pop() {
            stack.extend(tree.children(node));
            match tree.kind(node) {
                SyntaxKind::VarDef
                | SyntaxKind::CursorDef
                | SyntaxKind::ListDef
                | SyntaxKind::TableDef
                | SyntaxKind::WsDef
                | SyntaxKind::FuncDecl
                | SyntaxKind::FuncImpl
                | SyntaxKind::ErrorNode
                | SyntaxKind::Label
                | SyntaxKind::Goto => return None,
                SyntaxKind::Break | SyntaxKind::Continue if !self.in_loop(node, *block) => {
                    return None;
                }
                _ => {}
            }
        }

        // O que há entre a abertura e o fechamento do bloco, com os comentários.
        let (open, close) = self.block_delimiters(*block)?;
        let inner = TextRange::new(open.end(), close.start());
        let inside = &text[inner];
        let body_start = inner.start() + TextSize::of(inside) - TextSize::of(inside.trim_start());
        let body = TextRange::new(body_start, body_start + TextSize::of(inside.trim()));

        let nl = newline(text);
        let base = indentation(text, statement_range.start());
        let in_block = tree
            .parent(statement)
            .is_some_and(|p| CONTAINERS.contains(&tree.kind(p)));
        let replacement = if in_block {
            let lines = self.reindented(body, base, nl);
            // A primeira linha fica onde a chamada estava, que já tem a indentação.
            lines.strip_prefix(base).unwrap_or(&lines).to_owned()
        } else if body.is_empty() {
            "{ }".to_owned()
        } else {
            // A chamada é o comando sem bloco de um `se` ou de um laço, onde só cabe um comando.
            let lines = self.reindented(body, &format!("{base}    "), nl);
            format!("{{{nl}{lines}{nl}{base}}}")
        };
        Some(self.action(
            "Embutir função",
            CodeActionKind::REFACTOR_INLINE,
            vec![(statement_range, replacement)],
        ))
    }

    /// Trechos da abertura e do fechamento de um bloco, sem o `;` opcional depois do fim.
    fn block_delimiters(&self, block: NodeId) -> Option<(TextRange, TextRange)> {
        let tree = &self.doc.tree;
        let tokens = tree.tokens();
        let range = tree.token_range(block);
        let open = range.start;
        if !matches!(tokens.kind(open), SyntaxKind::LBrace | SyntaxKind::InicioKw) {
            return None;
        }
        let close = (open + 1..range.end)
            .rev()
            .find(|&t| !tokens.kind(t).is_trivia() && tokens.kind(t) != SyntaxKind::Semicolon)?;
        matches!(tokens.kind(close), SyntaxKind::RBrace | SyntaxKind::FimKw)
            .then(|| (tokens.range(open), tokens.range(close)))
    }

    /// Ação "Inverter o se". A condição passa a ser a contrária e os dois lados trocam de lugar.
    ///
    /// A linguagem não tem negação, então a condição contrária sai trocando cada comparação pela
    /// oposta e cada `e` por `ou`. Só vale quando a condição inteira é feita disso e quando há
    /// `senao`. O lado do `senao` passa para logo depois da condição, e ali um `se` sem bloco
    /// pegaria o `senao` para ele, então esse lado tem que ser um bloco ou um comando simples.
    fn invert_if(&self, offset: TextSize) -> Option<CodeAction> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let path = self.path_to(offset);
        let &node = path
            .iter()
            .rev()
            .find(|&&n| tree.kind(n) == SyntaxKind::If)?;
        let range = tree.text_range(node);
        if !self.clean(range) {
            return None;
        }
        let mut children = tree.children(node);
        let (condition, then, branch) = (children.next()?, children.next()?, children.next()?);
        if tree.kind(branch) != SyntaxKind::ElseBranch || is_statement(tree.kind(condition)) {
            return None;
        }
        // O cursor fica na palavra `se` ou na condição, não nos comandos de dentro.
        if offset >= tree.text_range(then).start() {
            return None;
        }
        let otherwise = tree.children(branch).next()?;
        let simple = matches!(
            tree.kind(otherwise),
            SyntaxKind::Block
                | SyntaxKind::Assign
                | SyntaxKind::CallStmt
                | SyntaxKind::FieldCommand
                | SyntaxKind::KeywordStmt
                | SyntaxKind::Goto
                | SyntaxKind::Break
                | SyntaxKind::Continue
        );
        if !simple {
            return None;
        }
        let mut edits = Vec::new();
        self.negate(condition, &mut edits)?;
        let (then_range, otherwise_range) = (tree.text_range(then), tree.text_range(otherwise));
        edits.push((then_range, text[otherwise_range].to_owned()));
        edits.push((otherwise_range, text[then_range].to_owned()));
        Some(self.action("Inverter o se", CodeActionKind::REFACTOR_REWRITE, edits))
    }

    /// Edições que trocam a condição pela contrária, ou `None` se alguma parte dela não tem
    /// contrária conhecida, como uma variável sozinha.
    fn negate(&self, node: NodeId, edits: &mut Vec<(TextRange, String)>) -> Option<()> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let tokens = tree.tokens();
        match tree.kind(node) {
            SyntaxKind::ParenExpr => self.negate(tree.children(node).next()?, edits),
            SyntaxKind::BinaryExpr => {
                let mut children = tree.children(node);
                let (left, right) = (children.next()?, children.next()?);
                let op = (tree.token_range(left).end..tree.token_range(right).start)
                    .find(|&t| !tokens.kind(t).is_trivia())?;
                let range = tokens.range(op);
                let upper = text[range].chars().all(|c| !c.is_ascii_lowercase());
                let word = |lower: &str| match upper {
                    true => lower.to_uppercase(),
                    false => lower.to_owned(),
                };
                let opposite = match tokens.kind(op) {
                    SyntaxKind::Eq => "<>".to_owned(),
                    SyntaxKind::Neq => "=".to_owned(),
                    SyntaxKind::Lt => ">=".to_owned(),
                    SyntaxKind::Gt => "<=".to_owned(),
                    SyntaxKind::LtEq => ">".to_owned(),
                    SyntaxKind::GtEq => "<".to_owned(),
                    SyntaxKind::EKw => word("ou"),
                    SyntaxKind::OuKw => word("e"),
                    _ => return None,
                };
                edits.push((range, opposite));
                if !matches!(tokens.kind(op), SyntaxKind::EKw | SyntaxKind::OuKw) {
                    return Some(());
                }
                for side in [left, right] {
                    // O `e` vale antes do `ou`. Um lado com `e` vira `ou` e, debaixo do `e` que
                    // este `ou` vira, precisa de parênteses para continuar junto.
                    let becomes_or = tokens.kind(op) == SyntaxKind::OuKw
                        && tree.kind(side) == SyntaxKind::BinaryExpr
                        && self.operator(side) == Some(SyntaxKind::EKw);
                    if becomes_or {
                        let range = tree.text_range(side);
                        edits.push((TextRange::empty(range.start()), "(".to_owned()));
                    }
                    self.negate(side, edits)?;
                    if becomes_or {
                        let range = tree.text_range(side);
                        edits.push((TextRange::empty(range.end()), ")".to_owned()));
                    }
                }
                Some(())
            }
            // `cur.Achou` e `cur.NaoAchou` são o contrário um do outro.
            SyntaxKind::FieldExpr => {
                let member = tree.children(node).nth(1)?;
                let range = tree.text_range(member);
                let member = &text[range];
                let opposite = match member {
                    m if m.eq_ignore_ascii_case("Achou") => "NaoAchou",
                    m if m.eq_ignore_ascii_case("NaoAchou") => "Achou",
                    _ => return None,
                };
                // Na caixa em que o nome está escrito.
                let opposite = if !member.contains(|c: char| c.is_ascii_lowercase()) {
                    opposite.to_uppercase()
                } else if !member.contains(|c: char| c.is_ascii_uppercase()) {
                    opposite.to_lowercase()
                } else {
                    opposite.to_owned()
                };
                edits.push((range, opposite));
                Some(())
            }
            _ => None,
        }
    }

    /// Operador de uma expressão de dois lados.
    fn operator(&self, node: NodeId) -> Option<SyntaxKind> {
        let tree = &self.doc.tree;
        let tokens = tree.tokens();
        let mut children = tree.children(node);
        let (left, right) = (children.next()?, children.next()?);
        (tree.token_range(left).end..tree.token_range(right).start)
            .map(|t| tokens.kind(t))
            .find(|k| !k.is_trivia())
    }

    /// Ações "Envolver com Se" e "Envolver com Enquanto". Os comandos selecionados vão para dentro
    /// de um bloco, com a condição vazia para preencher.
    ///
    /// A seleção cobre comandos inteiros e contíguos do mesmo bloco, como no "Extrair função". Um
    /// rótulo que um `vapara` de fora alcança ficaria dentro do bloco, e no `enquanto` um `pare`
    /// ou `continue` de um laço de fora passaria a valer para o laço novo, então nesses casos a
    /// ação não aparece.
    fn wrap(&self, selection: TextRange, keyword: &str) -> Option<CodeAction> {
        if selection.is_empty() {
            return None;
        }
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let (container, statements) = self.selected_statements(selection)?;
        let span = TextRange::new(
            tree.text_range(*statements.first()?).start(),
            tree.text_range(*statements.last()?).end(),
        );
        let mut has_label = false;
        let mut stack = statements.clone();
        while let Some(node) = stack.pop() {
            stack.extend(tree.children(node));
            match tree.kind(node) {
                SyntaxKind::Label => has_label = true,
                SyntaxKind::ErrorNode => return None,
                SyntaxKind::Break | SyntaxKind::Continue
                    if keyword == "Enquanto" && !self.in_loop(node, container) =>
                {
                    return None;
                }
                _ => {}
            }
        }
        if has_label && self.label_used_outside(span) {
            return None;
        }
        let nl = newline(text);
        let base = indentation(text, span.start());
        let body = self.reindented(span, &format!("{base}    "), nl);
        let head = self.keyword_case(keyword);
        Some(self.action(
            &format!("Envolver com {keyword}"),
            CodeActionKind::REFACTOR_REWRITE,
            vec![(span, format!("{head} () {{{nl}{body}{nl}{base}}}"))],
        ))
    }

    /// "Trocar chaves por Inicio e Fim" e o contrário, no bloco cuja abertura ou fechamento está
    /// no cursor. Para o compilador as duas formas são o mesmo token.
    fn toggle_block(&self, offset: TextSize) -> Option<CodeAction> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let (open, close) = self
            .path_to(offset)
            .into_iter()
            .rev()
            .filter(|&n| tree.kind(n) == SyntaxKind::Block)
            .filter_map(|n| self.block_delimiters(n))
            .find(|(open, close)| {
                open.contains_inclusive(offset) || close.contains_inclusive(offset)
            })?;
        if !self.clean(open) || !self.clean(close) {
            return None;
        }
        let token = first_token_ending_after(tree, open.start());
        let braces = tree.tokens().kind(token) == SyntaxKind::LBrace;
        // Uma palavra colada no que vem depois viraria outro nome, e uma chave colada numa palavra
        // é caractere inválido para o compilador.
        let word = |word: &str, range: TextRange| {
            let next = text[usize::from(range.end())..].chars().next();
            let glued = next.is_some_and(|c| !c.is_whitespace() && c != ';');
            let space = if glued { " " } else { "" };
            format!("{}{space}", self.keyword_case(word))
        };
        let (title, new_open, new_close) = if braces {
            (
                "Trocar chaves por Inicio e Fim",
                word("Inicio", open),
                word("Fim", close),
            )
        } else {
            (
                "Trocar Inicio e Fim por chaves",
                "{".to_owned(),
                "}".to_owned(),
            )
        };
        Some(self.action(
            title,
            CodeActionKind::REFACTOR_REWRITE,
            vec![(open, new_open), (close, new_close)],
        ))
    }
}
