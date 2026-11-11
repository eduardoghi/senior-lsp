//! Verificações sobre a árvore pronta.
//!
//! O parser aceita um superconjunto da linguagem. Aqui ficam as restrições do compilador que
//! dependem de onde uma construção aparece e os avisos de código que compila mas faz outra coisa
//! do que parece. Cada verificação foi conferida no compilador.

use text_size::TextRange;

use crate::SyntaxKind::{self, *};
use crate::builtins::{self, Context};
use crate::form::Inherited;
use crate::symbols::{DefKind, Symbols, VarType};

mod sql;
mod types;
use crate::{Element, NodeId, Tree};
pub use sql::cursor_columns;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    /// `2 * -f(1)`. Depois de `*`, `/` ou `e`, o `-` pega só o próximo token, e o resto sobra.
    MinusTakesOneToken,
    /// `x = a > 1`. Comparação no lado direito da atribuição só vale entre parênteses.
    ComparisonInAssignment,
    /// `x = 2 e -3` ou `x = 2 ou -3`. Compila, mas o valor não é gravado.
    LogicalMinus,
    /// Comando depois do bloco de uma função, que ainda faz parte dela.
    CodeAfterFunctionBlock,
    /// `funcao` dentro de um bloco de outra função que ficou aberto.
    FunctionInsideBlock,
    /// Parâmetro usado fora da sua função. Compila, mas na execução o valor é lixo.
    ForeignParameter,
    /// Variável numérica ou data no texto do `Mensagem`, que só aceita alfa.
    NonAlfaMessageText,
    /// Número, parênteses ou variável numérica no lado direito de atribuição alfa.
    AlfaAndNumber,
    /// Operador que não é `+` no lado direito de atribuição alfa.
    AlfaOperator,
    /// Alfa numa conta numérica ou comparado com número.
    NumberAndAlfa,
    /// Duas strings comparadas entre si.
    TwoStringLiterals,
    /// Alfa num parâmetro numérico.
    AlfaArgument,
    /// Número num parâmetro alfa.
    NumberArgument,
    /// Constante num parâmetro `end`, que só aceita variável.
    ConstantInEndParameter,
    /// `+` entre alfas dentro de uma comparação.
    AlfaOperatorInComparison,
    /// `'a'` com letra minúscula, que o compilador lê em maiúscula.
    LowercaseChar,
    /// Chamada a um nome que não é função do arquivo nem do ERP. O compilador lê o nome como
    /// variável e espera uma atribuição.
    UnknownFunction,
    TooManyArguments,
    TooFewArguments,
    /// Campo de tabela que só existe no dicionário de teste. Compila na base de teste e não em
    /// produção, como uma customização ainda não promovida.
    OnlyInTest,
    /// Constante entre as colunas do SELECT de um cursor, como `cur.SQL "SELECT 1 FROM T"`.
    CursorConstantColumn,
    /// `SQL_UsarSQLSenior2(x, 0)` sem `SQL_UsarAbrangencia(x, 0)`. Compila, e a execução para.
    NativeSqlWithScope,
    /// `cur.CAMPO` com um campo que não está no SELECT do cursor nem é operação dele.
    CursorFieldNotInSelect,
    /// `SQL_Retornar*(obj, "coluna", ...)` com uma coluna que não está no SELECT. Compila, e a
    /// execução para.
    ReturnedColumnNotInSelect,
    /// Variável, cursor, lista, tabela ou web service definido e nunca usado. Só na regra comum,
    /// porque na tela e no relatório as regras seguintes podem usar o nome.
    Unused,
}

