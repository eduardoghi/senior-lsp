//! Regras do mesmo formulário SGI.
//!
//! O ERP compila todas as regras de uma interface SGI num programa só, numa ordem fixa (veja
//! [`form_rules`]), e o que uma regra define vale nas seguintes. A exportação guarda o formulário
//! como pastas, cada regra num `rule.txt`, com a raiz dois níveis abaixo da pasta
//! `Formulários SGI` (`Formulários SGI/interface/<formulário>`).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
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

/// Funções e variáveis de outras regras, pelo nome em maiúsculas. A regra mais próxima vence. Os
/// mapas são ordenados para o completar listar sempre na mesma ordem.
/// Variáveis aqui são tudo que um `definir` cria fora de função, como variável, cursor, lista,
/// tabela e web service.
#[derive(Debug, Clone, Default)]
pub struct Inherited {
    functions: BTreeMap<String, InheritedDef>,
    variables: BTreeMap<String, InheritedDef>,
    /// Nomes das entradas e dos controles do relatório, em maiúsculas, mesmo os que uma regra
    /// também define.
    entries: std::collections::HashSet<String>,
    /// Dicionários de dados configurados, para o tipo de `TABELA.Campo`.
    pub dictionaries: crate::dictionary::Dictionaries,
    /// A regra usa o Senior SQL 2, pela exportação. `None` quando a exportação não diz.
    pub senior_sql2: Option<bool>,
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

    /// Nenhuma definição de outra regra, só os dicionários de dados.
    pub fn with_dictionaries(dictionaries: crate::dictionary::Dictionaries) -> Inherited {
        Inherited {
            dictionaries,
            ..Inherited::default()
        }
    }

    /// O que vale para uma regra fora de tela SGI e de modelo de relatório, os dicionários de
    /// dados e as opções que a exportação guarda ao lado dela.
    pub fn of_rule(rule: &Path) -> Inherited {
        Inherited {
            dictionaries: crate::dictionary::current(),
            senior_sql2: uses_senior_sql2(rule),
            ..Inherited::default()
        }
    }

    /// Junta as funções das regras dadas, da mais próxima para a mais distante.
    pub fn from_rules(rules: impl IntoIterator<Item = (PathBuf, String)>) -> Inherited {
        let rules: Vec<(PathBuf, RuleDefs)> = rules
            .into_iter()
            .map(|(path, text)| (path, RuleDefs::of(Arc::from(text))))
            .collect();
        Inherited::merge(
            rules
                .iter()
                .map(|(path, defs)| (path.as_path(), defs, None)),
            false,
        )
    }

    /// Junta as definições das regras de um modelo de relatório, que se enxergam umas às outras
    /// sem ordem. Um nome definido com tipos diferentes em duas regras fica sem tipo, para não
    /// dar erro de tipo com o tipo errado.
    ///
    /// As entradas do relatório entram como variáveis, menos as que alguma regra define com
    /// `definir`, que vale primeiro.
    pub fn from_model(model: &crate::report::Model) -> Inherited {
        let rules = model
            .rules
            .iter()
            .map(|(path, embedded)| (path.as_path(), embedded.defs(), Some(embedded)));
        let mut inherited = Inherited::merge(rules, true);
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

    /// Junta as definições das regras na ordem dada, e a primeira regra que define um nome vence.
    /// Com `unordered`, um nome definido com tipos diferentes em duas regras fica sem tipo.
    fn merge<'a>(
        rules: impl Iterator<
            Item = (
                &'a Path,
                &'a RuleDefs,
                Option<&'a Arc<crate::report::Embedded>>,
            ),
        >,
        unordered: bool,
    ) -> Inherited {
        let mut inherited = Inherited::default();
        // Pares de função e regra já contados em `rules`.
        let mut counted: std::collections::HashSet<(&str, usize)> =
            std::collections::HashSet::new();
        for (index, (path, rule, embedded)) in rules.enumerate() {
            for def in &rule.defs {
                if def.kind != DefKind::Function {
                    if let Some(existing) = inherited.variables.get_mut(&def.key) {
                        if unordered && existing.kind != def.kind {
                            existing.kind = DefKind::Variable(None);
                        }
                        continue;
                    }
                    let var = InheritedDef {
                        kind: def.kind,
                        path: path.to_path_buf(),
                        text: rule.text.clone(),
                        name: def.name,
                        decl: def.decl.clone(),
                        params: Vec::new(),
                        rules: 1,
                        entry: None,
                        embedded: embedded.cloned(),
                    };
                    inherited.variables.insert(def.key.clone(), var);
                    continue;
                }
                // Primeira vez que esta regra define esta função.
                let new_rule = counted.insert((&def.key, index));
                let entry = inherited.functions.entry(def.key.clone());
                // Na mesma regra, o `definir funcao` dá os parâmetros e a `funcao` dá o lugar.
                let existing = match entry {
                    std::collections::btree_map::Entry::Occupied(e) => {
                        let f = e.into_mut();
                        let same_rule = f.path == path && Arc::ptr_eq(&f.text, &rule.text);
                        if !same_rule {
                            // Conta a regra uma vez só, mesmo com `definir funcao` e `funcao`.
                            if new_rule {
                                f.rules += 1;
                            }
                            continue;
                        }
                        f
                    }
                    std::collections::btree_map::Entry::Vacant(e) => e.insert(InheritedDef {
                        kind: DefKind::Function,
                        path: path.to_path_buf(),
                        text: rule.text.clone(),
                        name: def.name,
                        decl: String::new(),
                        params: Vec::new(),
                        rules: 1,
                        entry: None,
                        embedded: embedded.cloned(),
                    }),
                };
                if def.is_decl || existing.decl.is_empty() {
                    existing.decl = def.decl.clone();
                    existing.params = def.params.clone();
                }
                if !def.is_decl {
                    existing.name = def.name;
                }
            }
        }
        inherited
    }
}

