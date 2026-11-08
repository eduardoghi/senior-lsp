//! Regras do mesmo formulário SGI.
//!
//! Numa tela SGI, a regra da raiz do formulário define funções que as regras das ações e das
//! tabelas chamam. A exportação guarda o formulário como pastas, cada regra num `rule.txt`, então
//! as regras acima de um arquivo são as das pastas acima dele, até a raiz do formulário, que fica
//! dois níveis abaixo da pasta `Formulários SGI` (`Formulários SGI/interface/<formulário>`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use text_size::TextRange;

use crate::builtins::Param;
use crate::symbols::{DefKind, Symbols, VarType};
use crate::{Element, NodeId, SyntaxKind, Tree};

/// Função definida em outra regra do formulário.
#[derive(Debug, Clone)]
pub struct InheritedFunction {
    pub path: PathBuf,
    /// Texto da regra onde a função está, para converter a posição em linha e coluna.
    pub text: Arc<str>,
    /// Nome na `funcao`, ou no `definir funcao` se a regra não tiver a implementação.
    pub name: TextRange,
    /// Primeira linha da declaração, como `definir funcao Soma(numero p);`.
    pub decl: String,
    pub params: Vec<Param>,
}

/// Funções das regras acima, pelo nome em maiúsculas. A regra mais próxima vence.
#[derive(Debug, Clone, Default)]
pub struct Inherited {
    functions: HashMap<String, InheritedFunction>,
}

impl Inherited {
    pub fn get(&self, name: &str) -> Option<&InheritedFunction> {
        self.functions.get(&name.to_uppercase())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.functions.contains_key(&name.to_uppercase())
    }

    /// Junta as funções das regras dadas, da mais próxima para a mais distante.
    pub fn from_rules(rules: impl IntoIterator<Item = (PathBuf, String)>) -> Inherited {
        let mut inherited = Inherited::default();
        for (path, text) in rules {
            let text: Arc<str> = text.into();
            let Ok(tree) = crate::parse(&text) else {
                continue;
            };
            let symbols = Symbols::build(&tree, &text);
            let functions = symbols
                .definitions
                .iter()
                .filter(|d| d.kind == DefKind::Function);
            for def in functions {
                let key = text[def.name].to_uppercase();
                let is_decl = tree.kind(def.node) == SyntaxKind::FuncDecl;
                let entry = inherited.functions.entry(key);
                // Na mesma regra, o `definir funcao` dá os parâmetros e a `funcao` dá o lugar.
                let existing = match entry {
                    std::collections::hash_map::Entry::Occupied(e) => {
                        let f = e.into_mut();
                        if f.path != path {
                            continue;
                        }
                        f
                    }
                    std::collections::hash_map::Entry::Vacant(e) => e.insert(InheritedFunction {
                        path: path.clone(),
                        text: text.clone(),
                        name: def.name,
                        decl: String::new(),
                        params: Vec::new(),
                    }),
                };
                if is_decl || existing.decl.is_empty() {
                    existing.decl = header(&tree, &text, def.node);
                    existing.params = params(&tree, &text, def.node);
                }
                if !is_decl {
                    existing.name = def.name;
                }
            }
        }
        inherited
    }
}

/// Funções definidas nas regras das pastas acima de `rule` dentro do mesmo formulário. Vazio fora
/// de um formulário SGI. Regra que não pode ser lida fica de fora.
pub fn inherited_functions(rule: &Path) -> Inherited {
    let Some(root) = form_root(rule) else {
        return Inherited::default();
    };
    let mut rules = Vec::new();
    let mut dir = rule.parent();
    while let Some(d) = dir.and_then(Path::parent) {
        if !d.starts_with(&root) {
            break;
        }
        let path = d.join(rule_file(d));
        if let Some(text) = fs::read(&path).ok().and_then(|b| crate::cp1252::decode(&b)) {
            rules.push((path, text));
        }
        dir = Some(d);
    }
    Inherited::from_rules(rules)
}

/// Caminho local de uma URI `file://`. `None` para outro esquema.
pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    Some(PathBuf::from(crate::builtins::percent_decode(rest)))
}

