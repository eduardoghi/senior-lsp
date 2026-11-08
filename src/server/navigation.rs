//! Pedidos de navegação do editor: ir para a definição, referências, hover e a lista de símbolos.

use lsp_types::{
    DocumentSymbol, Hover, HoverContents, Location, MarkupContent, MarkupKind, Position, Range,
    SymbolKind, Uri,
};
use text_size::{TextRange, TextSize};

use super::documents::Document;
use crate::builtins;
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
        let symbols = &self.doc.symbols;
        symbols
            .at(self.offset(position))
            .unwrap_or_default()
            .into_iter()
            .map(|i| self.location(symbols.definitions[i].name))
            .collect()
    }

    /// Usos do símbolo na posição, em ordem de texto, e as definições dele quando
    /// `include_declaration`.
    pub(crate) fn references(
        &self,
        position: Position,
        include_declaration: bool,
    ) -> Vec<Location> {
        let symbols = &self.doc.symbols;
        let Some(mut targets) = symbols.at(self.offset(position)) else {
            return Vec::new();
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
        ranges.into_iter().map(|r| self.location(r)).collect()
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

    /// Hover de uma chamada a função do ERP, como `IntParaAlfa`.
    fn builtin_hover(&self, offset: TextSize) -> Option<Hover> {
        let tree = &self.doc.tree;
        let callee = callee_at(tree, offset)?;
        let range = tree.text_range(callee);
        let builtin = builtins::lookup(&self.doc.text[range], self.doc.context)?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("```lsp\n{}\n```\nFunção do ERP", builtin.decl),
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