impl CheckKind {
    /// Erro de compilação. As outras verificações são avisos.
    pub fn is_error(self) -> bool {
        matches!(
            self,
            CheckKind::MinusTakesOneToken
                | CheckKind::ComparisonInAssignment
                | CheckKind::NonAlfaMessageText
                | CheckKind::AlfaAndNumber
                | CheckKind::AlfaOperator
                | CheckKind::NumberAndAlfa
                | CheckKind::TwoStringLiterals
                | CheckKind::AlfaArgument
                | CheckKind::NumberArgument
                | CheckKind::ConstantInEndParameter
                | CheckKind::AlfaOperatorInComparison
                | CheckKind::TooManyArguments
                | CheckKind::TooFewArguments
                | CheckKind::UnknownFunction
                | CheckKind::CursorConstantColumn
                | CheckKind::CursorFieldNotInSelect
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub kind: CheckKind,
    pub range: TextRange,
    /// Nome citado na mensagem, como a função ou o parâmetro.
    pub name: Option<NodeId>,
    /// Nome citado que não está no arquivo, como o parâmetro de uma função do ERP.
    pub label: Option<std::string::String>,
}

/// `inherited` são as funções definidas em outras regras que valem para esta, como a regra da
/// raiz de um formulário SGI.
/// Campo do dicionário de dados em `TABELA.Campo` (um `FieldExpr`), como o conferidor de tipos vê.
pub(crate) fn dictionary_field<'a>(
    tree: &Tree,
    symbols: &'a Symbols,
    text: &'a str,
    context: Context,
    inherited: &'a Inherited,
    node: NodeId,
) -> Option<crate::dictionary::Found<'a>> {
    static NO_SQL: std::sync::LazyLock<sql::Sources> = std::sync::LazyLock::new(Default::default);
    let scope = types::Scope {
        symbols,
        text,
        inherited,
        context,
        sql: &NO_SQL,
    };
    types::table_field(tree, &scope, node).map(|(found, _)| found)
}

pub fn validate(
    tree: &Tree,
    symbols: &Symbols,
    text: &str,
    context: Context,
    inherited: &Inherited,
) -> Vec<Check> {
    let mut checks = Vec::new();
    let sources = sql::Sources::build(tree, symbols, text, &inherited.dictionaries);
    let scope = types::Scope {
        symbols,
        text,
        inherited,
        context,
        sql: &sources,
    };
    let mut native_sql = sql::NativeSql::default();
    let mut stack = vec![tree.root()];
    while let Some(node) = stack.pop() {
        types::check(tree, &scope, context, node, &mut checks);
        match tree.kind(node) {
            BinaryExpr => binary(tree, node, &mut checks),
            Assign => assignment(tree, node, &mut checks),
            CallExpr => message_text(tree, &scope, node, &mut checks),
            CallStmt => {
                unknown_function(tree, symbols, text, context, inherited, node, &mut checks);
                native_sql.visit(tree, text, node);
                sources.check_returned_column(tree, text, node, &mut checks);
            }
            FieldCommand => sql::cursor_sql(tree, text, node, &mut checks),
            Literal => lowercase_char(tree, text, node, &mut checks),
            FieldExpr => {
                sources.check_cursor_field(tree, symbols, text, node, &mut checks);
                if let Some((found, _)) = types::table_field(tree, &scope, node)
                    && found.only_in_test
                {
                    checks.push(Check {
                        kind: CheckKind::OnlyInTest,
                        range: tree.text_range(node),
                        name: None,
                        label: None,
                    });
                }
            }
            FuncImpl => function(tree, node, &mut checks),
            _ => {}
        }
        let first = stack.len();
        stack.extend(tree.children(node));
        // Em ordem de texto, para os diagnósticos saírem na ordem do arquivo.
        stack[first..].reverse();
    }
    native_sql.finish(&mut checks);
    if context == Context::Rule {
        unused(tree, symbols, text, &mut checks);
    }
    checks.extend(
        symbols
            .references
            .iter()
            .filter(|r| r.foreign)
            .map(|r| Check {
                kind: CheckKind::ForeignParameter,
                range: r.range,
                name: None,
                label: None,
            }),
    );
    checks.sort_by_key(|c| c.range.start());
    checks
}

