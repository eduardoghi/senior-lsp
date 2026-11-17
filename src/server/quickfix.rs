//! Correção rápida que cria o `definir` de um nome usado sem definição.
//!
//! O compilador aceita o nome sem `definir` e cria uma variável numérica, o que esconde erro de
//! digitação e dá erro de tipo quando o valor é texto. A correção escreve a linha que faltava.

use std::collections::HashMap;

use lsp_types::{CodeAction, CodeActionKind, Position, TextEdit, WorkspaceEdit};
use text_size::{TextRange, TextSize};

use super::navigation::{Context, name_at};
use crate::builtins;
use crate::symbols::VarType;
use crate::{NodeId, SyntaxKind, Tree};

impl Context<'_> {
    /// A ação que define o nome no cursor, quando ele não tem `definir` em lugar nenhum, e a que
    /// define de uma vez todos os nomes assim do arquivo.
    pub(crate) fn define_actions(&self, position: Position) -> Vec<CodeAction> {
        // Dentro do JSON de modelo, a linha nova teria que ir para dentro do texto escapado.
        if self.embedding.is_some() {
            return Vec::new();
        }
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let offset = self.offset(position);
        let Some((node, false)) = name_at(tree, offset) else {
            return Vec::new();
        };
        let name = &text[tree.text_range(node)];
        let implicit = super::inlay::implicit_names(self.doc);
        let Some(&first) = implicit
            .iter()
            .find(|r| text[**r].eq_ignore_ascii_case(name))
        else {
            return Vec::new();
        };
        let mut actions = vec![self.define_action(&[first], None)];
        if implicit.len() > 1 {
            let title = format!("Definir os {} nomes sem definir", implicit.len());
            actions.push(self.define_action(&implicit, Some(title)));
        }
        actions
    }

    /// Ação que escreve o `definir` de cada nome, com o tipo pelo uso, antes do primeiro deles.
    /// `names` é o primeiro uso de cada um, em ordem de texto.
    fn define_action(&self, names: &[TextRange], title: Option<String>) -> CodeAction {
        let text = &self.doc.text;
        let newline = newline(text);
        let lines: Vec<String> = names
            .iter()
            .map(|&range| {
                let name = &text[range];
                let ty = type_of(&self.doc.tree, text, self.doc.context, name);
                let keyword = self.keyword_case(&format!("Definir {}", type_keyword(ty)));
                format!("{keyword} {name};")
            })
            .collect();
        let (at, insert) = self.insertion(names[0].start(), &lines.join(newline));
        let position = self.range(TextRange::empty(at)).start;
        let edit = TextEdit::new(lsp_types::Range::new(position, position), insert);
        #[allow(clippy::mutable_key_type)]
        let changes = HashMap::from([(self.uri.clone(), vec![edit])]);
        let single = title.is_none();
        CodeAction {
            title: title.unwrap_or_else(|| lines[0].clone()),
            kind: Some(CodeActionKind::QUICKFIX),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..WorkspaceEdit::default()
            }),
            is_preferred: Some(single),
            ..CodeAction::default()
        }
    }

    /// As palavras reservadas com a caixa que o arquivo já usa, pelo primeiro `definir` dele.
    pub(super) fn keyword_case(&self, words: &str) -> String {
        let text = &self.doc.text;
        let first = self.doc.symbols.definitions.iter().find_map(|d| {
            let start = usize::from(self.doc.tree.text_range(d.node).start());
            let word = text.get(start..start + 7)?;
            word.eq_ignore_ascii_case("definir").then_some(word)
        });
        match first {
            Some("definir") => words.to_lowercase(),
            Some("DEFINIR") => words.to_uppercase(),
            _ => words.to_owned(),
        }
    }

    /// Onde a linha entra, logo depois do último `definir` do nível de cima antes do uso, ou no
    /// começo do arquivo, e o texto com a quebra de linha do arquivo.
    pub(super) fn insertion(&self, use_at: TextSize, line: &str) -> (TextSize, String) {
        let tree = &self.doc.tree;
        let last = self
            .doc
            .symbols
            .definitions
            .iter()
            .filter(|d| tree.parent(d.node) == Some(tree.root()))
            .map(|d| tree.text_range(d.node).end())
            .filter(|&end| end <= use_at)
            .max();
        self.insert_after(last, line)
    }

    /// Onde a linha entra para ficar na linha de baixo de `last`, ou no começo do arquivo sem
    /// `last`, e o texto com a quebra de linha do arquivo.
    pub(super) fn insert_after(&self, last: Option<TextSize>, line: &str) -> (TextSize, String) {
        let text = &self.doc.text;
        let newline = newline(text);
        let Some(end) = last else {
            return (TextSize::from(0), format!("{line}{newline}"));
        };
        match text[usize::from(end)..].find('\n') {
            Some(i) => (
                end + TextSize::from(i as u32 + 1),
                format!("{line}{newline}"),
            ),
            None => (TextSize::of(text.as_str()), format!("{newline}{line}")),
        }
    }
}

