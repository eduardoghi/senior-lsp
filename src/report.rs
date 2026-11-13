//! Regras dentro dos modelos de relatório.
//!
//! A exportação guarda cada modelo numa pasta (`Modelos/CCCC001.GER`) com vários JSON, e o código
//! de cada regra fica num objeto `rule`, uma linha por item de `lines`. O `prop_name` diz o
//! evento, como `SOnPrint` de um campo ou `SFunctionRule` com as funções do modelo.
//!
//! O texto da regra é montado com as linhas decodificadas, separadas por `\n`, e um mapa leva
//! cada posição desse texto de volta para o JSON, contando as aspas e os escapes. O leitor é
//! próprio porque precisa das posições, e porque alguns JSON da exportação têm caractere de
//! controle cru dentro de string, que um leitor estrito recusa.

use text_size::{TextRange, TextSize};

/// Regra de um JSON com o texto do JSON, para levar as posições dela ao arquivo.
#[derive(Debug, Clone)]
pub struct Embedded {
    pub json: std::sync::Arc<str>,
    pub rule: EmbeddedRule,
    /// O que a regra define para as outras do modelo, tirado dela no primeiro uso.
    defs: std::sync::OnceLock<crate::form::RuleDefs>,
}

impl Embedded {
    pub fn new(json: std::sync::Arc<str>, rule: EmbeddedRule) -> Embedded {
        Embedded {
            json,
            rule,
            defs: std::sync::OnceLock::new(),
        }
    }

    /// Definições da regra, com um parse só enquanto ela existir. A regra de um JSON que não
    /// mudou é a mesma de uma tecla para a outra, então o parse dela não se repete.
    pub(crate) fn defs(&self) -> &crate::form::RuleDefs {
        self.defs
            .get_or_init(|| crate::form::RuleDefs::of(self.rule.text.as_str().into()))
    }
}

/// Nome cadastrado no modelo fora das regras, que vale nelas como variável sem `definir`. É uma
/// entrada da lista `entrance`, como `ECodFil`, ou um controle cujo valor a regra lê ou define.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    /// Tipo da variável que o ERP cria para o nome antes de compilar as regras. Na entrada vem do
    /// `type_var`, com 1, 2 e 5 para número, 3 para alfa e 4 para data, e a alfa de um caractere
    /// (`size` 1), como a lista com S para sintético e A para analítico, vira número com o código do
    /// caractere. Variável do sistema (6) fica sem tipo, porque depende de qual é.
    pub ty: Option<crate::symbols::VarType>,
    pub description: String,
    /// Posição do nome no JSON, o conteúdo entre as aspas de `sytem_varName` ou do `name` do
    /// controle.
    pub name_range: TextRange,
    /// No campo de busca da tela SGI, a ligação (`link`) e o campo da tabela de destino que ele
    /// mostra (`resultField`).
    pub lookup: Option<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Entrance,
    /// Campo de tela SGI, da interface ou de uma tabela, lido do `definition.json` da pasta
    /// `fields`.
    Field,
    /// Controle pela classe, como `TSFormula`.
    Control(Control),
}

/// Controles cujo nome aparece nas regras. A descrição ganha `definir alfa` com o nome dela antes
/// das regras, e o total vira variável numérica do sistema. A fórmula não é declarada, e o nome dela
/// nas regras é a variável numérica que o compilador cria para nome sem `definir`. O `type_var` da
/// fórmula só diz como a máscara formata o valor. Memorando e código de barras imprimem a variável
/// de mesmo nome, que a regra define. Os outros, como campo de tabela e desenho, não têm valor que a
/// regra use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Formula,
    Total,
    Descricao,
    Memorando,
    CodigoBarras,
}

impl Control {
    fn from_class(class: &str) -> Option<Control> {
        Some(match class {
            "TSFormula" => Control::Formula,
            "TSTotal" => Control::Total,
            "TSDescricao" => Control::Descricao,
            "TSMemoran" => Control::Memorando,
            "TSCodBarra" => Control::CodigoBarras,
            _ => return None,
        })
    }

    fn ty(self) -> Option<crate::symbols::VarType> {
        use crate::symbols::VarType;
        match self {
            Control::Descricao => Some(VarType::Alfa),
            Control::Formula | Control::Total => Some(VarType::Numero),
            Control::Memorando | Control::CodigoBarras => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Control::Formula => "fórmula",
            Control::Total => "total",
            Control::Descricao => "descrição",
            Control::Memorando => "memorando",
            Control::CodigoBarras => "código de barras",
        }
    }
}

/// Uma regra encontrada no JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedRule {
    /// Evento da regra, como `SOnPrint`. Vazio se o objeto não tem `prop_name`.
    pub prop: String,
    pub text: String,
    /// Trechos em que o texto da regra e o JSON andam juntos, byte a byte. Cada um guarda o
    /// começo no texto, o começo no JSON e o tamanho. Um escape (`\"`, `\n`) ocupa mais bytes no
    /// JSON do que no texto, então fica entre dois trechos, com um trecho vazio na barra.
    runs: Vec<(u32, u32, u32)>,
    /// Começo e fim, no JSON, de cada linha (o conteúdo entre as aspas).
    lines: Vec<(u32, u32)>,
}

