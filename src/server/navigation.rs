//! Pedidos de navegação do editor: ir para a definição, referências, hover e a lista de símbolos.

use lsp_types::{
    CompletionItem, CompletionItemKind, DocumentSymbol, Hover, HoverContents, Location,
    MarkupContent, MarkupKind, Position, Range, SymbolKind, Uri,
};
use text_size::{TextRange, TextSize};

use super::documents::Document;
use std::fs;
use std::path::Path;

use crate::builtins;
use crate::form::InheritedDef;
use crate::line_index::{ColumnEncoding, LineCol};
use crate::symbols::{DefKind, VarType};
use crate::{NodeId, SyntaxKind};

pub(crate) struct Context<'a> {
    pub(crate) doc: &'a Document,
    pub(crate) uri: &'a Uri,
    pub(crate) encoding: ColumnEncoding,
}

impl Context<'_> {
    fn offset(&self, position: Position) -> TextSize {
        let lc = LineCol {
            line: position.line,
            col: position.character,
        };
        self.doc.index.offset(&self.doc.text, lc, self.encoding)
    }

    fn range(&self, range: TextRange) -> Range {
        let p = |offset| {
            let lc = self
                .doc
                .index
                .line_col(&self.doc.text, offset, self.encoding);
            Position::new(lc.line, lc.col)
        };
        Range::new(p(range.start()), p(range.end()))
    }

    fn location(&self, range: TextRange) -> Location {
        Location::new(self.uri.clone(), self.range(range))
    }

    pub(crate) fn definition(&self, position: Position) -> Vec<Location> {
        let offset = self.offset(position);
        let symbols = &self.doc.symbols;
        let Some(targets) = symbols.at(offset) else {
            return self.inherited_definition(offset).into_iter().collect();
        };
        targets
            .into_iter()
            .map(|i| self.location(symbols.definitions[i].name))
            .collect()
    }

    /// Lugar de uma função ou variável usada aqui e definida em outra regra do formulário SGI.
    fn inherited_definition(&self, offset: TextSize) -> Option<Location> {
        let (_, def) = self.inherited_at(offset)?;
        self.location_in(&def.path, &def.text, def.name)
    }

    /// Nome na posição que vem de uma regra acima no formulário, com o trecho dele aqui.
    fn inherited_at(&self, offset: TextSize) -> Option<(TextRange, &InheritedDef)> {
        let (node, is_callee) = name_at(&self.doc.tree, offset)?;
        let range = self.doc.tree.text_range(node);
        let name = &self.doc.text[range];
        let def = if is_callee {
            self.doc.inherited.get(name)
        } else {
            self.doc.inherited.variable(name)
        }?;
        Some((range, def))
    }

    fn location_in(&self, path: &Path, text: &str, range: TextRange) -> Option<Location> {
        let uri = crate::form::uri_from_path(path).parse().ok()?;
        Some(Location::new(uri, self.range_in(text, range)))
    }

    /// Usos de `name` nas regras abaixo de `rule` no formulário que não o definem de novo. A
    /// regra aberta no editor usa o texto do editor, e as outras são lidas do disco.
    fn uses_below(&self, rule: &Path, name: &str) -> Vec<Location> {
        let current = crate::form::path_from_uri(self.uri.as_str());
        let mut out = Vec::new();
        for path in crate::form::descendant_rules(rule) {
            if current.as_deref() == Some(path.as_path()) {
                let ranges = crate::form::uses(&self.doc.text, name, false);
                out.extend(ranges.into_iter().map(|r| self.location(r)));
                continue;
            }
            let Some(text) = fs::read(&path).ok().and_then(|b| crate::cp1252::decode(&b)) else {
                continue;
            };
            for r in crate::form::uses(&text, name, false) {
                out.extend(self.location_in(&path, &text, r));
            }
        }
        out
    }

    /// Referências de um nome usado aqui e definido numa regra acima. Inclui a definição, os usos
    /// na regra dela e os usos nas regras abaixo dela, esta inclusive.
    fn inherited_references(&self, offset: TextSize, include_declaration: bool) -> Vec<Location> {
        let Some((range, def)) = self.inherited_at(offset) else {
            return Vec::new();
        };
        let name = &self.doc.text[range];
        let mut out = Vec::new();
        if include_declaration {
            out.extend(self.location_in(&def.path, &def.text, def.name));
        }
        for r in crate::form::uses(&def.text, name, true) {
            out.extend(self.location_in(&def.path, &def.text, r));
        }
        out.extend(self.uses_below(&def.path, name));
        out
    }

    /// Trecho de outro texto em linha e coluna, na codificação negociada.
    fn range_in(&self, text: &str, range: TextRange) -> Range {
        let index = crate::line_index::LineIndex::new(text);
        let p = |offset| {
            let lc = index.line_col(text, offset, self.encoding);
            Position::new(lc.line, lc.col)
        };
        Range::new(p(range.start()), p(range.end()))
    }

    /// Usos do símbolo na posição, em ordem de texto, e as definições dele quando
    /// `include_declaration`.
    pub(crate) fn references(
        &self,
        position: Position,
        include_declaration: bool,
    ) -> Vec<Location> {
        let offset = self.offset(position);
        let symbols = &self.doc.symbols;
        let Some(mut targets) = symbols.at(offset) else {
            return self.inherited_references(offset, include_declaration);
        };
        // Do `definir funcao` chega na `funcao` pelos usos, que alcançam as duas.
        for r in &symbols.references {
            if r.targets.iter().any(|t| targets.contains(t)) {
                for &t in &r.targets {
                    if !targets.contains(&t) {
                        targets.push(t);
                    }
                }
            }
        }
        let mut ranges: Vec<TextRange> = symbols
            .references
            .iter()
            .filter(|r| r.targets.iter().any(|t| targets.contains(t)))
            .map(|r| r.range)
            .collect();
        if include_declaration {
            ranges.extend(targets.iter().map(|&t| symbols.definitions[t].name));
        }
        ranges.sort_by_key(|r| r.start());
        let mut out: Vec<Location> = ranges.into_iter().map(|r| self.location(r)).collect();
        // Função ou variável deste arquivo num formulário SGI também é usada nas regras abaixo.
        let first = &symbols.definitions[targets[0]];
        let global = !matches!(first.kind, DefKind::Parameter(_) | DefKind::Label);
        if global && self.doc.context == builtins::Context::Sgi {
            if let Some(path) = crate::form::path_from_uri(self.uri.as_str()) {
                out.extend(self.uses_below(&path, &self.doc.text[first.name]));
            }
        }
        out
    }

    pub(crate) fn hover(&self, position: Position) -> Option<Hover> {
        let offset = self.offset(position);
        let symbols = &self.doc.symbols;
        let Some(targets) = symbols.at(offset) else {
            return self.builtin_hover(offset);
        };
        let range = symbols
            .references
            .iter()
            .find(|r| r.range.contains_inclusive(offset))
            .map_or_else(|| symbols.definitions[targets[0]].name, |r| r.range);
        // Um bloco só, com a primeira definição. Um parâmetro mostra a assinatura da função.
        let first = &symbols.definitions[targets[0]];
        let node = match first.kind {
            DefKind::Parameter(_) => self
                .doc
                .tree
                .parent(first.node)
                .and_then(|l| self.doc.tree.parent(l)),
            _ => None,
        }
        .unwrap_or(first.node);
        let line = |t: usize| self.range(symbols.definitions[t].name).start.line + 1;
        let mut value = format!(
            "```lsp\n{}\n```\n{} na linha {}",
            self.header(node),
            describe(first.kind),
            line(targets[0])
        );
        if targets.len() > 1 {
            let others: Vec<_> = targets[1..].iter().map(|&t| line(t).to_string()).collect();
            value.push_str(&format!(", também na linha {}", others.join(", ")));
        }
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(self.range(range)),
        })
    }

    /// Nomes que cabem na posição, filtrados pelo começo já digitado, sem diferença de caixa.
    /// Vêm primeiro as definições do arquivo, depois as das regras acima no formulário SGI, as
    /// funções do ERP e as palavras da linguagem. Dentro de string, de comentário ou depois de um
    /// ponto, que é campo de cursor ou de tabela, não há sugestão.
    pub(crate) fn completion(&self, position: Position) -> Vec<CompletionItem> {
        let offset = self.offset(position);
        let text = &self.doc.text;
        let tree = &self.doc.tree;
        if inside_literal_or_comment(tree, offset) {
            return Vec::new();
        }
        let before = &text[..usize::from(offset)];
        let start = before
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |i| {
                i + before[i..].chars().next().map_or(1, char::len_utf8)
            });
        if before[..start].trim_end().ends_with('.') {
            return Vec::new();
        }
        let prefix = &before[start..];
        let matches = |name: &str| {
            name.len() >= prefix.len()
                && name.is_char_boundary(prefix.len())
                && name[..prefix.len()].eq_ignore_ascii_case(prefix)
        };
        let mut seen = std::collections::HashSet::new();
        let mut items = Vec::new();
        let mut add = |name: &str, kind, detail: String, rank: char| {
            if matches(name) && seen.insert(name.to_uppercase()) {
                items.push(CompletionItem {
                    label: name.to_owned(),
                    kind: Some(kind),
                    detail: Some(detail),
                    sort_text: Some(format!("{rank}{}", name.to_lowercase())),
                    ..CompletionItem::default()
                });
            }
        };

        // Um parâmetro só vale dentro da `funcao` dele.
        let function = enclosing_function_at(tree, offset);
        for def in &self.doc.symbols.definitions {
            let kind = match def.kind {
                DefKind::Label => continue,
                DefKind::Parameter(_) => {
                    let owner = tree.parent(def.node).and_then(|l| tree.parent(l));
                    if owner.is_none() || owner != function {
                        continue;
                    }
                    CompletionItemKind::VARIABLE
                }
                DefKind::Function => CompletionItemKind::FUNCTION,
                _ => CompletionItemKind::VARIABLE,
            };
            add(&text[def.name], kind, self.header(def.node), '0');
        }
        for def in self.doc.inherited.functions() {
            add(
                &def.text[def.name],
                CompletionItemKind::FUNCTION,
                def.decl.clone(),
                '1',
            );
        }
        for def in self.doc.inherited.variables() {
            add(
                &def.text[def.name],
                CompletionItemKind::VARIABLE,
                def.decl.clone(),
                '1',
            );
        }
        for builtin in builtins::all(self.doc.context) {
            add(
                builtin.name,
                CompletionItemKind::FUNCTION,
                builtin.decl.to_owned(),
                '2',
            );
        }
        for &word in KEYWORDS {
            add(
                word,
                CompletionItemKind::KEYWORD,
                "palavra da linguagem".to_owned(),
                '3',
            );
        }
        items
    }

    /// Hover de uma chamada a função do ERP, como `IntParaAlfa`, ou de uma função ou variável de
    /// outra regra do formulário SGI.
    fn builtin_hover(&self, offset: TextSize) -> Option<Hover> {
        let tree = &self.doc.tree;
        let builtin = callee_at(tree, offset).and_then(|callee| {
            let range = tree.text_range(callee);
            let found = builtins::lookup(&self.doc.text[range], self.doc.context)?;
            Some((range, found))
        });
        let (range, value) = if let Some((range, builtin)) = builtin {
            (
                range,
                format!("```lsp\n{}\n```\nFunção do ERP", builtin.decl),
            )
        } else {
            let (range, def) = self.inherited_at(offset)?;
            let line = self.range_in(&def.text, def.name).start.line + 1;
            let value = format!(
                "```lsp\n{}\n```\n{} em {}, linha {line}",
                def.decl,
                describe(def.kind),
                def.path.display()
            );
            (range, value)
        };
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(self.range(range)),
        })
    }

    /// Primeira linha da definição, sem o `{` que abre o corpo de uma função.
    fn header(&self, node: NodeId) -> String {
        let text = &self.doc.text[self.doc.tree.text_range(node)];
        let first = text.lines().next().unwrap_or(text).trim_end();
        let first = first.strip_suffix('{').unwrap_or(first).trim_end();
        first.to_owned()
    }

    /// Variáveis, cursores, listas, tabelas, web services e funções, com o que foi definido
    /// dentro de cada `funcao` como filho dela. Parâmetros e `definir funcao` ficam de fora,
    /// porque a `funcao` já aparece.
    #[allow(deprecated)]
    pub(crate) fn document_symbols(&self) -> Vec<DocumentSymbol> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let mut top: Vec<DocumentSymbol> = Vec::new();
        // Índice em `top` de cada função, para pendurar o que foi definido dentro dela.
        let mut functions: Vec<(NodeId, usize)> = Vec::new();
        for def in &self.doc.symbols.definitions {
            let (kind, detail) = match def.kind {
                DefKind::Parameter(_) => continue,
                DefKind::Function if tree.kind(def.node) == SyntaxKind::FuncDecl => continue,
                DefKind::Variable(t) => (SymbolKind::VARIABLE, t.map(type_name)),
                DefKind::Cursor => (SymbolKind::OBJECT, Some("cursor")),
                DefKind::List => (SymbolKind::ARRAY, Some("lista")),
                DefKind::Table => (SymbolKind::STRUCT, Some("tabela")),
                DefKind::WebService => (SymbolKind::INTERFACE, Some("web service")),
                DefKind::Function => (SymbolKind::FUNCTION, None),
                DefKind::Label => (SymbolKind::KEY, Some("rótulo")),
            };
            let symbol = DocumentSymbol {
                name: text[def.name].to_owned(),
                detail: detail.map(str::to_owned),
                kind,
                tags: None,
                deprecated: None,
                range: self.range(tree.text_range(def.node)),
                selection_range: self.range(def.name),
                children: None,
            };
            // Uma `funcao` dentro do bloco aberto de outra continua sendo de primeiro nível.
            let owner = (kind != SymbolKind::FUNCTION)
                .then_some(def.node)
                .and_then(|n| enclosing_function(tree, n))
                .and_then(|f| functions.iter().find(|(n, _)| *n == f).map(|&(_, i)| i));
            match owner {
                Some(i) => top[i].children.get_or_insert_with(Vec::new).push(symbol),
                None => {
                    if kind == SymbolKind::FUNCTION {
                        functions.push((def.node, top.len()));
                    }
                    top.push(symbol);
                }
            }
        }
        top
    }
}