/// O que uma regra define para as outras, tirado dela com um parse só. Juntar as definições de
/// várias regras ([`Inherited::merge`]) não lê mais o texto, então o resultado de cada regra pode
/// ser guardado enquanto ela não muda.
#[derive(Debug, Clone)]
pub(crate) struct RuleDefs {
    /// Texto da regra, onde ficam as posições das definições.
    text: Arc<str>,
    defs: Vec<RuleDef>,
}

/// Variável, cursor, lista, tabela, web service ou função que a regra define. Parâmetro e rótulo
/// ficam de fora, porque só valem na regra deles.
#[derive(Debug, Clone)]
struct RuleDef {
    /// Nome em maiúsculas.
    key: String,
    kind: DefKind,
    name: TextRange,
    /// A definição é o `definir funcao`, e não a `funcao`.
    is_decl: bool,
    /// Primeira linha da definição.
    decl: String,
    /// Parâmetros, nas funções.
    params: Vec<Param>,
}

impl RuleDefs {
    pub(crate) fn of(text: Arc<str>) -> RuleDefs {
        let mut defs = Vec::new();
        if let Ok(tree) = crate::parse(&text) {
            let symbols = Symbols::build(&tree, &text);
            for def in &symbols.definitions {
                if matches!(def.kind, DefKind::Parameter(_) | DefKind::Label) {
                    continue;
                }
                let function = def.kind == DefKind::Function;
                defs.push(RuleDef {
                    key: text[def.name].to_uppercase(),
                    kind: def.kind,
                    name: def.name,
                    is_decl: function && tree.kind(def.node) == SyntaxKind::FuncDecl,
                    decl: header(&tree, &text, def.node),
                    params: if function {
                        params(&tree, &text, def.node)
                    } else {
                        Vec::new()
                    },
                });
            }
        }
        RuleDefs { text, defs }
    }
}

/// Data de alteração e tamanho de um arquivo ou de uma pasta, para saber se mudou. `None` quando
/// não existe. A data da pasta muda quando uma entrada dela é criada, apagada ou renomeada.
pub(crate) type Stamp = Option<(std::time::SystemTime, u64)>;

