//! Ir para a regra da porta de um web service interno a partir do `definir`.
//!
//! Em `Definir interno.com.empresa.vendas.CalcularComissao ws;` o primeiro trecho é o
//! provedor, o último é a porta e o meio é o serviço. A exportação do gerenciador de artefatos
//! guarda a regra da porta em `Web Services/<provedor>/<serviço>/<porta>/Rule.txt`, e a pasta
//! `Web Services` é procurada subindo a partir do arquivo aberto.

use std::path::{Path, PathBuf};

use lsp_types::{Location, Position, Range, Uri};
use text_size::TextSize;

use super::navigation::Context;
use crate::SyntaxKind;

impl Context<'_> {
    /// Regra da porta do web service cujo nome qualificado está em `offset`.
    pub(crate) fn webservice_definition(&self, offset: TextSize) -> Option<Location> {
        let tree = &self.doc.tree;
        let mut node = tree.root();
        let qualified = loop {
            let child = tree
                .children(node)
                .find(|&c| tree.text_range(c).contains_inclusive(offset))?;
            if tree.kind(child) == SyntaxKind::QualifiedName {
                break child;
            }
            node = child;
        };
        if tree.parent(qualified).map(|p| tree.kind(p)) != Some(SyntaxKind::WsDef) {
            return None;
        }
        let name = &self.doc.text[tree.text_range(qualified)];
        let parts: Vec<&str> = name.split('.').map(str::trim).collect();
        let [provider, service @ .., port] = parts.as_slice() else {
            return None;
        };
        if service.is_empty() {
            return None;
        }
        let service = service.join(".");
        let file = crate::form::path_from_uri(self.uri.as_str())?;
        let rule = file.ancestors().skip(1).find_map(|dir| {
            let root = child(dir, "Web Services")?;
            let port_dir = child(&child(&child(&root, provider)?, &service)?, port)?;
            child(&port_dir, "Rule.txt")
        })?;
        let uri: Uri = crate::form::uri_from_path(&rule).parse().ok()?;
        Some(Location::new(
            uri,
            Range::new(Position::new(0, 0), Position::new(0, 0)),
        ))
    }
}

/// Entrada da pasta com o nome, sem diferenciar maiúsculas, como o ERP e o Windows.
fn child(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}