/// URI `file://` de um caminho local, com os bytes fora de letras, dígitos e `/-._~` em `%XX`.
pub fn uri_from_path(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &b in path.to_string_lossy().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// Primeira linha da definição, sem o `{` que abre o corpo.
fn header(tree: &Tree, text: &str, node: NodeId) -> String {
    let full = &text[tree.text_range(node)];
    let first = full.lines().next().unwrap_or(full).trim_end();
    first
        .strip_suffix('{')
        .unwrap_or(first)
        .trim_end()
        .to_owned()
}

fn params(tree: &Tree, text: &str, function: NodeId) -> Vec<Param> {
    let Some(list) = tree
        .children(function)
        .find(|&c| tree.kind(c) == SyntaxKind::ParamList)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for param in tree.children(list) {
        let mut ty = None;
        let mut by_ref = false;
        for e in tree.elements(param) {
            if let Element::Token(t) = e {
                match tree.tokens().kind(t) {
                    SyntaxKind::AlfaKw => ty = Some(VarType::Alfa),
                    SyntaxKind::NumeroKw => ty = Some(VarType::Numero),
                    SyntaxKind::DataKw => ty = Some(VarType::Data),
                    SyntaxKind::EndKw => by_ref = true,
                    _ => {}
                }
            }
        }
        let name = tree
            .children(param)
            .find(|&c| tree.kind(c) == SyntaxKind::Name)
            .map_or("", |n| &text[tree.text_range(n)]);
        if let Some(ty) = ty {
            out.push(Param {
                ty,
                by_ref,
                name: name.to_owned(),
            });
        }
    }
    out
}

/// Pasta do formulário, dois níveis abaixo de `Formulários SGI`.
fn form_root(rule: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = rule.components().collect();
    let i = parts.iter().position(|c| {
        c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("Formulários SGI")
    })?;
    // Precisa ter pelo menos o tipo, o formulário e uma pasta dentro dele antes do arquivo.
    if parts.len() < i + 5 {
        return None;
    }
    Some(parts[..i + 3].iter().collect())
}

/// Nome do arquivo de regra na pasta, sem diferença de caixa. `rule.txt` se não houver.
fn rule_file(dir: &Path) -> String {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.eq_ignore_ascii_case("rule.txt"))
        .unwrap_or_else(|| "rule.txt".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn junta_as_funcoes_das_pastas_acima() {
        let base = std::env::temp_dir().join(format!("senior-lsp-form-{}", std::process::id()));
        let form = base.join("Formulários SGI/interface/F1");
        let action = form.join("actions/A1");
        fs::create_dir_all(&action).unwrap();
        fs::write(
            form.join("rule.txt"),
            "definir funcao Global(numero p, alfa end r);\nfuncao Global(numero p, alfa end r); { }",
        )
        .unwrap();
        fs::write(form.join("actions/rule.txt"), "definir funcao DoMeio();").unwrap();
        fs::write(action.join("rule.txt"), "definir funcao Propria();").unwrap();
        let inherited = inherited_functions(&action.join("rule.txt"));
        assert!(inherited.contains("global") && inherited.contains("DOMEIO"));
        // A própria regra não entra, porque o editor manda o texto dela.
        assert!(!inherited.contains("Propria"));

        let global = inherited.get("Global").unwrap();
        assert_eq!(global.path, form.join("rule.txt"));
        assert_eq!(global.decl, "definir funcao Global(numero p, alfa end r);");
        // O lugar é o da `funcao`, na segunda linha.
        assert_eq!(
            &global.text[..usize::from(global.name.start())]
                .matches('\n')
                .count(),
            &1
        );
        assert_eq!(global.params.len(), 2);
        assert!(global.params[1].by_ref);

        // Fora de formulário SGI não há regras herdadas.
        assert!(!inherited_functions(&base.join("regras/R1/Rule.txt")).contains("Global"));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn uri_e_caminho_vao_e_voltam() {
        let path = Path::new("/c/Formulários SGI/interface/F 1/rule.txt");
        let uri = uri_from_path(path);
        assert_eq!(
            uri,
            "file:///c/Formul%C3%A1rios%20SGI/interface/F%201/rule.txt"
        );
        assert_eq!(path_from_uri(&uri).unwrap(), path);
    }
}
