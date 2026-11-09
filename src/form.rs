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

/// Função ou variável definida em outra regra do formulário.
#[derive(Debug, Clone)]
pub struct InheritedDef {
    pub kind: DefKind,
    pub path: PathBuf,
    /// Texto da regra onde a função está, para converter a posição em linha e coluna.
    pub text: Arc<str>,
    /// Nome na `funcao`, ou no `definir funcao` se a regra não tiver a implementação.
    pub name: TextRange,
    /// Primeira linha da declaração, como `definir funcao Soma(numero p);`.
    pub decl: String,
    /// Parâmetros, nas funções.
    pub params: Vec<Param>,
    /// Quantas regras definem a função. Num modelo de relatório, mais de uma deixa em dúvida qual
    /// assinatura vale.
    pub rules: usize,
    /// Entrada ou controle do relatório, cadastrado no modelo e não por `definir`.
    pub entry: Option<crate::report::EntryKind>,
    /// Regra de modelo de relatório de onde a definição veio, para levar a posição ao JSON.
    /// `None` quando `text` é o próprio arquivo.
    pub embedded: Option<Arc<crate::report::Embedded>>,
}

/// Funções e variáveis das regras acima, pelo nome em maiúsculas. A regra mais próxima vence.
/// Variáveis aqui são tudo que um `definir` cria fora de função, como variável, cursor, lista,
/// tabela e web service.
#[derive(Debug, Clone, Default)]
pub struct Inherited {
    functions: HashMap<String, InheritedDef>,
    variables: HashMap<String, InheritedDef>,
    /// Nomes das entradas e dos controles do relatório, em maiúsculas, mesmo os que uma regra
    /// também define.
    entries: std::collections::HashSet<String>,
}

impl Inherited {
    pub fn get(&self, name: &str) -> Option<&InheritedDef> {
        self.functions.get(&name.to_uppercase())
    }

    pub fn functions(&self) -> impl Iterator<Item = &InheritedDef> {
        self.functions.values()
    }

    pub fn variables(&self) -> impl Iterator<Item = &InheritedDef> {
        self.variables.values()
    }

    /// Diz se o nome é de uma entrada ou de um controle do relatório. Ele vem do cadastro do
    /// modelo, então renomear só nas regras quebra a ligação.
    pub fn is_entry(&self, name: &str) -> bool {
        self.entries.contains(&name.to_uppercase())
    }

    pub fn variable(&self, name: &str) -> Option<&InheritedDef> {
        self.variables.get(&name.to_uppercase())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.functions.contains_key(&name.to_uppercase())
    }

    /// Junta as funções das regras dadas, da mais próxima para a mais distante.
    pub fn from_rules(rules: impl IntoIterator<Item = (PathBuf, String)>) -> Inherited {
        let rules = rules
            .into_iter()
            .map(|(path, text)| (path, Arc::from(text), None));
        Inherited::collect(rules, false)
    }

