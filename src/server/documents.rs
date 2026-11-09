//! Documentos abertos no editor.
//!
//! O texto vem sempre do editor, pelo `didOpen` e pelo `didChange`. Assim o que ainda não foi salvo
//! também é analisado. A análise é refeita a cada mudança e fica guardada para os pedidos do
//! editor, como ir para a definição. As regras vizinhas, como as de cima num formulário SGI e as
//! outras de um modelo de relatório, são lidas do disco.

use std::collections::HashMap;
use std::sync::Arc;

use lsp_types::Uri;

use crate::builtins::Context;
use crate::form::Inherited;
use crate::line_index::LineIndex;
use crate::symbols::Symbols;
use crate::{TextTooLarge, Tree};

pub(crate) struct Document {
    pub(crate) text: String,
    pub(crate) version: i32,
    pub(crate) tree: Tree,
    pub(crate) symbols: Symbols,
    pub(crate) index: LineIndex,
    pub(crate) context: Context,
    /// Funções e variáveis de outras regras que valem para esta, como as de cima num formulário
    /// SGI ou as das outras regras de um modelo de relatório.
    pub(crate) inherited: Arc<Inherited>,
}

/// JSON de um modelo de relatório, com cada regra dele analisada como um documento.
pub(crate) struct ReportDocument {
    pub(crate) version: i32,
    pub(crate) json: Arc<str>,
    pub(crate) index: LineIndex,
    pub(crate) rules: Vec<ReportRule>,
}

pub(crate) struct ReportRule {
    pub(crate) embedded: Arc<crate::report::Embedded>,
    pub(crate) doc: Document,
}

impl ReportDocument {
    /// Regra em que cai a posição do JSON, com a posição dentro dela.
    pub(crate) fn rule_at(&self, offset: text_size::TextSize) -> Option<&ReportRule> {
        self.rules
            .iter()
            .find(|r| r.embedded.rule.from_json(offset).is_some())
    }
}

#[derive(Default)]
pub(crate) struct Documents {
    open: HashMap<Uri, Document>,
    reports: HashMap<Uri, ReportDocument>,
}

impl Documents {
    pub(crate) fn set(&mut self, uri: Uri, text: String, version: i32) -> Result<(), TextTooLarge> {
        let tree = crate::parse(&text)?;
        let symbols = Symbols::build(&tree, &text);
        let index = LineIndex::new(&text);
        let context = Context::from_uri(uri.as_str());
        // As regras de cima são lidas do disco a cada mudança. Um formulário tem poucas.
        let inherited = Arc::new(match (context, crate::form::path_from_uri(uri.as_str())) {
            (Context::Sgi, Some(path)) => crate::form::inherited_functions(&path),
            _ => Inherited::default(),
        });
        let doc = Document {
            text,
            version,
            tree,
            symbols,
            index,
            context,
            inherited,
        };
        self.open.insert(uri, doc);
        Ok(())
    }

    /// Guarda o JSON de um modelo de relatório. As outras regras do modelo, nos outros JSON da
    /// pasta, são lidas do disco a cada mudança.
    pub(crate) fn set_report(
        &mut self,
        uri: Uri,
        json: String,
        version: i32,
    ) -> Result<(), TextTooLarge> {
        let path = crate::form::path_from_uri(uri.as_str()).unwrap_or_default();
        let model = crate::report::model_rules(&path, &json);
        let inherited = Arc::new(Inherited::from_model(model.iter().cloned()));
        let json: Arc<str> = json.into();
        let mut rules = Vec::new();
        for (rule_path, embedded) in model {
            if rule_path != path {
                continue;
            }
            let text = embedded.rule.text.clone();
            let tree = crate::parse(&text)?;
            let symbols = Symbols::build(&tree, &text);
            let index = LineIndex::new(&text);
            let doc = Document {
                text,
                version,
                tree,
                symbols,
                index,
                context: Context::Report,
                inherited: inherited.clone(),
            };
            rules.push(ReportRule { embedded, doc });
        }
        let index = LineIndex::new(&json);
        self.reports.insert(
            uri,
            ReportDocument {
                version,
                json,
                index,
                rules,
            },
        );
        Ok(())
    }

    pub(crate) fn get(&self, uri: &Uri) -> Option<&Document> {
        self.open.get(uri)
    }

    pub(crate) fn report(&self, uri: &Uri) -> Option<&ReportDocument> {
        self.reports.get(uri)
    }

    /// Tira o documento, regra ou JSON de modelo, e diz se ele estava aberto.
    pub(crate) fn remove(&mut self, uri: &Uri) -> bool {
        self.open.remove(uri).is_some() | self.reports.remove(uri).is_some()
    }
}

/// Diz se o endereço é de um JSON de modelo de relatório, na pasta `Modelos`.
pub(crate) fn is_report(uri: &Uri) -> bool {
    Context::from_uri(uri.as_str()) == Context::Report
}

/// Diz se o endereço é de uma regra. O Claude Code associa o servidor pela extensão, e `.txt`
/// pega todo arquivo de texto, então só o nome `Rule.txt`, sem diferenciar maiúsculas, é
/// analisado.
pub(crate) fn is_rule(uri: &Uri) -> bool {
    let s = uri.as_str();
    let end = s.find(['?', '#']).unwrap_or(s.len());
    let path = &s[..end];
    let name = path.rsplit('/').next().unwrap_or(path);
    name.eq_ignore_ascii_case("rule.txt")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    #[test]
    fn so_rule_txt_e_analisado() {
        assert!(is_rule(&uri("file:///home/u/regras/EXEMPLO.LSP/Rule.txt")));
        assert!(is_rule(&uri("file:///c%3A/x/rule.TXT")));
        assert!(!is_rule(&uri("file:///home/u/README.txt")));
        assert!(!is_rule(&uri("file:///home/u/Rule.txt.bak")));
        assert!(!is_rule(&uri("file:///home/u/MyRule.txt")));
    }
}
