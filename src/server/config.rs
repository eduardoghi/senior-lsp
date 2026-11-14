//! Arquivo `senior-lsp.json`, que muda o nível dos diagnósticos de uma pasta e lista as tabelas
//! que existem no banco fora do dicionário de dados.
//!
//! ```json
//! {
//!     "diagnostics": { "SEN048": "off", "SEN071": "hint" },
//!     "overrides": [
//!         { "files": "Modelos/**", "diagnostics": { "SEN048": "off" } }
//!     ],
//!     "extraTables": ["TB_CRIADA_NO_BANCO"]
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

/// Nível de cada código para um arquivo, já com os `overrides` aplicados, e as tabelas do
/// `extraTables`. O código que não está aqui fica como o servidor o emite.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Levels {
    levels: HashMap<String, Level>,
    extra_tables: Arc<HashSet<String>>,
}

impl Levels {
    #[cfg(test)]
    pub(crate) fn from_map(levels: HashMap<String, Level>) -> Levels {
        Levels {
            levels,
            extra_tables: Arc::default(),
        }
    }

    pub(crate) fn get(&self, code: &str) -> Option<Level> {
        self.levels.get(code).copied()
    }

    /// A tabela está no `extraTables`, que lista as que existem no banco fora do dicionário, como
    /// uma criada direto no banco. Espera o nome em maiúsculas.
    pub(crate) fn extra_table(&self, table: &str) -> bool {
        self.extra_tables.contains(table)
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
    extra_tables: Arc<HashSet<String>>,
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
            extra_tables: Arc::default(),
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
                "extraTables" => config.extra_tables = Arc::new(extra_tables(value, warnings)),
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
        Levels {
            levels,
            extra_tables: self.extra_tables.clone(),
        }
    }
}