pub(crate) fn stamp(path: &Path) -> Stamp {
    let meta = fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Pastas listadas e arquivos lidos para montar um resultado, cada um como estava antes da
/// leitura. Enquanto nenhum mudar, o resultado guardado continua valendo.
#[derive(Default)]
pub(crate) struct Watch(Vec<(PathBuf, Stamp)>);

impl Watch {
    /// Registra o caminho antes de ele ser lido. Uma mudança entre o registro e a leitura só faz
    /// o resultado ser montado de novo na próxima consulta.
    pub(crate) fn add(&mut self, path: &Path) {
        self.0.push((path.to_path_buf(), stamp(path)));
    }

    pub(crate) fn unchanged(&self) -> bool {
        self.0.iter().all(|(path, before)| stamp(path) == *before)
    }
}

/// O que já foi lido de um formulário SGI. A cada tecla numa regra de tela o servidor precisa das
/// definições das regras anteriores e dos campos da tela, que só mudam quando um arquivo do
/// formulário muda no disco. Com isto guardado, a tecla custa um `stat` por pasta e arquivo do
/// formulário, em vez de listar as pastas, ler os JSON e fazer o parse de todas as regras.
#[derive(Default)]
struct Form {
    /// Ordem e campos já foram montados uma vez.
    loaded: bool,
    /// O que foi listado e lido para a ordem das regras e para os campos.
    watch: Watch,
    /// Dicionários que deram o tipo dos campos de busca.
    dictionaries: crate::dictionary::Dictionaries,
    order: Vec<PathBuf>,
    fields: Vec<(PathBuf, Arc<str>, crate::report::Entry)>,
    /// Definições de cada regra já lida, com a data e o tamanho do arquivo. `None` para a regra
    /// que não pôde ser lida.
    rules: HashMap<PathBuf, (Stamp, Option<Arc<RuleDefs>>)>,
    /// Definições juntadas para a regra em cada posição da ordem. Esvazia quando qualquer coisa do
    /// formulário muda.
    merged: HashMap<usize, Arc<Inherited>>,
    /// Usos de nomes de fora em cada regra já lida para isso, com a data e o tamanho do arquivo.
    uses: HashMap<PathBuf, (Stamp, Option<Arc<RuleUses>>)>,
}

/// Quantos formulários ficam guardados. Passando disso o cache recomeça vazio.
const MAX_FORMS: usize = 8;

thread_local! {
    static FORMS: RefCell<HashMap<PathBuf, Form>> = RefCell::default();
}

/// Roda `f` com o formulário de `root`, com a ordem das regras e os campos em dia.
fn with_form<R>(root: &Path, f: impl FnOnce(&mut Form) -> R) -> R {
    let dictionaries = crate::dictionary::current();
    FORMS.with_borrow_mut(|forms| {
        if !forms.contains_key(root) && forms.len() >= MAX_FORMS {
            forms.clear();
        }
        let form = forms.entry(root.to_path_buf()).or_default();
        if !form.loaded || !form.dictionaries.same(&dictionaries) || !form.watch.unchanged() {
            let mut watch = Watch::default();
            form.order = list_rules(root, &mut watch);
            form.fields = form_fields(root, &dictionaries, &mut watch);
            form.watch = watch;
            form.dictionaries = dictionaries;
            form.merged.clear();
            form.loaded = true;
            let Form {
                rules, uses, order, ..
            } = form;
            rules.retain(|path, _| order.contains(path));
            uses.retain(|path, _| order.contains(path));
        }
        f(form)
    })
}

impl Form {
    /// Definições que valem para a regra que tem `before` regras antes dela.
    fn inherited(&mut self, before: usize) -> Arc<Inherited> {
        let mut changed = false;
        for path in &self.order[..before] {
            let now = stamp(path);
            if self.rules.get(path).is_some_and(|(read, _)| *read == now) {
                continue;
            }
            let defs = fs::read(path)
                .ok()
                .and_then(|b| crate::cp1252::decode(&b))
                .map(|text| Arc::new(RuleDefs::of(Arc::from(text))));
            self.rules.insert(path.clone(), (now, defs));
            changed = true;
        }
        if changed {
            self.merged.clear();
        }
        if let Some(inherited) = self.merged.get(&before) {
            return inherited.clone();
        }
        // A regra mais próxima vem primeiro, e o `merge` dá preferência a ela. Regra que não pôde
        // ser lida fica de fora.
        let rules = self.order[..before].iter().rev().filter_map(|path| {
            let (_, defs) = self.rules.get(path)?;
            Some((path.as_path(), defs.as_deref()?, None))
        });
        let mut inherited = Inherited::merge(rules, false);
        inherited.add_entries(self.fields.iter().map(|(p, j, e)| (p, j, e)));
        inherited.dictionaries = self.dictionaries.clone();
        // A opção fica no JSON da própria regra, que entra na vigia para o que foi montado
        // aqui não valer depois de ele mudar.
        if let Some(rule) = self.order.get(before) {
            inherited.senior_sql2 = uses_senior_sql2(rule);
            for name in RULE_OPTIONS {
                self.watch.add(&rule.with_file_name(name));
            }
        }
        let inherited = Arc::new(inherited);
        self.merged.insert(before, inherited.clone());
        inherited
    }
}

impl Form {
    /// Usos de nomes de fora nas regras a partir da posição `from`, na ordem de compilação.
    /// `skip` fica sem os usos, e a regra que não pôde ser lida também.
    fn uses_from(
        &mut self,
        from: usize,
        skip: Option<&Path>,
    ) -> Vec<(PathBuf, Option<Arc<RuleUses>>)> {
        let mut out = Vec::new();
        for path in &self.order[from..] {
            if skip == Some(path.as_path()) {
                out.push((path.clone(), None));
                continue;
            }
            let now = stamp(path);
            let uses = match self.uses.get(path) {
                Some((read, uses)) if *read == now => uses.clone(),
                _ => {
                    let uses = fs::read(path)
                        .ok()
                        .and_then(|b| crate::cp1252::decode(&b))
                        .map(|text| Arc::new(RuleUses::of(Arc::from(text))));
                    self.uses.insert(path.clone(), (now, uses.clone()));
                    uses
                }
            };
            out.push((path.clone(), uses));
        }
        out
    }
}

/// Usos, numa regra, dos nomes que ela não define, que vêm de outra regra do formulário. Ficam
/// de fora o campo depois de ponto e o nome que aponta para uma definição da própria regra.
#[derive(Debug)]
pub struct RuleUses {
    pub text: Arc<str>,
    pub index: crate::line_index::LineIndex,
    /// Trechos de cada nome, em ordem de texto, pelo nome em maiúsculas.
    foreign: HashMap<String, Vec<TextRange>>,
}

impl RuleUses {
    fn of(text: Arc<str>) -> RuleUses {
        let mut foreign: HashMap<String, Vec<TextRange>> = HashMap::new();
        if let Ok(tree) = crate::parse(&text) {
            let symbols = Symbols::build(&tree, &text);
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
                if !member && symbols.reference_at(range).is_none() {
                    foreign
                        .entry(text[range].to_ascii_uppercase())
                        .or_default()
                        .push(range);
                }
            }
        }
        for ranges in foreign.values_mut() {
            ranges.sort_by_key(|r| r.start());
        }
        RuleUses {
            index: crate::line_index::LineIndex::new(&text),
            text,
            foreign,
        }
    }

    /// Usos de `name`, sem diferença de caixa, em ordem de texto.
    pub fn of_name(&self, name: &str) -> &[TextRange] {
        self.foreign
            .get(&name.to_ascii_uppercase())
            .map_or(&[], Vec::as_slice)
    }
}

