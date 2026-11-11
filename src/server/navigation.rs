//! Pedidos de navegação do editor: ir para a definição, referências, hover e a lista de símbolos.

use lsp_types::{
    CompletionItem, CompletionItemKind, DocumentSymbol, Hover, HoverContents, Location,
    MarkupContent, MarkupKind, Position, Range, SymbolKind, TextEdit, Uri, WorkspaceEdit,
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
    /// Regra de dentro de um JSON de modelo de relatório. As posições do editor são do JSON e
    /// passam pelo mapa da regra nos dois sentidos.
    pub(crate) embedding: Option<Embedding<'a>>,
}

#[derive(Clone, Copy)]
pub(crate) struct Embedding<'a> {
    pub(crate) rule: &'a crate::report::EmbeddedRule,
    pub(crate) json: &'a str,
    pub(crate) index: &'a crate::line_index::LineIndex,
}

impl Context<'_> {
    pub(crate) fn offset(&self, position: Position) -> TextSize {
        let lc = LineCol {
            line: position.line,
            col: position.character,
        };
        match self.embedding {
            Some(e) => {
                let json = e.index.offset(e.json, lc, self.encoding);
                e.rule.from_json(json).unwrap_or_default()
            }
            None => self.doc.index.offset(&self.doc.text, lc, self.encoding),
        }
    }

    pub(crate) fn range(&self, range: TextRange) -> Range {
        let (text, index, range) = match self.embedding {
            Some(e) => (e.json, e.index, e.rule.range_to_json(range)),
            None => (self.doc.text.as_str(), &self.doc.index, range),
        };
        let p = |offset| {
            let lc = index.line_col(text, offset, self.encoding);
            Position::new(lc.line, lc.col)
        };
        Range::new(p(range.start()), p(range.end()))
    }

    /// Lugar de uma definição de outra regra, no arquivo dela. Uma regra de modelo de relatório
    /// leva a posição ao JSON pelo mapa dela.
    fn def_location(&self, def: &InheritedDef, range: TextRange) -> Option<Location> {
        match &def.embedded {
            Some(e) => {
                let json_range = e.rule.range_to_json(range);
                self.location_in(&def.path, &e.json, json_range)
            }
            None => self.location_in(&def.path, &def.text, range),
        }
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
        self.def_location(def, def.name)
    }

    /// Nome na posição que vem de uma regra anterior do formulário, com o trecho dele aqui.
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

    /// Usos de `name` nas regras que vêm depois de `rule` no formulário que não o definem de novo. A
    /// regra aberta no editor usa o texto do editor, e as outras são lidas do disco.
    fn uses_after(&self, rule: &Path, name: &str) -> Vec<Location> {
        let current = crate::form::path_from_uri(self.uri.as_str());
        let mut out = Vec::new();
        for path in crate::form::following_rules(rule) {
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

    /// Referências de um nome usado aqui e definido numa regra anterior. Inclui a definição, os usos
    /// na regra dela e os usos nas regras seguintes, esta inclusive.
    fn inherited_references(&self, offset: TextSize, include_declaration: bool) -> Vec<Location> {
        let Some((range, def)) = self.inherited_at(offset) else {
            return Vec::new();
        };
        let name = &self.doc.text[range];
        let mut out = Vec::new();
        if include_declaration {
            out.extend(self.def_location(def, def.name));
        }
        for r in crate::form::uses(&def.text, name, true) {
            out.extend(self.def_location(def, r));
        }
        out.extend(self.uses_after(&def.path, name));
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

    /// Nome na posição que vale no modelo de relatório inteiro, e se é função. Parâmetro e rótulo
    /// ficam de fora, porque valem só na regra deles.
    fn model_symbol_at(&self, offset: TextSize) -> Option<(std::string::String, bool)> {
        let symbols = &self.doc.symbols;
        if let Some(targets) = symbols.at(offset) {
            let def = &symbols.definitions[targets[0]];
            if matches!(def.kind, DefKind::Parameter(_) | DefKind::Label) {
                return None;
            }
            let name = self.doc.text[def.name].to_owned();
            return Some((name, def.kind == DefKind::Function));
        }
        let (range, def) = self.inherited_at(offset)?;
        Some((
            self.doc.text[range].to_owned(),
            def.kind == DefKind::Function,
        ))
    }

    /// Definições e usos de `name` em todas as regras do modelo de relatório. Numa regra entram os
    /// usos sem `definir` nela e os que apontam para uma definição dela que não seja parâmetro.
    /// O JSON aberto no editor entra com o texto e o endereço do editor.
    fn model_references(
        &self,
        name: &str,
        function: bool,
        include_declaration: bool,
    ) -> Vec<Location> {
        let (Some(embedding), Some(path)) = (
            self.embedding,
            crate::form::path_from_uri(self.uri.as_str()),
        ) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut indexes: std::collections::HashMap<
            std::path::PathBuf,
            crate::line_index::LineIndex,
        > = std::collections::HashMap::new();
        let model = crate::report::model(&path, embedding.json);
        if include_declaration && !function {
            for (entry_path, json, entry) in &model.entries {
                if !entry.name.eq_ignore_ascii_case(name) {
                    continue;
                }
                let uri = if *entry_path == path {
                    Some(self.uri.clone())
                } else {
                    crate::form::uri_from_path(entry_path).parse().ok()
                };
                if let Some(uri) = uri {
                    out.push(Location::new(uri, self.range_in(json, entry.name_range)));
                }
            }
        }
        for (rule_path, embedded) in model.rules {
            let text = &embedded.rule.text;
            let Ok(tree) = crate::parse(text) else {
                continue;
            };
            let symbols = crate::symbols::Symbols::build(&tree, text);
            let mut ranges = Vec::new();
            if include_declaration {
                ranges.extend(
                    symbols
                        .definitions
                        .iter()
                        .filter(|d| text[d.name].eq_ignore_ascii_case(name))
                        .filter(|d| (d.kind == DefKind::Function) == function)
                        .filter(|d| !matches!(d.kind, DefKind::Parameter(_) | DefKind::Label))
                        .map(|d| d.name),
                );
            }
            let mut stack = vec![tree.root()];
            while let Some(node) = stack.pop() {
                stack.extend(tree.children(node));
                let Some((_, is_callee)) = name_ref_kind(&tree, node) else {
                    continue;
                };
                let range = tree.text_range(node);
                if is_callee != function || !text[range].eq_ignore_ascii_case(name) {
                    continue;
                }
                let to_param = symbols.reference_at(range).is_some_and(|r| {
                    r.targets
                        .iter()
                        .any(|&t| matches!(symbols.definitions[t].kind, DefKind::Parameter(_)))
                });
                if !to_param {
                    ranges.push(range);
                }
            }
            ranges.sort_by_key(|r| r.start());
            let uri = if rule_path == path {
                Some(self.uri.clone())
            } else {
                crate::form::uri_from_path(&rule_path).parse().ok()
            };
            let Some(uri) = uri else {
                continue;
            };
            let index = indexes
                .entry(rule_path.clone())
                .or_insert_with(|| crate::line_index::LineIndex::new(&embedded.json));
            for range in ranges {
                let json_range = embedded.rule.range_to_json(range);
                let p = |offset| {
                    let lc = index.line_col(&embedded.json, offset, self.encoding);
                    Position::new(lc.line, lc.col)
                };
                let range = Range::new(p(json_range.start()), p(json_range.end()));
                out.push(Location::new(uri.clone(), range));
            }
        }
        out
    }

    /// Usos do símbolo na posição, em ordem de texto, e as definições dele quando
    /// `include_declaration`.
    pub(crate) fn references(
        &self,
        position: Position,
        include_declaration: bool,
    ) -> Vec<Location> {
        let offset = self.offset(position);
        // Num modelo de relatório, uma variável ou função vale em todas as regras dele.
        if self.embedding.is_some()
            && let Some((name, function)) = self.model_symbol_at(offset)
        {
            return self.model_references(&name, function, include_declaration);
        }
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
        // Função ou variável deste arquivo num formulário SGI também é usada nas regras seguintes.
        let first = &symbols.definitions[targets[0]];
        let global = !matches!(first.kind, DefKind::Parameter(_) | DefKind::Label);
        if global
            && self.doc.context == builtins::Context::Sgi
            && let Some(path) = crate::form::path_from_uri(self.uri.as_str())
        {
            out.extend(self.uses_after(&path, &self.doc.text[first.name]));
        }
        out
    }

    pub(crate) fn hover(&self, position: Position) -> Option<Hover> {
        let offset = self.offset(position);
        if let Some(hover) = self.dictionary_hover(offset) {
            return Some(hover);
        }
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

    /// Trecho do nome que pode ser renomeado na posição. `None` quando ali não há nome definido
    /// no arquivo nem numa regra anterior do formulário, como uma função do ERP.
    pub(crate) fn prepare_rename(&self, position: Position) -> Option<Range> {
        let offset = self.offset(position);
        // Renomear uma entrada do relatório mudaria o cadastro dela no modelo, mesmo quando uma
        // regra também a define com `definir`.
        if let Some((node, _)) = name_at(&self.doc.tree, offset) {
            let name = &self.doc.text[self.doc.tree.text_range(node)];
            if self.doc.inherited.is_entry(name) {
                return None;
            }
        }
        if let Some(def) = self
            .doc
            .symbols
            .definitions
            .iter()
            .find(|d| d.name.contains_inclusive(offset))
            && self.doc.inherited.is_entry(&self.doc.text[def.name])
        {
            return None;
        }
        let symbols = &self.doc.symbols;
        let local = symbols
            .references
            .iter()
            .map(|r| r.range)
            .chain(symbols.definitions.iter().map(|d| d.name))
            .find(|r| r.contains_inclusive(offset));
        match local {
            Some(range) => Some(self.range(range)),
            None => self
                .inherited_at(offset)
                .map(|(range, _)| self.range(range)),
        }
    }

    /// Troca o nome na posição por `new_name` em todos os lugares que o "achar referências"
    /// encontra, nos arquivos do formulário SGI inclusive. Recusa nome que não é identificador,
    /// palavra reservada e nome que já existe no arquivo.
    // O `WorkspaceEdit` do protocolo guarda as trocas num mapa com `Uri` como chave, e o `Uri` do
    // lsp-types tem um cache interno que o clippy vê como chave mutável.
    #[allow(clippy::mutable_key_type)]
    pub(crate) fn rename(
        &self,
        position: Position,
        new_name: &str,
    ) -> Result<Option<WorkspaceEdit>, std::string::String> {
        let valid = new_name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && new_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && new_name.len() <= 100;
        if !valid {
            return Err(format!(
                "\"{new_name}\" não é um nome válido. Use letras, dígitos e _, começando por letra \
                 ou _, com até 100 caracteres."
            ));
        }
        if SyntaxKind::keyword(new_name).is_some() {
            return Err(format!("\"{new_name}\" é palavra reservada da linguagem."));
        }
        let Some(range) = self.prepare_rename(position) else {
            return Ok(None);
        };
        let text = &self.doc.text;
        let start = self.offset(range.start);
        let end = self.offset(range.end);
        let old = &text[usize::from(start)..usize::from(end)];
        let taken = self
            .doc
            .symbols
            .definitions
            .iter()
            .any(|d| text[d.name].eq_ignore_ascii_case(new_name));
        if taken && !old.eq_ignore_ascii_case(new_name) {
            return Err(format!("Já existe \"{new_name}\" definido neste arquivo."));
        }
        // Num modelo de relatório, o nome novo também não pode existir nas outras regras.
        let in_model = self.doc.inherited.get(new_name).is_some()
            || self.doc.inherited.variable(new_name).is_some();
        if self.embedding.is_some() && in_model && !old.eq_ignore_ascii_case(new_name) {
            return Err(format!("Já existe \"{new_name}\" definido neste modelo."));
        }
        let mut changes: std::collections::HashMap<Uri, Vec<TextEdit>> =
            std::collections::HashMap::new();
        for location in self.references(position, true) {
            let edit = TextEdit::new(location.range, new_name.to_owned());
            changes.entry(location.uri).or_default().push(edit);
        }
        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            ..WorkspaceEdit::default()
        }))
    }

    /// Nomes que cabem na posição, filtrados pelo começo já digitado, sem diferença de caixa.
    /// Vêm primeiro as definições do arquivo, depois as das regras anteriores do formulário SGI, as
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
    /// Campo de tabela do dicionário de dados, com o tipo e a descrição.
    fn dictionary_hover(&self, offset: TextSize) -> Option<Hover> {
        let tree = &self.doc.tree;
        let node = field_expr_at(tree, offset)?;
        let found = crate::validate::dictionary_field(
            tree,
            &self.doc.symbols,
            &self.doc.text,
            self.doc.context,
            &self.doc.inherited,
            node,
        )?;
        let range = tree.text_range(node);
        let origin = if found.only_in_test {
            "Campo que só existe no dicionário de teste"
        } else {
            "Campo do dicionário de dados"
        };
        let mut value = format!(
            "```lsp\n{} {}\n```\n{}\n\n{origin}",
            found.field.ty, &self.doc.text[range], found.field.description
        );
        for line in field_links(&found) {
            value.push_str("\n\n");
            value.push_str(&line);
        }
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(self.range(range)),
        })
    }

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
            let line = self
                .def_location(def, def.name)
                .map_or(0, |l| l.range.start.line + 1);
            let what = match def.entry {
                Some(crate::report::EntryKind::Entrance) => "Entrada do relatório".to_owned(),
                Some(crate::report::EntryKind::Field) => "Campo da tela".to_owned(),
                Some(crate::report::EntryKind::Control(c)) => {
                    format!("Controle de {} do relatório", c.label())
                }
                None => describe(def.kind).to_owned(),
            };
            let value = format!(
                "```lsp\n{}\n```\n{what} em {}, linha {line}",
                def.decl,
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
    pub(crate) fn header(&self, node: NodeId) -> String {
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

/// Se o nó é um nome de uso, fora de campo depois de ponto, e se ele é o nome chamado numa chamada.
fn name_ref_kind(tree: &crate::Tree, node: NodeId) -> Option<(NodeId, bool)> {
    if tree.kind(node) != SyntaxKind::NameRef
        || tree.tokens().kind(tree.token_range(node).start) != SyntaxKind::Ident
    {
        return None;
    }
    let parent = tree.parent(node)?;
    let first = tree.children(parent).next() == Some(node);
    match tree.kind(parent) {
        SyntaxKind::FieldExpr if !first => None,
        SyntaxKind::CallExpr if first => Some((node, true)),
        _ => Some((node, false)),
    }
}

/// Nome que contém `offset`, fora de campo depois de ponto, e se ele é o nome chamado numa
/// chamada.
pub(crate) fn name_at(tree: &crate::Tree, offset: TextSize) -> Option<(NodeId, bool)> {
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

/// Valores, máscara, chave primária, índices, verificações, relacionamentos e gatilhos do campo, uma
/// linha de hover para cada.
fn field_links(found: &crate::dictionary::Found) -> Vec<String> {
    const MAX_INCOMING: usize = 5;
    const MAX_VALUES: usize = 12;
    let (dictionary, table, field) = (found.dictionary, found.table, found.field);
    let names = |positions: &[usize]| {
        positions
            .iter()
            .filter_map(|&i| table.fields.get(i).map(|f| f.name.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out = Vec::new();
    let enumeration = field
        .ty
        .enumeration
        .as_ref()
        .and_then(|name| dictionary.enumerations.get(&name.to_uppercase()));
    if let Some(e) = enumeration {
        let shown: Vec<_> = e
            .values
            .iter()
            .take(MAX_VALUES)
            .map(|(value, description)| format!("{value} = {description}"))
            .collect();
        if !shown.is_empty() {
            let more = e.values.len().saturating_sub(MAX_VALUES);
            let more = if more > 0 {
                format!(" e mais {more}")
            } else {
                String::new()
            };
            out.push(format!("Valores {}{more}", shown.join(", ")));
        }
    }
    let mut format_parts = Vec::new();
    if !field.mask.trim().is_empty() {
        format_parts.push(format!("Máscara {}", field.mask.trim()));
    }
    if !field.initial.trim().is_empty() {
        format_parts.push(format!("valor inicial {}", field.initial.trim()));
    }
    if !format_parts.is_empty() {
        out.push(format_parts.join(", "));
    }
    let position = table.fields.iter().position(|f| std::ptr::eq(f, field));
    if let Some(i) = position {
        if table.primary_key.contains(&i) {
            out.push(format!(
                "Faz parte da chave primária ({})",
                names(&table.primary_key)
            ));
        }
        let keys: Vec<_> = table
            .uniques
            .iter()
            .chain(&table.indexes)
            .filter(|k| k.fields.contains(&i))
            .map(|k| {
                let unique = if k.unique { ", único" } else { "" };
                format!("{} ({}{unique})", k.name, names(&k.fields))
            })
            .collect();
        if !keys.is_empty() {
            out.push(format!("Índices {}", keys.join("; ")));
        }
    }
    let same = |a: &str| a.eq_ignore_ascii_case(&field.name);
    for check in &table.checks {
        let cites = check
            .condition
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .any(same);
        if cites {
            out.push(format!(
                "Check constraint {} ({})",
                check.name,
                check.condition.trim()
            ));
        }
    }
    for relation in dictionary.relations_of(table) {
        if relation.from_table != table.name {
            continue;
        }
        if let Some(j) = relation.from_fields.iter().position(|f| same(f)) {
            let target = relation.to_fields.get(j).map_or("", String::as_str);
            out.push(format!(
                "Aponta para {}.{target} ({})",
                relation.to_table, relation.name
            ));
        }
    }
    if let Some(link) = &field.link {
        let kind = if dictionary
            .views
            .iter()
            .any(|v| v.name.eq_ignore_ascii_case(&link.target))
        {
            "na view "
        } else {
            "em "
        };
        let mut line = format!(
            "Busca {kind}{} por {}",
            link.target,
            link.target_fields.join(", ")
        );
        let same_names = link.from_fields.len() == link.target_fields.len()
            && link
                .from_fields
                .iter()
                .zip(&link.target_fields)
                .all(|(a, b)| a.eq_ignore_ascii_case(b));
        if !same_names {
            line.push_str(&format!(" (com {})", link.from_fields.join(", ")));
        }
        if let Some(result) = &link.result {
            line.push_str(&format!(", mostra {result}"));
        }
        out.push(line);
    }
    let incoming: Vec<_> = dictionary
        .relations_of(table)
        .filter(|r| r.to_table == table.name)
        .filter_map(|r| {
            let j = r.to_fields.iter().position(|f| same(f))?;
            Some(format!("{}.{}", r.from_table, r.from_fields.get(j)?))
        })
        .collect();
    if !incoming.is_empty() {
        let shown = incoming[..incoming.len().min(MAX_INCOMING)].join(", ");
        let more = if incoming.len() > MAX_INCOMING {
            format!(" e mais {}", incoming.len() - MAX_INCOMING)
        } else {
            String::new()
        };
        out.push(format!("Referenciado por {shown}{more}"));
    }
    if !table.triggers.is_empty() {
        let names: Vec<_> = table.triggers.iter().map(|t| t.name.as_str()).collect();
        out.push(format!("Triggers da tabela {}", names.join(", ")));
    }
    out
}

/// `FieldExpr` mais interno que contém `offset`, com a base e o campo como nomes simples.
fn field_expr_at(tree: &crate::Tree, offset: TextSize) -> Option<NodeId> {
    let mut node = tree.root();
    let mut found = None;
    while let Some(child) = tree
        .children(node)
        .find(|&c| tree.text_range(c).contains_inclusive(offset))
    {
        if tree.kind(child) == SyntaxKind::FieldExpr
            && tree
                .children(child)
                .all(|c| tree.kind(c) == SyntaxKind::NameRef)
        {
            found = Some(child);
        }
        node = child;
    }
    found
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

#[cfg(test)]
mod tests {
    use crate::tbs::{
        Check, Dictionary, Enumeration, Field, FieldKind, FieldType, Key, Link, Relation, Table,
        Trigger, View,
    };

    fn field(name: &str) -> Field {
        Field {
            name: name.to_owned(),
            ty: FieldType {
                kind: FieldKind::Number,
                size: 4,
                decimals: 0,
                enumeration: None,
                base: None,
            },
            ..Field::default()
        }
    }

    #[test]
    fn hover_mostra_chave_indices_e_relacionamentos() {
        let relation = |from: &str, to: &str| Relation {
            name: format!("IR_{from}"),
            from_table: from.to_owned(),
            from_fields: vec!["CodEmp".to_owned()],
            to_table: to.to_owned(),
            to_fields: vec!["CodEmp".to_owned()],
        };
        let mut relations = vec![relation("E070FIL", "E070EMP")];
        relations.extend((1..=6).map(|i| relation(&format!("E1{i:02}XXX"), "E070FIL")));
        let filial = Table {
            name: "E070FIL".to_owned(),
            fields: vec![field("CodEmp"), field("CodFil")],
            primary_key: vec![0, 1],
            indexes: vec![Key {
                name: "E070FILIndice1".to_owned(),
                fields: vec![0],
                unique: false,
            }],
            relations: (0..relations.len()).collect(),
            ..Table::default()
        };
        let dictionary = Dictionary {
            tables: vec![filial],
            relations,
            ..Dictionary::default()
        };
        assert_eq!(
            super::field_links(&crate::dictionary::Found {
                dictionary: &dictionary,
                table: &dictionary.tables[0],
                field: &dictionary.tables[0].fields[0],
                only_in_test: false,
            })
            .len(),
            4
        );
        // Enumeração, máscara, verificação e gatilho numa tabela à parte.
        let mut situacao = field("SitFil");
        situacao.ty.enumeration = Some("LSitFil".to_owned());
        situacao.mask = "U".to_owned();
        situacao.initial = "A".to_owned();
        let outra = Table {
            name: "E070OUT".to_owned(),
            fields: vec![situacao],
            checks: vec![Check {
                name: "CK_SITFIL".to_owned(),
                condition: "SitFil IN ('A','I')".to_owned(),
            }],
            triggers: vec![Trigger {
                name: "T070OUT".to_owned(),
                code: Vec::new(),
            }],
            ..Table::default()
        };
        let mut enumerations = std::collections::HashMap::new();
        enumerations.insert(
            "LSITFIL".to_owned(),
            Enumeration {
                name: "LSitFil".to_owned(),
                base: FieldType::default(),
                values: vec![
                    ("A".to_owned(), "Ativa".to_owned()),
                    ("I".to_owned(), "Inativa".to_owned()),
                ],
            },
        );
        let extra = Dictionary {
            tables: vec![outra],
            enumerations,
            ..Dictionary::default()
        };
        assert_eq!(
            super::field_links(&crate::dictionary::Found {
                dictionary: &extra,
                table: &extra.tables[0],
                field: &extra.tables[0].fields[0],
                only_in_test: false,
            }),
            [
                "Valores A = Ativa, I = Inativa",
                "Máscara U, valor inicial A",
                "Check constraint CK_SITFIL (SitFil IN ('A','I'))",
                "Triggers da tabela T070OUT",
            ]
        );
        let table = &dictionary.tables[0];
        let found = crate::dictionary::Found {
            dictionary: &dictionary,
            table,
            field: &table.fields[0],
            only_in_test: false,
        };
        assert_eq!(
            super::field_links(&found),
            [
                "Faz parte da chave primária (CodEmp, CodFil)",
                "Índices E070FILIndice1 (CodEmp)",
                "Aponta para E070EMP.CodEmp (IR_E070FIL)",
                "Referenciado por E101XXX.CodEmp, E102XXX.CodEmp, E103XXX.CodEmp, \
                 E104XXX.CodEmp, E105XXX.CodEmp e mais 1",
            ]
        );
    }

    #[test]
    fn hover_mostra_a_busca_do_campo() {
        let link = |from: &[&str], target: &str, to: &[&str], result: Option<&str>| Link {
            from_fields: from.iter().map(|s| s.to_string()).collect(),
            target: target.to_owned(),
            target_fields: to.iter().map(|s| s.to_string()).collect(),
            result: result.map(str::to_owned),
        };
        let mut familia = field("CodFam");
        familia.link = Some(link(
            &["CodEmp", "CodFam"],
            "E012FAM",
            &["CodEmp", "CodFam"],
            Some("DesFam"),
        ));
        let mut fornecedor = field("ForPad");
        fornecedor.link = Some(link(&["ForPad"], "VW_FOR", &["CodFor"], None));
        let dictionary = Dictionary {
            tables: vec![Table {
                name: "E075PRO".to_owned(),
                fields: vec![familia, fornecedor],
                ..Table::default()
            }],
            views: vec![View {
                name: "VW_FOR".to_owned(),
                ..View::default()
            }],
            ..Dictionary::default()
        };
        let table = &dictionary.tables[0];
        let lines = |i: usize| {
            super::field_links(&crate::dictionary::Found {
                dictionary: &dictionary,
                table,
                field: &table.fields[i],
                only_in_test: false,
            })
        };
        assert_eq!(
            lines(0),
            ["Busca em E012FAM por CodEmp, CodFam, mostra DesFam"]
        );
        assert_eq!(lines(1), ["Busca na view VW_FOR por CodFor (com ForPad)"]);
    }
}
