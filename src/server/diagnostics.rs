//! Erros do lexer e do parser e verificações do validador como diagnósticos do protocolo.
//!
//! Cada tipo de erro tem um código estável (`SEN001`...) e uma mensagem. Quando o texto do
//! compilador da Senior foi conferido para o mesmo caso, ele aparece entre parênteses, para quem
//! conhece o erro do ERP reconhecer. Todos os erros de hoje foram conferidos no compilador e
//! nenhum aparece nas regras que compilam. Um tipo novo sem essa conferência começa desligado em
//! [`describe_parse`].

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};
use text_size::TextRange;

use super::config::Levels;
use super::directives;
use super::documents::Document;
use crate::line_index::{ColumnEncoding, LineIndex};
use crate::validate::{Check, CheckKind};
use crate::{LexErrorKind, ParseErrorKind, SyntaxKind, Tree};

/// Diagnóstico ainda na posição do texto da regra, antes das diretivas, da configuração e da
/// conversão para linha e coluna.
pub(crate) struct Found {
    pub(crate) range: TextRange,
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) severity: DiagnosticSeverity,
}

/// Diagnósticos da regra para o editor. `extra` traz os que o servidor acrescenta fora do
/// validador, como o do web service. Depois de todos prontos, as diretivas do arquivo escondem os
/// que citam (menos os erros de sintaxe) e o `levels` do `senior-lsp.json` troca ou tira o nível de
/// cada código.
///
/// `to_range` converte a posição no texto da regra, que num modelo de relatório não é a do arquivo.
pub(crate) fn diagnostics(
    doc: &Document,
    extra: Vec<Found>,
    levels: &Levels,
    to_range: &dyn Fn(TextRange) -> Range,
) -> Vec<Diagnostic> {
    let mut found = found(doc);
    found.extend(extra);
    let (mut found, notices) = directives::filter(
        &doc.text,
        &doc.index,
        doc.scan(),
        || doc.code_lines(),
        found,
        |f| (f.code, f.range.start()),
    );
    found.extend(notices.into_iter().map(|n| Found {
        range: n.range,
        code: n.code,
        message: n.message,
        severity: DiagnosticSeverity::WARNING,
    }));
    found
        .into_iter()
        .filter_map(|f| {
            let severity = match levels.get(f.code) {
                Some(level) => level.severity()?,
                None => f.severity,
            };
            Some(Diagnostic {
                range: to_range(f.range),
                severity: Some(severity),
                code: Some(NumberOrString::String(f.code.to_owned())),
                source: Some("senior-lsp".to_owned()),
                message: f.message,
                // O editor esmaece o nome sem uso, o código que não executa e a diretiva sem efeito.
                tags: matches!(f.code, "SEN068" | "SEN070" | "SEN071" | "SEN073" | "SEN076")
                    .then(|| vec![lsp_types::DiagnosticTag::UNNECESSARY]),
                ..Diagnostic::default()
            })
        })
        .collect()
}

/// Erros do lexer e do parser e avisos do validador, antes das diretivas.
fn found(doc: &Document) -> Vec<Found> {
    let (text, tree) = (doc.text.as_str(), &doc.tree);
    let lex = tree
        .tokens()
        .errors()
        .iter()
        .map(|e| (e.range, describe_lex(e.kind, &text[e.range])));
    let parse = tree
        .errors()
        .iter()
        .filter_map(|e| Some((e.range, describe_parse(e.kind)?)));
    let errors = lex
        .chain(parse)
        .map(|(range, described)| (range, described, DiagnosticSeverity::ERROR));
    let checks = doc.checks().iter().map(|c| {
        let severity = if c.kind.is_error() {
            DiagnosticSeverity::ERROR
        } else if matches!(
            c.kind,
            CheckKind::Unused
                | CheckKind::UnusedFunction
                | CheckKind::CursorColumnUnused
                | CheckKind::CursorStarUnused
        ) {
            DiagnosticSeverity::HINT
        } else {
            DiagnosticSeverity::WARNING
        };
        (c.range, describe_check(c, tree, text), severity)
    });
    errors
        .chain(checks)
        .map(|(range, (code, message), severity)| Found {
            range,
            code,
            message,
            severity,
        })
        .collect()
}

