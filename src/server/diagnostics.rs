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
use crate::validate::{Check, CheckKind, Sql2Error};
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
    let mut found = found(doc, levels);
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
                tags: matches!(
                    f.code,
                    "SEN068" | "SEN070" | "SEN071" | "SEN073" | "SEN076" | "SEN091"
                )
                .then(|| vec![lsp_types::DiagnosticTag::UNNECESSARY]),
                ..Diagnostic::default()
            })
        })
        .collect()
}

/// Erros do lexer e do parser e avisos do validador, antes das diretivas. A tabela que o
/// `senior-lsp.json` diz existir no banco não sai no SEN086.
fn found(doc: &Document, levels: &Levels) -> Vec<Found> {
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
    let extra_table = |c: &&Check| {
        c.kind == CheckKind::NativeUnknownTable
            && c.label.as_deref().is_some_and(|t| levels.extra_table(t))
    };
    let checks = doc.checks().iter().filter(|c| !extra_table(c)).map(|c| {
        let severity = severity_of(c.kind);
        let (code, mut message) = describe_check(c, tree, text);
        if let Some((_, name)) = super::suggest::suggestion(c, doc) {
            message.push_str(&format!(" Você quis dizer {name}?"));
        }
        (c.range, (code, message), severity)
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

/// Nível do aviso do validador, antes do `senior-lsp.json`.
fn severity_of(kind: CheckKind) -> DiagnosticSeverity {
    if kind.is_error() {
        DiagnosticSeverity::ERROR
    } else if matches!(
        kind,
        CheckKind::Unused
            | CheckKind::UnusedFunction
            | CheckKind::UnusedParameter
            | CheckKind::CursorColumnUnused
            | CheckKind::CursorStarUnused
            | CheckKind::RepeatedParameter
            | CheckKind::RepeatedColumn
            | CheckKind::RepeatedLabel
            | CheckKind::LabelsSameStart
            | CheckKind::TableSizeOverflow
    ) {
        DiagnosticSeverity::HINT
    } else {
        DiagnosticSeverity::WARNING
    }
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
        CheckKind::NativeUnknownTable => (
            "SEN086",
            format!(
                "A tabela {name} não está no dicionário de dados. O SQL nativo vai para o banco \
                 como está, e a execução para se a tabela também não existir lá. Uma tabela \
                 criada direto no banco vai no extraTables do senior-lsp.json."
            ),
        ),
        CheckKind::NativeUnknownField => {
            let (table, field) = name.split_once('.').unwrap_or(("", name));
            (
                "SEN087",
                format!(
                    "O campo {field} não existe na tabela {table} no dicionário de dados. O SQL \
                     nativo vai para o banco como está, e a execução para se o campo também não \
                     existir lá."
                ),
            )
        }
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
                "A tabela {name} não está no dicionário de dados, e o Senior SQL 2 só compila \
                 o comando com a tabela existindo (\"{name}\" inexistente na base de dados)."
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
        CheckKind::Sql2InsertValues => {
            let mut parts = name.splitn(3, '|');
            let mut part = || parts.next().unwrap_or("");
            let (columns, values, table) = (part(), part(), part());
            let count = |text: &str| text.parse::<usize>().unwrap_or(0);
            let quantity = if count(values) < count(columns) {
                "menos"
            } else {
                "mais"
            };
            let has = if table.is_empty() {
                format!("O INSERT tem {columns} campos e {values} valores")
            } else {
                format!(
                    "O INSERT sem lista de campos precisa dos {columns} campos de {table} e tem \
                     {values} valores"
                )
            };
            (
                "SEN084",
                format!(
                    "{has}, e a regra usa o Senior SQL 2 (Cláusula VALUES tem {quantity} valores \
                     do que o necessário)."
                ),
            )
        }
        CheckKind::CommandSql2(error) => {
            // O texto entre parênteses é o da mensagem que o ERP mostra na execução.
            let (explanation, erp) = match error {
                Sql2Error::Join => (
                    "O Senior SQL 2 não aceita JOIN. Junte as tabelas pela vírgula no FROM e \
                     pela condição no WHERE."
                        .to_owned(),
                    format!("Era esperado <fim-de-arquivo>, mas {name} foi encontrado"),
                ),
                Sql2Error::UnknownTable => (
                    format!("A tabela {name} não está no dicionário de dados."),
                    format!("Tabela ou view \"{name}\" inexistente na base de dados"),
                ),
                Sql2Error::UnknownSymbol => (
                    format!("O Senior SQL 2 não acha {name} nas tabelas do FROM."),
                    format!("Símbolo não encontrado no comando: \"{name}\""),
                ),
                Sql2Error::UnknownField => {
                    let (table, field) = name.split_once('.').unwrap_or(("", name));
                    (
                        format!("O campo {field} não existe na tabela {table}."),
                        format!("Campo não encontrado na tabela \"{table}\": \"{field}\""),
                    )
                }
                Sql2Error::AmbiguousField => (
                    format!(
                        "Mais de uma tabela do FROM tem o campo {name}, e o Senior SQL 2 exige a \
                         tabela na frente."
                    ),
                    format!(
                        "Ambiguidade detectada: há mais de uma tabela que possui o campo \
                         \"{name}\""
                    ),
                ),
                Sql2Error::OrderBy => (
                    "O Senior SQL 2 só aceita no ORDER BY o que está no SELECT.".to_owned(),
                    format!(
                        "A expressão \"{name}\" não aparece na lista de expressões selecionadas \
                         e por isso não pode ser utilizado no Order By"
                    ),
                ),
                Sql2Error::InsertValues => {
                    let mut parts = name.splitn(3, '|').map(|p| p.parse::<usize>().unwrap_or(0));
                    let (columns, values) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
                    let quantity = if values < columns { "menos" } else { "mais" };
                    (
                        format!("O INSERT tem {values} valores para {columns} campos."),
                        format!("Cláusula VALUES tem {quantity} valores do que o necessário"),
                    )
                }
            };
            (
                "SEN088",
                format!(
                    "{explanation} O SQL_DefinirComando compila, e a execução para com o Senior \
                     SQL 2 ({erp})."
                ),
            )
        }
        CheckKind::LoopClauseOutsideLoop => (
            "SEN089",
            format!(
                "{name} só pode ser usado dentro de um para ou enquanto (Esta cláusula só pode ser \
                 usada com o comando \"Para\" ou \"Enquanto\")."
            ),
        ),
        CheckKind::ContinueSkipsStep => (
            "SEN090",
            format!(
                "O Continue volta ao começo do para sem rodar o incremento ({name}). Se nada mais \
                 mudar o contador, o laço não termina."
            ),
        ),
        CheckKind::TableWithoutSize => (
            "SEN092",
            format!(
                "A tabela {name} precisa do tamanho entre colchetes depois do nome, como em \
                 Definir Tabela {name}[10] (Definição inválida: faltou \"]\")."
            ),
        ),
        CheckKind::TableWithoutColumns => (
            "SEN092",
            format!(
                "A tabela {name} não tem nenhuma coluna entre as chaves (Definição inválida: tipo \
                 de variável inválido: FIM)."
            ),
        ),
        CheckKind::TableWithoutIndex => {
            let (table, column) = name.split_once('|').unwrap_or((name, ""));
            (
                "SEN093",
                format!(
                    "A coluna {column} precisa do índice da linha, como em {table}[1].{column} \
                     (Erro na variável indexada, falta \"[\")."
                ),
            )
        }
        CheckKind::TableColumnWithoutIndex => {
            let (table, column) = name.split_once('|').unwrap_or((name, ""));
            (
                "SEN093",
                format!(
                    "A coluna {column} da tabela {table} tem tamanho na definição, e precisa do \
                     segundo índice, como em {table}[1].{column}[1] (Erro na variável indexada, \
                     falta \"[\")."
                ),
            )
        }
        CheckKind::TableUnknownColumn => {
            let (table, column) = name.split_once('|').unwrap_or((name, ""));
            (
                "SEN093",
                format!(
                    "A tabela {table} não tem a coluna {column} (Erro de sintaxe, variável da \
                     tabela não está definida)."
                ),
            )
        }
        CheckKind::IndexOutOfRange => {
            let mut parts = name.splitn(4, '|');
            let mut part = || parts.next().unwrap_or("");
            let (index, what, owner, limit) = (part(), part(), part(), part());
            let range = match (what, limit) {
                ("coluna", _) => format!(
                    "O segundo índice de {owner} vai de 1 até o número de linhas da tabela, que \
                     é {limit}"
                ),
                (_, "0") => format!("{owner} tem tamanho 0, então todo índice literal é recusado"),
                _ => format!("{owner} vai de 1 a {limit}"),
            };
            // O menos de `t[-1]` não chega ao índice, e o ERP recusa por outro motivo.
            let erp = if index.starts_with('-') {
                "Erro de sintaxe, tipos incompatíveis: NUMERO e ALFA".to_owned()
            } else {
                format!("Erro indexando fora dos limites: {index}")
            };
            (
                "SEN094",
                format!("O índice {index} está fora dos limites. {range} ({erp})."),
            )
        }
        CheckKind::CursorCommandArgument => (
            "SEN095",
            format!("O {name} do cursor não recebe valor (Erro na pontuação, falta \")\")."),
        ),
        CheckKind::CursorFlagCalled => (
            "SEN095",
            format!(
                "{name} é uma propriedade do cursor e não leva parênteses. O compilador recusa \
                 com \"( não é permitido aqui\"."
            ),
        ),
        CheckKind::CursorScopeWithoutArgument => (
            "SEN095",
            "UsaAbrangencia precisa do valor, como em UsaAbrangencia(0) (parâmetro esperado)."
                .to_owned(),
        ),
        CheckKind::CursorTranslationWithoutSql2 => (
            "SEN095",
            "UsaTraducao só existe no Senior SQL 2, e esta regra usa o SQL nativo (Só é permitido \
             usar a função \"USATRADUCAO\" na linguagem SQL Senior 2)."
                .to_owned(),
        ),
        CheckKind::CursorMemberAssigned if name.eq_ignore_ascii_case("SQL") => (
            "SEN095",
            "O SQL do cursor se escreve sem =, como em cur.SQL \"SELECT ...\" (Erro na variável \
             de registro, falta \"comando SQL\")."
                .to_owned(),
        ),
        CheckKind::CursorMemberAssigned => (
            "SEN095",
            format!(
                "O cursor só se lê, então {name} não recebe valor (não é possível atribuir \
                 valores a campos de cursores)."
            ),
        ),
        CheckKind::ListNotMember => {
            let (list, member) = name.split_once('|').unwrap_or((name, ""));
            (
                "SEN096",
                format!(
                    "{member} não é método, função nem propriedade da lista, e nenhum \
                     AdicionarCampo cria um campo com esse nome em {list} (falta comando ou \
                     campo)."
                ),
            )
        }
        CheckKind::ListFunctionWithoutParentheses => (
            "SEN096",
            format!("{name} é função da lista e precisa dos parênteses (falta \"(\")."),
        ),
        CheckKind::ListPropertyWithParentheses => (
            "SEN096",
            format!("{name} é propriedade da lista e não leva parênteses (falta \";\")."),
        ),
        CheckKind::ListFieldWithoutName => (
            "SEN096",
            "O AdicionarCampo precisa do nome do campo em texto (falta nome do campo).".to_owned(),
        ),
        CheckKind::ListSizeOnNonAlfa => (
            "SEN096",
            "O tamanho só vale para campo alfa (tamanho do campo somente pode ser definido para \
             campos alfanuméricos)."
                .to_owned(),
        ),
        CheckKind::ListRepeatedField => {
            let (list, field) = name.split_once('|').unwrap_or((name, ""));
            (
                "SEN096",
                format!("A lista já tem o campo {field} ({list}{field} já existe)."),
            )
        }
        CheckKind::ListKeyWithoutFields => (
            "SEN096",
            "A Chave precisa do texto com os campos (faltam campos).".to_owned(),
        ),
        CheckKind::UnknownLabel => (
            "SEN097",
            format!(
                "Não há rótulo {name} neste arquivo. Rótulo e função ficam em tabelas separadas, \
                 então o nome de uma função também não serve (Falta declaração do \
                 \"Label/Função\" = \"{}\").",
                name.to_uppercase()
            ),
        ),
        CheckKind::UndeclaredFunction => (
            "SEN098",
            format!(
                "A funcao {name} precisa de um Definir Funcao com o mesmo nome antes dela no \
                 arquivo (Função não declarada anteriormente)."
            ),
        ),
        CheckKind::FunctionNameTooLong => {
            let (function, length) = name.split_once('|').unwrap_or((name, ""));
            (
                "SEN099",
                format!(
                    "O nome {function} tem {length} caracteres, e o compilador corta em 31 e não \
                     reconhece a chamada (Erro na pontuação, falta \"=\")."
                ),
            )
        }
        CheckKind::AlfaConcatenationInArgument => (
            "SEN100",
            "O argumento de função não aceita concatenação de alfa. Monte o texto numa variável \
             antes da chamada (Variáveis alfanuméricas ou operadores não compatíveis)."
                .to_owned(),
        ),
        CheckKind::ConflictingDefinition => {
            let (variable, before) = name.split_once('|').unwrap_or((name, ""));
            (
                "SEN101",
                format!(
                    "{variable} já foi definida como {before}, e o mesmo nome com outro tipo não \
                     compila (Definição inválida: variável \"{}\" já está definida como \
                     {before}).",
                    variable.to_uppercase()
                ),
            )
        }
        CheckKind::CallWithoutImplementation => (
            "SEN102",
            format!(
                "A função {name} está declarada e nenhuma funcao a implementa neste arquivo. \
                 Compila, mas esta chamada para na execução com \"Instrução inválida ao \
                 Executar a regra.\""
            ),
        ),
        CheckKind::CallImplementedLater => (
            "SEN102",
            format!(
                "A função {name} só é implementada numa regra que vem depois desta na tela. \
                 Compila, mas esta chamada para na execução com \"Não é possível determinar a \
                 regra atual para o interpretador.\""
            ),
        ),
        CheckKind::CallToLongFunctionName => (
            "SEN103",
            format!(
                "O nome {name} tem 31 caracteres. Compila, mas o ERP não resolve a chamada, e a \
                 execução para com \"Instrução inválida ao Executar a regra.\""
            ),
        ),
        CheckKind::LiteralInAlfaParameter => (
            "SEN104",
            format!(
                "Texto literal no parâmetro {name}, que é alfa, de uma função da regra. Compila, \
                 mas a execução para com \"Instrução inválida ao Executar a regra.\" Passe uma \
                 variável."
            ),
        ),
        CheckKind::AlfaTableIndex => (
            "SEN105",
            format!(
                "Índice alfa em {name}. O ERP lê o índice como 0, mesmo com o texto \"1\", e a \
                 execução para com \"Indexando fora dos limites (0)\"."
            ),
        ),
        CheckKind::ColumnIndexBeyondSize => {
            let mut parts = name.splitn(3, '|');
            let mut part = || parts.next().unwrap_or("");
            let (index, column, size) = (part(), part(), part());
            (
                "SEN106",
                format!(
                    "A coluna {column} tem tamanho {size}, e o índice {index} passa dele. Compila \
                     e não dá erro, mas grava na linha seguinte da tabela."
                ),
            )
        }
        CheckKind::RepeatedDeclaration => (
            "SEN107",
            format!(
                "Definir Funcao {name} repetido com outra assinatura. Só a primeira declaração \
                 vale, e esta é ignorada. As chamadas são conferidas contra a primeira."
            ),
        ),
        CheckKind::RepeatedParameter => (
            "SEN108",
            format!(
                "O parâmetro {name} aparece mais de uma vez na função. O corpo só enxerga o \
                 último argumento."
            ),
        ),
        CheckKind::RepeatedColumn => (
            "SEN109",
            format!(
                "A coluna {name} já existe na tabela. O nome acessa só a primeira, e o tipo \
                 também é o da primeira."
            ),
        ),
        CheckKind::RepeatedLabel => (
            "SEN110",
            format!("O rótulo {name} já existe neste arquivo, e o VaPara vai para o último."),
        ),
        CheckKind::LabelsSameStart => {
            let (label, other) = name.split_once('|').unwrap_or((name, ""));
            (
                "SEN111",
                format!(
                    "Os rótulos {label} e {other} têm os mesmos 30 primeiros caracteres, e o ERP \
                     trata os dois como o mesmo rótulo."
                ),
            )
        }
        CheckKind::TableSizeOverflow => {
            let (size, real) = name.split_once('|').unwrap_or((name, ""));
            (
                "SEN112",
                format!(
                    "O tamanho {size} passa de 65535, e o ERP o guarda em 16 bits. A tabela fica \
                     com {real} linhas."
                ),
            )
        }
        CheckKind::UnusedParameter => (
            "SEN091",
            format!("O parâmetro {name} não é usado no corpo da função."),
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
        CheckKind::EndlessLoop => (
            "SEN085",
            "O enquanto com a condição vazia repete para sempre, e o bloco não tem Pare, VaPara, \
             Cancel nem outra saída. A regra trava quando chega aqui."
                .to_owned(),
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
            message(CheckKind::CommandSql2(Sql2Error::OrderBy), "A", "NOMEMP").1,
            "O Senior SQL 2 só aceita no ORDER BY o que está no SELECT. O SQL_DefinirComando \
             compila, e a execução para com o Senior SQL 2 (A expressão \"NOMEMP\" não aparece \
             na lista de expressões selecionadas e por isso não pode ser utilizado no Order By)."
        );
        assert_eq!(
            message(CheckKind::Sql2InsertValues, "A", "2|1").1,
            "O INSERT tem 2 campos e 1 valores, e a regra usa o Senior SQL 2 (Cláusula VALUES \
             tem menos valores do que o necessário)."
        );
        assert_eq!(
            message(CheckKind::Sql2InsertValues, "A", "1|2").1,
            "O INSERT tem 1 campos e 2 valores, e a regra usa o Senior SQL 2 (Cláusula VALUES \
             tem mais valores do que o necessário)."
        );
        assert_eq!(
            message(CheckKind::Sql2InsertValues, "T", "35|1|T").1,
            "O INSERT sem lista de campos precisa dos 35 campos de T e tem 1 valores, e a regra \
             usa o Senior SQL 2 (Cláusula VALUES tem menos valores do que o necessário)."
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
    fn nivel_e_mensagem_das_conferencias_de_tabela_cursor_lista_e_funcao() {
        const ERROR: DiagnosticSeverity = DiagnosticSeverity::ERROR;
        const WARNING: DiagnosticSeverity = DiagnosticSeverity::WARNING;
        const HINT: DiagnosticSeverity = DiagnosticSeverity::HINT;
        let tabela = "Definir Tabela t[3] = { Numero a; Numero v[2]; };\nDefinir Numero n;\n";
        let funcao = "Definir Funcao f(Alfa p);\n";
        let impl_ = "\nFuncao f(Alfa p);\nInicio\nn = 1;\nFim;";
        // Um trecho por código, com o código e o nível esperados. Os trechos de função põem a
        // `funcao` no fim, que o corpo dela vai até o fim do arquivo.
        let casos: Vec<(std::string::String, &str, DiagnosticSeverity)> = vec![
            ("Definir Tabela t = { Numero a; };".into(), "SEN092", ERROR),
            ("Definir Tabela t[3] = { };".into(), "SEN092", ERROR),
            (format!("{tabela}n = t.a;"), "SEN093", ERROR),
            (format!("{tabela}n = t[1].v;"), "SEN093", ERROR),
            (format!("{tabela}n = t[1].zz;"), "SEN093", ERROR),
            (format!("{tabela}n = t[4].a;"), "SEN094", ERROR),
            ("Definir Cursor c;\nc.AbrirCursor(1);".into(), "SEN095", ERROR),
            ("Definir Lista l;\nl.Foo();".into(), "SEN096", ERROR),
            ("se (1 = 1) { VaPara sai; }".into(), "SEN097", ERROR),
            ("Funcao f();\nInicio\nFim;".into(), "SEN098", ERROR),
            (
                "Definir Funcao fAbcdefghijklmnopqrstuvwxyz12345();\nfAbcdefghijklmnopqrstuvwxyz12345();"
                    .into(),
                "SEN099",
                ERROR,
            ),
            (
                format!("Definir Alfa a;\n{funcao}f(\"x\" + a);{impl_}"),
                "SEN100",
                ERROR,
            ),
            ("Definir Numero n;\nDefinir Alfa n;\nn = 1;".into(), "SEN101", ERROR),
            ("Definir Funcao f();\nf();".into(), "SEN102", WARNING),
            (
                "Definir Funcao fAbcdefghijklmnopqrstuvwxyz1234();\nfAbcdefghijklmnopqrstuvwxyz1234();"
                    .into(),
                "SEN103",
                WARNING,
            ),
            (format!("{funcao}f(\"a\");{impl_}"), "SEN104", WARNING),
            (
                format!("{tabela}Definir Alfa aTxt;\nn = t[aTxt].a;"),
                "SEN105",
                WARNING,
            ),
            (format!("{tabela}t[1].v[3] = 5;"), "SEN106", WARNING),
            (
                "Definir Funcao f(Numero a);\nDefinir Funcao f(Numero a, Numero b);".into(),
                "SEN107",
                WARNING,
            ),
            (
                "Definir Funcao f(Numero a, Numero a);\nf(1, 2);\nFuncao f(Numero a, Numero a);\nInicio\na = 1;\nFim;"
                    .into(),
                "SEN108",
                HINT,
            ),
            ("Definir Tabela t[2] = { Numero a; Numero a; };".into(), "SEN109", HINT),
            ("x = 1;\nr:\nx = 2;\nr:\nx = 3;".into(), "SEN110", HINT),
        ];
        for (text, code, severity) in casos {
            let d: Vec<_> = run(&text, ColumnEncoding::Utf16)
                .into_iter()
                .filter(|d| d.code == Some(NumberOrString::String(code.to_owned())))
                .collect();
            assert!(!d.is_empty(), "{code} em {text}");
            assert_eq!(d[0].severity, Some(severity), "{code} em {text}");
            assert_eq!(d[0].tags, None, "{code} em {text}");
        }
        let hints = |text: &str, code: &str| {
            run(text, ColumnEncoding::Utf16)
                .into_iter()
                .filter(|d| d.code == Some(NumberOrString::String(code.to_owned())))
                .map(|d| (d.severity.unwrap(), d.message))
                .collect::<Vec<_>>()
        };
        let rotulos = "x = 1;\nrotuloComTrintaCaracteresAbcdeA:\nx = 2;\nrotuloComTrintaCaracteresAbcdeB:\nx = 3;";
        assert_eq!(hints(rotulos, "SEN111")[0].0, HINT);
        let grande = hints("Definir Tabela t[70000] = { Numero a; };", "SEN112");
        assert_eq!(grande[0].0, HINT);
        assert_eq!(
            grande[0].1,
            "O tamanho 70000 passa de 65535, e o ERP o guarda em 16 bits. A tabela fica com 4464 \
             linhas."
        );
        assert_eq!(
            hints(&format!("{tabela}n = t[0].a;"), "SEN094")[0].1,
            "O índice 0 está fora dos limites. t vai de 1 a 3 (Erro indexando fora dos limites: 0)."
        );
        assert_eq!(
            hints(&format!("{tabela}t[1].v[4] = 1;"), "SEN094")[0].1,
            "O índice 4 está fora dos limites. O segundo índice de t.v vai de 1 até o número de \
             linhas da tabela, que é 3 (Erro indexando fora dos limites: 4)."
        );
        assert_eq!(
            hints(
                "Definir Tabela t[0] = { Numero a; };\nDefinir Numero n;\nn = t[1].a;",
                "SEN094"
            )[0]
            .1,
            "O índice 1 está fora dos limites. t tem tamanho 0, então todo índice literal é \
             recusado (Erro indexando fora dos limites: 1)."
        );
        assert!(
            hints("Definir Cursor c;\nc.SQL = \"SELECT A FROM T\";", "SEN095")[0]
                .1
                .starts_with("O SQL do cursor se escreve sem =")
        );
        assert_eq!(
            hints("Definir Numero n;\nDefinir Alfa N;", "SEN101")[0].1,
            "N já foi definida como número, e o mesmo nome com outro tipo não compila (Definição \
             inválida: variável \"N\" já está definida como número)."
        );
    }

    /// Todos os tipos de aviso. O `match` sem curinga não compila quando aparece um tipo novo, e o
    /// teste dos códigos passa a cobrir ele também.
    fn all_kinds() -> Vec<CheckKind> {
        use crate::validate::Sql2Error;
        let kinds = vec![
            CheckKind::MinusTakesOneToken,
            CheckKind::ComparisonInAssignment,
            CheckKind::LogicalMinus,
            CheckKind::CodeAfterFunctionBlock,
            CheckKind::FunctionInsideBlock,
            CheckKind::ForeignParameter,
            CheckKind::NonAlfaMessageText,
            CheckKind::AlfaAndNumber,
            CheckKind::AlfaOperator,
            CheckKind::NumberAndAlfa,
            CheckKind::TwoStringLiterals,
            CheckKind::AlfaArgument,
            CheckKind::NumberArgument,
            CheckKind::ConstantInEndParameter,
            CheckKind::AlfaOperatorInComparison,
            CheckKind::LowercaseChar,
            CheckKind::UnknownFunction,
            CheckKind::TooManyArguments,
            CheckKind::TooFewArguments,
            CheckKind::OnlyInTest,
            CheckKind::CursorConstantColumn,
            CheckKind::NativeSqlWithScope,
            CheckKind::NativeUnknownTable,
            CheckKind::NativeUnknownField,
            CheckKind::CursorFieldNotInSelect,
            CheckKind::CursorUnknownTable,
            CheckKind::CursorUnknownField,
            CheckKind::CursorColumnWithoutTable,
            CheckKind::CursorRepeatedField,
            CheckKind::CursorUnknownParameter,
            CheckKind::Sql2Join,
            CheckKind::Sql2UnknownTable,
            CheckKind::Sql2UnknownSymbol,
            CheckKind::Sql2UnknownField,
            CheckKind::Sql2AmbiguousField,
            CheckKind::Sql2OrderBy,
            CheckKind::Sql2InsertValues,
            CheckKind::ReturnedColumnNotInSelect,
            CheckKind::Unused,
            CheckKind::UnusedFunction,
            CheckKind::CursorColumnUnused,
            CheckKind::CursorStarUnused,
            CheckKind::Unreachable,
            CheckKind::EndlessLoop,
            CheckKind::LoopClauseOutsideLoop,
            CheckKind::ContinueSkipsStep,
            CheckKind::UnusedParameter,
            CheckKind::TableWithoutSize,
            CheckKind::TableWithoutColumns,
            CheckKind::TableWithoutIndex,
            CheckKind::TableColumnWithoutIndex,
            CheckKind::TableUnknownColumn,
            CheckKind::IndexOutOfRange,
            CheckKind::CursorCommandArgument,
            CheckKind::CursorFlagCalled,
            CheckKind::CursorScopeWithoutArgument,
            CheckKind::CursorTranslationWithoutSql2,
            CheckKind::CursorMemberAssigned,
            CheckKind::ListNotMember,
            CheckKind::ListFunctionWithoutParentheses,
            CheckKind::ListPropertyWithParentheses,
            CheckKind::ListFieldWithoutName,
            CheckKind::ListSizeOnNonAlfa,
            CheckKind::ListRepeatedField,
            CheckKind::ListKeyWithoutFields,
            CheckKind::UnknownLabel,
            CheckKind::UndeclaredFunction,
            CheckKind::FunctionNameTooLong,
            CheckKind::AlfaConcatenationInArgument,
            CheckKind::ConflictingDefinition,
            CheckKind::CallWithoutImplementation,
            CheckKind::CallImplementedLater,
            CheckKind::CallToLongFunctionName,
            CheckKind::LiteralInAlfaParameter,
            CheckKind::AlfaTableIndex,
            CheckKind::ColumnIndexBeyondSize,
            CheckKind::RepeatedDeclaration,
            CheckKind::RepeatedParameter,
            CheckKind::RepeatedColumn,
            CheckKind::RepeatedLabel,
            CheckKind::LabelsSameStart,
            CheckKind::TableSizeOverflow,
            CheckKind::CommandSql2(Sql2Error::Join),
            CheckKind::CommandSql2(Sql2Error::UnknownTable),
            CheckKind::CommandSql2(Sql2Error::UnknownSymbol),
            CheckKind::CommandSql2(Sql2Error::UnknownField),
            CheckKind::CommandSql2(Sql2Error::AmbiguousField),
            CheckKind::CommandSql2(Sql2Error::OrderBy),
            CheckKind::CommandSql2(Sql2Error::InsertValues),
        ];
        for kind in &kinds {
            match kind {
                CheckKind::MinusTakesOneToken
                | CheckKind::ComparisonInAssignment
                | CheckKind::LogicalMinus
                | CheckKind::CodeAfterFunctionBlock
                | CheckKind::FunctionInsideBlock
                | CheckKind::ForeignParameter
                | CheckKind::NonAlfaMessageText
                | CheckKind::AlfaAndNumber
                | CheckKind::AlfaOperator
                | CheckKind::NumberAndAlfa
                | CheckKind::TwoStringLiterals
                | CheckKind::AlfaArgument
                | CheckKind::NumberArgument
                | CheckKind::ConstantInEndParameter
                | CheckKind::AlfaOperatorInComparison
                | CheckKind::LowercaseChar
                | CheckKind::UnknownFunction
                | CheckKind::TooManyArguments
                | CheckKind::TooFewArguments
                | CheckKind::OnlyInTest
                | CheckKind::CursorConstantColumn
                | CheckKind::NativeSqlWithScope
                | CheckKind::NativeUnknownTable
                | CheckKind::NativeUnknownField
                | CheckKind::CursorFieldNotInSelect
                | CheckKind::CursorUnknownTable
                | CheckKind::CursorUnknownField
                | CheckKind::CursorColumnWithoutTable
                | CheckKind::CursorRepeatedField
                | CheckKind::CursorUnknownParameter
                | CheckKind::Sql2Join
                | CheckKind::Sql2UnknownTable
                | CheckKind::Sql2UnknownSymbol
                | CheckKind::Sql2UnknownField
                | CheckKind::Sql2AmbiguousField
                | CheckKind::Sql2OrderBy
                | CheckKind::Sql2InsertValues
                | CheckKind::ReturnedColumnNotInSelect
                | CheckKind::Unused
                | CheckKind::UnusedFunction
                | CheckKind::CursorColumnUnused
                | CheckKind::CursorStarUnused
                | CheckKind::Unreachable
                | CheckKind::EndlessLoop
                | CheckKind::LoopClauseOutsideLoop
                | CheckKind::ContinueSkipsStep
                | CheckKind::UnusedParameter
                | CheckKind::TableWithoutSize
                | CheckKind::TableWithoutColumns
                | CheckKind::TableWithoutIndex
                | CheckKind::TableColumnWithoutIndex
                | CheckKind::TableUnknownColumn
                | CheckKind::IndexOutOfRange
                | CheckKind::CursorCommandArgument
                | CheckKind::CursorFlagCalled
                | CheckKind::CursorScopeWithoutArgument
                | CheckKind::CursorTranslationWithoutSql2
                | CheckKind::CursorMemberAssigned
                | CheckKind::ListNotMember
                | CheckKind::ListFunctionWithoutParentheses
                | CheckKind::ListPropertyWithParentheses
                | CheckKind::ListFieldWithoutName
                | CheckKind::ListSizeOnNonAlfa
                | CheckKind::ListRepeatedField
                | CheckKind::ListKeyWithoutFields
                | CheckKind::UnknownLabel
                | CheckKind::UndeclaredFunction
                | CheckKind::FunctionNameTooLong
                | CheckKind::AlfaConcatenationInArgument
                | CheckKind::ConflictingDefinition
                | CheckKind::CallWithoutImplementation
                | CheckKind::CallImplementedLater
                | CheckKind::CallToLongFunctionName
                | CheckKind::LiteralInAlfaParameter
                | CheckKind::AlfaTableIndex
                | CheckKind::ColumnIndexBeyondSize
                | CheckKind::RepeatedDeclaration
                | CheckKind::RepeatedParameter
                | CheckKind::RepeatedColumn
                | CheckKind::RepeatedLabel
                | CheckKind::LabelsSameStart
                | CheckKind::TableSizeOverflow
                | CheckKind::CommandSql2(_) => {}
            }
        }
        kinds
    }

    #[test]
    fn todo_aviso_tem_o_codigo_e_o_nivel_da_documentacao() {
        // A tabela de códigos de docs/diagnosticos.md, que é o que o usuário lê.
        let docs = include_str!("../../docs/diagnosticos.md");
        let levels: std::collections::HashMap<&str, &str> = docs
            .lines()
            .filter_map(|line| {
                let mut cells = line.strip_prefix("| SEN")?.split('|');
                let code = line.get(2..8)?;
                cells.next()?;
                Some((code, cells.next()?.trim()))
            })
            .collect();
        let text = "x = 1;";
        let tree = crate::parse(text).unwrap();
        for kind in all_kinds() {
            let check = Check {
                kind,
                range: text_size::TextRange::at(0.into(), 1.into()),
                name: None,
                label: Some("X".to_owned()),
            };
            let (code, message) = describe_check(&check, &tree, text);
            assert!(!message.is_empty(), "{kind:?}");
            let level = levels
                .get(code)
                .unwrap_or_else(|| panic!("{code} de {kind:?} não está na documentação"));
            let expected = match *level {
                "erro" => DiagnosticSeverity::ERROR,
                "aviso" => DiagnosticSeverity::WARNING,
                "dica" => DiagnosticSeverity::HINT,
                other => panic!("nível {other} de {code}"),
            };
            assert_eq!(severity_of(kind), expected, "{code} de {kind:?}");
        }
    }

    #[test]
    fn codigo_inalcancavel_e_aviso_esmaecido() {
        let text = "Cancel;\nx = 1;\ny = 2;\nCancel(1);";
        let d = run(text, ColumnEncoding::Utf16);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, Some(NumberOrString::String("SEN071".to_owned())));
        assert_eq!(d[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(d[0].message, "Código depois de Cancel não é executado.");
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
        let text = "Cancel;\n@ senior-lsp-disable-next-line SEN071, SEN020 @\nx = 1;\ny = ;";
        let d = diags(text, ColumnEncoding::Utf16);
        let codes: Vec<_> = d.iter().map(|(c, _, _)| c.as_str()).collect();
        // O SEN071 some, o SEN020 da diretiva é recusado e o erro de sintaxe da linha 3 continua.
        assert_eq!(codes, ["SEN021", "SEN072"]);
    }

    #[test]
    fn configuracao_troca_o_nivel_e_tira_o_codigo() {
        let uri: lsp_types::Uri = "file:///regras/R.LSP/Rule.txt".parse().unwrap();
        let mut documents = Documents::default();
        let text = "Cancel;\nx = 1;\nx = 2 e -3;";
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