/// Usos de nomes de fora em cada regra que vem depois de `rule` no formulário, na ordem de
/// compilação. Vazio se `rule` não está num formulário SGI. `skip` vem sem os usos, para a regra
/// aberta no editor, que vale pelo texto dele. As outras valem como estão no disco, e o que foi
/// lido de cada uma fica guardado enquanto o arquivo não muda.
pub fn following_uses(rule: &Path, skip: Option<&Path>) -> Vec<(PathBuf, Option<Arc<RuleUses>>)> {
    let Some(root) = form_dir(rule) else {
        return Vec::new();
    };
    with_form(&root, |form| match position(&form.order, rule) {
        Some(i) => form.uses_from(i + 1, skip),
        None => Vec::new(),
    })
}

/// Funções e variáveis que valem para `rule` num formulário SGI, as das regras que vêm antes dela
/// na ordem de compilação e os campos da tela. Fora de um formulário SGI, só os dicionários de
/// dados. As regras vizinhas valem como estão no disco.
pub fn inherited_functions(rule: &Path) -> Arc<Inherited> {
    let Some(root) = form_dir(rule) else {
        return Arc::new(Inherited::of_rule(rule));
    };
    with_form(&root, |form| {
        let before = position(&form.order, rule).unwrap_or(0);
        form.inherited(before)
    })
}

/// Arquivos da exportação que ficam ao lado do texto da regra e guardam as opções dela, o da
/// regra comum, o do web service e o da tela SGI.
const RULE_OPTIONS: [&str; 3] = ["General_Infos.json", "Rule.json", "definition.json"];