impl EmbeddedRule {
    /// Posição no JSON de uma posição do texto da regra. A quebra entre duas linhas vai para o
    /// fim da linha de cima no JSON.
    pub fn to_json(&self, offset: TextSize) -> TextSize {
        let v = u32::from(offset);
        let i = self.runs.partition_point(|&(start, _, _)| start <= v);
        if i == 0 {
            return TextSize::new(self.lines.first().map_or(0, |l| l.0));
        }
        let (start, json, len) = self.runs[i - 1];
        TextSize::new(json + (v - start).min(len))
    }

    pub fn range_to_json(&self, range: TextRange) -> TextRange {
        let start = self.to_json(range.start());
        let end = self.to_json(range.end()).max(start);
        TextRange::new(start, end)
    }

    /// Posição no texto da regra de uma posição do JSON, ou `None` se ela não cai numa linha
    /// desta regra.
    pub fn from_json(&self, offset: TextSize) -> Option<TextSize> {
        let j = u32::from(offset);
        if !self.lines.iter().any(|&(s, e)| s <= j && j <= e) {
            return None;
        }
        // O trecho que contém a posição, ou o último antes dela quando ela cai num escape.
        let best = self
            .runs
            .iter()
            .filter(|&&(_, json, _)| json <= j)
            .max_by_key(|&&(_, json, _)| json)?;
        let (start, json, len) = *best;
        // Dentro do texto e numa fronteira de caractere, para quem recorta o texto com ela.
        let mut v = ((start + (j - json).min(len)) as usize).min(self.text.len());
        while !self.text.is_char_boundary(v) {
            v -= 1;
        }
        Some(TextSize::new(v as u32))
    }
}

/// Regras e entradas de todos os JSON de um modelo.
#[derive(Debug, Default)]
pub struct Model {
    pub rules: Vec<(std::path::PathBuf, std::sync::Arc<Embedded>)>,
    /// Cada entrada com o JSON onde está cadastrada.
    pub entries: Vec<(std::path::PathBuf, std::sync::Arc<str>, Entry)>,
    /// Texto de cada JSON lido, para a análise da pasta não ler e decodificar o arquivo de novo.
    pub texts: Vec<(std::path::PathBuf, std::sync::Arc<str>)>,
}

/// Regras de todos os JSON do modelo de `json_path`.
pub fn model_rules(
    json_path: &std::path::Path,
    current: &str,
) -> Vec<(std::path::PathBuf, std::sync::Arc<Embedded>)> {
    model(json_path, current).rules
}

/// Diz se o arquivo é uma regra de relatório separada do JSON, em `Modelos/<modelo>/regras/*.txt`.
/// Quem versiona os relatórios pode tirar o código das regras do JSON para esses arquivos, e o
/// JSON fica com uma referência no `lines`, que o leitor ignora por não ser uma lista.
pub fn is_rule_file(path: &std::path::Path) -> bool {
    let txt = path
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("txt"));
    let in_rules = path
        .parent()
        .and_then(|p| p.file_name())
        .is_some_and(|n| n.eq_ignore_ascii_case("regras"));
    let in_models = path.ancestors().any(|a| {
        a.file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("Modelos"))
    });
    txt && in_rules && in_models
}

/// O que foi lido de um arquivo do modelo, um JSON ou uma regra separada.
struct FileScan {
    /// Texto do arquivo.
    text: std::sync::Arc<str>,
    rules: Vec<std::sync::Arc<Embedded>>,
    entries: Vec<Entry>,
}

/// O que já foi lido de cada arquivo de um modelo, com a data e o tamanho dele. `None` para o
/// arquivo que não pôde ser lido.
type ModelFiles = std::collections::HashMap<
    std::path::PathBuf,
    (crate::form::Stamp, Option<std::sync::Arc<FileScan>>),
>;

/// Quantos modelos ficam guardados. Passando disso o cache recomeça vazio.
const MAX_MODELS: usize = 4;

thread_local! {
    /// Arquivos já lidos de cada modelo, pela pasta dele. Com um JSON do modelo aberto no editor,
    /// cada tecla precisa das regras e das entradas dos outros arquivos da pasta, que só mudam
    /// quando o arquivo muda no disco.
    static MODELS: std::cell::RefCell<std::collections::HashMap<std::path::PathBuf, ModelFiles>> =
        std::cell::RefCell::default();
}

/// Regras e entradas de todos os JSON do modelo de `json_path`, a pasta dele, e das regras
/// separadas na pasta `regras`. `json_path` pode ser um JSON ou uma regra separada. O arquivo aberto
/// no editor entra com `current`, e os outros valem como estão no disco, em UTF-8 ou cp1252. O que
/// foi lido de cada um fica guardado enquanto a data e o tamanho dele não mudam.
pub fn model(json_path: &std::path::Path, current: &str) -> Model {
    let Some(dir) = model_dir(json_path) else {
        return Model::default();
    };
    MODELS.with_borrow_mut(|models| {
        if !models.contains_key(dir) && models.len() >= MAX_MODELS {
            models.clear();
        }
        let files = models.entry(dir.to_path_buf()).or_default();
        build(dir, json_path, current, Some(files))
    })
}

