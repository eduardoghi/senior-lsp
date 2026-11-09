//! Regras do mesmo formulário SGI.
//!
//! O ERP compila todas as regras de uma interface SGI num programa só, numa ordem fixa (veja
//! [`form_rules`]), e o que uma regra define vale nas seguintes. A exportação guarda o formulário
//! como pastas, cada regra num `rule.txt`, com a raiz dois níveis abaixo da pasta
//! `Formulários SGI` (`Formulários SGI/interface/<formulário>`).

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

/// Funções e variáveis de outras regras, pelo nome em maiúsculas. A regra mais próxima vence.
/// Variáveis aqui são tudo que um `definir` cria fora de função, como variável, cursor, lista,
/// tabela e web service.
#[derive(Debug, Clone, Default)]
pub struct Inherited {
    functions: HashMap<String, InheritedDef>,
    variables: HashMap<String, InheritedDef>,
    /// Nomes das entradas e dos controles do relatório, em maiúsculas, mesmo os que uma regra
    /// também define.
    entries: std::collections::HashSet<String>,
    /// Dicionários de dados configurados, para o tipo de `TABELA.Campo`.
    pub dictionaries: crate::dictionary::Dictionaries,
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
            crate::report::EntryKind::Control(_) | crate::report::EntryKind::Field => 2,
        };
        let mut entries: Vec<_> = model.entries.iter().collect();
        entries.sort_by_key(|(_, _, e)| order(e));
        inherited.add_entries(entries.into_iter().map(|(p, j, e)| (p, j, e)));
        inherited
    }

    /// Acrescenta nomes cadastrados fora das regras, como entradas de relatório e campos de tela,
    /// que só valem quando nenhuma regra definiu o nome.
    fn add_entries<'a>(
        &mut self,
        entries: impl Iterator<Item = (&'a PathBuf, &'a Arc<str>, &'a crate::report::Entry)>,
    ) {
        let inherited = self;
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
                    crate::report::EntryKind::Field => {
                        format!("{ty} {}  @ {} @", entry.name, entry.description)
                    }
                },
                params: Vec::new(),
                rules: 1,
                entry: Some(entry.kind),
                embedded: None,
            };
            inherited.variables.insert(key, def);
        }
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

/// Funções e variáveis que valem para `rule` num formulário SGI, as das regras que vêm antes dela
/// na ordem de compilação e os campos da tela. Vazio fora de um formulário SGI. Regra que não pode
/// ser lida fica de fora.
pub fn inherited_functions(rule: &Path) -> Inherited {
    let Some(root) = form_dir(rule) else {
        return Inherited::default();
    };
    let order = form_rules(&root);
    let before = position(&order, rule).map_or(&order[..0], |i| &order[..i]);
    // A regra mais próxima vem primeiro, e `from_rules` dá preferência a ela.
    let rules = before.iter().rev().filter_map(|path| {
        let text = fs::read(path)
            .ok()
            .and_then(|b| crate::cp1252::decode(&b))?;
        Some((path.clone(), text))
    });
    let mut inherited = Inherited::from_rules(rules.collect::<Vec<_>>());
    let dictionaries = crate::dictionary::current();
    let fields = form_fields(&root, &dictionaries);
    inherited.add_entries(fields.iter().map(|(p, j, e)| (p, j, e)));
    inherited.dictionaries = dictionaries;
    inherited
}

/// Regras do formulário na ordem em que o ERP compila. O ERP junta todas as regras da interface
/// num programa só, e uma variável definida numa regra vale nas seguintes. A ordem é a regra da
/// interface, as ações da interface e, para cada tabela, a regra dela e as ações dela. Tabelas e
/// ações da interface vão em ordem alfabética, como a importação do gerenciador de artefatos
/// grava, e as ações da tabela na ordem da lista `actions` do `definition.json` dela.
pub fn form_rules(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // A pasta entra mesmo sem o arquivo, porque a regra pode estar só no editor, ainda sem salvar.
    let mut push = |dir: &Path| out.push(dir.join(rule_file(dir)));
    push(root);
    for action in subdirs(&root.join("actions")) {
        push(&action);
    }
    for table in subdirs(&root.join("tables")) {
        push(&table);
        let listed = fs::read(table.join("definition.json"))
            .ok()
            .and_then(|b| {
                std::string::String::from_utf8(b.clone())
                    .ok()
                    .or_else(|| crate::cp1252::decode(&b))
            })
            .unwrap_or_default()
            .to_lowercase();
        let actions_at = listed.find("\"actions\"").unwrap_or(listed.len());
        let mut actions = subdirs(&table.join("actions"));
        actions.sort_by_key(|a| {
            let name = a
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let at = listed[actions_at..]
                .find(&format!("\"{name}\""))
                .unwrap_or(usize::MAX);
            (at, name)
        });
        for action in actions {
            push(&action);
        }
    }
    out
}

