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

/// Regras de todos os JSON do modelo de `json_path`, a pasta dele. O arquivo aberto no editor
/// entra com `current`, e os outros são lidos do disco, em UTF-8 ou cp1252.
pub fn model_rules(
    json_path: &std::path::Path,
    current: &str,
) -> Vec<(std::path::PathBuf, std::sync::Arc<Embedded>)> {
    let mut out = Vec::new();
    let Some(dir) = json_path.parent() else {
        return out;
    };
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
    for path in files {
        let text = if path == json_path {
            Some(current.to_owned())
        } else {
            std::fs::read(&path)
                .ok()
                .and_then(|b| match String::from_utf8(b) {
                    Ok(t) => Some(t),
                    Err(e) => crate::cp1252::decode(e.as_bytes()),
                })
        };
        let Some(text) = text else {
            continue;
        };
        let json: std::sync::Arc<str> = text.into();
        for rule in extract(&json) {
            let embedded = Embedded {
                json: json.clone(),
                rule,
            };
            out.push((path.clone(), std::sync::Arc::new(embedded)));
        }
    }
    out
}

/// Todas as regras com código do JSON, na ordem do texto.
pub fn extract(json: &str) -> Vec<EmbeddedRule> {
    let mut scanner = Scanner {
        bytes: json.as_bytes(),
        pos: 0,
        depth: 0,
    };
    let mut rules = Vec::new();
    scanner.value(&mut rules, None);
    rules.retain(|r| !r.text.trim().is_empty());
    rules
}

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
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
            Some(b'"') => {
                self.string();
            }
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

    fn object(&mut self, rules: &mut Vec<EmbeddedRule>, is_rule: bool) {
        self.pos += 1;
        let mut prop = String::new();
        let mut lines: Vec<JsonString> = Vec::new();
        loop {
            match self.peek() {
                Some(b'}') | None => {
                    self.pos += 1;
                    break;
                }
                Some(b',') => self.pos += 1,
                Some(b'"') => {
                    let key = self.string().value;
                    if self.peek() == Some(b':') {
                        self.pos += 1;
                    }
                    if is_rule && key == "lines" && self.peek() == Some(b'[') {
                        lines = self.string_array();
                    } else if is_rule && key == "prop_name" && self.peek() == Some(b'"') {
                        prop = self.string().value;
                    } else {
                        self.value(rules, Some(&key));
                    }
                }
                Some(_) => self.pos += 1,
            }
        }
        if is_rule {
            rules.push(assemble(prop, lines));
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
            {
                if !piece.is_empty() {
                    runs.push((value.len() as u32, from as u32, piece.len() as u32));
                    value.push_str(piece);
                }
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

impl Scanner<'_> {
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
    fn caractere_de_controle_cru_e_unicode() {
        let json = "{\"rule\": {\"lines\": [\"a\tb\", \"\\u00e7\\ud83d\\ude00\"]}}";
        let rule = &extract(json)[0];
        assert_eq!(rule.text, "a\tb\nç😀");
    }
}
