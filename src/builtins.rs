//! Funções que o ERP já traz, como `IntParaAlfa`, com as declarações que o compilador usa para
//! conferir as chamadas.
//!
//! Todo compilador recebe o grupo de regra, e a tela SGI e o relatório acrescentam as funções
//! deles. Um nome fora desses grupos e sem `definir funcao` no arquivo não compila. Os arquivos têm uma
//! declaração por linha, na ordem em que o compilador registra, e vale a primeira de cada nome.

use rustc_hash::FxHashMap;
use std::sync::LazyLock;

use crate::symbols::VarType;

/// Onde a regra roda, pela pasta da exportação do gerenciador de artefatos. O compilador da regra
/// de web service não acrescenta funções, então ela conta como regra comum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// Regra comum, como a de identificador e a de web service, e tudo fora de `Formulários SGI`.
    Rule,
    /// Regra de tela SGI, na pasta `Formulários SGI`.
    Sgi,
    /// Regra de modelo de relatório, nos JSON da pasta `Modelos` ou separada em `regras/*.txt`.
    Report,
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
        } else if segments.any(|s| s.eq_ignore_ascii_case("Modelos"))
            && (path.to_ascii_lowercase().ends_with(".json")
                || crate::report::is_rule_file(std::path::Path::new(&path)))
        {
            Context::Report
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
    /// Contexto de cada função de `builtins`, na mesma posição, ou `None` quando existe em todos.
    contexts: Vec<Option<Context>>,
    /// Nome em maiúsculas para o índice em `builtins` e o contexto em que ela existe, ou `None`
    /// quando existe em todos.
    index: FxHashMap<String, (usize, Option<Context>)>,
}

static TABLE: LazyLock<Table> = LazyLock::new(|| {
    let mut table = Table {
        builtins: Vec::new(),
        contexts: Vec::new(),
        index: FxHashMap::default(),
    };
    for (source, only) in [
        (include_str!("builtins/regra.txt"), None),
        (include_str!("builtins/sgi.txt"), Some(Context::Sgi)),
        (
            include_str!("builtins/relatorio.txt"),
            Some(Context::Report),
        ),
    ] {
        for line in source.lines().filter(|l| !l.trim().is_empty()) {
            let builtin = parse_decl(line).expect("declaração embutida fora do formato");
            let key = builtin.name.to_uppercase();
            if !table.index.contains_key(&key) {
                table.index.insert(key, (table.builtins.len(), only));
                table.builtins.push(builtin);
                table.contexts.push(only);
            }
        }
    }
    table
});

/// Variáveis do sistema, como `DatSis` e `NomUsu`, pelo nome em maiúsculas. O ERP registra todas
/// juntas quando cria o ambiente das regras, sem depender de onde a regra roda. O arquivo traz uma
/// por linha como `definir`, com o tipo que o compilador recebe.
static SYSTEM_VARIABLES: LazyLock<FxHashMap<String, VarType>> = LazyLock::new(|| {
    include_str!("builtins/variaveis.txt")
        .lines()
        .filter_map(|line| {
            let mut words = line.trim_end_matches(';').split_whitespace().skip(1);
            let ty = match words.next()? {
                "Alfa" => VarType::Alfa,
                "Numero" => VarType::Numero,
                "Data" => VarType::Data,
                _ => return None,
            };
            Some((words.next()?.to_uppercase(), ty))
        })
        .collect()
});

/// Campos que o SGI acrescenta a toda tela, todos alfa. Eles não aparecem na exportação.
const SGI_FIELDS: [&str; 4] = ["CODOPE", "MSGERRO", "MSGAVISO", "TELOPE"];

/// Tipo da variável do sistema com esse nome, incluindo os campos que a tela SGI sempre tem.
pub fn system_variable(name: &str, context: Context) -> Option<VarType> {
    let upper = name.to_uppercase();
    if context == Context::Sgi && SGI_FIELDS.contains(&upper.as_str()) {
        return Some(VarType::Alfa);
    }
    SYSTEM_VARIABLES.get(&upper).copied()
}

/// Todas as funções do ERP do contexto, na ordem dos arquivos de declaração. Percorrer o índice
/// daria uma ordem que muda com o espalhamento, e o completar ficaria diferente a cada execução.
pub fn all(context: Context) -> impl Iterator<Item = &'static Builtin> {
    TABLE
        .builtins
        .iter()
        .zip(&TABLE.contexts)
        .filter(move |&(_, only)| only.is_none_or(|c| c == context))
        .map(|(builtin, _)| builtin)
}

/// Função do ERP com esse nome no contexto, sem diferença de caixa.
pub fn lookup(name: &str, context: Context) -> Option<&'static Builtin> {
    let &(i, only) = TABLE.index.get(&name.to_uppercase())?;
    only.is_none_or(|c| c == context)
        .then(|| &TABLE.builtins[i])
}

/// Nomes antigos que o ERP troca pelos atuais antes de compilar uma regra de versão anterior,
/// como `ConverteParaMaiusculo2` por `ConverteParaMaiusculo`. Como a troca pode mexer nos
/// parâmetros, a chamada a um deles não é conferida, só deixa de ser nome desconhecido.
pub fn converted(name: &str) -> Option<&'static str> {
    const NAMES: [(&str, &str); 3] = [
        ("ConverteParaMaiusculo2", "ConverteParaMaiusculo"),
        ("ConverteParaMinusculo2", "ConverteParaMinusculo"),
        ("EnviarEMail2", "EnviarEMail"),
    ];
    NAMES
        .iter()
        .find(|(old, _)| old.eq_ignore_ascii_case(name))
        .map(|&(_, new)| new)
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
        if bytes[i] == b'%'
            && let Some(b) = text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
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
    fn lista_as_funcoes_na_ordem_dos_arquivos() {
        let nomes: Vec<&str> = all(Context::Rule).take(3).map(|b| b.name).collect();
        assert_eq!(
            nomes,
            [
                "ExecutaRegraRecursiva",
                "ConverteDataToDB",
                "ConverteDataBanco"
            ]
        );
        assert!(all(Context::Rule).all(|b| lookup(b.name, Context::Rule).is_some()));
        assert!(all(Context::Rule).count() < all(Context::Sgi).count());
    }

    #[test]
    fn le_todas_as_declaracoes() {
        assert_eq!(TABLE.builtins.len(), 899 + 257);
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
        assert!(lookup("AbrirTelaSGI", Context::Sgi).is_some());
        assert!(lookup("IntParaAlfa", Context::Sgi).is_some());
        assert!(lookup("InsClauSQLWhere", Context::Sgi).is_none());
        assert!(lookup("InsClauSQLWhere", Context::Report).is_some());
        assert!(lookup("AlteraControle", Context::Report).is_some());
        assert!(lookup("AlteraControle", Context::Rule).is_none());
        assert!(lookup("AbrirTelaSGI", Context::Report).is_none());
        assert!(lookup("IntParaAlfa", Context::Report).is_some());
        assert_eq!(
            converted("converteparamaiusculo2"),
            Some("ConverteParaMaiusculo")
        );
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
            Context::Rule
        );
        assert_eq!(
            ctx("file:///c/Modelos/CCCC001.GER/Cabecalho.json"),
            Context::Report
        );
    }
}