/// Regras que vêm depois de `rule` na ordem de compilação do formulário. Vazio se `rule` não está
/// num formulário SGI.
pub fn following_rules(rule: &Path) -> Vec<PathBuf> {
    let Some(root) = form_dir(rule) else {
        return Vec::new();
    };
    let order = form_rules(&root);
    match position(&order, rule) {
        Some(i) => order[i + 1..].to_vec(),
        None => Vec::new(),
    }
}

/// Posição de `rule` na ordem, pela pasta, porque o nome do arquivo pode variar de caixa.
fn position(order: &[PathBuf], rule: &Path) -> Option<usize> {
    let dir = rule.parent()?;
    order.iter().position(|p| p.parent() == Some(dir))
}

/// Subpastas, sem seguir link simbólico, em ordem alfabética sem diferença de caixa.
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    out.sort_by_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    out
}

/// Campos da tela, que valem como variáveis em todas as regras do formulário. Os da interface
/// ficam com o nome do cadastro, e os de cada tabela com o nome da tabela na frente
/// (`E160CTR_CodEmp`) e também com `_old` no fim, como o ERP monta. Os campos que o SGI acrescenta sozinho, como `CodOpe`,
/// não estão na exportação e ficam em `builtins`.
///
/// O campo de busca mostra um campo de outra tabela, e o tipo dele vem do dicionário de dados,
/// pela tabela de destino da ligação. Sem dicionário, ou com a ligação ou o campo fora dele, fica
/// sem tipo.
fn form_fields(
    root: &Path,
    dictionaries: &crate::dictionary::Dictionaries,
) -> Vec<(PathBuf, Arc<str>, crate::report::Entry)> {
    let links = if dictionaries.is_empty() {
        HashMap::new()
    } else {
        read_json(&root.join("interfaces").join("definition.json"))
            .map(|json| link_targets(&json))
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    let mut add = |path: PathBuf, prefix: &str| {
        let Some(json) = read_json(&path) else {
            return;
        };
        for mut entry in crate::report::scan(&json).1 {
            if entry.kind != crate::report::EntryKind::Field {
                continue;
            }
            if let Some((link, field)) = &entry.lookup {
                entry.ty = links
                    .get(&link.to_uppercase())
                    .and_then(|table| dictionaries.field(table, field))
                    .and_then(|found| found.field.ty.var_type());
            }
            if prefix.is_empty() {
                out.push((path.clone(), json.clone(), entry));
                continue;
            }
            // O campo da tabela vem também com o valor anterior, em `TABELA_CAMPO_old`.
            entry.name = format!("{prefix}_{}", entry.name);
            let mut old = entry.clone();
            old.name.push_str("_old");
            out.push((path.clone(), json.clone(), entry));
            out.push((path.clone(), json.clone(), old));
        }
    };
    add(root.join("fields").join("definition.json"), "");
    let mut tables: Vec<_> = fs::read_dir(root.join("tables"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    tables.sort();
    for table in tables {
        let name = table
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        add(table.join("fields").join("definition.json"), &name);
    }
    out
}

fn read_json(path: &Path) -> Option<Arc<str>> {
    let bytes = fs::read(path).ok()?;
    match std::string::String::from_utf8(bytes) {
        Ok(t) => Some(t.into()),
        Err(e) => crate::cp1252::decode(e.as_bytes()).map(Into::into),
    }
}

/// Tabela de destino de cada ligação da interface, pelo nome da ligação em maiúsculas. Cada
/// ligação é um objeto com `name` e `targetTable`, e os campos dela (`source` e `target`) não têm
/// `name`, então o último `name` antes do `targetTable` é o da ligação.
fn link_targets(json: &str) -> HashMap<std::string::String, std::string::String> {
    let value = |at: usize| -> Option<&str> {
        let rest = json[at..].trim_start().strip_prefix(':')?.trim_start();
        let rest = rest.strip_prefix('"')?;
        Some(&rest[..rest.find('"')?])
    };
    let mut out = HashMap::new();
    let mut name = None;
    let mut pos = 0;
    while let Some(i) = json[pos..].find('"') {
        let start = pos + i + 1;
        let Some(len) = json[start..].find('"') else {
            break;
        };
        let key = &json[start..start + len];
        pos = start + len + 1;
        match key {
            "name" => name = value(pos),
            "targetTable" => {
                if let (Some(n), Some(t)) = (name.take(), value(pos)) {
                    out.insert(n.to_uppercase(), t.to_owned());
                }
            }
            _ => {}
        }
    }
    out
}

/// Usos de `name` no texto de uma regra, fora de campo depois de ponto. Com `own`, só os que
/// apontam para uma definição do próprio texto. Sem `own`, só os que não apontam para nada dele,
/// que vêm de outra regra.
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

/// Pasta do formulário, dois níveis abaixo de `Formulários SGI`, para um arquivo dentro dela em
/// qualquer nível.
fn form_dir(rule: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = rule.components().collect();
    let i = parts.iter().position(|c| {
        c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("Formulários SGI")
    })?;
    // Precisa ter pelo menos o tipo e o formulário antes do arquivo.
    if parts.len() < i + 4 {
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
    fn junta_as_funcoes_das_regras_anteriores() {
        let base = std::env::temp_dir().join(format!("senior-lsp-form-{}", std::process::id()));
        let form = base.join("Formulários SGI/interface/F1");
        fs::create_dir_all(form.join("actions/A1")).unwrap();
        fs::create_dir_all(form.join("tables/T1")).unwrap();
        fs::write(
            form.join("rule.txt"),
            "definir funcao Global(numero p, alfa end r);\nfuncao Global(numero p, alfa end r); { }",
        )
        .unwrap();
        fs::write(
            form.join("actions/A1/rule.txt"),
            "definir funcao DaAcao();\ndefinir numero nAcao;",
        )
        .unwrap();
        fs::write(
            form.join("tables/T1/rule.txt"),
            "definir funcao DaTabela();",
        )
        .unwrap();
        let inherited = inherited_functions(&form.join("tables/T1/rule.txt"));
        assert!(inherited.contains("global") && inherited.contains("DAACAO"));
        // A própria regra não entra, porque o editor manda o texto dela.
        assert!(!inherited.contains("DaTabela"));
        // A regra da tabela vem depois da ação da interface, então não vale nela.
        assert!(!inherited_functions(&form.join("actions/A1/rule.txt")).contains("DaTabela"));

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
        let var = inherited.variable("NACAO").unwrap();
        assert_eq!(var.kind, DefKind::Variable(Some(VarType::Numero)));
        assert_eq!(var.decl, "definir numero nAcao;");

        // Fora de formulário SGI não há regras herdadas.
        assert!(!inherited_functions(&base.join("regras/R1/Rule.txt")).contains("Global"));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn campos_da_tela() {
        let base = std::env::temp_dir().join(format!("senior-lsp-campos-{}", std::process::id()));
        let form = base.join("Formulários SGI/interface/F1");
        fs::create_dir_all(form.join("fields")).unwrap();
        fs::create_dir_all(form.join("tables/E070EMP/fields")).unwrap();
        fs::write(
            form.join("fields/definition.json"),
            r#"[{"isSimpleField": true, "name": "vDatSis", "kind": 6}]"#,
        )
        .unwrap();
        fs::write(
            form.join("tables/E070EMP/fields/definition.json"),
            r#"[{"isSimpleField": true, "name": "NomEmp", "kind": 11},
                {"isSimpleField": false, "name": "LBusca", "resultField": "X"}]"#,
        )
        .unwrap();
        fs::write(form.join("rule.txt"), "").unwrap();
        let inherited = inherited_functions(&form.join("rule.txt"));
        let kind = |name: &str| inherited.variable(name).map(|v| v.kind);
        assert_eq!(
            kind("vdatsis"),
            Some(DefKind::Variable(Some(VarType::Data)))
        );
        assert_eq!(
            kind("E070EMP_NomEmp"),
            Some(DefKind::Variable(Some(VarType::Alfa)))
        );
        assert_eq!(
            kind("E070EMP_NomEmp_old"),
            Some(DefKind::Variable(Some(VarType::Alfa)))
        );
        // Campo de busca pega o tipo do campo de outra tabela, que a exportação não traz.
        assert_eq!(kind("E070EMP_LBusca"), Some(DefKind::Variable(None)));
        assert!(kind("vDatSis_old").is_none());
        assert!(inherited.is_entry("E070EMP_NomEmp"));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn campo_de_busca_pelo_dicionario() {
        let base = std::env::temp_dir().join(format!("senior-lsp-busca-{}", std::process::id()));
        let form = base.join("Formulários SGI/interface/F1");
        fs::create_dir_all(form.join("interfaces")).unwrap();
        fs::create_dir_all(form.join("tables/E070EMP/fields")).unwrap();
        fs::write(
            form.join("interfaces/definition.json"),
            r#"{"links": [{"name": "TNS_SAIDA", "sourceTable": "E070EMP", "targetTable": "E001TNS",
                "fields": [{"source": "USU_TnsTeste", "target": "CodTns"}]}]}"#,
        )
        .unwrap();
        fs::write(
            form.join("tables/E070EMP/fields/definition.json"),
            r#"[{"isSimpleField": false, "name": "LTnsSai", "link": "TNS_SAIDA",
                 "relatedField": "USU_TnsTeste", "resultField": "DesTns"},
                {"isSimpleField": false, "name": "LOutra", "link": "OUTRA", "resultField": "X"}]"#,
        )
        .unwrap();
        use crate::tbs::{Dictionary, Field, FieldKind, FieldType, Table};
        let tns = Table {
            name: "E001TNS".to_owned(),
            description: std::string::String::new(),
            fields: vec![Field {
                name: "DesTns".to_owned(),
                ty: FieldType {
                    kind: FieldKind::Text,
                    size: 60,
                    decimals: 0,
                    enumeration: None,
                    base: None,
                },
                description: std::string::String::new(),
            }],
        };
        let dictionaries = crate::dictionary::Dictionaries {
            production: Some(Arc::new(Dictionary { tables: vec![tns] })),
            test: None,
        };
        let fields = form_fields(&form, &dictionaries);
        let ty = |name: &str| {
            fields
                .iter()
                .find(|(_, _, e)| e.name == name)
                .map(|(_, _, e)| e.ty)
        };
        assert_eq!(ty("E070EMP_LTnsSai"), Some(Some(VarType::Alfa)));
        assert_eq!(ty("E070EMP_LTnsSai_old"), Some(Some(VarType::Alfa)));
        // Ligação que não existe fica sem tipo.
        assert_eq!(ty("E070EMP_LOutra"), Some(None));
        // Sem dicionário, o campo de busca fica sem tipo.
        let sem = form_fields(&form, &crate::dictionary::Dictionaries::default());
        assert!(sem.iter().all(|(_, _, e)| e.ty.is_none()));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn ordem_das_regras_e_usos() {
        let base = std::env::temp_dir().join(format!("senior-lsp-desc-{}", std::process::id()));
        let form = base.join("Formulários SGI/interface/F1");
        for dir in [
            "actions/A1",
            "tables/T2",
            "tables/T1/actions/A",
            "tables/T1/actions/B",
        ] {
            fs::create_dir_all(form.join(dir)).unwrap();
        }
        for p in [
            "rule.txt",
            "actions/A1/rule.txt",
            "tables/T1/rule.txt",
            "tables/T1/actions/A/rule.txt",
            "tables/T1/actions/B/rule.txt",
            "tables/T2/Rule.txt",
        ] {
            fs::write(form.join(p), "").unwrap();
        }
        // As ações da tabela seguem a lista do `definition.json`, não a ordem alfabética.
        fs::write(
            form.join("tables/T1/definition.json"),
            r#"{"name": "T1", "actions": [{"name": "B"}, {"name": "A"}]}"#,
        )
        .unwrap();
        assert_eq!(
            form_rules(&form),
            [
                form.join("rule.txt"),
                form.join("actions/A1/rule.txt"),
                form.join("tables/T1/rule.txt"),
                form.join("tables/T1/actions/B/rule.txt"),
                form.join("tables/T1/actions/A/rule.txt"),
                form.join("tables/T2/Rule.txt"),
            ]
        );
        assert_eq!(
            following_rules(&form.join("tables/T1/actions/A/rule.txt")),
            [form.join("tables/T2/Rule.txt")]
        );
        assert!(following_rules(&base.join("regras/R/Rule.txt")).is_empty());
        // Link simbólico não entra na lista.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&form, form.join("actions/volta")).unwrap();
            assert_eq!(form_rules(&form).len(), 6);
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