/// Como [`model`], sem guardar nada. Serve à análise da pasta inteira, que passa uma vez só por
/// cada modelo.
pub fn model_uncached(json_path: &std::path::Path, current: &str) -> Model {
    match model_dir(json_path) {
        Some(dir) => build(dir, json_path, current, None),
        None => Model::default(),
    }
}

/// Pasta do modelo de um JSON ou de uma regra separada, que fica em `regras`, um nível abaixo.
fn model_dir(json_path: &std::path::Path) -> Option<&std::path::Path> {
    if is_rule_file(json_path) {
        json_path.parent().and_then(|p| p.parent())
    } else {
        json_path.parent()
    }
}

fn build(
    dir: &std::path::Path,
    json_path: &std::path::Path,
    current: &str,
    mut cache: Option<&mut ModelFiles>,
) -> Model {
    let mut out = Model::default();
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("json"))
        })
        .collect();
    files.sort();
    let rules_dir = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("regras")
                && e.path().is_dir()
        });
    let mut separated: Vec<_> = rules_dir
        .map(|d| std::fs::read_dir(d.path()).into_iter().flatten().flatten())
        .into_iter()
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_rule_file(p))
        .collect();
    separated.sort();
    if let Some(cache) = cache.as_deref_mut() {
        // O arquivo que saiu da pasta não fica guardado.
        cache.retain(|path, _| files.contains(path) || separated.contains(path));
    }

    for path in files {
        let Some(scan) = file_scan(&path, json_path, current, cache.as_deref_mut(), scan_json)
        else {
            continue;
        };
        out.texts.push((path.clone(), scan.text.clone()));
        for rule in &scan.rules {
            out.rules.push((path.clone(), rule.clone()));
        }
        for entry in &scan.entries {
            out.entries
                .push((path.clone(), scan.text.clone(), entry.clone()));
        }
    }
    for path in separated {
        let read = |text| scan_rule_file(&path, text);
        let Some(scan) = file_scan(&path, json_path, current, cache.as_deref_mut(), read) else {
            continue;
        };
        for rule in &scan.rules {
            out.rules.push((path.clone(), rule.clone()));
        }
    }
    out
}

/// O que `read` tira do arquivo. O aberto no editor é lido de `current` a cada vez, e os outros
/// vêm do disco, ou do `cache` quando a data e o tamanho não mudaram.
fn file_scan(
    path: &std::path::Path,
    json_path: &std::path::Path,
    current: &str,
    cache: Option<&mut ModelFiles>,
    read: impl FnOnce(String) -> FileScan,
) -> Option<std::sync::Arc<FileScan>> {
    if path == json_path {
        return Some(std::sync::Arc::new(read(current.to_owned())));
    }
    // A data vem antes da leitura. Uma mudança entre as duas só faz o arquivo ser lido de novo
    // na próxima consulta.
    let now = cache.as_ref().map(|_| crate::form::stamp(path));
    if let (Some(cache), Some(now)) = (&cache, now)
        && let Some((before, scan)) = cache.get(path)
        && *before == now
    {
        return scan.clone();
    }
    let scan = std::fs::read(path)
        .ok()
        .and_then(|b| match String::from_utf8(b) {
            Ok(t) => Some(t),
            Err(e) => crate::cp1252::decode(e.as_bytes()),
        })
        .map(|text| std::sync::Arc::new(read(text)));
    if let (Some(cache), Some(now)) = (cache, now) {
        cache.insert(path.to_path_buf(), (now, scan.clone()));
    }
    scan
}

/// Regras e entradas de um JSON do modelo.
fn scan_json(text: String) -> FileScan {
    let text: std::sync::Arc<str> = text.into();
    let (rules, entries) = scan(&text);
    FileScan {
        rules: rules
            .into_iter()
            .map(|rule| std::sync::Arc::new(Embedded::new(text.clone(), rule)))
            .collect(),
        entries,
        text,
    }
}

/// Regra de um arquivo separado, ou nenhuma se o arquivo está vazio.
fn scan_rule_file(path: &std::path::Path, text: String) -> FileScan {
    // O evento é a última parte do nome, como `SOnPrint` em `Detalhe.Campo01.SOnPrint.txt`.
    let prop = path
        .file_stem()
        .map(|s| s.to_string_lossy())
        .and_then(|s| s.rsplit('.').next().map(str::to_owned))
        .unwrap_or_default();
    let rule = from_text(prop, &text);
    let text: std::sync::Arc<str> = text.into();
    let rules = if rule.text.trim().is_empty() {
        Vec::new()
    } else {
        vec![std::sync::Arc::new(Embedded::new(text.clone(), rule))]
    };
    FileScan {
        text,
        rules,
        entries: Vec::new(),
    }
}