/// Conversão da posição no texto do documento para linha e coluna do editor.
pub(crate) fn lsp_range(
    doc: &Document,
    encoding: ColumnEncoding,
) -> impl Fn(TextRange) -> Range + '_ {
    move |range| to_lsp(&doc.text, &doc.index, encoding, range)
}

fn to_lsp(text: &str, index: &LineIndex, encoding: ColumnEncoding, range: TextRange) -> Range {
    let p = |offset| {
        let lc = index.line_col(text, offset, encoding);
        Position::new(lc.line, lc.col)
    };
    Range::new(p(range.start()), p(range.end()))
}

fn describe_lex(kind: LexErrorKind, text: &str) -> (&'static str, String) {
    match kind {
        LexErrorKind::InvalidChar => ("SEN001", format!("O caractere \"{text}\" é inválido.")),
        LexErrorKind::UnterminatedString => (
            "SEN002",
            "String sem as aspas de fechamento (cadeia de caracteres sem terminação).".to_owned(),
        ),
        LexErrorKind::UnterminatedAtComment => (
            "SEN003",
            "Comentário sem o @ de fechamento na mesma linha (comentário inválido).".to_owned(),
        ),
        LexErrorKind::UnterminatedBlockComment => (
            "SEN004",
            "Comentário /* sem o */ de fechamento (comentário sem terminação).".to_owned(),
        ),
        LexErrorKind::IdentTooLong => (
            "SEN005",
            "Nome com mais de 100 caracteres (identificador muito extenso).".to_owned(),
        ),
        LexErrorKind::MalformedChar => (
            "SEN006",
            "Literal de caractere sem o apóstrofo de fechamento (falta ').".to_owned(),
        ),
        LexErrorKind::MalformedNumber => (
            "SEN007",
            "Número que o compilador não aceita (identificador mal-formado).".to_owned(),
        ),
        LexErrorKind::LineTooLong => (
            "SEN008",
            "Linha maior do que o compilador aceita (linha para o compilador é muito grande)."
                .to_owned(),
        ),
    }
}

/// Mensagem de um erro do parser, ou `None` para não publicar.
fn describe_parse(kind: ParseErrorKind) -> Option<(&'static str, String)> {
    Some(match kind {
        ParseErrorKind::Expected(token) => ("SEN020", format!("Falta {}.", expected(token))),
        ParseErrorKind::ExpectedExpression => ("SEN021", "Falta um valor.".to_owned()),
        ParseErrorKind::ChainedComparison => (
            "SEN022",
            "Só cabe uma comparação por expressão. Use parênteses, como em (a = 1) e (b = 2)."
                .to_owned(),
        ),
        ParseErrorKind::ExpectedStatement => (
            "SEN023",
            "Isto não começa um comando (falta valor, expressão ou comando).".to_owned(),
        ),
        ParseErrorKind::UnmatchedEnd => (
            "SEN024",
            "Fim de bloco sem início (Cláusula \"Fim\" sem Cláusula \"Início\").".to_owned(),
        ),
        ParseErrorKind::TooDeep => (
            "SEN025",
            "Aninhamento grande demais. O resto do arquivo não foi analisado.".to_owned(),
        ),
        ParseErrorKind::ExpectedType => (
            "SEN026",
            "Falta o tipo, que é alfa, numero ou data.".to_owned(),
        ),
        ParseErrorKind::InvalidMessageType => (
            "SEN027",
            "O tipo do Mensagem é Erro, Refaz ou Retorna (tipo de mensagem inválido).".to_owned(),
        ),
        ParseErrorKind::ExpectedMessageText => (
            "SEN028",
            "O texto do Mensagem tem que ser uma string ou uma variável alfa \
             (Cláusula \"Mensagem\", falta \"mensagem\")."
                .to_owned(),
        ),
    })
}