/// Definições sem nenhum uso. Um nome citado dentro de um texto conta como usado, porque o SQL de
/// um cursor ou do `ExecSql` lê a variável como `:nome`, e há regras que passam o nome da
/// variável num texto para uma função preencher.
fn unused(tree: &Tree, symbols: &Symbols, text: &str, checks: &mut Vec<Check>) {
    let mut used = vec![false; symbols.definitions.len()];
    for target in symbols.references.iter().flat_map(|r| &r.targets) {
        used[*target] = true;
    }
    let tokens = tree.tokens();
    let mut in_sql = std::collections::HashSet::new();
    for i in 0..tokens.len() {
        if tokens.kind(i) == SyntaxKind::String {
            for name in words(tokens.text(i, text)) {
                in_sql.insert(name.to_ascii_uppercase());
            }
        }
    }
    for (i, def) in symbols.definitions.iter().enumerate() {
        let kind = matches!(
            def.kind,
            DefKind::Variable(_)
                | DefKind::Cursor
                | DefKind::List
                | DefKind::Table
                | DefKind::WebService
        );
        let name = &text[def.name];
        if kind && !used[i] && !in_sql.contains(&name.to_ascii_uppercase()) {
            checks.push(Check {
                kind: CheckKind::Unused,
                range: def.name,
                name: None,
                label: Some(name.to_owned()),
            });
        }
    }
}

/// Palavras de um texto que podem ser nome, como `nCodEmp` em `"WHERE CODEMP = :nCodEmp"`.
fn words(literal: &str) -> impl Iterator<Item = &str> {
    literal
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| w.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_'))
}

fn binary(tree: &Tree, node: NodeId, checks: &mut Vec<Check>) {
    let (Some(op), Some(rhs)) = (operator(tree, node), tree.children(node).last()) else {
        return;
    };
    if tree.kind(rhs) != PrefixExpr || operator(tree, rhs) != Some(Minus) {
        return;
    }
    let operand = tree.children(rhs).next();
    let single_token = operand.is_some_and(|o| matches!(tree.kind(o), NameRef | Literal));
    if matches!(op, Star | Slash | EKw) && !single_token {
        if let Some(operand) = operand {
            checks.push(Check {
                kind: CheckKind::MinusTakesOneToken,
                range: tree.text_range(operand),
                name: None,
                label: None,
            });
        }
    } else if matches!(op, EKw | OuKw) {
        checks.push(Check {
            kind: CheckKind::LogicalMinus,
            range: tree.text_range(rhs),
            name: None,
            label: None,
        });
    }
}

fn assignment(tree: &Tree, node: NodeId, checks: &mut Vec<Check>) {
    let mut children = tree.children(node);
    let (Some(_), Some(rhs)) = (children.next(), children.next()) else {
        return;
    };
    if tree.kind(rhs) != BinaryExpr {
        return;
    }
    let Some(op) = operator_token(tree, rhs) else {
        return;
    };
    if matches!(
        tree.tokens().kind(op),
        Eq | Lt | Gt | LtEq | GtEq | Neq | RelRun
    ) {
        checks.push(Check {
            kind: CheckKind::ComparisonInAssignment,
            range: tree.tokens().range(op),
            name: None,
            label: None,
        });
    }
}

/// O compilador põe o texto dos tokens em maiúsculas, inclusive o do apóstrofo, então `'a'` vale
/// 65, o código do `A`. Só letras sem acento mudam.
fn lowercase_char(tree: &Tree, text: &str, node: NodeId, checks: &mut Vec<Check>) {
    let range = tree.text_range(node);
    let token = tree.token_range(node).start;
    if tree.tokens().kind(token) == Char && text[range].bytes().any(|b| b.is_ascii_lowercase()) {
        checks.push(Check {
            kind: CheckKind::LowercaseChar,
            range,
            name: None,
            label: None,
        });
    }
}

/// `Nome(...);` como comando, com um nome sem `definir funcao` e fora das funções do ERP.
fn unknown_function(
    tree: &Tree,
    symbols: &Symbols,
    text: &str,
    context: Context,
    inherited: &Inherited,
    node: NodeId,
    checks: &mut Vec<Check>,
) {
    let Some(call) = tree
        .children(node)
        .next()
        .filter(|&c| tree.kind(c) == CallExpr)
    else {
        return;
    };
    let Some(callee) = tree
        .children(call)
        .next()
        .filter(|&c| tree.kind(c) == NameRef)
    else {
        return;
    };
    let range = tree.text_range(callee);
    let is_ident = tree.tokens().kind(tree.token_range(callee).start) == Ident;
    if is_ident
        && symbols.reference_at(range).is_none()
        && builtins::lookup(&text[range], context).is_none()
        && builtins::converted(&text[range]).is_none()
        && !inherited.contains(&text[range])
    {
        checks.push(Check {
            kind: CheckKind::UnknownFunction,
            range,
            name: None,
            label: None,
        });
    }
}

