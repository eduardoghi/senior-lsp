//! Documentos abertos no editor.
//!
//! O texto vem sempre do editor, pelo `didOpen` e pelo `didChange`, e nunca do disco. Assim o que
//! ainda não foi salvo também é analisado. A análise é refeita a cada mudança e fica guardada
//! para os pedidos do editor, como ir para a definição.

use std::collections::HashMap;

use lsp_types::Uri;

use crate::line_index::LineIndex;
use crate::symbols::Symbols;
use crate::{TextTooLarge, Tree};

pub(crate) struct Document {
    pub(crate) text: String,
    pub(crate) version: i32,
    pub(crate) tree: Tree,
    pub(crate) symbols: Symbols,
    pub(crate) index: LineIndex,
}

#[derive(Default)]
pub(crate) struct Documents {
    open: HashMap<Uri, Document>,
}

impl Documents {
    pub(crate) fn set(&mut self, uri: Uri, text: String, version: i32) -> Result<(), TextTooLarge> {
        let tree = crate::parse(&text)?;
        let symbols = Symbols::build(&tree, &text);
        let index = LineIndex::new(&text);
        let doc = Document {
            text,
            version,
            tree,
            symbols,
            index,
        };
        self.open.insert(uri, doc);
        Ok(())
    }

    pub(crate) fn get(&self, uri: &Uri) -> Option<&Document> {
        self.open.get(uri)
    }

    pub(crate) fn remove(&mut self, uri: &Uri) {
        self.open.remove(uri);
    }
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