fn describe_check(check: &Check, tree: &Tree, text: &str) -> (&'static str, String) {
    let name = match (check.name, &check.label) {
        (Some(node), _) => &text[tree.text_range(node)],
        (None, Some(label)) => label.as_str(),
        (None, None) => "",
    };
    match check.kind {
        CheckKind::MinusTakesOneToken => (
            "SEN040",
            "Depois de *, / ou e, o - pega só o próximo token, e o resto sobra (falta \";\")."
                .to_owned(),
        ),
        CheckKind::ComparisonInAssignment => (
            "SEN041",
            "Comparação na atribuição só vale entre parênteses, como em x = (a > 1) \
             (falta \";\")."
                .to_owned(),
        ),
        CheckKind::LogicalMinus => (
            "SEN060",
            "e ou ou seguido de - compila, mas numa atribuição o valor não é gravado.".to_owned(),
        ),
        CheckKind::CodeAfterFunctionBlock => (
            "SEN061",
            format!(
                "Este comando faz parte da função {name}. O corpo de uma função vai até a \
                 próxima funcao ou o fim do arquivo, não só até o fim do bloco."
            ),
        ),
        CheckKind::ForeignParameter => (
            "SEN063",
            "Parâmetro de outra função. Compila, mas o parâmetro só existe durante a chamada \
             da função dele, e aqui o valor lido é lixo."
                .to_owned(),
        ),
        CheckKind::NonAlfaMessageText => (
            "SEN042",
            "O texto do Mensagem tem que ser alfa, e esta variável não é \
             (Cláusula \"Mensagem\", falta \"mensagem\")."
                .to_owned(),
        ),
        CheckKind::AlfaAndNumber => (
            "SEN043",
            "Atribuição alfa só aceita string, apóstrofo ou variável alfa ligados por +, sem \
             parênteses (tipos incompatíveis: ALFA e NUMERO)."
                .to_owned(),
        ),
        CheckKind::AlfaOperator => (
            "SEN044",
            "Atribuição alfa só aceita + entre os valores (falta \";\").".to_owned(),
        ),
        CheckKind::NumberAndAlfa => (
            "SEN045",
            "Valor alfa onde se espera número (tipos incompatíveis: NUMERO e ALFA).".to_owned(),
        ),
        CheckKind::TwoStringLiterals => (
            "SEN046",
            "Não dá para comparar duas strings entre si (não podem ser 2 constantes literais)."
                .to_owned(),
        ),
        CheckKind::AlfaArgument => (
            "SEN047",
            format!("Valor alfa para o parâmetro {name}, que é numérico (são incompatíveis)."),
        ),
        CheckKind::NumberArgument => (
            "SEN050",
            format!("Valor numérico para o parâmetro {name}, que é alfa (são incompatíveis)."),
        ),
        CheckKind::AlfaOperatorInComparison => (
            "SEN051",
            "Numa comparação, alfa não aceita + nem outro operador (Variáveis alfanuméricas ou \
             operadores não compatíveis)."
                .to_owned(),
        ),
        CheckKind::ConstantInEndParameter => (
            "SEN053",
            format!(
                "O parâmetro {name} é end e devolve um valor, então precisa de uma variável (não \
                 pode ser constante)."
            ),
        ),
        CheckKind::UnknownFunction => (
            "SEN052",
            "Não há função com esse nome no ERP nem definir funcao no arquivo. O compilador lê o \
             nome como variável (falta \"=\")."
                .to_owned(),
        ),
        CheckKind::OnlyInTest => (
            "SEN065",
            format!(
                "O campo {} só existe no dicionário de teste. A regra compila na base de teste e \
                 não em produção.",
                &text[check.range]
            ),
        ),
        CheckKind::LowercaseChar => (
            "SEN064",
            "O compilador lê o apóstrofo em maiúscula, então esta letra vale como maiúscula, \
             na comparação e na concatenação."
                .to_owned(),
        ),
        CheckKind::TooManyArguments => (
            "SEN048",
            "Mais valores do que parâmetros na chamada (Muitos parâmetros na chamada da \
             função)."
                .to_owned(),
        ),
        CheckKind::TooFewArguments => (
            "SEN049",
            "Menos valores do que parâmetros na chamada (Faltam parâmetros na chamada da \
             função)."
                .to_owned(),
        ),
        CheckKind::CursorConstantColumn => (
            "SEN054",
            format!(
                "O SELECT do cursor não aceita a constante {name} como coluna, porque o \
                 compilador lê cada coluna como campo de uma tabela. Coluna sem tabela e * \
                 compilam."
            ),
        ),
        CheckKind::NativeSqlWithScope => (
            "SEN066",
            format!(
                "SQL nativo em {name} sem SQL_UsarAbrangencia({name}, 0). Compila, mas a \
                 execução para com \"Não é suportado o uso de abrangência de usuário com SQL \
                 nativo\"."
            ),
        ),
        CheckKind::CursorFieldNotInSelect => (
            "SEN055",
            format!(
                "O campo {} não está no SELECT do cursor {name} (Erro na variável de registro, \
                 falta \"campo ou operação não existem\").",
                &text[check.range]
            ),
        ),
        CheckKind::CursorUnknownTable => (
            "SEN074",
            format!(
                "O SELECT do cursor lê {name} como tabela, e ela não está no dicionário de dados \
                 (RowSet inexistente: {name}). As colunas do cursor não aceitam apelido de tabela \
                 nem função."
            ),
        ),
        CheckKind::CursorUnknownField => {
            let (table, field) = name.split_once('.').unwrap_or(("", name));
            // A coluna escrita sem a tabela é a única em que o trecho marcado não tem ponto.
            let first_table = if text[check.range].contains('.') {
                ""
            } else {
                " A coluna sem tabela é procurada na primeira tabela do FROM."
            };
            (
                "SEN075",
                format!(
                    "O campo {field} não existe na tabela {table} (Campo \"{field}\" inexistente \
                     na tabela/view \"{table}\").{first_table}"
                ),
            )
        }
        CheckKind::CursorColumnWithoutTable => (
            "SEN077",
            format!(
                "O compilador lê {name} como uma coluna com apelido, e essa precisa da tabela na \
                 frente (O campo precisa ser informado com a tabela). O SELECT do cursor não \
                 aceita DISTINCT."
            ),
        ),
        CheckKind::CursorRepeatedField => (
            "SEN078",
            format!(
                "O SELECT do cursor já tem um campo {name} de outra tabela, e o cursor guarda um \
                 campo por nome (Campo de nome \"{name}\" está sendo selecionado repetidamente \
                 no cursor)."
            ),
        ),
        CheckKind::CursorUnknownParameter => {
            let name = &text[check.range];
            (
                "SEN079",
                format!(
                    "A variável {name} ainda não apareceu na regra (identificador \"{name}\" não \
                     foi declarado ou utilizado na regra). O SQL do cursor só aceita como \
                     parâmetro a variável definida ou usada antes do comando."
                ),
            )
        }
        CheckKind::Sql2Join => (
            "SEN080",
            format!(
                "A regra usa o Senior SQL 2, que não aceita JOIN no SQL do cursor (Era esperado \
                 <fim-de-arquivo>, mas {name} foi encontrado). Junte as tabelas pela vírgula no \
                 FROM e pela condição no WHERE."
            ),
        ),
        CheckKind::Sql2UnknownTable => (
            "SEN081",
            format!(
                "A tabela {name} não está no dicionário de dados, e o Senior SQL 2 confere as \
                 tabelas do FROM (Tabela ou view \"{name}\" inexistente na base de dados)."
            ),
        ),
        CheckKind::Sql2UnknownSymbol => (
            "SEN082",
            format!(
                "O Senior SQL 2 não acha {name} nas tabelas do FROM (Símbolo não encontrado no \
                 comando: \"{name}\")."
            ),
        ),
        CheckKind::Sql2UnknownField => {
            let (table, field) = name.split_once('.').unwrap_or(("", name));
            (
                "SEN082",
                format!(
                    "O campo {field} não existe na tabela {table} (Campo não encontrado na \
                     tabela \"{table}\": \"{field}\")."
                ),
            )
        }
        CheckKind::Sql2AmbiguousField => (
            "SEN082",
            format!(
                "Mais de uma tabela do FROM tem o campo {name}, e o Senior SQL 2 exige a tabela \
                 na frente (Ambiguidade detectada: há mais de uma tabela que possui o campo \
                 \"{name}\")."
            ),
        ),
        CheckKind::Sql2OrderBy => (
            "SEN083",
            format!(
                "A regra usa o Senior SQL 2, que só aceita no ORDER BY o que está no SELECT (A \
                 expressão \"{name}\" não aparece na lista de expressões selecionadas e por \
                 isso não pode ser utilizado no Order By)."
            ),
        ),
        CheckKind::CursorColumnUnused => (
            "SEN076",
            format!(
                "A regra não lê o campo {} que o SELECT do cursor {name} busca.",
                &text[check.range]
            ),
        ),
        CheckKind::CursorStarUnused => {
            let mut parts = name.splitn(4, '|');
            let mut part = || parts.next().unwrap_or("");
            let (cursor, table, total, used) = (part(), part(), part(), part());
            let used = if used.is_empty() {
                "não lê nenhum".to_owned()
            } else {
                format!("só lê {used}")
            };
            (
                "SEN076",
                format!(
                    "O * busca os {total} campos de {table} para o cursor {cursor}, e a regra \
                     {used}."
                ),
            )
        }
        CheckKind::ReturnedColumnNotInSelect => (
            "SEN067",
            format!(
                "A coluna {} não está no SELECT de {name}. Compila, mas a execução para com \
                 \"O campo não foi encontrado\".",
                &text[check.range]
            ),
        ),
        CheckKind::Unreachable => (
            "SEN071",
            format!("Código depois de {name} não é executado."),
        ),
        CheckKind::UnusedFunction => (
            "SEN070",
            format!("A função {name} nunca é chamada neste arquivo."),
        ),
        CheckKind::Unused => (
            "SEN068",
            format!("{name} é definido e nunca usado neste arquivo."),
        ),
        CheckKind::FunctionInsideBlock => (
            "SEN062",
            format!(
                "A função {name} começa com um bloco de antes dela ainda aberto. O compilador \
                 conta os blocos no arquivo todo, e o próximo fim de bloco fecha aquele."
            ),
        ),
    }
}