    /// Junta as definições das regras de um modelo de relatório, que se enxergam umas às outras
    /// sem ordem. Um nome definido com tipos diferentes em duas regras fica sem tipo, para não
    /// dar erro de tipo com o tipo errado.
    ///
    /// As entradas do relatório entram como variáveis, menos as que alguma regra define com
    /// `definir`, que vale primeiro.
    pub fn from_model(model: &crate::report::Model) -> Inherited {
        let rules = model.rules.iter().map(|(path, embedded)| {
            let text: Arc<str> = Arc::from(embedded.rule.text.as_str());
            (path.clone(), text, Some(embedded.clone()))
        });
        let mut inherited = Inherited::collect(rules, true);
        // A regra que o ERP monta antes das outras define as entradas e depois as descrições. Os
        // outros controles só valem se nada definiu o nome.
        let order = |e: &crate::report::Entry| match e.kind {
            crate::report::EntryKind::Entrance => 0,
            crate::report::EntryKind::Control(crate::report::Control::Descricao) => 1,
            crate::report::EntryKind::Control(_) => 2,
        };
        let mut entries: Vec<_> = model.entries.iter().collect();
        entries.sort_by_key(|(_, _, e)| order(e));
        for (path, json, entry) in entries {
            let key = entry.name.to_uppercase();
            inherited.entries.insert(key.clone());
            if inherited.variables.contains_key(&key) {
                continue;
            }
            let ty = entry.ty.map_or("", |t| match t {
                VarType::Alfa => "alfa",
                VarType::Numero => "numero",
                VarType::Data => "data",
            });
            let def = InheritedDef {
                kind: DefKind::Variable(entry.ty),
                path: path.clone(),
                text: json.clone(),
                name: entry.name_range,
                decl: match entry.kind {
                    crate::report::EntryKind::Entrance => {
                        format!("{ty} {}  @ {} @", entry.name, entry.description)
                    }
                    crate::report::EntryKind::Control(_) => format!("{ty} {}", entry.name),
                },
                params: Vec::new(),
                rules: 1,
                entry: Some(entry.kind),
                embedded: None,
            };
            inherited.variables.insert(key, def);
        }
        inherited
    }

