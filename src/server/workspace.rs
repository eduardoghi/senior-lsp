//! Diagnósticos das regras da pasta inteira, sem abrir cada arquivo.
//!
//! Ligado pela opção `analisar_pasta`. Ao iniciar, o servidor procura nas pastas do workspace os
//! `Rule.txt` e os JSON de `Modelos`, analisa cada um como está no disco e publica os
//! diagnósticos. Fica desligado por padrão porque no Claude Code os diagnósticos entram na
//! conversa, e milhares de arquivos encheriam o contexto.

use std::path::{Path, PathBuf};

use lsp_types::Uri;

use super::documents::{Documents, is_report, is_report_rule, is_rule};
use super::{Result, Server};

/// Pastas que nunca têm regra e só custam tempo.
const SKIPPED: [&str; 4] = [".git", "node_modules", "target", ".vscode"];

impl Server<'_> {
    /// Analisa todas as regras das pastas do workspace.
    pub(super) fn analyze_workspace(&mut self) -> Result<()> {
        if !self.analyze_folders {
            return Ok(());
        }
        let mut files = Vec::new();
        for folder in &self.folders {
            collect(folder, &mut files);
        }
        // Os JSON de um modelo dependem uns dos outros. Montar o modelo uma vez por pasta evita
        // reler a pasta inteira para cada JSON, que num repositório com milhares de modelos
        // levava dezenas de segundos.
        let mut models: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = Default::default();
        for path in files {
            let Ok(uri) = crate::form::uri_from_path(&path).parse::<Uri>() else {
                continue;
            };
            // A regra separada fica em `regras`, um nível abaixo da pasta do modelo.
            let model_dir = if is_report(&uri) {
                path.parent()
            } else if is_report_rule(&uri) {
                path.parent().and_then(|p| p.parent())
            } else {
                None
            };
            if let Some(dir) = model_dir {
                models.entry(dir.to_path_buf()).or_default().push(path);
            } else if is_rule(&uri) {
                self.analyze_rule(&path, &uri, false)?;
            }
        }
        for paths in models.values() {
            self.analyze_model(paths)?;
        }
        // As regras foram lidas e analisadas, e as definições delas já estão no índice.
        self.index.set_loaded();
        Ok(())
    }

    /// Os JSON e as regras separadas de um modelo, com o modelo montado uma vez.
    fn analyze_model(&mut self, paths: &[PathBuf]) -> Result<()> {
        let Some(first) = paths.first() else {
            return Ok(());
        };
        let Some(first_json) = read(first, false) else {
            return Ok(());
        };
        let model = crate::report::model(first, &first_json);
        let inherited = super::documents::model_inherited(&model);
        for path in paths {
            let Ok(uri) = crate::form::uri_from_path(path).parse::<Uri>() else {
                continue;
            };
            if self.documents.report(&uri).is_some() || self.documents.get(&uri).is_some() {
                continue;
            }
            let mut documents = Documents::default();
            if is_report_rule(&uri) {
                let Some(text) = read(path, true) else {
                    continue;
                };
                let context = crate::builtins::Context::Report;
                documents.set_with(uri.clone(), text, 0, context, inherited.clone())?;
                self.keep_symbols(&documents, &uri, path);
            } else {
                // O modelo já leu e decodificou todos os JSON da pasta.
                let known = model.texts.iter().find(|(p, _)| p == path);
                let Some(json) = known
                    .map(|(_, text)| text.clone())
                    .or_else(|| read(path, false).map(Into::into))
                else {
                    continue;
                };
                documents.set_report_in(uri.clone(), json, 0, &model, inherited.clone())?;
            }
            self.send_disk(&documents, uri, false)?;
        }
        Ok(())
    }

    /// Regra lida do disco. `publish_empty` publica também a lista vazia, para limpar o que o
    /// editor mostrava antes.
    fn analyze_rule(&mut self, path: &Path, uri: &Uri, publish_empty: bool) -> Result<()> {
        if self.documents.get(uri).is_some() {
            return Ok(());
        }
        let Some(text) = read(path, true) else {
            return Ok(());
        };
        let mut documents = Documents::default();
        documents.set(uri.clone(), text, 0, self.rule_context(uri))?;
        self.keep_symbols(&documents, uri, path);
        self.send_disk(&documents, uri.clone(), publish_empty)
    }

    /// Guarda no índice de símbolos as definições da regra que acabou de ser analisada.
    fn keep_symbols(&mut self, documents: &Documents, uri: &Uri, path: &Path) {
        if let Some(doc) = documents.get(uri) {
            self.index
                .insert(path, &doc.text, &doc.symbols, &doc.index, self.encoding);
        }
    }

    fn send_disk(&self, documents: &Documents, uri: Uri, publish_empty: bool) -> Result<()> {
        if let Some((diagnostics, _)) = self.diagnostics_of(documents, &uri)
            && (publish_empty || !diagnostics.is_empty() || self.published.borrow().contains(&uri))
        {
            self.send_diagnostics(uri, diagnostics, None)?;
        }
        Ok(())
    }

    /// Analisa o arquivo como está no disco e publica, a não ser que ele esteja aberto, quando
    /// vale o texto do editor.
    pub(super) fn analyze_disk(&mut self, path: &Path) -> Result<()> {
        let Ok(uri) = crate::form::uri_from_path(path).parse::<Uri>() else {
            return Ok(());
        };
        if self.documents.get(&uri).is_some() || self.documents.report(&uri).is_some() {
            return Ok(());
        }
        if is_report(&uri) {
            let Some(json) = read(path, false) else {
                return Ok(());
            };
            let mut documents = Documents::default();
            documents.set_report(uri.clone(), json, 0)?;
            self.send_disk(&documents, uri, true)
        } else if is_rule(&uri) || is_report_rule(&uri) {
            self.analyze_rule(path, &uri, true)
        } else {
            Ok(())
        }
    }

    /// Arquivo do disco que entra na análise da pasta.
    pub(super) fn analyzes(&self, path: &Path) -> bool {
        self.analyze_folders
            && crate::form::uri_from_path(path)
                .parse::<Uri>()
                .is_ok_and(|u| is_rule(&u) || is_report(&u) || is_report_rule(&u))
    }
}

/// Regras e JSON de modelo abaixo de `dir`.
pub(super) fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        match entry.file_type() {
            Ok(t) if t.is_dir() => {
                if !SKIPPED.iter().any(|s| name.eq_ignore_ascii_case(s)) {
                    collect(&path, files);
                }
            }
            Ok(t) if t.is_file() => {
                let candidate = name.eq_ignore_ascii_case("rule.txt")
                    || name.to_ascii_lowercase().ends_with(".json")
                    || crate::report::is_rule_file(&path);
                if candidate {
                    files.push(path);
                }
            }
            _ => {}
        }
    }
}

/// Texto do arquivo. As regras vêm em cp1252, como o gerenciador de artefatos exporta. O JSON
/// é lido em UTF-8 e, se não for, em cp1252.
pub(super) fn read(path: &Path, rule: bool) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if rule {
        return crate::cp1252::decode(&bytes);
    }
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(e) => crate::cp1252::decode(e.as_bytes()),
    }
}
