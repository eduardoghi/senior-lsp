//! Busca de símbolos nas regras da pasta aberta, o `workspace/symbol` do editor.
//!
//! O índice guarda só o nome, o tipo e a posição de cada definição, sem a árvore da regra, para
//! caber na memória com milhares de arquivos. Ele é montado na primeira busca, lendo os
//! `Rule.txt` e as regras de relatório separadas das pastas do workspace, ou ao iniciar quando a
//! análise da pasta está ligada, que já lê e analisa esses arquivos. Depois disso só muda com os
//! eventos de arquivo do editor. Os documentos abertos entram na busca com o texto do editor, no
//! lugar do que o índice tem do disco. Os JSON de modelo de relatório ficam de fora, porque são
//! dezenas de milhares e a regra deles é lida do JSON.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use lsp_types::{Location, Position, Range, SymbolInformation, SymbolKind, Uri};

use super::Server;
use super::workspace::{collect, read};
use crate::line_index::{ColumnEncoding, LineIndex};
use crate::symbols::{DefKind, Symbols};

/// Número máximo de símbolos devolvidos numa busca.
const LIMIT: usize = 500;

/// Uma definição, com a posição do nome. O nome nunca atravessa linhas.
struct Entry {
    name: Box<str>,
    kind: SymbolKind,
    line: u32,
    start: u32,
    end: u32,
}

/// Definições de um arquivo e o nome mostrado como contêiner delas.
struct File {
    container: Box<str>,
    entries: Vec<Entry>,
}

#[derive(Default)]
pub(super) struct SymbolIndex {
    files: BTreeMap<PathBuf, File>,
    /// O índice já tem todas as regras das pastas. Antes disso, eventos de arquivo são ignorados,
    /// porque a primeira busca lê tudo do disco.
    loaded: bool,
}

impl SymbolIndex {
    pub(super) fn set_loaded(&mut self) {
        self.loaded = true;
    }

    /// Guarda as definições do arquivo, no lugar das que havia.
    pub(super) fn insert(
        &mut self,
        path: &Path,
        text: &str,
        symbols: &Symbols,
        index: &LineIndex,
        encoding: ColumnEncoding,
    ) {
        let file = File {
            container: container_name(path).into(),
            entries: entries(text, symbols, index, encoding),
        };
        self.files.insert(path.to_path_buf(), file);
    }

    pub(super) fn remove(&mut self, path: &Path) {
        self.files.remove(path);
    }
}

/// Diz se o arquivo é uma regra, a que o índice guarda.
pub(super) fn is_rule_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("rule.txt"))
        || crate::report::is_rule_file(path)
}

/// Nome da regra. A do gerenciador fica em `<NOME>.LSP/Rule.txt`, e o nome é a pasta sem o
/// `.LSP`. A regra de relatório separada leva o nome do arquivo sem a extensão.
fn container_name(path: &Path) -> String {
    let is_manager_rule = path
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("rule.txt"));
    let part = if is_manager_rule {
        path.parent().and_then(|p| p.file_name())
    } else {
        path.file_stem()
    };
    let name = part.map(|n| n.to_string_lossy().into_owned());
    let name = name.unwrap_or_default();
    match name
        .len()
        .checked_sub(4)
        .filter(|&at| name.is_char_boundary(at))
    {
        Some(at) if name[at..].eq_ignore_ascii_case(".lsp") => name[..at].to_owned(),
        _ => name,
    }
}

/// Funções, `definir` e rótulos do arquivo. Os parâmetros ficam de fora, porque só existem na
/// função. Toda variável é global no compilador, inclusive a definida dentro de uma função.
fn entries(
    text: &str,
    symbols: &Symbols,
    index: &LineIndex,
    encoding: ColumnEncoding,
) -> Vec<Entry> {
    symbols
        .definitions
        .iter()
        .filter_map(|def| {
            let kind = match def.kind {
                DefKind::Parameter(_) => return None,
                DefKind::Variable(_) => SymbolKind::VARIABLE,
                DefKind::Cursor => SymbolKind::OBJECT,
                DefKind::List => SymbolKind::ARRAY,
                DefKind::Table => SymbolKind::STRUCT,
                DefKind::WebService => SymbolKind::INTERFACE,
                DefKind::Function => SymbolKind::FUNCTION,
                DefKind::Label => SymbolKind::KEY,
            };
            let start = index.line_col(text, def.name.start(), encoding);
            let end = index.line_col(text, def.name.end(), encoding);
            Some(Entry {
                name: text[def.name].into(),
                kind,
                line: start.line,
                start: start.col,
                end: end.col,
            })
        })
        .collect()
}

/// Posição do nome da definição no arquivo.
fn range(entry: &Entry) -> Range {
    Range::new(
        Position::new(entry.line, entry.start),
        Position::new(entry.line, entry.end),
    )
}

/// Nota da correspondência de `name` com a consulta, que vem em minúsculas. Zero é o nome que
/// começa com a consulta, um é o que a contém e dois é o que tem as letras dela em ordem, com
/// outras no meio. `lower` é só um espaço de trabalho, para não alocar a cada nome.
fn score(name: &str, query: &str, lower: &mut String) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    lower.clear();
    lower.extend(name.chars().flat_map(char::to_lowercase));
    if lower.starts_with(query) {
        return Some(0);
    }
    if lower.contains(query) {
        return Some(1);
    }
    let mut wanted = query.chars().peekable();
    for c in lower.chars() {
        if wanted.peek() == Some(&c) {
            wanted.next();
        }
    }
    wanted.peek().is_none().then_some(2)
}

