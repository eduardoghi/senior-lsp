//! Erros do lexer e do parser e verificações do validador como diagnósticos do protocolo.
//!
//! Cada tipo de erro tem um código estável (`SEN001`...) e uma mensagem. Quando o texto do
//! compilador da Senior foi conferido para o mesmo caso, ele aparece entre parênteses, para quem
//! conhece o erro do ERP reconhecer. Todos os erros de hoje foram conferidos no compilador e
//! nenhum aparece nas regras que compilam. Um tipo novo sem essa conferência começa desligado em
//! [`describe_parse`].

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};
use text_size::TextRange;

use crate::line_index::{ColumnEncoding, LineIndex};
use crate::symbols::Symbols;
use crate::validate::{Check, CheckKind, validate};
use crate::{LexErrorKind, ParseErrorKind, SyntaxKind, Tree};

pub(crate) fn diagnostics(
    text: &str,
    tree: &Tree,
    symbols: &Symbols,
    index: &LineIndex,
    encoding: ColumnEncoding,
) -> Vec<Diagnostic> {
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
    let checks = validate(tree, symbols, text).into_iter().map(|c| {
        let severity = if c.kind.is_error() {
            DiagnosticSeverity::ERROR
        } else {
            DiagnosticSeverity::WARNING
        };
        (c.range, describe_check(&c, tree, text), severity)
    });
    errors
        .chain(checks)
        .map(|(range, (code, message), severity)| Diagnostic {
            range: to_lsp(text, index, encoding, range),
            severity: Some(severity),
            code: Some(NumberOrString::String(code.to_owned())),
            source: Some("senior-lsp".to_owned()),
            message,
            ..Diagnostic::default()
        })
        .collect()
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
    let name = check.name.map_or("", |name| &text[tree.text_range(name)]);
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

    /// Código, início e fim de cada diagnóstico, com linha e coluna.
    type Resumo = (String, (u32, u32), (u32, u32));

    fn diags(text: &str, encoding: ColumnEncoding) -> Vec<Resumo> {
        let tree = crate::parse(text).unwrap();
        diagnostics(
            text,
            &tree,
            &Symbols::build(&tree, text),
            &LineIndex::new(text),
            encoding,
        )
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
        let tree = crate::parse(text).unwrap();
        let d = diagnostics(
            text,
            &tree,
            &Symbols::build(&tree, text),
            &LineIndex::new(text),
            ColumnEncoding::Utf16,
        );
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
    fn mensagem_do_caractere_invalido_mostra_o_caractere() {
        let text = "x = 1; #";
        let tree = crate::parse(text).unwrap();
        let d = diagnostics(
            text,
            &tree,
            &Symbols::build(&tree, text),
            &LineIndex::new(text),
            ColumnEncoding::Utf16,
        );
        assert_eq!(d[0].message, "O caractere \"#\" é inválido.");
    }
}