/// Regra de um arquivo de texto. O "JSON" dela é o próprio arquivo, e cada linha anda junto com
/// o texto da regra, sem escapes. Só o fim de linha muda, porque o texto usa `\n` e o arquivo
/// pode ter CRLF.
pub fn from_text(prop: String, file: &str) -> EmbeddedRule {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut lines = Vec::new();
    let mut start = 0usize;
    for (i, line) in file.split('\n').enumerate() {
        let content = line.strip_suffix('\r').unwrap_or(line);
        if i > 0 {
            text.push('\n');
        }
        let base = text.len() as u32;
        runs.push((base, start as u32, content.len() as u32));
        text.push_str(content);
        lines.push((start as u32, (start + content.len()) as u32));
        start += line.len() + 1;
    }
    EmbeddedRule {
        prop,
        text,
        runs,
        lines,
    }
}

/// Todas as regras com código do JSON, na ordem do texto.
pub fn extract(json: &str) -> Vec<EmbeddedRule> {
    scan(json).0
}

/// Regras com código e entradas do JSON.
pub fn scan(json: &str) -> (Vec<EmbeddedRule>, Vec<Entry>) {
    let mut scanner = Scanner {
        bytes: json.as_bytes(),
        pos: 0,
        entries: Vec::new(),
        depth: 0,
    };
    let mut rules = Vec::new();
    scanner.value(&mut rules, None);
    rules.retain(|r| !r.text.trim().is_empty());
    (rules, scanner.entries)
}

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
    entries: Vec<Entry>,
    /// Níveis de `[` e `{` abertos. Passando de [`MAX_DEPTH`], o resto não é lido, para um JSON
    /// muito aninhado não estourar a pilha da recursão.
    depth: u32,
}

const MAX_DEPTH: u32 = 256;

/// String lida do JSON, com o conteúdo decodificado e os trechos sem escape.
struct JsonString {
    value: String,
    /// (começo no valor, começo no JSON, tamanho)
    runs: Vec<(u32, u32, u32)>,
    start: u32,
    end: u32,
}