    fn collect(
        rules: impl Iterator<Item = (PathBuf, Arc<str>, Option<Arc<crate::report::Embedded>>)>,
        unordered: bool,
    ) -> Inherited {
        let mut inherited = Inherited::default();
        // Pares de função e regra já contados em `rules`.
        let mut counted: std::collections::HashSet<(String, (PathBuf, usize))> =
            std::collections::HashSet::new();
        for (path, text, embedded) in rules {
            let Ok(tree) = crate::parse(&text) else {
                continue;
            };
            let symbols = Symbols::build(&tree, &text);
            for def in &symbols.definitions {
                let key = text[def.name].to_uppercase();
                if def.kind != DefKind::Function {
                    let is_value = !matches!(def.kind, DefKind::Parameter(_) | DefKind::Label);
                    if !is_value {
                        continue;
                    }
                    if let Some(existing) = inherited.variables.get_mut(&key) {
                        if unordered && existing.kind != def.kind {
                            existing.kind = DefKind::Variable(None);
                        }
                        continue;
                    }
                    let var = InheritedDef {
                        kind: def.kind,
                        path: path.clone(),
                        text: text.clone(),
                        name: def.name,
                        decl: header(&tree, &text, def.node),
                        params: Vec::new(),
                        rules: 1,
                        entry: None,
                        embedded: embedded.clone(),
                    };
                    inherited.variables.insert(key, var);
                    continue;
                }
                let is_decl = tree.kind(def.node) == SyntaxKind::FuncDecl;
                // Primeira vez que esta regra define esta função.
                let new_rule = counted.insert((key.clone(), rule_key(&path, &text)));
                let entry = inherited.functions.entry(key);
                // Na mesma regra, o `definir funcao` dá os parâmetros e a `funcao` dá o lugar.
                let existing = match entry {
                    std::collections::hash_map::Entry::Occupied(e) => {
                        let f = e.into_mut();
                        let same_rule = f.path == path && Arc::ptr_eq(&f.text, &text);
                        if !same_rule {
                            // Conta a regra uma vez só, mesmo com `definir funcao` e `funcao`.
                            if new_rule {
                                f.rules += 1;
                            }
                            continue;
                        }
                        f
                    }
                    std::collections::hash_map::Entry::Vacant(e) => e.insert(InheritedDef {
                        kind: DefKind::Function,
                        path: path.clone(),
                        text: text.clone(),
                        name: def.name,
                        decl: String::new(),
                        params: Vec::new(),
                        rules: 1,
                        entry: None,
                        embedded: embedded.clone(),
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

/// Regras abaixo de `rule` no formulário, os `rule.txt` das subpastas da pasta dele, em ordem.
/// Vazio se `rule` não está num formulário SGI.
pub fn descendant_rules(rule: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !in_form(rule) {
        return out;
    }
    let Some(dir) = rule.parent() else {
        return out;
    };
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        for entry in fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = entry.path();
            // `file_type` não segue link simbólico. Seguir um link para uma pasta acima faria a
            // busca rodar sem fim.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file()
                && path != rule
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("rule.txt"))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Usos de `name` no texto de uma regra, fora de campo depois de ponto. Com `own`, só os que
/// apontam para uma definição do próprio texto. Sem `own`, só os que não apontam para nada dele,
/// que vêm de uma regra acima.
pub fn uses(text: &str, name: &str, own: bool) -> Vec<TextRange> {
    let Ok(tree) = crate::parse(text) else {
        return Vec::new();
    };
    let symbols = Symbols::build(&tree, text);
    let mut out = Vec::new();
    let mut stack = vec![tree.root()];
    while let Some(node) = stack.pop() {
        stack.extend(tree.children(node));
        if tree.kind(node) != SyntaxKind::NameRef {
            continue;
        }
        let range = tree.text_range(node);
        let member = tree.parent(node).is_some_and(|p| {
            tree.kind(p) == SyntaxKind::FieldExpr && tree.children(p).next() != Some(node)
        });
        if member || !text[range].eq_ignore_ascii_case(name) {
            continue;
        }
        if symbols.reference_at(range).is_some() == own {
            out.push(range);
        }
    }
    out.sort_by_key(|r| r.start());
    out
}

/// Diz se o caminho está dentro de um formulário SGI, a raiz dele inclusive.
fn in_form(rule: &Path) -> bool {
    let parts: Vec<_> = rule.components().collect();
    parts
        .iter()
        .position(|c| {
            c.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case("Formulários SGI")
        })
        .is_some_and(|i| parts.len() >= i + 4)
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

/// Identifica uma regra pelo arquivo e pelo endereço do texto dela.
fn rule_key(path: &Path, text: &Arc<str>) -> (PathBuf, usize) {
    (path.to_path_buf(), Arc::as_ptr(text).cast::<u8>() as usize)
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
        fs::write(
            form.join("actions/rule.txt"),
            "definir funcao DoMeio();\ndefinir numero nMeio;",
        )
        .unwrap();
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
        assert!(inherited.variable("Global").is_none());
        let var = inherited.variable("NMEIO").unwrap();
        assert_eq!(var.kind, DefKind::Variable(Some(VarType::Numero)));
        assert_eq!(var.decl, "definir numero nMeio;");

        // Fora de formulário SGI não há regras herdadas.
        assert!(!inherited_functions(&base.join("regras/R1/Rule.txt")).contains("Global"));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn regras_abaixo_e_usos() {
        let base = std::env::temp_dir().join(format!("senior-lsp-desc-{}", std::process::id()));
        let form = base.join("Formulários SGI/interface/F1");
        fs::create_dir_all(form.join("actions/A1")).unwrap();
        fs::create_dir_all(form.join("tables/T1/actions/A2")).unwrap();
        for p in [
            "rule.txt",
            "actions/A1/rule.txt",
            "tables/T1/actions/A2/rule.txt",
        ] {
            fs::write(form.join(p), "").unwrap();
        }
        let below = descendant_rules(&form.join("rule.txt"));
        assert_eq!(
            below,
            [
                form.join("actions/A1/rule.txt"),
                form.join("tables/T1/actions/A2/rule.txt")
            ]
        );
        assert!(descendant_rules(&base.join("regras/R/Rule.txt")).is_empty());
        // Link para uma pasta acima não faz a busca rodar sem fim.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&form, form.join("actions/A1/volta")).unwrap();
            assert_eq!(descendant_rules(&form.join("rule.txt")).len(), 2);
        }
        fs::remove_dir_all(&base).unwrap();

        let text = "definir numero x;\nx = n + 1; cur.n = 2; f(n);";
        assert_eq!(uses(text, "x", true).len(), 1);
        assert!(uses(text, "x", false).is_empty());
        // `cur.n` é campo e não conta.
        assert_eq!(uses(text, "N", false).len(), 2);
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