/// Palavras sugeridas pelo completar nomes, na grafia mais comum nas regras.
const KEYWORDS: &[&str] = &[
    "Definir",
    "Funcao",
    "Se",
    "Senao",
    "Enquanto",
    "Para",
    "Inicio",
    "Fim",
    "VaPara",
    "Pare",
    "Continue",
    "Alfa",
    "Numero",
    "Data",
    "Cursor",
    "Lista",
    "Tabela",
    "Mensagem",
    "Erro",
    "Refaz",
    "Retorna",
    "ExecSql",
    "ExecSqlEx",
];

/// Diz se `offset` cai no meio de uma string, de um apóstrofo ou de um comentário.
fn inside_literal_or_comment(tree: &crate::Tree, offset: TextSize) -> bool {
    let tokens = tree.tokens();
    let (mut lo, mut hi) = (0, tokens.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if tokens.range(mid).end() < offset {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo >= tokens.len() {
        return false;
    }
    let range = tokens.range(lo);
    range.start() < offset
        && offset < range.end()
        && matches!(
            tokens.kind(lo),
            SyntaxKind::String
                | SyntaxKind::Char
                | SyntaxKind::AtComment
                | SyntaxKind::BlockComment
        )
}

/// `funcao` cujo trecho contém `offset`, a mais interna.
fn enclosing_function_at(tree: &crate::Tree, offset: TextSize) -> Option<NodeId> {
    let mut node = tree.root();
    let mut found = None;
    while let Some(child) = tree
        .children(node)
        .find(|&c| tree.text_range(c).contains_inclusive(offset))
    {
        if tree.kind(child) == SyntaxKind::FuncImpl {
            found = Some(child);
        }
        node = child;
    }
    found
}

/// Nome que contém `offset`, fora de campo depois de ponto, e se ele é o nome chamado numa
/// chamada.
fn name_at(tree: &crate::Tree, offset: TextSize) -> Option<(NodeId, bool)> {
    let mut node = tree.root();
    loop {
        let child = tree
            .children(node)
            .find(|&c| tree.text_range(c).contains_inclusive(offset))?;
        if tree.kind(child) == SyntaxKind::NameRef {
            let parent = tree.parent(child)?;
            let first = tree.children(parent).next() == Some(child);
            return match tree.kind(parent) {
                SyntaxKind::FieldExpr if !first => None,
                SyntaxKind::CallExpr if first => Some((child, true)),
                _ => Some((child, false)),
            };
        }
        node = child;
    }
}

/// Nome chamado (`NameRef` que é o primeiro filho de um `CallExpr`) que contém `offset`.
fn callee_at(tree: &crate::Tree, offset: TextSize) -> Option<NodeId> {
    let mut node = tree.root();
    loop {
        let child = tree
            .children(node)
            .find(|&c| tree.text_range(c).contains_inclusive(offset))?;
        if tree.kind(child) == SyntaxKind::NameRef {
            let call = tree.parent(child)?;
            let first = tree.children(call).next() == Some(child);
            return (tree.kind(call) == SyntaxKind::CallExpr && first).then_some(child);
        }
        node = child;
    }
}

/// `funcao` mais próxima em volta do nó, sem contar o próprio nó.
fn enclosing_function(tree: &crate::Tree, node: NodeId) -> Option<NodeId> {
    let mut current = tree.parent(node);
    while let Some(n) = current {
        if tree.kind(n) == SyntaxKind::FuncImpl {
            return Some(n);
        }
        current = tree.parent(n);
    }
    None
}

fn type_name(t: VarType) -> &'static str {
    match t {
        VarType::Alfa => "alfa",
        VarType::Numero => "numero",
        VarType::Data => "data",
    }
}

fn describe(kind: DefKind) -> &'static str {
    match kind {
        DefKind::Variable(_) => "Variável definida",
        DefKind::Parameter(_) => "Parâmetro definido",
        DefKind::Cursor => "Cursor definido",
        DefKind::List => "Lista definida",
        DefKind::Table => "Tabela definida",
        DefKind::WebService => "Web service definido",
        DefKind::Function => "Função definida",
        DefKind::Label => "Rótulo",
    }
}