fn message_text(tree: &Tree, scope: &types::Scope, node: NodeId, checks: &mut Vec<Check>) {
    let mut children = tree.children(node);
    let (Some(callee), Some(args)) = (children.next(), children.next()) else {
        return;
    };
    if tree.tokens().kind(tree.token_range(callee).start) != MensagemKw {
        return;
    }
    let Some(text) = tree
        .children(args)
        .nth(1)
        .filter(|&t| tree.kind(t) == NameRef)
    else {
        return;
    };
    let range = tree.text_range(text);
    let numeric = |kind| {
        matches!(
            kind,
            DefKind::Variable(Some(VarType::Numero | VarType::Data))
                | DefKind::Parameter(Some(VarType::Numero | VarType::Data))
        )
    };
    let is_numeric = match scope.symbols.reference_at(range) {
        Some(reference) => reference
            .targets
            .iter()
            .all(|&t| numeric(scope.symbols.definitions[t].kind)),
        None => scope
            .inherited
            .variable(&scope.text[range])
            .is_some_and(|v| numeric(v.kind)),
    };
    if is_numeric {
        checks.push(Check {
            kind: CheckKind::NonAlfaMessageText,
            range,
            name: None,
            label: None,
        });
    }
}

fn function(tree: &Tree, node: NodeId, checks: &mut Vec<Check>) {
    let name = tree.children(node).find(|&c| tree.kind(c) == Name);
    let Some(name) = name else {
        return;
    };
    let mut ancestor = tree.parent(node);
    while let Some(a) = ancestor {
        if tree.kind(a) == Block {
            checks.push(Check {
                kind: CheckKind::FunctionInsideBlock,
                range: tree.text_range(name),
                name: Some(name),
                label: None,
            });
            break;
        }
        ancestor = tree.parent(a);
    }
    // Só o primeiro comando depois do bloco, para não sublinhar o resto do arquivo. Definições
    // não executam, então não contam.
    let mut body = tree
        .children(node)
        .skip_while(|&c| matches!(tree.kind(c), Name | ParamList));
    if !body.next().is_some_and(|c| tree.kind(c) == Block) {
        return;
    }
    let is_definition = |c: NodeId| {
        matches!(
            tree.kind(c),
            VarDef | CursorDef | ListDef | TableDef | FuncDecl | WsDef
        )
    };
    if let Some(after) = body.find(|&c| !is_definition(c)) {
        checks.push(Check {
            kind: CheckKind::CodeAfterFunctionBlock,
            range: tree.text_range(after),
            name: Some(name),
            label: None,
        });
    }
}

/// Primeiro token entre os filhos que não é trivia, que numa expressão é o operador.
pub(super) fn operator_token(tree: &Tree, node: NodeId) -> Option<usize> {
    tree.elements(node).find_map(|e| match e {
        Element::Token(t) if !tree.tokens().kind(t).is_trivia() => Some(t),
        _ => None,
    })
}

