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
