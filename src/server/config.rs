//! Arquivo `senior-lsp.json`, que muda o nível dos diagnósticos de uma pasta.
//!
//! ```json
//! {
//!     "diagnostics": { "SEN048": "off", "SEN071": "hint" },
//!     "overrides": [
//!         { "files": "Modelos/**", "diagnostics": { "SEN048": "off" } }
//!     ]
//! }
//! ```
//!
//! O servidor procura o arquivo subindo a partir da pasta do arquivo analisado, como o
//! `.editorconfig`, e usa o primeiro que achar. Os níveis são `off`, `hint`, `info`, `warning` e
//! `error`. Os `overrides` valem na ordem, e o último que casa com o arquivo vence. O `files` é um
//! glob relativo à pasta do `senior-lsp.json`, sem diferenciar maiúsculas, e aceita um texto ou uma
//! lista. Um arquivo com JSON inválido é ignorado, e o servidor avisa o editor.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use lsp_types::DiagnosticSeverity;
use serde_json::Value;

use super::directives;

pub(crate) const FILE_NAME: &str = "senior-lsp.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Off,
    Hint,
    Info,
    Warning,
    Error,
}

impl Level {
    fn parse(name: &str) -> Option<Level> {
        Some(match name.to_ascii_lowercase().as_str() {
            "off" => Level::Off,
            "hint" => Level::Hint,
            "info" => Level::Info,
            "warning" => Level::Warning,
            "error" => Level::Error,
            _ => return None,
        })
    }

    /// Severidade do protocolo, ou `None` para o diagnóstico que some.
    pub(crate) fn severity(self) -> Option<DiagnosticSeverity> {
        match self {
            Level::Off => None,
            Level::Hint => Some(DiagnosticSeverity::HINT),
            Level::Info => Some(DiagnosticSeverity::INFORMATION),
            Level::Warning => Some(DiagnosticSeverity::WARNING),
            Level::Error => Some(DiagnosticSeverity::ERROR),
        }
    }
}

/// Nível de cada código para um arquivo, já com os `overrides` aplicados. O código que não está
/// aqui fica como o servidor o emite.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Levels(HashMap<String, Level>);

impl Levels {
    #[cfg(test)]
    pub(crate) fn from_map(map: HashMap<String, Level>) -> Levels {
        Levels(map)
    }

    pub(crate) fn get(&self, code: &str) -> Option<Level> {
        self.0.get(code).copied()
    }
}

struct Override {
    files: GlobSet,
    levels: HashMap<String, Level>,
}

pub(crate) struct Config {
    dir: PathBuf,
    levels: HashMap<String, Level>,
    overrides: Vec<Override>,
}

impl Config {
    /// Lê o conteúdo de um `senior-lsp.json` que fica em `dir`. O erro é a razão de o arquivo
    /// inteiro ser ignorado, e `warnings` recebe o que foi ignorado dentro de um arquivo válido.
    fn parse(dir: &Path, text: &str, warnings: &mut Vec<String>) -> Result<Config, String> {
        let text = text.trim_start_matches('\u{feff}');
        let root: Value = serde_json::from_str(text).map_err(|e| format!("JSON inválido ({e})"))?;
        let Value::Object(root) = root else {
            return Err("a raiz precisa ser um objeto".to_owned());
        };
        let mut config = Config {
            dir: dir.to_path_buf(),
            levels: HashMap::new(),
            overrides: Vec::new(),
        };
        for (key, value) in &root {
            match key.as_str() {
                "diagnostics" => config.levels = levels(value, "diagnostics", warnings),
                "overrides" => {
                    let Value::Array(list) = value else {
                        warnings.push("\"overrides\" precisa ser uma lista.".to_owned());
                        continue;
                    };
                    for (i, entry) in list.iter().enumerate() {
                        if let Some(o) = parse_override(entry, i, warnings) {
                            config.overrides.push(o);
                        }
                    }
                }
                other => warnings.push(format!("A chave \"{other}\" não é usada.")),
            }
        }
        Ok(config)
    }

    /// Níveis que valem para o arquivo.
    pub(crate) fn levels_for(&self, file: &Path) -> Levels {
        let mut levels = self.levels.clone();
        if let Ok(relative) = file.strip_prefix(&self.dir) {
            let relative = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            for o in self
                .overrides
                .iter()
                .filter(|o| o.files.is_match(&relative))
            {
                levels.extend(o.levels.iter().map(|(k, v)| (k.clone(), *v)));
            }
        }
        Levels(levels)
    }
}