fn operator(tree: &Tree, node: NodeId) -> Option<SyntaxKind> {
    operator_token(tree, node).map(|t| tree.tokens().kind(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use CheckKind::*;

    #[test]
    fn definicao_sem_uso() {
        let text = "definir alfa aUsada; definir alfa aSobra; definir numero nNoSql;\n\
                    definir cursor cSobra; definir lista lSobra; definir numero nAtribuida;\n\
                    definir numero nPeloNome;\n\
                    aUsada = \"SELECT 1 FROM T WHERE A = :nNoSql\"; nAtribuida = 1;\n\
                    aUsada = \"nPeloNome\";\n\
                    Mensagem(Retorna, aUsada);";
        let unused: Vec<_> = checks(text)
            .into_iter()
            .filter(|(kind, _)| *kind == CheckKind::Unused)
            .map(|(_, name)| name)
            .collect();
        assert_eq!(unused, ["aSobra", "cSobra", "lSobra"]);
    }

    #[test]
    fn sem_aviso_de_sem_uso_fora_da_regra_comum() {
        let text = "definir alfa aSobra;";
        let tree = crate::parse(text).unwrap();
        let symbols = Symbols::build(&tree, text);
        for context in [Context::Sgi, Context::Report] {
            let checks = validate(&tree, &symbols, text, context, &Inherited::default());
            assert!(checks.is_empty(), "{context:?}");
        }
    }

    #[test]
    fn palavras_de_um_texto() {
        let found: Vec<_> = words("\"A = :nCodEmp AND B=:x_1, 9x\"").collect();
        assert_eq!(found, ["A", "nCodEmp", "AND", "B", "x_1"]);
    }

    pub(super) fn checks(text: &str) -> Vec<(CheckKind, &str)> {
        let tree = crate::parse(text).unwrap();
        assert!(tree.errors().is_empty(), "{text}");
        validate(
            &tree,
            &Symbols::build(&tree, text),
            text,
            Context::Rule,
            &Inherited::default(),
        )
        .into_iter()
        .map(|c| (c.kind, &text[c.range]))
        .collect()
    }

    #[test]
    fn constante_no_select_do_cursor() {
        assert_eq!(
            checks("definir cursor c;\nc.SQL \"SELECT 123, T.A FROM T\";"),
            [(CursorConstantColumn, "\"SELECT 123, T.A FROM T\"")]
        );
        assert!(
            checks("definir cursor c;\nc.SQL \"SELECT T.A, B, * FROM T WHERE A = 1\";").is_empty()
        );
        // O SQL_DefinirComando é lido na execução, onde constante vale.
        assert!(checks("SQL_DefinirComando(aQ, \"SELECT 1 FROM DUAL\");").is_empty());
    }

    #[test]
    fn campo_do_cursor_fora_do_select() {
        let text = "definir cursor c;\nc.SQL \"SELECT T.A, B AS X FROM T\";\nc.AbrirCursor();\n\
                    se (c.Achou) { y = c.a; y = c.X; y = c.Nao; c.Proximo(); }\nc.FecharCursor();";
        assert_eq!(checks(text), [(CursorFieldNotInSelect, "Nao")]);
        // Com dois SQL, vale o campo de qualquer um.
        let dois = "definir cursor c;\nc.SQL \"SELECT A FROM T\";\nc.SQL \"SELECT B FROM T\";\n\
                    y = c.A + c.B;";
        assert!(checks(dois).is_empty());
        // SQL numa variável, ou * sem dicionário, deixam o cursor sem conferência.
        assert!(checks("definir cursor c;\nc.SQL aSql;\ny = c.Z;").is_empty());
        assert!(checks("definir cursor c;\nc.SQL \"SELECT * FROM T\";\ny = c.Z;").is_empty());
    }

    #[test]
    fn coluna_do_sql_retornar_fora_do_select() {
        let text = "SQL_UsarAbrangencia(aQ, 0);\nSQL_UsarSQLSenior2(aQ, 0);\n\
                    SQL_DefinirComando(aQ, \"SELECT 123.45 AS num, T.TXT FROM T\");\n\
                    SQL_RetornarAlfa(aQ, \"num\", a);\nSQL_RetornarAlfa(aQ, \"txt\", a);\n\
                    SQL_RetornarAlfa(aQ, \"naoexiste\", a);";
        assert_eq!(checks(text), [(ReturnedColumnNotInSelect, "\"naoexiste\"")]);
        // Comando montado na execução não é conferido.
        assert!(
            checks("SQL_DefinirComando(aQ, \"SELECT A FROM \" + aTab);\nSQL_RetornarAlfa(aQ, \"z\", a);")
                .is_empty()
        );
    }

    #[test]
    fn sql_nativo_sem_desligar_a_abrangencia() {
        assert_eq!(
            checks("SQL_Criar(aQ);\nSQL_UsarSQLSenior2(aQ, 0);"),
            [(NativeSqlWithScope, "SQL_UsarSQLSenior2")]
        );
        assert!(
            checks("SQL_UsarSQLSenior2(aq, 0);\nSQL_UsarAbrangencia(AQ, 0);\nSQL_UsarSQLSenior2(aB, 1);")
                .is_empty()
        );
    }

    #[test]
    fn menos_depois_de_vezes_barra_e_e() {
        assert_eq!(checks("x = 2 * -(3);"), [(MinusTakesOneToken, "(3)")]);
        assert_eq!(checks("x = 2 * -f(1);"), [(MinusTakesOneToken, "f(1)")]);
        assert_eq!(checks("x = 6 / -(3);"), [(MinusTakesOneToken, "(3)")]);
        // Depois de `e` com mais de um token é erro, e o aviso do `e -` não se soma.
        assert_eq!(checks("x = 1 e -(3);"), [(MinusTakesOneToken, "(3)")]);
        assert!(checks("x = 2 * -3; x = 2 * -y; x = 2 - -3; x = -f(1) * 2;").is_empty());
    }

    #[test]
    fn comparacao_na_atribuicao() {
        assert_eq!(checks("x = a > 1;"), [(ComparisonInAssignment, ">")]);
        assert_eq!(
            checks("aTxt = aTxt > \"b\";"),
            [(ComparisonInAssignment, ">")]
        );
        assert_eq!(checks("x = a = b;"), [(ComparisonInAssignment, "=")]);
        assert_eq!(checks("x = a + 1 <> b;"), [(ComparisonInAssignment, "<>")]);
        assert!(
            checks("definir funcao f(numero p);\nx = (a > 0); se (a > 1) { f(a > 1); } x++;")
                .is_empty()
        );
    }

    #[test]
    fn e_ou_seguido_de_menos() {
        assert_eq!(checks("x = 2 e -3;"), [(LogicalMinus, "-3")]);
        assert_eq!(checks("x = 2 ou -3;"), [(LogicalMinus, "-3")]);
        assert!(checks("x = 2 e 3; x = (a = 1) e (b = 2);").is_empty());
    }

    #[test]
    fn comando_depois_do_bloco_da_funcao() {
        let text = "definir funcao f();\nfuncao f(); { x = 1; }\nx = 5;\ny = 6;";
        assert_eq!(checks(text), [(CodeAfterFunctionBlock, "x = 5;")]);
        // Sem bloco, os comandos são o corpo, e é isso que parece.
        assert!(checks("definir funcao f();\nfuncao f();\nx = 5;").is_empty());
        assert!(checks("definir funcao f();\nfuncao f(); { x = 1; }").is_empty());
        let text = "definir funcao f();\nfuncao f(); { x = 1; }\ndefinir funcao g();\nx = 5;";
        assert_eq!(checks(text), [(CodeAfterFunctionBlock, "x = 5;")]);
    }

    #[test]
    fn funcao_dentro_de_bloco_aberto() {
        let text = "definir funcao a(); definir funcao b();\n\
                    funcao a(); { x = 1;\nfuncao b(); { y = 2; }\n}";
        assert_eq!(checks(text), [(FunctionInsideBlock, "b")]);
    }

    #[test]
    fn variavel_numerica_ou_data_no_mensagem() {
        let text = "definir data d; definir alfa a;\n\
                    Mensagem(Retorna, d); Mensagem(Retorna, a);";
        assert_eq!(checks(text), [(NonAlfaMessageText, "d")]);
        let text = "definir numero n; definir alfa a;\n\
                    Mensagem(Retorna, n); Mensagem(Retorna, a); x = Mensagem(Erro, n);";
        assert_eq!(
            checks(text),
            [(NonAlfaMessageText, "n"), (NonAlfaMessageText, "n")]
        );
        // Parâmetro numérico também, e nome sem definir fica sem aviso.
        let text = "definir funcao f(numero p);\nfuncao f(numero p); { Mensagem(Erro, p); }";
        assert_eq!(checks(text), [(NonAlfaMessageText, "p")]);
        assert!(checks("Mensagem(Erro, aSemDefinir);").is_empty());
    }

    #[test]
    fn funcoes_do_erp() {
        let text = "definir alfa a; definir numero n;\n\
                    IntParaAlfa(n, a);\n\
                    IntParaAlfa(n);\n\
                    IntParaAlfa(a, a);\n\
                    IntParaAlfaa(n, a);";
        assert_eq!(
            checks(text),
            [
                (TooFewArguments, "IntParaAlfa"),
                (AlfaArgument, "a"),
                (UnknownFunction, "IntParaAlfaa"),
            ]
        );
    }

    #[test]
    fn funcao_desconhecida_por_contexto() {
        let run = |text: &str, context, inherited: &[&str]| -> Vec<CheckKind> {
            let tree = crate::parse(text).unwrap();
            let rules = inherited.iter().map(|name| {
                let path = std::path::PathBuf::from("/f/rule.txt");
                (path, format!("definir funcao {name}();"))
            });
            let inherited = Inherited::from_rules(rules);
            validate(
                &tree,
                &Symbols::build(&tree, text),
                text,
                context,
                &inherited,
            )
            .into_iter()
            .map(|c| c.kind)
            .collect()
        };
        let sgi = "AbrirTelaSGI(\"T\");";
        assert_eq!(run(sgi, Context::Rule, &[]), [UnknownFunction]);
        assert!(run(sgi, Context::Sgi, &[]).is_empty());
        // Função da raiz do formulário SGI fica sem erro.
        assert!(run("CallWs();", Context::Sgi, &["CallWs"]).is_empty());
        // Função definida no arquivo, método de cursor e Mensagem não são nome desconhecido.
        let text = "definir funcao f(); definir cursor c;\nf(); c.AbrirCursor(); Mensagem(Erro, \"x\");\nfuncao f(); { }";
        assert!(run(text, Context::Rule, &[]).is_empty());
    }

    #[test]
    fn parametros_de_funcao_herdada() {
        let rules = [(
            std::path::PathBuf::from("/f/rule.txt"),
            "definir funcao CallWs(numero p, alfa end r);".to_owned(),
        )];
        let inherited = Inherited::from_rules(rules);
        let text = "definir alfa a;\nCallWs(1, a); CallWs(\"x\", a); CallWs(1); CallWs(1, \"y\");";
        let tree = crate::parse(text).unwrap();
        let symbols = Symbols::build(&tree, text);
        let kinds: Vec<_> = validate(&tree, &symbols, text, Context::Sgi, &inherited)
            .into_iter()
            .map(|c| (c.kind, c.label))
            .collect();
        assert_eq!(
            kinds,
            [
                (AlfaArgument, Some("p".to_owned())),
                (TooFewArguments, None),
                (ConstantInEndParameter, Some("r".to_owned())),
            ]
        );
    }

    #[test]
    fn tipo_de_variavel_herdada() {
        let rules = [(
            std::path::PathBuf::from("/f/rule.txt"),
            "definir alfa aRaiz; definir numero nRaiz;".to_owned(),
        )];
        let inherited = Inherited::from_rules(rules);
        let text = "aRaiz = \"x\"; aRaiz = nRaiz; nRaiz = aRaiz; Mensagem(Erro, nRaiz);";
        let tree = crate::parse(text).unwrap();
        let symbols = Symbols::build(&tree, text);
        let kinds: Vec<_> = validate(&tree, &symbols, text, Context::Sgi, &inherited)
            .into_iter()
            .map(|c| (c.kind, &text[c.range]))
            .collect();
        assert_eq!(
            kinds,
            [
                (AlfaAndNumber, "nRaiz"),
                (NumberAndAlfa, "aRaiz"),
                (NonAlfaMessageText, "nRaiz"),
            ]
        );
    }

    #[test]
    fn apostrofo_minusculo() {
        assert_eq!(
            checks("se (aTxt = 'a') { aTxt = \"x\" + 'c'; }"),
            [(LowercaseChar, "'a'"), (LowercaseChar, "'c'")]
        );
        assert!(checks("x = 'A'; x = '1'; x = 'é'; x = ''';").is_empty());
    }

    #[test]
    fn parametro_fora_da_funcao() {
        let text = "definir funcao f(numero p);\ndefinir funcao g();\n\
                    funcao f(numero p); { }\nfuncao g(); { r = p; }";
        assert_eq!(checks(text), [(ForeignParameter, "p")]);
    }
}