fn expected(token: SyntaxKind) -> &'static str {
    match token {
        SyntaxKind::Semicolon => "\";\"",
        SyntaxKind::RParen => "\")\"",
        SyntaxKind::LParen => "\"(\"",
        SyntaxKind::RBracket => "\"]\"",
        SyntaxKind::RBrace => "\"}\"",
        SyntaxKind::LBrace => "\"{\"",
        SyntaxKind::Comma => "\",\"",
        SyntaxKind::Eq => "\"=\"",
        SyntaxKind::Ident => "um nome",
        _ => "um token",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::Context;
    use crate::server::documents::Documents;

    /// Código, início e fim de cada diagnóstico, com linha e coluna.
    type Resumo = (String, (u32, u32), (u32, u32));

    /// Diagnósticos de uma regra comum, sem configuração.
    fn run(text: &str, encoding: ColumnEncoding) -> Vec<Diagnostic> {
        let uri: lsp_types::Uri = "file:///regras/R.LSP/Rule.txt".parse().unwrap();
        let mut documents = Documents::default();
        documents
            .set(uri.clone(), text.to_owned(), 1, Context::Rule)
            .unwrap();
        let doc = documents.get(&uri).unwrap();
        diagnostics(
            doc,
            Vec::new(),
            &Levels::default(),
            &lsp_range(doc, encoding),
        )
    }

    fn diags(text: &str, encoding: ColumnEncoding) -> Vec<Resumo> {
        run(text, encoding)
            .into_iter()
            .map(|d| {
                let NumberOrString::String(code) = d.code.unwrap() else {
                    unreachable!()
                };
                (
                    code,
                    (d.range.start.line, d.range.start.character),
                    (d.range.end.line, d.range.end.character),
                )
            })
            .collect()
    }

    #[test]
    fn mensagens_das_colunas_do_cursor() {
        let text = "c.SQL \"SELECT A.B, C FROM T\";";
        let tree = crate::parse(text).unwrap();
        let message = |kind, piece: &str, label: &str| {
            let at = text.find(piece).unwrap() as u32;
            let check = Check {
                kind,
                range: text_size::TextRange::at(at.into(), (piece.len() as u32).into()),
                name: None,
                label: Some(label.to_owned()),
            };
            describe_check(&check, &tree, text)
        };
        assert_eq!(
            message(CheckKind::CursorUnknownTable, "A", "A"),
            (
                "SEN074",
                "O SELECT do cursor lê A como tabela, e ela não está no dicionário de dados \
                 (RowSet inexistente: A). As colunas do cursor não aceitam apelido de tabela nem \
                 função."
                    .to_owned()
            )
        );
        assert_eq!(
            message(CheckKind::CursorUnknownField, "A.B", "A.B"),
            (
                "SEN075",
                "O campo B não existe na tabela A (Campo \"B\" inexistente na tabela/view \"A\")."
                    .to_owned()
            )
        );
        assert_eq!(
            message(CheckKind::CursorColumnUnused, "A.B", "c"),
            (
                "SEN076",
                "A regra não lê o campo A.B que o SELECT do cursor c busca.".to_owned()
            )
        );
        assert_eq!(
            message(CheckKind::CursorStarUnused, "A", "C|T|35|X, Y").1,
            "O * busca os 35 campos de T para o cursor C, e a regra só lê X, Y."
        );
        assert_eq!(
            message(CheckKind::CursorStarUnused, "A", "C|T|35|").1,
            "O * busca os 35 campos de T para o cursor C, e a regra não lê nenhum."
        );
        assert_eq!(
            message(CheckKind::CursorUnknownField, "C", "T.C").1,
            "O campo C não existe na tabela T (Campo \"C\" inexistente na tabela/view \"T\"). A \
             coluna sem tabela é procurada na primeira tabela do FROM."
        );
    }

    #[test]
    fn erro_de_lexer_e_de_parser() {
        let text = "x = 1;\ny = #;";
        assert_eq!(
            diags(text, ColumnEncoding::Utf16),
            [
                ("SEN001".to_owned(), (1, 4), (1, 5)),
                ("SEN021".to_owned(), (1, 4), (1, 5)),
            ]
        );
    }

    #[test]
    fn coluna_depois_de_acento_e_emoji() {
        // `ç` e o emoji estão numa string, e o erro é o `;` que falta no fim.
        let text = "x = \"ção😀\"";
        let utf16 = diags(text, ColumnEncoding::Utf16);
        let utf8 = diags(text, ColumnEncoding::Utf8);
        assert_eq!(utf16[0].0, "SEN020");
        // 4 + aspas + ç ã o (3) + emoji (2) + aspas = 11 em UTF-16.
        assert_eq!(utf16[0].1, (0, 11));
        // Em bytes são 4 + 1 + 2 + 2 + 1 + 4 + 1 = 15.
        assert_eq!(utf8[0].1, (0, 15));
    }

    #[test]
    fn regra_sem_erro_nao_tem_diagnostico() {
        let text = "definir alfa a;\nse (a = \"x\") { a = \"y\"; }";
        assert!(diags(text, ColumnEncoding::Utf16).is_empty());
    }

    #[test]
    fn aviso_do_validador_e_warning() {
        let text = "definir funcao f();\nfuncao f(); { x = 1; }\nx = 2 e -3;";
        // A função do exemplo não é chamada, e a dica disso não é o assunto aqui.
        let d: Vec<_> = run(text, ColumnEncoding::Utf16)
            .into_iter()
            .filter(|d| d.code != Some(NumberOrString::String("SEN070".to_owned())))
            .collect();
        let resumo: Vec<_> = d
            .iter()
            .map(|d| {
                (
                    d.code.clone().unwrap(),
                    d.severity.unwrap(),
                    d.range.start.line,
                )
            })
            .collect();
        let code = |c: &str| NumberOrString::String(c.to_owned());
        assert_eq!(
            resumo,
            [
                (code("SEN061"), DiagnosticSeverity::WARNING, 2),
                (code("SEN060"), DiagnosticSeverity::WARNING, 2),
            ]
        );
        assert!(d[0].message.contains("função f."));
    }

    #[test]
    fn codigo_inalcancavel_e_aviso_esmaecido() {
        let text = "Pare;\nx = 1;\ny = 2;\nCancel(1);";
        let d = run(text, ColumnEncoding::Utf16);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, Some(NumberOrString::String("SEN071".to_owned())));
        assert_eq!(d[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(d[0].message, "Código depois de Pare não é executado.");
        assert_eq!(d[0].tags, Some(vec![lsp_types::DiagnosticTag::UNNECESSARY]));
        // Do começo da primeira instrução ao fim da última antes do fim do bloco.
        assert_eq!(
            (
                d[0].range.start.line,
                d[0].range.end.line,
                d[0].range.end.character
            ),
            (1, 3, 10)
        );
    }

    #[test]
    fn mensagem_do_caractere_invalido_mostra_o_caractere() {
        let d = run("x = 1; #", ColumnEncoding::Utf16);
        assert_eq!(d[0].message, "O caractere \"#\" é inválido.");
    }

    #[test]
    fn diretiva_esconde_o_aviso_mas_nao_o_erro_de_sintaxe() {
        let text = "Pare;\n@ senior-lsp-disable-next-line SEN071, SEN020 @\nx = 1;\ny = ;";
        let d = diags(text, ColumnEncoding::Utf16);
        let codes: Vec<_> = d.iter().map(|(c, _, _)| c.as_str()).collect();
        // O SEN071 some, o SEN020 da diretiva é recusado e o erro de sintaxe da linha 3 continua.
        assert_eq!(codes, ["SEN021", "SEN072"]);
    }

    #[test]
    fn configuracao_troca_o_nivel_e_tira_o_codigo() {
        let uri: lsp_types::Uri = "file:///regras/R.LSP/Rule.txt".parse().unwrap();
        let mut documents = Documents::default();
        let text = "Pare;\nx = 1;\nx = 2 e -3;";
        documents
            .set(uri.clone(), text.to_owned(), 1, Context::Rule)
            .unwrap();
        let doc = documents.get(&uri).unwrap();
        let mut levels = std::collections::HashMap::new();
        levels.insert("SEN071".to_owned(), super::super::config::Level::Off);
        levels.insert("SEN060".to_owned(), super::super::config::Level::Error);
        let levels = Levels::from_map(levels);
        let d = diagnostics(
            doc,
            Vec::new(),
            &levels,
            &lsp_range(doc, ColumnEncoding::Utf16),
        );
        let found: Vec<_> = d
            .iter()
            .map(|d| (d.code.clone().unwrap(), d.severity.unwrap()))
            .collect();
        assert_eq!(
            found,
            [(
                NumberOrString::String("SEN060".to_owned()),
                DiagnosticSeverity::ERROR
            )]
        );
    }
}
