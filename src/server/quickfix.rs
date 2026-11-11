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
    /// A ação que define o nome no cursor, quando ele não tem `definir` em lugar nenhum.
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
        let range = tree.text_range(node);
        let name = &text[range];
        if !self.undefined(node, range, name) {
            return Vec::new();
        }
        let ty = type_of(tree, text, self.doc.context, name);
        let line = self.keyword_case(&format!("Definir {}", type_keyword(ty)));
        let line = format!("{line} {name};");
        let (at, insert) = self.insertion(range.start(), &line);
        let position = self.range(TextRange::empty(at)).start;
        let edit = TextEdit::new(lsp_types::Range::new(position, position), insert);
        #[allow(clippy::mutable_key_type)]
        let changes = HashMap::from([(self.uri.clone(), vec![edit])]);
        vec![CodeAction {
            title: line,
            kind: Some(CodeActionKind::QUICKFIX),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..WorkspaceEdit::default()
            }),
            is_preferred: Some(true),
            ..CodeAction::default()
        }]
    }

    /// O nome não tem definição no arquivo, nas outras regras nem no ERP.
    fn undefined(&self, node: NodeId, range: TextRange, name: &str) -> bool {
        let tree = &self.doc.tree;
        let inherited = &self.doc.inherited;
        let ident = tree.tokens().kind(tree.token_range(node).start) == SyntaxKind::Ident;
        // As variáveis que cada identificador acrescenta começam com VS ou VE, como `VSCodEmp`, e
        // o servidor não tem a lista delas.
        // Compara bytes, porque um nome pode começar com letra acentuada.
        let identifier_variable = self.doc.context == builtins::Context::Rule
            && name.len() > 2
            && name
                .as_bytes()
                .get(..2)
                .is_some_and(|p| p.eq_ignore_ascii_case(b"VS") || p.eq_ignore_ascii_case(b"VE"));
        ident
            && !identifier_variable
            && self.doc.symbols.reference_at(range).is_none()
            && builtins::system_variable(name, self.doc.context).is_none()
            && inherited.variable(name).is_none()
            && !inherited.is_entry(name)
    }

    /// As palavras reservadas com a caixa que o arquivo já usa, pelo primeiro `definir` dele.
    fn keyword_case(&self, words: &str) -> String {
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
    fn insertion(&self, use_at: TextSize, line: &str) -> (TextSize, String) {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let last = self
            .doc
            .symbols
            .definitions
            .iter()
            .filter(|d| tree.parent(d.node) == Some(tree.root()))
            .map(|d| tree.text_range(d.node).end())
            .filter(|&end| end <= use_at)
            .max();
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
            .filter_map(|c| self.fix(c))
            .collect()
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