/// Quantas pastas ficam guardadas. Passando disso o cache recomeça vazio.
const MAX_SQL_MODES: usize = 256;

thread_local! {
    /// A opção de cada pasta de regra já lida, com os arquivos que foram consultados.
    static SQL_MODES: RefCell<HashMap<PathBuf, (Watch, Option<bool>)>> = RefCell::default();
}

/// Diz se a regra usa o Senior SQL 2, pela opção `useSQLSenior2` do JSON que a exportação grava
/// ao lado do texto dela. `None` quando não há o arquivo ou ele não tem a opção, como na regra
/// fora de uma exportação. O arquivo só é lido de novo quando muda.
pub fn uses_senior_sql2(rule: &Path) -> Option<bool> {
    let dir = rule.parent()?;
    SQL_MODES.with_borrow_mut(|modes| {
        if let Some((watch, mode)) = modes.get(dir)
            && watch.unchanged()
        {
            return *mode;
        }
        let mut watch = Watch::default();
        let mut mode = None;
        for name in RULE_OPTIONS {
            let path = dir.join(name);
            watch.add(&path);
            mode = fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|json| json.get("useSQLSenior2")?.as_bool());
            if mode.is_some() {
                break;
            }
        }
        if modes.len() >= MAX_SQL_MODES {
            modes.clear();
        }
        modes.insert(dir.to_path_buf(), (watch, mode));
        mode
    })
}

/// Regras do formulário na ordem em que o ERP compila. O ERP junta todas as regras da interface
/// num programa só, e uma variável definida numa regra vale nas seguintes. A ordem é a regra da
/// interface, as ações da interface e, para cada tabela, a regra dela e as ações dela. Tabelas e
/// ações da interface vão em ordem alfabética, como a importação do gerenciador de artefatos
/// grava, e as ações da tabela na ordem da lista `actions` do `definition.json` dela.
pub fn form_rules(root: &Path) -> Vec<PathBuf> {
    list_rules(root, &mut Watch::default())
}