/// Onde as definições de uma fonte estão.
enum Place<'a> {
    Path(&'a Path),
    Uri(&'a Uri),
}

/// Documento aberto, com as definições do texto do editor.
struct Open {
    uri: Uri,
    path: Option<PathBuf>,
    file: File,
}

struct Hit<'a> {
    score: u8,
    entry: &'a Entry,
    container: &'a str,
    place: &'a Place<'a>,
}

/// As melhores correspondências com a consulta, no máximo `limit`. Com a consulta vazia, as
/// primeiras na ordem das fontes.
fn best<'a>(sources: &'a [(Place<'a>, &'a File)], query: &str, limit: usize) -> Vec<Hit<'a>> {
    let query = query.trim().to_lowercase();
    let mut lower = String::new();
    let mut hits = Vec::new();
    for (place, file) in sources {
        for entry in &file.entries {
            if query.is_empty() && hits.len() == limit {
                return hits;
            }
            if let Some(score) = score(&entry.name, &query, &mut lower) {
                hits.push(Hit {
                    score,
                    entry,
                    container: &file.container,
                    place,
                });
            }
        }
    }
    // A ordenação é estável, então nota e tamanho iguais ficam na ordem dos arquivos.
    hits.sort_by_key(|h| (h.score, h.entry.name.len()));
    hits.truncate(limit);
    hits
}

impl Server<'_> {
    /// Símbolos das regras que casam com a consulta.
    #[allow(deprecated)]
    pub(super) fn workspace_symbols(&mut self, query: &str) -> Vec<SymbolInformation> {
        if !self.index.loaded {
            self.load_symbols();
        }
        let open = self.open_symbols();
        let open_paths: HashSet<&Path> = open.iter().filter_map(|o| o.path.as_deref()).collect();
        let mut sources: Vec<(Place, &File)> = self
            .index
            .files
            .iter()
            .filter(|(path, _)| !open_paths.contains(path.as_path()))
            .map(|(path, file)| (Place::Path(path), file))
            .collect();
        sources.extend(open.iter().map(|o| (Place::Uri(&o.uri), &o.file)));
        best(&sources, query, LIMIT)
            .into_iter()
            .filter_map(|hit| {
                let uri = match hit.place {
                    Place::Uri(uri) => (*uri).clone(),
                    Place::Path(path) => crate::form::uri_from_path(path).parse().ok()?,
                };
                Some(SymbolInformation {
                    name: hit.entry.name.to_string(),
                    kind: hit.entry.kind,
                    tags: None,
                    deprecated: None,
                    location: Location::new(uri, range(hit.entry)),
                    container_name: Some(hit.container.to_owned()),
                })
            })
            .collect()
    }

    /// Lê do disco todas as regras das pastas do workspace.
    fn load_symbols(&mut self) {
        let mut files = Vec::new();
        for folder in &self.folders {
            collect(folder, &mut files);
        }
        for path in files.iter().filter(|p| is_rule_path(p)) {
            self.reload_symbols(path);
        }
        self.index.set_loaded();
    }

    /// Lê o arquivo do disco de novo e troca as definições dele no índice.
    pub(super) fn reload_symbols(&mut self, path: &Path) {
        let Some(text) = read(path, true) else {
            self.index.remove(path);
            return;
        };
        let Ok(tree) = crate::parse(&text) else {
            self.index.remove(path);
            return;
        };
        let symbols = Symbols::build(&tree, &text);
        let index = LineIndex::new(&text);
        self.index
            .insert(path, &text, &symbols, &index, self.encoding);
    }

    /// Reage a um evento de arquivo do editor.
    pub(super) fn update_symbols(&mut self, path: &Path, deleted: bool) {
        if !self.index.loaded || !is_rule_path(path) {
            return;
        }
        if deleted {
            self.index.remove(path);
        } else {
            self.reload_symbols(path);
        }
    }

    fn open_symbols(&self) -> Vec<Open> {
        self.documents
            .open_rules()
            .map(|(uri, doc)| {
                let path = crate::form::path_from_uri(uri.as_str());
                let container = path.as_deref().map(container_name).unwrap_or_default();
                let entries = entries(&doc.text, &doc.symbols, &doc.index, self.encoding);
                Open {
                    uri: uri.clone(),
                    path,
                    file: File {
                        container: container.into(),
                        entries,
                    },
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank(name: &str, query: &str) -> Option<u8> {
        score(name, &query.to_lowercase(), &mut String::new())
    }

    #[test]
    fn nota_da_correspondencia() {
        assert_eq!(rank("CalculaIcms", "calc"), Some(0));
        assert_eq!(rank("CalculaIcms", "ICMS"), Some(1));
        assert_eq!(rank("CalculaIcms", "cci"), Some(2));
        assert_eq!(rank("CalculaIcms", "icmsx"), None);
        assert_eq!(rank("CalculaIcms", "scl"), None);
        assert_eq!(rank("Ação", "aç"), Some(0));
        assert_eq!(rank("qualquer", ""), Some(0));
    }

    #[test]
    fn nome_do_conteiner() {
        let name = |p: &str| container_name(Path::new(p));
        assert_eq!(name("/x/regras/REGRA01.LSP/Rule.txt"), "REGRA01");
        assert_eq!(name("/x/regras/regra01.lsp/rule.txt"), "regra01");
        assert_eq!(
            name("/x/Formulários SGI/interface/F001EXE/rule.txt"),
            "F001EXE"
        );
        assert_eq!(
            name("/x/Modelos/M1.GER/regras/Cabecalho.Campo01.SOnPrint.txt"),
            "Cabecalho.Campo01.SOnPrint"
        );
        assert_eq!(name("Rule.txt"), "");
    }
}