impl Scanner<'_> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }

    /// Lê um valor. `key` é a chave dele no objeto de cima, para achar `rule`.
    fn value(&mut self, rules: &mut Vec<EmbeddedRule>, key: Option<&str>) {
        if matches!(self.peek(), Some(b'{' | b'[')) {
            if self.depth >= MAX_DEPTH {
                self.pos = self.bytes.len();
                return;
            }
            self.depth += 1;
        }
        match self.peek() {
            Some(b'{') => {
                self.object(rules, key == Some("rule"));
                self.depth -= 1;
            }
            Some(b'[') => {
                self.pos += 1;
                loop {
                    match self.peek() {
                        Some(b']') | None => {
                            self.pos += 1;
                            break;
                        }
                        Some(b',') => self.pos += 1,
                        _ => {
                            // Num JSON malformado, como `[}`, o valor pode não andar. Pular um
                            // byte garante que o laço termina.
                            let before = self.pos;
                            self.value(rules, None);
                            if self.pos == before {
                                self.pos += 1;
                            }
                        }
                    }
                }
                self.depth -= 1;
            }
            Some(b'"') => self.skip_string(),
            Some(_) => {
                // Número, `true`, `false` ou `null`.
                while self.pos < self.bytes.len()
                    && !matches!(self.bytes[self.pos], b',' | b'}' | b']')
                    && !self.bytes[self.pos].is_ascii_whitespace()
                {
                    self.pos += 1;
                }
            }
            None => {}
        }
    }

    /// Lê um inteiro sem sinal. Outro valor no lugar é pulado e dá `None`.
    fn integer(&mut self, rules: &mut Vec<EmbeddedRule>) -> Option<u32> {
        self.skip_ws();
        let start = self.pos;
        while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.pos += 1;
        }
        if self.pos == start {
            self.value(rules, None);
            return None;
        }
        std::str::from_utf8(&self.bytes[start..self.pos])
            .ok()
            .and_then(|n| n.parse().ok())
    }

    fn object(&mut self, rules: &mut Vec<EmbeddedRule>, is_rule: bool) {
        self.pos += 1;
        let mut prop = String::new();
        let mut lines: Vec<JsonString> = Vec::new();
        let mut entry_name: Option<JsonString> = None;
        let mut type_var = None;
        let mut description = String::new();
        let mut size = None;
        let mut class = None;
        let mut control_name: Option<JsonString> = None;
        let mut simple = None;
        let mut field_kind = None;
        let mut information = String::new();
        let mut link = None;
        let mut result_field = None;
        loop {
            match self.peek() {
                Some(b'}') | None => {
                    self.pos += 1;
                    break;
                }
                Some(b',') => self.pos += 1,
                Some(b'"') => {
                    let key = self.key();
                    if self.peek() == Some(b':') {
                        self.pos += 1;
                    }
                    if is_rule && key == "lines" && self.peek() == Some(b'[') {
                        lines = self.string_array();
                    } else if is_rule && key == "prop_name" && self.peek() == Some(b'"') {
                        prop = self.string().value;
                    } else if key == "sytem_varName" && self.peek() == Some(b'"') {
                        entry_name = Some(self.string());
                    } else if key == "description" && self.peek() == Some(b'"') {
                        description = self.string().value;
                    } else if key == "classname" && self.peek() == Some(b'"') {
                        class = Control::from_class(&self.string().value);
                    } else if key == "name" && self.peek() == Some(b'"') {
                        control_name = Some(self.string());
                    } else if key == "isSimpleField" {
                        simple = Some(self.peek() == Some(b't'));
                        self.value(rules, None);
                    } else if key == "kind" {
                        field_kind = self.integer(rules);
                    } else if key == "information" && self.peek() == Some(b'"') {
                        information = self.string().value;
                    } else if key == "link" && self.peek() == Some(b'"') {
                        link = Some(self.string().value);
                    } else if key == "resultField" && self.peek() == Some(b'"') {
                        result_field = Some(self.string().value);
                    } else if key == "type_var" {
                        type_var = self.integer(rules);
                    } else if key == "size" {
                        size = self.integer(rules);
                    } else {
                        self.value(rules, Some(&*key));
                    }
                }
                Some(_) => self.pos += 1,
            }
        }
        if is_rule {
            rules.push(assemble(prop, lines));
        }
        if let Some(name) = entry_name {
            use crate::symbols::VarType;
            let ty = match type_var {
                Some(1 | 2 | 5) => Some(VarType::Numero),
                Some(3) if size == Some(1) => Some(VarType::Numero),
                Some(3) => Some(VarType::Alfa),
                Some(4) => Some(VarType::Data),
                _ => None,
            };
            self.entries.push(Entry {
                name: name.value,
                kind: EntryKind::Entrance,
                ty,
                description,
                name_range: TextRange::new(TextSize::new(name.start), TextSize::new(name.end)),
                lookup: None,
            });
        } else if let Some(name) = control_name.take_if(|_| simple.is_some()) {
            let simple = simple == Some(true);
            use crate::symbols::VarType;
            // `kind` numera 2 booleano, 3 número, 4 a 10 data em unidades diferentes, 11 alfa e 12
            // blob. Campo de busca (`isSimpleField` falso) não traz tipo, porque mostra um campo de
            // outra tabela.
            let ty = match field_kind {
                _ if !simple => None,
                Some(3) => Some(VarType::Numero),
                Some(4..=10) => Some(VarType::Data),
                Some(11) => Some(VarType::Alfa),
                _ => None,
            };
            self.entries.push(Entry {
                name: name.value,
                kind: EntryKind::Field,
                ty,
                description: information,
                name_range: TextRange::new(TextSize::new(name.start), TextSize::new(name.end)),
                lookup: link.zip(result_field).filter(|_| !simple),
            });
        } else if let (Some(control), Some(name)) = (class, control_name) {
            self.entries.push(Entry {
                name: name.value,
                kind: EntryKind::Control(control),
                ty: control.ty(),
                description: String::new(),
                name_range: TextRange::new(TextSize::new(name.start), TextSize::new(name.end)),
                lookup: None,
            });
        }
    }

    fn string_array(&mut self) -> Vec<JsonString> {
        self.pos += 1;
        let mut out = Vec::new();
        loop {
            match self.peek() {
                Some(b']') | None => {
                    self.pos += 1;
                    break;
                }
                Some(b',') => self.pos += 1,
                Some(b'"') => out.push(self.string()),
                Some(_) => self.pos += 1,
            }
        }
        out
    }

    /// Lê uma string a partir das aspas. Caractere de controle cru fica no valor.
    fn string(&mut self) -> JsonString {
        self.pos += 1;
        let start = self.pos as u32;
        let mut value = String::new();
        let mut runs = Vec::new();
        let mut run_start = self.pos;
        // Só entra no mapa o trecho que de fato entrou no valor.
        let flush = |value: &mut String,
                     runs: &mut Vec<(u32, u32, u32)>,
                     from: usize,
                     to: usize,
                     bytes: &[u8]| {
            if let Some(piece) = bytes
                .get(from..to)
                .and_then(|b| std::str::from_utf8(b).ok())
                && !piece.is_empty()
            {
                runs.push((value.len() as u32, from as u32, piece.len() as u32));
                value.push_str(piece);
            }
        };
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'"' => break,
                b'\\' => {
                    flush(&mut value, &mut runs, run_start, self.pos, self.bytes);
                    // Marca o começo do escape, para a posição dele no texto voltar para a barra.
                    runs.push((value.len() as u32, self.pos as u32, 0));
                    // O escape pode estar cortado no fim do texto.
                    let Some(&escape) = self.bytes.get(self.pos + 1) else {
                        self.pos = self.bytes.len();
                        break;
                    };
                    self.pos += 2;
                    match escape {
                        b'n' => value.push('\n'),
                        b't' => value.push('\t'),
                        b'r' => value.push('\r'),
                        b'b' => value.push('\u{8}'),
                        b'f' => value.push('\u{c}'),
                        b'u' if self.hex4(self.pos).is_some() => {
                            let mut code = self.hex4(self.pos);
                            self.pos += 4;
                            // Par substituto de UTF-16, como `\ud83d\ude00`.
                            if let Some(high @ 0xd800..=0xdbff) = code {
                                let low = (self.bytes.get(self.pos..self.pos + 2) == Some(b"\\u"))
                                    .then(|| self.hex4(self.pos + 2))
                                    .flatten();
                                if let Some(low @ 0xdc00..=0xdfff) = low {
                                    self.pos += 6;
                                    code = Some(0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00));
                                }
                            }
                            value.push(code.and_then(char::from_u32).unwrap_or('\u{fffd}'));
                        }
                        // Escape desconhecido ou `\u` sem quatro dígitos. O caractere depois da
                        // barra entra inteiro, mesmo com acento, para o mapa não cair no meio dele.
                        _ => {
                            let start = self.pos - 1;
                            let width = utf8_width(escape);
                            let end = (start + width).min(self.bytes.len());
                            let ch = std::str::from_utf8(&self.bytes[start..end]).ok();
                            value.push(ch.and_then(|c| c.chars().next()).unwrap_or('\u{fffd}'));
                            self.pos = end;
                        }
                    }
                    run_start = self.pos;
                }
                _ => self.pos += 1,
            }
        }
        let end = self.pos.min(self.bytes.len());
        flush(&mut value, &mut runs, run_start, end, self.bytes);
        self.pos = end + 1;
        JsonString {
            value,
            runs,
            start,
            end: end as u32,
        }
    }
}