/// Mapa de código para nível. O que não vale vira aviso e fica de fora.
fn levels(value: &Value, place: &str, warnings: &mut Vec<String>) -> HashMap<String, Level> {
    let mut out = HashMap::new();
    let Value::Object(map) = value else {
        warnings.push(format!(
            "\"{place}\" precisa ser um objeto de código e nível."
        ));
        return out;
    };
    for (code, level) in map {
        let upper = code.to_ascii_uppercase();
        let Some(level) = level.as_str().and_then(Level::parse) else {
            warnings.push(format!(
                "O nível de {code} em \"{place}\" precisa ser off, hint, info, warning ou error."
            ));
            continue;
        };
        if directives::is_syntax(&upper) {
            warnings.push(format!("{upper} é erro de sintaxe e não muda de nível."));
        } else if !directives::is_known(&upper) {
            warnings.push(format!(
                "{code} não é um código de diagnóstico do senior-lsp."
            ));
        } else {
            out.insert(upper, level);
        }
    }
    out
}

fn parse_override(entry: &Value, i: usize, warnings: &mut Vec<String>) -> Option<Override> {
    let place = format!("overrides[{i}]");
    let Value::Object(map) = entry else {
        warnings.push(format!("\"{place}\" precisa ser um objeto."));
        return None;
    };
    let mut patterns: Vec<&str> = Vec::new();
    match map.get("files") {
        Some(Value::String(p)) => patterns.push(p),
        Some(Value::Array(list)) => patterns.extend(list.iter().filter_map(Value::as_str)),
        _ => {}
    }
    if patterns.is_empty() {
        warnings.push(format!(
            "\"{place}\" precisa de \"files\" com um glob ou uma lista de globs."
        ));
        return None;
    }
    let mut files = GlobSetBuilder::new();
    for pattern in patterns {
        let pattern = pattern.trim_start_matches("./");
        // Uma pasta, como `Modelos/`, vale para tudo o que há dentro dela.
        let pattern = match pattern.strip_suffix('/') {
            Some(dir) => format!("{dir}/**"),
            None => pattern.to_owned(),
        };
        let glob = GlobBuilder::new(&pattern)
            .case_insensitive(true)
            .literal_separator(true)
            .build();
        match glob {
            Ok(glob) => {
                files.add(glob);
            }
            Err(e) => warnings.push(format!(
                "O glob \"{pattern}\" de \"{place}\" é inválido ({e})."
            )),
        }
    }
    let files = files.build().ok()?;
    let mut inner = HashMap::new();
    for (key, value) in map {
        match key.as_str() {
            "files" => {}
            "diagnostics" => inner = levels(value, &format!("{place}.diagnostics"), warnings),
            other => warnings.push(format!("A chave \"{other}\" de \"{place}\" não é usada.")),
        }
    }
    Some(Override {
        files,
        levels: inner,
    })
}

/// Configurações já lidas, por pasta e por arquivo. O arquivo muda pouco, então a busca e a leitura
/// ficam guardadas até o editor avisar que um `senior-lsp.json` mudou.
#[derive(Default)]
pub(crate) struct Configs {
    /// `senior-lsp.json` que vale para uma pasta, ou nenhum.
    dirs: HashMap<PathBuf, Option<PathBuf>>,
    /// Conteúdo de cada `senior-lsp.json`, ou nenhum quando foi ignorado.
    files: HashMap<PathBuf, Option<Arc<Config>>>,
    /// Avisos já mandados ao editor, para não repetir a cada arquivo aberto enquanto o
    /// `senior-lsp.json` continua o mesmo.
    warned: HashSet<String>,
}

impl Configs {
    /// Esquece o que foi lido, para ler de novo na próxima consulta. Os avisos já mandados não se
    /// repetem.
    pub(crate) fn clear(&mut self) {
        self.dirs.clear();
        self.files.clear();
    }

    /// Como [`Configs::clear`], para quando o editor avisa que um `senior-lsp.json` mudou. Os avisos
    /// valem de novo, porque o arquivo pode ter piorado.
    pub(crate) fn reload(&mut self) {
        self.clear();
        self.warned.clear();
    }

    /// Níveis do arquivo. O que há para avisar o editor, como JSON inválido, vai para `warnings`
    /// uma vez por leitura do `senior-lsp.json`.
    pub(crate) fn levels(&mut self, file: &Path, warnings: &mut Vec<String>) -> Levels {
        let Some(path) = self.find(file) else {
            return Levels::default();
        };
        let config = self.files.entry(path.clone()).or_insert_with(|| {
            let mut found = Vec::new();
            let config = load(&path, &mut found);
            warnings.extend(found.into_iter().filter(|w| self.warned.insert(w.clone())));
            config
        });
        config
            .as_ref()
            .map(|c| c.levels_for(file))
            .unwrap_or_default()
    }