/// Nomes do `extraTables`, em maiúsculas. O que não é texto vira aviso e fica de fora.
fn extra_tables(value: &Value, warnings: &mut Vec<String>) -> HashSet<String> {
    let Value::Array(list) = value else {
        warnings.push("\"extraTables\" precisa ser uma lista de nomes de tabela.".to_owned());
        return HashSet::new();
    };
    let mut tables = HashSet::new();
    for (i, entry) in list.iter().enumerate() {
        match entry.as_str().map(str::trim).filter(|t| !t.is_empty()) {
            Some(table) => {
                tables.insert(table.to_ascii_uppercase());
            }
            None => warnings.push(format!(
                "\"extraTables\"[{i}] = {entry} não é um nome de tabela e foi ignorado."
            )),
        }
    }
    tables
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
    pub(crate) fn find(&mut self, file: &Path) -> Option<PathBuf> {
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

/// Comando da correção do SEN086, com o URI da regra e o nome da tabela.
pub(crate) const ADD_EXTRA_TABLE: &str = "senior-lsp.acrescentarTabelaExtra";

impl super::Server<'_> {
    /// A correção do SEN086 que acrescenta a tabela ao `extraTables` do `senior-lsp.json` que vale
    /// para a regra. É um comando, e não uma edição, porque o servidor grava o arquivo no disco e
    /// lê de novo a configuração. Uma edição deixaria o arquivo aberto sem salvar no editor, e o
    /// aviso continuaria até a pessoa salvar.
    pub(super) fn extra_table_actions(
        &self,
        context: &super::navigation::Context,
        range: lsp_types::Range,
    ) -> Vec<lsp_types::CodeAction> {
        let Some(file) = crate::form::path_from_uri(context.uri.as_str()) else {
            return Vec::new();
        };
        let (start, end) = (context.offset(range.start), context.offset(range.end));
        let wanted = text_size::TextRange::new(start.min(end), end.max(start));
        let levels = self.levels_of(context.uri);
        let mut tables: Vec<&str> = context
            .doc
            .checks()
            .iter()
            .filter(|c| c.kind == crate::validate::CheckKind::NativeUnknownTable)
            .filter(|c| c.range.intersect(wanted).is_some() || c.range.contains_inclusive(start))
            .filter_map(|c| c.label.as_deref())
            .filter(|t| !levels.extra_table(t))
            .collect();
        tables.dedup();
        tables
            .into_iter()
            .filter(|table| self.extra_table_file(&file, table).is_some())
            .map(|table| {
                let title = format!("Acrescentar {table} ao extraTables do {FILE_NAME}");
                let arguments = vec![
                    serde_json::json!(context.uri.as_str()),
                    serde_json::json!(table),
                ];
                lsp_types::CodeAction {
                    title: title.clone(),
                    kind: Some(lsp_types::CodeActionKind::QUICKFIX),
                    command: Some(lsp_types::Command::new(
                        title,
                        ADD_EXTRA_TABLE.to_owned(),
                        Some(arguments),
                    )),
                    ..lsp_types::CodeAction::default()
                }
            })
            .collect()
    }

    /// O `senior-lsp.json` que vale para a regra e o texto dele com a tabela no `extraTables`.
    /// Sem o arquivo, é um novo na pasta do workspace que tem a regra, ou na pasta da regra.
    fn extra_table_file(&self, rule: &Path, table: &str) -> Option<(PathBuf, String)> {
        let name = serde_json::to_string(table).ok()?;
        if let Some(path) = self.configs.borrow_mut().find(rule) {
            let text = super::workspace::read(&path, false)?;
            let (at, insert) = extra_table_insertion(&text, table)?;
            let text = format!("{}{insert}{}", &text[..at], &text[at..]);
            return Some((path, text));
        }
        let dir = self
            .folders
            .iter()
            .filter(|folder| rule.starts_with(folder))
            .max_by_key(|folder| folder.components().count())
            .map(PathBuf::as_path)
            .or_else(|| rule.parent())?;
        let path = dir.join(FILE_NAME);
        // Um arquivo que a busca não viu, criado depois dela sem aviso do editor, não pode ser
        // trocado por um novo.
        if path.exists() {
            return None;
        }
        Some((path, format!("{{\n    \"extraTables\": [{name}]\n}}\n")))
    }

    /// Atende o comando da correção do SEN086, que recebe o URI da regra e a tabela.
    pub(super) fn add_extra_table(
        &mut self,
        arguments: &[serde_json::Value],
    ) -> std::result::Result<(), String> {
        let [uri, table] = arguments else {
            return Err("o comando recebe o URI da regra e a tabela".to_owned());
        };
        let (Some(uri), Some(table)) = (uri.as_str(), table.as_str()) else {
            return Err("o URI e a tabela são textos".to_owned());
        };
        let rule = crate::form::path_from_uri(uri).ok_or("o URI não é de um arquivo")?;
        let (path, text) = self
            .extra_table_file(&rule, &table.to_ascii_uppercase())
            .ok_or_else(|| format!("o {FILE_NAME} da regra não pôde ser lido ou não é válido"))?;
        std::fs::write(&path, text)
            .map_err(|e| format!("não foi possível gravar {}: {e}", path.display()))?;
        self.reload_config().map_err(|e| e.to_string())
    }
}

/// Onde acrescentar `table` ao `extraTables` de um `senior-lsp.json`, com o texto a inserir. A
/// chave que falta entra logo depois do `{`, com o recuo das outras. `None` quando o arquivo não
/// é um objeto JSON válido ou o `extraTables` dele não é uma lista.
pub(crate) fn extra_table_insertion(text: &str, table: &str) -> Option<(usize, String)> {
    let root: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let name = serde_json::to_string(table).ok()?;
    match root.as_object()?.get("extraTables") {
        Some(Value::Array(_)) => {}
        Some(_) => return None,
        None => {
            let open = text.find('{')?;
            let rest = &text[open + 1..];
            let first = rest.find(|c: char| !c.is_whitespace())?;
            if rest[first..].starts_with('}') {
                return Some((open + 1, format!("\n    \"extraTables\": [{name}]\n")));
            }
            // O recuo da primeira chave, que vem depois da última quebra de linha antes dela.
            let before = &rest[..first];
            let indent = before.rsplit('\n').next().filter(|_| before.contains('\n'));
            let (newline, indent) = match indent {
                Some(indent) => ("\n", indent),
                None => ("", " "),
            };
            return Some((
                open + 1,
                format!("{newline}{indent}\"extraTables\": [{name}],"),
            ));
        }
    }
    let (open, close) = top_level_array(text, "extraTables")?;
    let last = text[open + 1..close].trim_end().len();
    if last == 0 {
        Some((open + 1, name))
    } else {
        Some((open + 1 + last, format!(", {name}")))
    }
}

/// Posição do `[` e do `]` da lista da chave `key` no primeiro nível do objeto.
fn top_level_array(text: &str, key: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let (mut depth, mut at) = (0usize, 0usize);
    let mut key_found = false;
    let mut open = None;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => {
                let start = at;
                at += 1;
                while at < bytes.len() && bytes[at] != b'"' {
                    at += if bytes[at] == b'\\' { 2 } else { 1 };
                }
                let after = text[(at + 1).min(text.len())..].trim_start();
                if depth == 1 && open.is_none() && after.starts_with(':') {
                    key_found = text.get(start + 1..at) == Some(key);
                }
            }
            b'{' | b'[' => {
                if bytes[at] == b'[' && depth == 1 && key_found && open.is_none() {
                    open = Some((at, depth + 1));
                }
                depth += 1;
            }
            b'}' | b']' => {
                depth = depth.checked_sub(1)?;
                if let Some((start, inner)) = open
                    && bytes[at] == b']'
                    && depth + 1 == inner
                {
                    return Some((start, at));
                }
            }
            b',' if depth == 1 => key_found = false,
            _ => {}
        }
        at += 1;
    }
    None
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

    #[test]
    fn tabelas_extras_em_maiusculas_e_o_que_nao_e_nome_vira_aviso() {
        let (config, warnings) = parse(r#"{ "extraTables": ["tb_a", " TB_B ", 3, ""] }"#);
        let levels = config.unwrap().levels_for(Path::new("/repo/R/Rule.txt"));
        assert!(levels.extra_table("TB_A") && levels.extra_table("TB_B"));
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        let (_, warnings) = parse(r#"{ "extraTables": "TB_A" }"#);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn acrescenta_ao_extratables() {
        let add = |text: &str| {
            let (at, insert) = extra_table_insertion(text, "TB_X").unwrap();
            format!("{}{insert}{}", &text[..at], &text[at..])
        };
        assert_eq!(add("{}"), "{\n    \"extraTables\": [\"TB_X\"]\n}");
        assert_eq!(
            add("{\n  \"diagnostics\": {}\n}"),
            "{\n  \"extraTables\": [\"TB_X\"],\n  \"diagnostics\": {}\n}"
        );
        assert_eq!(
            add(r#"{"diagnostics": {"SEN048": "off"}}"#),
            r#"{ "extraTables": ["TB_X"],"diagnostics": {"SEN048": "off"}}"#
        );
        assert_eq!(
            add(r#"{ "extraTables": [] }"#),
            r#"{ "extraTables": ["TB_X"] }"#
        );
        assert_eq!(
            add("{\n    \"extraTables\": [\n        \"TB_A\"\n    ]\n}"),
            "{\n    \"extraTables\": [\n        \"TB_A\", \"TB_X\"\n    ]\n}"
        );
        // A lista de mesmo nome dentro de um override não é a do primeiro nível.
        assert_eq!(
            add(r#"{"overrides": [{"extraTables": ["Z"]}], "extraTables": ["A"]}"#),
            r#"{"overrides": [{"extraTables": ["Z"]}], "extraTables": ["A", "TB_X"]}"#
        );
        assert!(extra_table_insertion(r#"{"extraTables": "A"}"#, "TB_X").is_none());
        assert!(extra_table_insertion("{", "TB_X").is_none());
        assert!(extra_table_insertion("[]", "TB_X").is_none());
    }
}