impl<'a> Scanner<'a> {
    /// Chave de objeto a partir das aspas. Sem escape, que é o caso de quase toda chave, devolve o
    /// trecho do próprio JSON, sem alocar.
    fn key(&mut self) -> std::borrow::Cow<'a, str> {
        let bytes: &'a [u8] = self.bytes;
        let start = self.pos + 1;
        let rest = bytes.get(start..).unwrap_or_default();
        if let Some(k) = memchr::memchr2(b'"', b'\\', rest)
            && rest[k] == b'"'
            && let Ok(key) = std::str::from_utf8(&rest[..k])
        {
            self.pos = start + k + 1;
            return std::borrow::Cow::Borrowed(key);
        }
        std::borrow::Cow::Owned(self.string().value)
    }

    /// Pula uma string a partir das aspas, sem montar o valor, e para no mesmo lugar que a
    /// [`Scanner::string`]. Num escape basta pular o byte logo depois da barra, que pode ser aspas
    /// ou outra barra. O resto do escape, como os dígitos do `\u` ou o fim de um caractere com
    /// acento, nunca é aspas nem barra.
    fn skip_string(&mut self) {
        let bytes = self.bytes;
        let mut at = self.pos + 1;
        while let Some(k) = memchr::memchr2(b'"', b'\\', bytes.get(at..).unwrap_or_default()) {
            if bytes[at + k] == b'"' {
                self.pos = at + k + 1;
                return;
            }
            at += k + 2;
        }
        self.pos = bytes.len() + 1;
    }

    /// Valor dos quatro dígitos hexadecimais em `at`, ou `None` se não forem quatro dígitos.
    fn hex4(&self, at: usize) -> Option<u32> {
        let digits = self.bytes.get(at..at + 4)?;
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
    }
}

/// Tamanho do caractere UTF-8 que começa com este byte.
fn utf8_width(first: u8) -> usize {
    match first {
        0xf0..=0xf7 => 4,
        0xe0..=0xef => 3,
        0xc0..=0xdf => 2,
        _ => 1,
    }
}