fn type_keyword(ty: VarType) -> &'static str {
    match ty {
        VarType::Alfa => "Alfa",
        VarType::Numero => "Numero",
        VarType::Data => "Data",
    }
}

/// Tipo pelo uso do nome no arquivo. Primeiro uma atribuição com texto do lado direito, depois um
/// parâmetro de função do ERP que recebe o nome, depois a letra inicial que as regras costumam
/// usar (`aTexto`, `nValor`, `dData`). Sem nada disso, número, que é o que o compilador cria.
fn type_of(tree: &Tree, text: &str, context: builtins::Context, name: &str) -> VarType {
    let mut from_call = None;
    let mut stack = vec![tree.root()];
    while let Some(node) = stack.pop() {
        stack.extend(tree.children(node));
        match tree.kind(node) {
            SyntaxKind::Assign => {
                let mut children = tree.children(node);
                let (Some(lhs), Some(rhs)) = (children.next(), children.last()) else {
                    continue;
                };
                if tree.kind(lhs) == SyntaxKind::NameRef
                    && text[tree.text_range(lhs)].eq_ignore_ascii_case(name)
                    && has_string(tree, rhs)
                {
                    return VarType::Alfa;
                }
            }
            SyntaxKind::CallExpr if from_call.is_none() => {
                from_call = argument_type(tree, text, context, node, name);
            }
            _ => {}
        }
    }
    if let Some(ty) = from_call {
        return ty;
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some('a'), Some(c)) if c.is_ascii_uppercase() => VarType::Alfa,
        (Some('d'), Some(c)) if c.is_ascii_uppercase() => VarType::Data,
        _ => VarType::Numero,
    }
}

/// O lado direito tem um texto entre aspas, como `"x"` ou `aA + "x"`.
fn has_string(tree: &Tree, node: NodeId) -> bool {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if tree.kind(n) == SyntaxKind::Literal
            && tree.tokens().kind(tree.token_range(n).start) == SyntaxKind::String
        {
            return true;
        }
        // O texto passado a uma função não diz nada sobre o resultado dela.
        if tree.kind(n) != SyntaxKind::CallExpr {
            stack.extend(tree.children(n));
        }
    }
    false
}

/// Tipo do parâmetro de função do ERP que recebe o nome como argumento.
fn argument_type(
    tree: &Tree,
    text: &str,
    context: builtins::Context,
    call: NodeId,
    name: &str,
) -> Option<VarType> {
    let mut children = tree.children(call);
    let callee = children.next()?;
    let args = children.find(|&c| tree.kind(c) == SyntaxKind::ArgList)?;
    let builtin = builtins::lookup(&text[tree.text_range(callee)], context)?;
    tree.children(args).enumerate().find_map(|(i, arg)| {
        (tree.kind(arg) == SyntaxKind::NameRef
            && text[tree.text_range(arg)].eq_ignore_ascii_case(name))
        .then(|| builtin.params.get(i).map(|p| p.ty))
        .flatten()
    })
}

