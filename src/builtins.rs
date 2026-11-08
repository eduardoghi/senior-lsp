//! Funções que o ERP já traz, como `IntParaAlfa`, com as declarações que o compilador usa para
//! conferir as chamadas.
//!
//! Todo compilador recebe o grupo de regra, e a tela SGI acrescenta as funções dela. Um nome
//! fora desses grupos e sem `definir funcao` no arquivo não compila. Os arquivos têm uma
//! declaração por linha, na ordem em que o compilador registra, e vale a primeira de cada nome.
//!
//! As declarações são lidas aqui, e não pelo parser da linguagem, porque algumas usam palavras
//! reservadas como nome de parâmetro (`Numero Data`, `Numero End Fim`).

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::symbols::VarType;

/// Onde a regra roda, pela pasta da exportação do gerenciador de artefatos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// Regra de identificador e tudo que não tem pasta conhecida.
    Rule,
    /// Regra de tela SGI, na pasta `Formulários SGI`.
    Sgi,
    /// Regra de web service. Recebe o grupo de regra, mas ainda não se sabe se o compilador de
    /// web service acrescenta funções, então nome desconhecido não é acusado.
    WebService,
}

impl Context {
    /// Contexto pelo caminho do arquivo, em forma de URI (`file:///...`).
    pub fn from_uri(uri: &str) -> Context {
        let path = percent_decode(uri);
        let mut segments = path.split('/');
        if segments
            .clone()
            .any(|s| s.eq_ignore_ascii_case("Formulários SGI"))
        {
            Context::Sgi
        } else if segments.any(|s| s.eq_ignore_ascii_case("Web Services")) {
            Context::WebService
        } else {
            Context::Rule
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub ty: VarType,
    pub by_ref: bool,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Builtin {
    pub name: &'static str,
    /// A declaração como o compilador registra, sem comentários.
    pub decl: &'static str,
    pub params: Vec<Param>,
}

struct Table {
    builtins: Vec<Builtin>,
    /// Nome em maiúsculas para o índice em `builtins` e se a função é só de tela SGI.
    index: HashMap<String, (usize, bool)>,
}

static TABLE: LazyLock<Table> = LazyLock::new(|| {
    let mut table = Table {
        builtins: Vec::new(),
        index: HashMap::new(),
    };
    for (source, sgi) in [
        (include_str!("builtins/regra.txt"), false),
        (include_str!("builtins/sgi.txt"), true),
    ] {
        for line in source.lines().filter(|l| !l.trim().is_empty()) {
            let builtin = parse_decl(line).expect("declaração embutida fora do formato");
            let key = builtin.name.to_uppercase();
            if !table.index.contains_key(&key) {
                table.index.insert(key, (table.builtins.len(), sgi));
                table.builtins.push(builtin);
            }
        }
    }
    table
});

/// Função do ERP com esse nome no contexto, sem diferença de caixa.
pub fn lookup(name: &str, context: Context) -> Option<&'static Builtin> {
    let &(i, sgi) = TABLE.index.get(&name.to_uppercase())?;
    (!sgi || context == Context::Sgi).then(|| &TABLE.builtins[i])
}

/// Lê `Funcao Nome(Tipo [End] nome, ...);`. Comentários `/* */` no meio são ignorados.
fn parse_decl(line: &'static str) -> Option<Builtin> {
    let decl = line.trim();
    let rest = strip_prefix_ignore_case(decl, "funcao")?.trim_start();
    let open = rest.find('(')?;
    let close = rest.rfind(')')?;
    let name = rest[..open].trim();
    let mut params = Vec::new();
    for param in without_comments(&rest[open + 1..close]).split(',') {
        let words: Vec<&str> = param.split_whitespace().collect();
        let (ty, by_ref, param_name) = match words.as_slice() {
            [] => continue,
            [ty, name] => (*ty, false, *name),
            [ty, e, name] if e.eq_ignore_ascii_case("end") => (*ty, true, *name),
            _ => return None,
        };
        let ty = match ty.to_ascii_lowercase().as_str() {
            "alfa" => VarType::Alfa,
            "numero" => VarType::Numero,
            "data" => VarType::Data,
            _ => return None,
        };
        params.push(Param {
            ty,
            by_ref,
            name: param_name.to_owned(),
        });
    }
    Some(Builtin { name, decl, params })
}

fn without_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start + 2..]
            .find("*/")
            .map_or("", |end| &rest[start + 2 + end + 2..]);
    }
    out.push_str(rest);
    out
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// Decodifica `%XX` em bytes de UTF-8. Sequência inválida fica como está.
pub(crate) fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(b) = text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_todas_as_declaracoes() {
        assert_eq!(TABLE.builtins.len(), 899);
        let f = lookup("intparaalfa", Context::Rule).unwrap();
        assert_eq!(f.name, "IntParaAlfa");
        assert_eq!(
            f.params,
            [
                Param {
                    ty: VarType::Numero,
                    by_ref: false,
                    name: "Origem".to_owned()
                },
                Param {
                    ty: VarType::Alfa,
                    by_ref: true,
                    name: "Retorno".to_owned()
                },
            ]
        );
    }

    #[test]
    fn parametro_com_nome_reservado_e_comentario() {
        let f = lookup("VerificaDiaUtil", Context::Rule).unwrap();
        assert_eq!(f.params[0].name, "Data");
        let f = lookup("ExecProg", Context::Rule).unwrap();
        assert_eq!(f.params.len(), 3);
        assert_eq!(f.params[2].name, "pEsperar");
    }

    #[test]
    fn funcao_de_sgi_so_na_tela() {
        assert!(lookup("AbrirTelaSGI", Context::Rule).is_none());
        assert!(lookup("AbrirTelaSGI", Context::WebService).is_none());
        assert!(lookup("AbrirTelaSGI", Context::Sgi).is_some());
        assert!(lookup("IntParaAlfa", Context::Sgi).is_some());
        assert!(lookup("InsClauSQLWhere", Context::Sgi).is_none());
    }

    #[test]
    fn contexto_pela_pasta() {
        let ctx = |s| Context::from_uri(s);
        assert_eq!(ctx("file:///c/regras/REGRA1.LSP/Rule.txt"), Context::Rule);
        assert_eq!(
            ctx("file:///c/Formul%C3%A1rios%20SGI/interface/X/rule.txt"),
            Context::Sgi
        );
        assert_eq!(
            ctx("file:///c/Formulários SGI/window/X/rule.txt"),
            Context::Sgi
        );
        assert_eq!(
            ctx("file:///c/Web%20Services/interno/X/Rule.txt"),
            Context::WebService
        );
    }
}