fn assemble(prop: String, lines: Vec<JsonString>) -> EmbeddedRule {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut spans = Vec::new();
    for (i, line) in lines.into_iter().enumerate() {
        if i > 0 {
            text.push('\n');
        }
        let base = text.len() as u32;
        runs.extend(line.runs.iter().map(|&(v, j, len)| (base + v, j, len)));
        // A linha vazia ainda precisa de um trecho, para a posição dela existir.
        if line.runs.is_empty() {
            runs.push((base, line.start, 0));
        }
        text.push_str(&line.value);
        spans.push((line.start, line.end));
    }
    EmbeddedRule {
        prop,
        text,
        runs,
        lines: spans,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanner(json: &str) -> Scanner<'_> {
        Scanner {
            bytes: json.as_bytes(),
            pos: 0,
            entries: Vec::new(),
            depth: 0,
        }
    }

    #[test]
    fn pular_e_ler_a_chave_param_onde_a_string_para() {
        let casos = [
            r#""simples" x"#,
            r#""aspas \" e barra \\" x"#,
            r#""fim com barra \\\\" x"#,
            r#""😀 e ç" x"#,
            r#""escape desconhecido \é" x"#,
            r#""\u" x"#,
            "\"acentuação ção\" x",
            r#""sem fechar \"#,
            r#""sem fechar"#,
            r#""""#,
        ];
        for caso in casos {
            let mut lida = scanner(caso);
            let valor = lida.string().value;
            let mut pulada = scanner(caso);
            pulada.skip_string();
            assert_eq!(pulada.pos, lida.pos, "skip_string em {caso}");
            let mut chave = scanner(caso);
            assert_eq!(chave.key(), valor, "key em {caso}");
            assert_eq!(chave.pos, lida.pos, "key em {caso}");
        }
    }

    const JSON: &str = r#"{
  "children": [
    { "name": "x", "rule": { "prop_name": "SOnPrint", "lines": ["Definir Alfa a;", "", "a = \"x\" + 'b';"], "number": 1 } }
  ],
  "before_print": { "rule": { "lines": [], "prop_name": "SOnBeforePrint" } }
}"#;

    #[test]
    fn acha_as_regras_com_codigo() {
        let rules = extract(JSON);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].prop, "SOnPrint");
        assert_eq!(rules[0].text, "Definir Alfa a;\n\na = \"x\" + 'b';");
    }

    #[test]
    fn posicoes_vao_e_voltam() {
        let rule = &extract(JSON)[0];
        // O `a` da terceira linha, depois de duas quebras.
        let v = rule.text.rfind("a =").unwrap() as u32;
        let j = rule.to_json(TextSize::new(v));
        assert_eq!(&JSON[usize::from(j)..usize::from(j) + 3], "a =");
        assert_eq!(rule.from_json(j), Some(TextSize::new(v)));
        // O começo do escape volta para a barra.
        let vq = rule.text.find('"').unwrap() as u32;
        let jq = rule.to_json(TextSize::new(vq));
        assert_eq!(&JSON[usize::from(jq)..usize::from(jq) + 2], "\\\"");
        // Depois do escape `\"`, o `x` do texto cai no `x` do JSON.
        let vx = rule.text.find("\"x\"").unwrap() as u32 + 1;
        let jx = rule.to_json(TextSize::new(vx));
        assert_eq!(&JSON[usize::from(jx)..usize::from(jx) + 1], "x");
        // Fora das linhas da regra não há posição.
        assert_eq!(rule.from_json(TextSize::new(2)), None);
    }

    #[test]
    fn json_malformado_ou_muito_aninhado_termina() {
        assert!(extract("[}").is_empty());
        assert!(extract("{\"a\": [1, }, ]").is_empty());
        let deep = "[".repeat(100_000);
        assert!(extract(&deep).is_empty());
        let deep = "{\"a\":".repeat(100_000);
        assert!(extract(&deep).is_empty());
    }

    #[test]
    fn escape_desconhecido_com_acento_mantem_o_mapa_alinhado() {
        for json in [
            "{\"rule\": {\"lines\": [\"a\\\u{e9}b c\"]}}",
            "{\"rule\": {\"lines\": [\"x\\u12\u{e9}y\"]}}",
            "{\"rule\": {\"lines\": [\"fim\\",
        ] {
            for rule in extract(json) {
                // Toda fronteira de caractere do texto vai para uma fronteira do JSON e volta.
                for (i, _) in rule.text.char_indices() {
                    let j = usize::from(rule.to_json(TextSize::new(i as u32)));
                    assert!(json.is_char_boundary(j), "{json:?} {i}");
                    let back = rule.from_json(TextSize::new(j as u32)).unwrap();
                    assert!(rule.text.is_char_boundary(usize::from(back)));
                }
            }
        }
        let rule = &extract("{\"rule\": {\"lines\": [\"a\\\u{e9}b\"]}}")[0];
        assert_eq!(rule.text, "a\u{e9}b");
    }

    #[test]
    fn ve_a_mudanca_dos_outros_arquivos_do_modelo() {
        let base = std::env::temp_dir().join(format!("senior-lsp-modelo-{}", std::process::id()));
        let dir = base.join("Modelos/M1.GER");
        std::fs::create_dir_all(dir.join("regras")).unwrap();
        let rule =
            |code: &str| format!(r#"{{"rule": {{"prop_name": "SOnPrint", "lines": ["{code}"]}}}}"#);
        let open = dir.join("Aberto.json");
        let other = dir.join("Outro.json");
        let separated = dir.join("regras/Outro.Campo.SOnPrint.txt");
        std::fs::write(&open, rule("no disco")).unwrap();
        std::fs::write(&other, rule("definir numero nUm;")).unwrap();
        std::fs::write(&separated, "definir alfa aSeparada;").unwrap();
        let texts = |model: &Model| -> Vec<String> {
            model
                .rules
                .iter()
                .map(|(_, e)| e.rule.text.clone())
                .collect()
        };

        // O arquivo aberto vale pelo texto do editor, e os outros pelo disco.
        let first = model(&open, &rule("do editor"));
        assert_eq!(
            texts(&first),
            [
                "do editor",
                "definir numero nUm;",
                "definir alfa aSeparada;"
            ]
        );
        // Sem mudança no disco, a regra dos outros arquivos é a mesma em memória, com as
        // definições já tiradas, e a do arquivo aberto é lida de novo.
        first.rules[1].1.defs();
        let again = model(&open, &rule("do editor"));
        assert!(std::sync::Arc::ptr_eq(&first.rules[1].1, &again.rules[1].1));
        assert!(std::sync::Arc::ptr_eq(&first.rules[2].1, &again.rules[2].1));
        assert!(!std::sync::Arc::ptr_eq(
            &first.rules[0].1,
            &again.rules[0].1
        ));
        // Sem guardar, tudo vem do disco, menos o aberto.
        let uncached = model_uncached(&open, &rule("do editor"));
        assert_eq!(texts(&uncached), texts(&first));
        assert!(!std::sync::Arc::ptr_eq(
            &first.rules[1].1,
            &uncached.rules[1].1
        ));

        // Outro arquivo alterado, com tamanho diferente.
        std::fs::write(&other, rule("definir numero nDois; nDois = 2;")).unwrap();
        std::fs::write(&separated, "definir alfa aOutra; aOutra = \"x\";").unwrap();
        let changed = model(&open, &rule("do editor"));
        assert_eq!(
            texts(&changed),
            [
                "do editor",
                "definir numero nDois; nDois = 2;",
                "definir alfa aOutra; aOutra = \"x\";"
            ]
        );

        // A regra separada aberta no editor também vale pelo texto do editor.
        let from_rule = model(&separated, "definir data dEditor;");
        assert_eq!(
            texts(&from_rule),
            [
                "no disco",
                "definir numero nDois; nDois = 2;",
                "definir data dEditor;"
            ]
        );

        // Arquivo apagado e arquivo novo.
        std::fs::remove_file(&other).unwrap();
        std::fs::remove_file(&separated).unwrap();
        std::fs::write(dir.join("Novo.json"), rule("definir numero nNovo;")).unwrap();
        let removed = model(&open, &rule("do editor"));
        assert_eq!(texts(&removed), ["do editor", "definir numero nNovo;"]);
        assert_eq!(removed.texts.len(), 2);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn guarda_poucos_modelos() {
        let base = std::env::temp_dir().join(format!("senior-lsp-modelos-{}", std::process::id()));
        let file = |n: usize| base.join(format!("Modelos/M{n}.GER/A.json"));
        for n in 0..=MAX_MODELS {
            std::fs::create_dir_all(file(n).parent().unwrap()).unwrap();
            std::fs::write(file(n), "{}").unwrap();
            let other = file(n).with_file_name("B.json");
            let json = format!(r#"{{"rule": {{"lines": ["definir numero n{n};"]}}}}"#);
            std::fs::write(other, json).unwrap();
        }
        // Passando do limite o cache recomeça vazio, e o resultado continua certo.
        for n in 0..=MAX_MODELS {
            let rules = model(&file(n), "{}").rules;
            assert_eq!(rules[0].1.rule.text, format!("definir numero n{n};"));
        }
        assert_eq!(MODELS.with_borrow(std::collections::HashMap::len), 1);
        assert_eq!(model(&file(0), "{}").rules.len(), 1);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn entradas_do_relatorio() {
        let json = r#"{ "entrance": [
  { "sytem_varName": "ECodEmp", "description": "Empresa", "type_var": 1 },
  { "sytem_varName": "ECodFil", "description": "Filial(+)", "type_var": 3, "embrance": "true" },
  { "sytem_varName": "EDatIni", "description": "Mês/Ano", "type_var": 4 },
  { "sytem_varName": "EAnaSin", "description": "Analítico / Sintético", "type_var": 3, "entrance_type": 3, "size": 1 },
  { "sytem_varName": "ETipo", "description": "Tipo", "type_var": 3, "entrance_type": 3, "size": 2 }
] }"#;
        let (_, entries) = scan(json);
        let names: Vec<_> = entries.iter().map(|e| (e.name.as_str(), e.ty)).collect();
        use crate::symbols::VarType;
        assert_eq!(
            names,
            [
                ("ECodEmp", Some(VarType::Numero)),
                ("ECodFil", Some(VarType::Alfa)),
                ("EDatIni", Some(VarType::Data)),
                ("EAnaSin", Some(VarType::Numero)),
                ("ETipo", Some(VarType::Alfa))
            ]
        );
        assert_eq!(&json[entries[1].name_range], "ECodFil");
        assert_eq!(entries[1].description, "Filial(+)");
    }

    #[test]
    fn controles_com_valor_viram_nomes_do_modelo() {
        let json = r#"{ "classname": "TSDetail", "name": "Detalhe_1", "children": [
  { "classname": "TSFormula", "name": "FSaldo", "font": { "name": "Arial" }, "formula": { "type_var": 5 } },
  { "classname": "TSCadastro", "name": "Cadastro001" },
  { "name": "DSigFil", "classname": "TSDescricao" }
] }"#;
        let (_, entries) = scan(json);
        let found: Vec<_> = entries.iter().map(|e| (e.name.as_str(), e.kind)).collect();
        assert_eq!(
            found,
            [
                ("FSaldo", EntryKind::Control(Control::Formula)),
                ("DSigFil", EntryKind::Control(Control::Descricao))
            ]
        );
        assert_eq!(&json[entries[1].name_range], "DSigFil");
        use crate::symbols::VarType;
        assert_eq!(entries[0].ty, Some(VarType::Numero));
        assert_eq!(entries[1].ty, Some(VarType::Alfa));
    }

    #[test]
    fn caractere_de_controle_cru_e_unicode() {
        let json = "{\"rule\": {\"lines\": [\"a\tb\", \"\\u00e7\\ud83d\\ude00\"]}}";
        let rule = &extract(json)[0];
        assert_eq!(rule.text, "a\tb\nç😀");
    }
}