impl Context<'_> {
    /// Correções dos avisos que tocam o trecho pedido pelo editor.
    pub(crate) fn check_fixes(&self, range: lsp_types::Range) -> Vec<CodeAction> {
        if self.embedding.is_some() {
            return Vec::new();
        }
        let doc = self.doc;
        let start = self.offset(range.start);
        let end = self.offset(range.end);
        let wanted = TextRange::new(start.min(end), end.max(start));
        doc.checks()
            .iter()
            .filter(|c| c.range.intersect(wanted).is_some() || c.range.contains_inclusive(start))
            .flat_map(|c| [self.fix(c), self.replace_with_suggestion(c)])
            .flatten()
            .collect()
    }

    /// Troca o nome que não existe pelo mais parecido, quando há um.
    fn replace_with_suggestion(&self, check: &crate::validate::Check) -> Option<CodeAction> {
        let (range, name) = super::suggest::suggestion(check, self.doc)?;
        let edit = TextEdit::new(self.range(range), name.clone());
        #[allow(clippy::mutable_key_type)]
        let changes = HashMap::from([(self.uri.clone(), vec![edit])]);
        Some(CodeAction {
            title: format!("Trocar por {name}"),
            kind: Some(CodeActionKind::QUICKFIX),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..WorkspaceEdit::default()
            }),
            ..CodeAction::default()
        })
    }

    fn fix(&self, check: &crate::validate::Check) -> Option<CodeAction> {
        use crate::validate::CheckKind;
        let text = &self.doc.text;
        let (title, edits) = match check.kind {
            CheckKind::LowercaseChar => {
                let upper = text[check.range].to_ascii_uppercase();
                (format!("Trocar por {upper}"), vec![(check.range, upper)])
            }
            CheckKind::ComparisonInAssignment => {
                let rhs = self.comparison_rhs(check.range)?;
                (
                    "Pôr a comparação entre parênteses".to_owned(),
                    vec![
                        (TextRange::empty(rhs.start()), "(".to_owned()),
                        (TextRange::empty(rhs.end()), ")".to_owned()),
                    ],
                )
            }
            CheckKind::Unused => {
                let name = check.label.clone().unwrap_or_default();
                (
                    format!("Apagar a definição de {name}"),
                    vec![(self.definition_statement(check.range)?, String::new())],
                )
            }
            CheckKind::NativeSqlWithScope => {
                let (statement, object) = self.native_sql_call(check.range)?;
                let indent = indentation(text, statement.start());
                let line = format!("SQL_UsarAbrangencia({object}, 0);");
                (
                    format!("Acrescentar {line}"),
                    vec![(
                        TextRange::empty(statement.start()),
                        format!("{line}{}{indent}", newline(text)),
                    )],
                )
            }
            CheckKind::UnknownFunction => self.declare_function(check.range)?,
            CheckKind::CursorColumnUnused => (
                format!("Tirar {} do SELECT", &text[check.range]),
                vec![(self.column_with_comma(check.range)?, String::new())],
            ),
            CheckKind::CursorStarUnused => {
                // O fim do rótulo traz os campos que a regra lê.
                let used = check.label.as_deref()?.splitn(4, '|').nth(3)?;
                if used.is_empty() {
                    return None;
                }
                (
                    format!("Trocar o * por {used}"),
                    vec![(check.range, used.to_owned())],
                )
            }
            _ => return None,
        };
        let edits = edits
            .into_iter()
            .map(|(range, new_text)| TextEdit::new(self.range(range), new_text))
            .collect();
        #[allow(clippy::mutable_key_type)]
        let changes = HashMap::from([(self.uri.clone(), edits)]);
        Some(CodeAction {
            title,
            kind: Some(CodeActionKind::QUICKFIX),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..WorkspaceEdit::default()
            }),
            ..CodeAction::default()
        })
    }

    /// Uma ação só com as correções que não mudam o que a regra faz: o apóstrofo minúsculo vira
    /// maiúsculo (SEN064), que é como o compilador já o lê, sai o `definir` sem uso (SEN068) e
    /// sai a coluna do SELECT que a regra não lê (SEN076). O `*` fica, porque trocá-lo pelos
    /// campos lidos depende do dicionário estar em dia. Correções que caem no mesmo trecho, como
    /// duas colunas vizinhas que levam a mesma vírgula, ficam para a próxima vez.
    ///
    /// Só entram os avisos que o editor mostra (`shown`), depois das diretivas e dos níveis do
    /// `senior-lsp.json`. Um aviso escondido é uma escolha da pessoa, como um `definir` que o ERP
    /// lê por fora, e o "corrigir tudo" ao salvar não pode desfazer essa escolha.
    pub(crate) fn fix_all(&self, shown: &[lsp_types::Diagnostic]) -> Option<CodeAction> {
        use crate::validate::CheckKind;
        if self.embedding.is_some() {
            return None;
        }
        let text = &self.doc.text;
        let visible = |code: &str, range: TextRange| {
            let range = self.range(range);
            shown.iter().any(|d| {
                d.range == range
                    && matches!(&d.code, Some(lsp_types::NumberOrString::String(c)) if c == code)
            })
        };
        let mut edits: Vec<(TextRange, String)> = self
            .doc
            .checks()
            .iter()
            .filter_map(|check| match check.kind {
                CheckKind::LowercaseChar if visible("SEN064", check.range) => {
                    Some((check.range, text[check.range].to_ascii_uppercase()))
                }
                CheckKind::Unused if visible("SEN068", check.range) => {
                    Some((self.definition_statement(check.range)?, String::new()))
                }
                CheckKind::CursorColumnUnused if visible("SEN076", check.range) => {
                    Some((self.column_with_comma(check.range)?, String::new()))
                }
                _ => None,
            })
            .collect();
        edits.sort_by_key(|(range, _)| range.start());
        let mut end = TextSize::new(0);
        edits.retain(|(range, _)| {
            let keep = range.start() >= end;
            if keep {
                end = range.end();
            }
            keep
        });
        if edits.is_empty() {
            return None;
        }
        let edits = edits
            .into_iter()
            .map(|(range, new_text)| TextEdit::new(self.range(range), new_text))
            .collect();
        #[allow(clippy::mutable_key_type)]
        let changes = HashMap::from([(self.uri.clone(), edits)]);
        Some(CodeAction {
            title: "Corrigir o arquivo (apóstrofos, definir sem uso e colunas não lidas)"
                .to_owned(),
            kind: Some(CodeActionKind::SOURCE_FIX_ALL),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..WorkspaceEdit::default()
            }),
            ..CodeAction::default()
        })
    }

    /// Trecho da coluna do SELECT com a vírgula que a separa da vizinha. Sai a vírgula de depois,
    /// com o espaço e a emenda de linha que a seguem, ou a de antes quando a coluna é a última.
    fn column_with_comma(&self, column: TextRange) -> Option<TextRange> {
        let bytes = self.doc.text.as_bytes();
        let blank = |at: usize| matches!(bytes.get(at), Some(b' ' | b'\t'));
        let mut end = usize::from(column.end());
        while blank(end) {
            end += 1;
        }
        if bytes.get(end) == Some(&b',') {
            end += 1;
            while blank(end) {
                end += 1;
            }
            // A barra no fim da linha emenda a seguinte, que perde o recuo junto.
            if bytes.get(end) == Some(&b'\\') {
                let mut line = end + 1;
                while blank(line) || bytes.get(line) == Some(&b'\r') {
                    line += 1;
                }
                if bytes.get(line) == Some(&b'\n') {
                    end = line + 1;
                    while blank(end) {
                        end += 1;
                    }
                }
            }
            return Some(TextRange::new(column.start(), TextSize::new(end as u32)));
        }
        let mut start = usize::from(column.start());
        while start > 0 && matches!(bytes[start - 1], b' ' | b'\t' | b'\r' | b'\n' | b'\\') {
            start -= 1;
        }
        (start > 0 && bytes[start - 1] == b',')
            .then(|| TextRange::new(TextSize::new(start as u32 - 1), column.end()))
    }

    /// Lado direito da atribuição cujo operador de comparação está em `operator`.
    fn comparison_rhs(&self, operator: TextRange) -> Option<TextRange> {
        let tree = &self.doc.tree;
        let mut stack = vec![tree.root()];
        while let Some(node) = stack.pop() {
            stack.extend(tree.children(node));
            if tree.kind(node) != SyntaxKind::Assign {
                continue;
            }
            let rhs = tree.children(node).nth(1)?;
            let range = tree.text_range(rhs);
            if tree.kind(rhs) == SyntaxKind::BinaryExpr && range.contains_range(operator) {
                return Some(range);
            }
        }
        None
    }

    /// Trecho a apagar para tirar a definição do nome em `name`, com a linha inteira quando ela
    /// só tem essa definição.
    fn definition_statement(&self, name: TextRange) -> Option<TextRange> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let def = self
            .doc
            .symbols
            .definitions
            .iter()
            .find(|d| d.name == name)?;
        let node = tree.text_range(def.node);
        let mut end = usize::from(node.end());
        if text[end..].starts_with(';') {
            end += 1;
        }
        let start = usize::from(node.start());
        let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
        let alone =
            text[line_start..start].trim().is_empty() && text[end..line_end].trim().is_empty();
        let (start, end) = if alone {
            (line_start, line_end)
        } else {
            let spaces = text[end..].len() - text[end..].trim_start_matches([' ', '\t']).len();
            (start, end + spaces)
        };
        Some(TextRange::new(
            TextSize::from(start as u32),
            TextSize::from(end as u32),
        ))
    }

    /// Comando `SQL_UsarSQLSenior2(obj, 0);` com o nome da função em `callee`, e o `obj` como
    /// está no texto.
    fn native_sql_call(&self, callee: TextRange) -> Option<(TextRange, String)> {
        let tree = &self.doc.tree;
        let mut stack = vec![tree.root()];
        while let Some(node) = stack.pop() {
            stack.extend(tree.children(node));
            if tree.kind(node) != SyntaxKind::CallStmt {
                continue;
            }
            let call = tree.children(node).next()?;
            let mut parts = tree.children(call);
            let (Some(name), Some(args)) = (parts.next(), parts.next()) else {
                continue;
            };
            if tree.text_range(name) != callee {
                continue;
            }
            let object = tree.children(args).next()?;
            return Some((
                tree.text_range(node),
                self.doc.text[tree.text_range(object)].to_owned(),
            ));
        }
        None
    }

    /// Cria o `Definir Funcao` junto dos outros `definir` e a `funcao` vazia no fim do arquivo,
    /// com um parâmetro para cada argumento da chamada.
    fn declare_function(&self, callee: TextRange) -> Option<(String, Vec<(TextRange, String)>)> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let mut stack = vec![tree.root()];
        let mut args = None;
        while let Some(node) = stack.pop() {
            stack.extend(tree.children(node));
            if tree.kind(node) == SyntaxKind::CallExpr {
                let mut parts = tree.children(node);
                if parts.next().map(|n| tree.text_range(n)) == Some(callee) {
                    args = parts.next();
                    break;
                }
            }
        }
        let name = &text[callee];
        let mut params = Vec::new();
        let mut used = std::collections::HashSet::new();
        for (i, arg) in args.into_iter().flat_map(|a| tree.children(a)).enumerate() {
            let ty = self.argument_type(arg);
            let candidate = match tree.kind(arg) {
                SyntaxKind::NameRef => text[tree.text_range(arg)].to_owned(),
                _ => format!("Param{}", i + 1),
            };
            let mut param = candidate.clone();
            let mut n = 2;
            while !used.insert(param.to_ascii_uppercase()) {
                param = format!("{candidate}{n}");
                n += 1;
            }
            params.push(format!("{} {param}", self.keyword_case(type_keyword(ty))));
        }
        let signature = format!("{name}({})", params.join(", "));
        let declaration = format!("{} {signature};", self.keyword_case("Definir Funcao"));
        let (at, insert) = self.insertion(callee.start(), &declaration);
        let nl = newline(text);
        let body = format!(
            "{nl}{} {signature}; {{{nl}}}{nl}",
            self.keyword_case("Funcao")
        );
        let end = TextSize::of(text.as_str());
        let body = if text.ends_with('\n') {
            body
        } else {
            format!("{nl}{body}")
        };
        Some((
            format!("Declarar a função {name}"),
            vec![
                (TextRange::empty(at), insert),
                (TextRange::empty(end), body),
            ],
        ))
    }

    /// Tipo do argumento, pelo literal ou pela definição do nome. Número quando não dá para
    /// saber, que é o que o compilador assume.
    fn argument_type(&self, arg: NodeId) -> VarType {
        let tree = &self.doc.tree;
        match tree.kind(arg) {
            SyntaxKind::Literal
                if tree.tokens().kind(tree.token_range(arg).start) == SyntaxKind::String =>
            {
                VarType::Alfa
            }
            SyntaxKind::NameRef => self
                .doc
                .symbols
                .reference_at(tree.text_range(arg))
                .and_then(|r| r.targets.first())
                .and_then(|&t| match self.doc.symbols.definitions[t].kind {
                    crate::symbols::DefKind::Variable(ty)
                    | crate::symbols::DefKind::Parameter(ty) => ty,
                    _ => None,
                })
                .unwrap_or(VarType::Numero),
            _ => VarType::Numero,
        }
    }
}

pub(super) fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Espaços e tabulações entre o começo da linha e `at`.
pub(super) fn indentation(text: &str, at: TextSize) -> &str {
    let at = usize::from(at);
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let indent = &text[line_start..at];
    if indent.chars().all(|c| c == ' ' || c == '\t') {
        indent
    } else {
        ""
    }
}