    /// `senior-lsp.json` mais perto do arquivo, subindo pelas pastas.
    fn find(&mut self, file: &Path) -> Option<PathBuf> {
        let mut visited = Vec::new();
        let mut found = None;
        for dir in file.ancestors().skip(1) {
            if let Some(known) = self.dirs.get(dir) {
                found = known.clone();
                break;
            }
            visited.push(dir.to_path_buf());
            let candidate = dir.join(FILE_NAME);
            if candidate.is_file() {
                found = Some(candidate);
                break;
            }
        }
        for dir in visited {
            self.dirs.insert(dir, found.clone());
        }
        found
    }
}

fn load(path: &Path, warnings: &mut Vec<String>) -> Option<Arc<Config>> {
    let shown = path.display();
    let Some(text) = super::workspace::read(path, false) else {
        warnings.push(format!(
            "O senior-lsp.json em {shown} não pôde ser lido e foi ignorado."
        ));
        return None;
    };
    let mut notes = Vec::new();
    let dir = path.parent().unwrap_or(Path::new(""));
    match Config::parse(dir, &text, &mut notes) {
        Ok(config) => {
            warnings.extend(notes.into_iter().map(|n| format!("{shown} {n}")));
            Some(Arc::new(config))
        }
        Err(reason) => {
            warnings.push(format!(
                "O senior-lsp.json em {shown} foi ignorado, {reason}."
            ));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> (Result<Config, String>, Vec<String>) {
        let mut warnings = Vec::new();
        let config = Config::parse(Path::new("/repo"), text, &mut warnings);
        (config, warnings)
    }

    fn level(config: &Config, file: &str, code: &str) -> Option<Level> {
        config.levels_for(Path::new(file)).get(code)
    }

    #[test]
    fn niveis_e_override_na_ordem() {
        let (config, warnings) = parse(
            r#"{
                "diagnostics": { "SEN048": "off", "sen071": "hint" },
                "overrides": [
                    { "files": "Modelos/**", "diagnostics": { "SEN048": "error", "SEN061": "info" } },
                    { "files": ["**/Rule.txt", "*.json"], "diagnostics": { "SEN048": "warning" } }
                ]
            }"#,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let config = config.unwrap();
        assert_eq!(
            level(&config, "/repo/Regras/A/x.txt", "SEN048"),
            Some(Level::Off)
        );
        assert_eq!(
            level(&config, "/repo/Regras/A/x.txt", "SEN071"),
            Some(Level::Hint)
        );
        assert_eq!(level(&config, "/repo/Regras/A/x.txt", "SEN040"), None);
        // O primeiro override casa e o segundo não.
        assert_eq!(
            level(&config, "/repo/modelos/M1/a.json", "SEN048"),
            Some(Level::Error)
        );
        assert_eq!(
            level(&config, "/repo/Modelos/M1/a.json", "SEN061"),
            Some(Level::Info)
        );
        // Os dois casam e o último vence.
        assert_eq!(
            level(&config, "/repo/Modelos/A/Rule.txt", "SEN048"),
            Some(Level::Warning)
        );
        // O `*` não atravessa pastas.
        assert_eq!(
            level(&config, "/repo/b.json", "SEN048"),
            Some(Level::Warning)
        );
        assert_eq!(
            level(&config, "/repo/Regras/b.json", "SEN048"),
            Some(Level::Off)
        );
    }

    #[test]
    fn pasta_com_barra_vale_para_o_que_ha_dentro() {
        let (config, _) = parse(
            r#"{ "overrides": [ { "files": "Modelos/", "diagnostics": { "SEN048": "off" } } ] }"#,
        );
        let config = config.unwrap();
        assert_eq!(
            level(&config, "/repo/Modelos/M/x/y.json", "SEN048"),
            Some(Level::Off)
        );
        assert_eq!(level(&config, "/repo/Regras/y.json", "SEN048"), None);
    }

    #[test]
    fn json_invalido_e_raiz_errada_ignoram_o_arquivo() {
        assert!(parse("{ \"diagnostics\": ").0.is_err());
        assert!(parse("[]").0.is_err());
        // O BOM do Windows não atrapalha.
        assert!(parse("\u{feff}{}").0.is_ok());
    }

    #[test]
    fn o_que_nao_vale_vira_aviso_e_o_resto_continua() {
        let (config, warnings) = parse(
            r#"{
                "extra": 1,
                "diagnostics": { "SEN020": "off", "SEN999": "off", "SEN048": "muito", "SEN041": "info" },
                "overrides": [ { "diagnostics": {} }, { "files": "[", "diagnostics": {} }, 3 ]
            }"#,
        );
        let config = config.unwrap();
        assert_eq!(warnings.len(), 7, "{warnings:?}");
        assert_eq!(level(&config, "/repo/a", "SEN041"), Some(Level::Info));
        assert_eq!(level(&config, "/repo/a", "SEN020"), None);
        assert_eq!(level(&config, "/repo/a", "SEN048"), None);
    }
}