/// As regras de [`form_rules`], registrando em `watch` cada pasta listada e cada JSON lido.
fn list_rules(root: &Path, watch: &mut Watch) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // A pasta entra mesmo sem o arquivo, porque a regra pode estar só no editor, ainda sem salvar.
    let mut push = |dir: &Path, watch: &mut Watch| {
        watch.add(dir);
        out.push(dir.join(rule_file(dir)));
    };
    push(root, watch);
    let actions = root.join("actions");
    watch.add(&actions);
    for action in subdirs(&actions) {
        push(&action, watch);
    }
    let tables = root.join("tables");
    watch.add(&tables);
    for table in subdirs(&tables) {
        push(&table, watch);
        let definition = table.join("definition.json");
        watch.add(&definition);
        let listed = fs::read(definition)
            .ok()
            .and_then(|b| {
                std::string::String::from_utf8(b.clone())
                    .ok()
                    .or_else(|| crate::cp1252::decode(&b))
            })
            .unwrap_or_default()
            .to_lowercase();
        let actions_at = listed.find("\"actions\"").unwrap_or(listed.len());
        let table_actions = table.join("actions");
        watch.add(&table_actions);
        let mut actions = subdirs(&table_actions);
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
            push(&action, watch);
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
    with_form(&root, |form| match position(&form.order, rule) {
        Some(i) => form.order[i + 1..].to_vec(),
        None => Vec::new(),
    })
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
    watch: &mut Watch,
) -> Vec<(PathBuf, Arc<str>, crate::report::Entry)> {
    let links = if dictionaries.is_empty() {
        HashMap::new()
    } else {
        let interfaces = root.join("interfaces").join("definition.json");
        watch.add(&interfaces);
        read_json(&interfaces)
            .map(|json| link_targets(&json))
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    let mut add = |path: PathBuf, prefix: &str, watch: &mut Watch| {
        watch.add(&path);
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
    add(root.join("fields").join("definition.json"), "", watch);
    watch.add(&root.join("tables"));
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
        add(table.join("fields").join("definition.json"), &name, watch);
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
    fn opcao_de_senior_sql_2_da_exportacao() {
        let base = std::env::temp_dir().join(format!("senior-lsp-sql2-{}", std::process::id()));
        let rule = base.join("regras/R1.LSP/Rule.txt");
        fs::create_dir_all(rule.parent().unwrap()).unwrap();
        assert_eq!(uses_senior_sql2(&rule), None);
        // O arquivo criado depois é visto, e a mudança dele também.
        let options = rule.with_file_name("General_Infos.json");
        fs::write(&options, r#"{"debug": false, "useSQLSenior2": true}"#).unwrap();
        assert_eq!(uses_senior_sql2(&rule), Some(true));
        assert_eq!(Inherited::of_rule(&rule).senior_sql2, Some(true));
        fs::write(&options, r#"{"debug": false,  "useSQLSenior2": false}"#).unwrap();
        assert_eq!(uses_senior_sql2(&rule), Some(false));
        fs::write(&options, r#"{"debug": false}"#).unwrap();
        assert_eq!(uses_senior_sql2(&rule), None);
        // Web service e tela SGI guardam a opção em arquivos de outro nome.
        fs::write(
            rule.with_file_name("Rule.json"),
            r#"{"useSQLSenior2": true}"#,
        )
        .unwrap();
        assert_eq!(uses_senior_sql2(&rule), Some(true));
        let table = base.join("Formulários SGI/interface/F1/tables/T1/rule.txt");
        fs::create_dir_all(table.parent().unwrap()).unwrap();
        fs::write(&table, "definir numero n;").unwrap();
        assert_eq!(inherited_functions(&table).senior_sql2, None);
        fs::write(
            table.with_file_name("definition.json"),
            r#"{"name": "T1", "useSQLSenior2": true, "actions": []}"#,
        )
        .unwrap();
        assert_eq!(inherited_functions(&table).senior_sql2, Some(true));
        fs::remove_dir_all(&base).unwrap();
    }

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

    /// Espera o relógio do sistema de arquivos andar, para a mudança seguinte ter outra data.
    /// Sem isso, duas mudanças no mesmo instante do relógio podem ficar com a mesma data.
    fn tick() {
        std::thread::sleep(std::time::Duration::from_millis(30));
    }

    #[test]
    fn ve_a_mudanca_das_regras_vizinhas_no_disco() {
        let base = std::env::temp_dir().join(format!("senior-lsp-cache-{}", std::process::id()));
        let form = base.join("Formulários SGI/interface/F1");
        fs::create_dir_all(form.join("actions/A1")).unwrap();
        fs::create_dir_all(form.join("tables/T1/fields")).unwrap();
        fs::write(form.join("rule.txt"), "definir funcao Um();").unwrap();
        fs::write(form.join("actions/A1/rule.txt"), "definir numero nAcao;").unwrap();
        let table = form.join("tables/T1/rule.txt");
        fs::write(&table, "").unwrap();

        let first = inherited_functions(&table);
        assert!(first.contains("Um") && first.variable("nAcao").is_some());
        // Sem mudança no disco, a segunda consulta devolve o mesmo resultado guardado.
        assert!(Arc::ptr_eq(&first, &inherited_functions(&table)));

        // Regra vizinha alterada.
        tick();
        fs::write(
            form.join("rule.txt"),
            "definir funcao Dois();\ndefinir alfa aRaiz;",
        )
        .unwrap();
        let changed = inherited_functions(&table);
        assert!(!changed.contains("Um") && changed.contains("Dois"));
        assert!(changed.variable("aRaiz").is_some() && changed.variable("nAcao").is_some());

        // Regra vizinha criada, numa ação nova.
        tick();
        fs::create_dir_all(form.join("actions/A2")).unwrap();
        fs::write(form.join("actions/A2/rule.txt"), "definir funcao Nova();").unwrap();
        assert!(inherited_functions(&table).contains("Nova"));
        assert_eq!(following_rules(&form.join("actions/A1/rule.txt")).len(), 2);

        // Regra vizinha apagada, primeiro só o arquivo e depois a pasta.
        tick();
        fs::remove_file(form.join("actions/A1/rule.txt")).unwrap();
        let removed = inherited_functions(&table);
        assert!(removed.variable("nAcao").is_none() && removed.contains("Nova"));
        tick();
        fs::remove_dir_all(form.join("actions/A2")).unwrap();
        assert!(!inherited_functions(&table).contains("Nova"));

        // Campo da tela cadastrado depois.
        assert!(inherited_functions(&table).variable("T1_Campo").is_none());
        tick();
        fs::write(
            form.join("tables/T1/fields/definition.json"),
            r#"[{"isSimpleField": true, "name": "Campo", "kind": 11}]"#,
        )
        .unwrap();
        let fields = inherited_functions(&table);
        assert_eq!(
            fields.variable("T1_Campo").map(|v| v.kind),
            Some(DefKind::Variable(Some(VarType::Alfa)))
        );
        // A regra da raiz não herda regra nenhuma, só os campos.
        let root = inherited_functions(&form.join("rule.txt"));
        assert!(!root.contains("Dois") && root.variable("T1_Campo").is_some());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn usos_nas_regras_seguintes() {
        let base = std::env::temp_dir().join(format!("senior-lsp-usos-{}", std::process::id()));
        let form = base.join("Formulários SGI/interface/F1");
        fs::create_dir_all(form.join("actions/A1")).unwrap();
        fs::create_dir_all(form.join("actions/A2")).unwrap();
        let root = form.join("rule.txt");
        fs::write(&root, "definir funcao F();").unwrap();
        let a1 = form.join("actions/A1/rule.txt");
        let a2 = form.join("actions/A2/rule.txt");
        fs::write(
            &a1,
            "F();\nx = f + cur.F;\ndefinir numero Local;\nLocal = 1;",
        )
        .unwrap();
        fs::write(&a2, "F();").unwrap();

        let following = following_uses(&root, Some(&a2));
        assert_eq!(following.len(), 2);
        let (path, uses) = &following[0];
        let uses = uses.as_ref().unwrap();
        assert_eq!(path, &a1);
        // O campo depois do ponto e o nome definido na própria regra ficam de fora.
        let starts: Vec<u32> = uses.of_name("F").iter().map(|r| r.start().into()).collect();
        assert_eq!(starts, [0, 9]);
        assert_eq!(uses.of_name("f"), uses.of_name("F"));
        assert!(uses.of_name("Local").is_empty() && uses.of_name("cur").len() == 1);
        // O resultado é o mesmo de procurar um nome por vez.
        assert_eq!(uses.of_name("F"), super::uses(&uses.text, "F", false));
        // A regra pulada vem sem os usos, e a de fora de formulário não tem regras seguintes.
        assert!(following[1].0 == a2 && following[1].1.is_none());
        assert!(following_uses(&base.join("regras/R/Rule.txt"), None).is_empty());

        // Sem mudança no disco, é o mesmo resultado guardado. Com mudança, é lido de novo.
        let again = following_uses(&root, None);
        assert!(Arc::ptr_eq(uses, again[0].1.as_ref().unwrap()));
        assert_eq!(again[1].1.as_ref().unwrap().of_name("F").len(), 1);
        tick();
        fs::write(&a1, "x = 1;").unwrap();
        let changed = following_uses(&root, None);
        assert!(changed[0].1.as_ref().unwrap().of_name("F").is_empty());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn guarda_poucos_formularios() {
        let base = std::env::temp_dir().join(format!("senior-lsp-muitos-{}", std::process::id()));
        let rule =
            |n: usize| base.join(format!("Formulários SGI/interface/F{n}/actions/A/rule.txt"));
        for n in 0..=MAX_FORMS {
            fs::create_dir_all(rule(n).parent().unwrap()).unwrap();
            let root = base.join(format!("Formulários SGI/interface/F{n}/rule.txt"));
            fs::write(root, format!("definir funcao F{n}();")).unwrap();
        }
        // Passando do limite o cache recomeça vazio, e o resultado continua certo.
        for n in 0..=MAX_FORMS {
            assert!(inherited_functions(&rule(n)).contains(&format!("F{n}")));
        }
        assert_eq!(FORMS.with_borrow(HashMap::len), 1);
        assert!(inherited_functions(&rule(0)).contains("F0"));
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
                ..Field::default()
            }],
            ..Table::default()
        };
        let dictionaries = crate::dictionary::Dictionaries {
            production: Some(Arc::new(Dictionary {
                tables: vec![tns],
                ..Dictionary::default()
            })),
            test: None,
        };
        let fields = form_fields(&form, &dictionaries, &mut Watch::default());
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
        let sem = form_fields(
            &form,
            &crate::dictionary::Dictionaries::default(),
            &mut Watch::default(),
        );
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
