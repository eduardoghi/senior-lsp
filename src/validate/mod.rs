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

mod declarations;
mod objects;
mod sql;
mod types;
use crate::{Element, NodeId, Tree};
pub use sql::cursor_columns;
pub(crate) use sql::cursor_sql_literals;
pub(crate) use sql::prewarm;

/// O que o Senior SQL 2 recusa num comando, igual no cursor, no `ExecSql` e no
/// `SQL_DefinirComando`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sql2Error {
    Join,
    UnknownTable,
    UnknownSymbol,
    UnknownField,
    AmbiguousField,
    OrderBy,
    InsertValues,
}

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
    /// Tabela que o dicionário de dados não tem no SQL nativo, o de um `SQL_DefinirComando` com
    /// `SQL_UsarSQLSenior2(x, 0)` e o do cursor e do `ExecSql` na regra sem o Senior SQL 2.
    /// Compila, e o banco recusa na execução.
    NativeUnknownTable,
    /// Campo com a tabela ou o apelido na frente que a tabela não tem no dicionário de dados, no
    /// SQL nativo. Compila, e o banco recusa na execução.
    NativeUnknownField,
    /// `cur.CAMPO` com um campo que não está no SELECT do cursor nem é operação dele.
    CursorFieldNotInSelect,
    /// Coluna do SELECT de um cursor com uma tabela que não está no dicionário de dados, como um
    /// apelido de tabela.
    CursorUnknownTable,
    /// Coluna do SELECT de um cursor com um campo que a tabela não tem no dicionário de dados.
    CursorUnknownField,
    /// Coluna do SELECT de um cursor com algo depois de um espaço, como um apelido, e sem a
    /// tabela na frente.
    CursorColumnWithoutTable,
    /// O mesmo nome de campo vindo de duas tabelas no SELECT de um cursor.
    CursorRepeatedField,
    /// `:nome` no SQL de um cursor que ainda não foi definido nem usado na regra.
    CursorUnknownParameter,
    /// JOIN no SQL de um cursor de regra que usa o Senior SQL 2.
    Sql2Join,
    /// Tabela do FROM que não está no dicionário de dados, com o Senior SQL 2.
    Sql2UnknownTable,
    /// Nome do WHERE que o Senior SQL 2 não acha nas tabelas do FROM.
    Sql2UnknownSymbol,
    /// Campo do WHERE que a tabela escrita na frente dele não tem, com o Senior SQL 2.
    Sql2UnknownField,
    /// Campo do WHERE sem tabela na frente que mais de uma tabela do FROM tem, com o Senior
    /// SQL 2.
    Sql2AmbiguousField,
    /// Campo do ORDER BY que não está no SELECT, com o Senior SQL 2.
    Sql2OrderBy,
    /// INSERT com a quantidade de valores diferente da de campos, com o Senior SQL 2.
    Sql2InsertValues,
    /// O que o Senior SQL 2 recusa no SELECT literal de um `SQL_DefinirComando` de um objeto sem
    /// `SQL_UsarSQLSenior2(x, 0)`. O comando só é lido na execução, então compila, e a execução
    /// para.
    CommandSql2(Sql2Error),
    /// `SQL_Retornar*(obj, "coluna", ...)` com uma coluna que não está no SELECT. Compila, e a
    /// execução para.
    ReturnedColumnNotInSelect,
    /// Variável, cursor, lista, tabela ou web service definido e nunca usado. Só na regra comum,
    /// porque na tela e no relatório as regras seguintes podem usar o nome.
    Unused,
    /// Função definida e nunca chamada, na regra comum, pelo mesmo motivo.
    UnusedFunction,
    /// Coluna do SELECT de um cursor que a regra não lê, na regra comum, pelo mesmo motivo.
    CursorColumnUnused,
    /// `*` no SELECT de um cursor de que a regra só lê parte dos campos, também na regra comum.
    CursorStarUnused,
    /// Instruções depois de `Pare`, `Continue`, `Cancel`, `Cancelar` ou `VaPara` no mesmo bloco, que
    /// nunca executam.
    Unreachable,
    /// `enquanto ()` sem nada no bloco que saia do laço. A condição vazia vale como verdadeira.
    EndlessLoop,
    /// `Pare` ou `Continue` fora de um `para` ou `enquanto`.
    LoopClauseOutsideLoop,
    /// `Continue` num `para`, que volta sem rodar o incremento.
    ContinueSkipsStep,
    /// Parâmetro da `funcao` que o corpo não lê nem escreve.
    UnusedParameter,
    /// `Definir Tabela t = { ... }`, sem o tamanho entre colchetes depois do nome.
    TableWithoutSize,
    /// `Definir Tabela t[n] = { }`, sem nenhuma coluna.
    TableWithoutColumns,
    /// `t.coluna`, sem o índice da linha.
    TableWithoutIndex,
    /// `t[1].v` com `v` uma coluna numérica com tamanho, sem o segundo índice.
    TableColumnWithoutIndex,
    /// `t[1].zz` com uma coluna que a tabela não tem.
    TableUnknownColumn,
    /// Índice literal fora da faixa de uma tabela, de uma variável numérica ou de uma coluna.
    IndexOutOfRange,
    /// `AbrirCursor`, `FecharCursor` ou `Proximo` do cursor com argumento.
    CursorCommandArgument,
    /// `Achou()` ou `NaoAchou()`, que são propriedades do cursor.
    CursorFlagCalled,
    /// `UsaAbrangencia()` sem o valor.
    CursorScopeWithoutArgument,
    /// `UsaTraducao` do cursor numa regra que usa o SQL nativo.
    CursorTranslationWithoutSql2,
    /// Atribuição a um campo do cursor.
    CursorMemberAssigned,
    /// Membro da lista que não é método, função, propriedade nem campo criado com `AdicionarCampo`.
    ListNotMember,
    /// Função da lista, como `Primeiro`, sem os parênteses.
    ListFunctionWithoutParentheses,
    /// Propriedade da lista, como `NumReg`, com parênteses.
    ListPropertyWithParentheses,
    /// `AdicionarCampo` sem o nome do campo em texto.
    ListFieldWithoutName,
    /// `AdicionarCampo` com tamanho num campo que não é alfa.
    ListSizeOnNonAlfa,
    /// `AdicionarCampo` com o nome de um campo que a lista já tem.
    ListRepeatedField,
    /// `Chave` da lista sem o texto com os campos.
    ListKeyWithoutFields,
    /// `VaPara` para um nome que não é rótulo do arquivo.
    UnknownLabel,
    /// `funcao` sem um `Definir Funcao` do mesmo nome antes dela.
    UndeclaredFunction,
    /// Chamada a uma função com 32 caracteres ou mais no nome, que o ERP corta em 31.
    FunctionNameTooLong,
    /// Concatenação de alfa num argumento de função.
    AlfaConcatenationInArgument,
    /// `Definir` do mesmo nome com outro tipo.
    ConflictingDefinition,
    /// Chamada a uma função declarada que nenhuma `funcao` implementa.
    CallWithoutImplementation,
    /// Chamada a uma função com exatamente 31 caracteres no nome.
    CallToLongFunctionName,
    /// Texto literal num parâmetro alfa de função da regra.
    LiteralInAlfaParameter,
    /// Índice alfa numa tabela.
    AlfaTableIndex,
    /// Segundo índice literal que passa do tamanho da coluna sem passar do número de linhas.
    ColumnIndexBeyondSize,
    /// `Definir Funcao` repetido com outra assinatura.
    RepeatedDeclaration,
    /// Parâmetro repetido na mesma função.
    RepeatedParameter,
    /// Coluna repetida na mesma tabela.
    RepeatedColumn,
    /// Rótulo repetido.
    RepeatedLabel,
    /// Dois rótulos com os mesmos 30 primeiros caracteres.
    LabelsSameStart,
    /// Tamanho de tabela acima de 65535.
    TableSizeOverflow,
}

impl CheckKind {
    /// O erro do Senior SQL 2 de uma conferência dele, na compilação ou na execução.
    pub fn sql2_error(self) -> Option<Sql2Error> {
        Some(match self {
            CheckKind::Sql2Join => Sql2Error::Join,
            CheckKind::Sql2UnknownTable => Sql2Error::UnknownTable,
            CheckKind::Sql2UnknownSymbol => Sql2Error::UnknownSymbol,
            CheckKind::Sql2UnknownField => Sql2Error::UnknownField,
            CheckKind::Sql2AmbiguousField => Sql2Error::AmbiguousField,
            CheckKind::Sql2OrderBy => Sql2Error::OrderBy,
            CheckKind::Sql2InsertValues => Sql2Error::InsertValues,
            CheckKind::CommandSql2(error) => error,
            _ => return None,
        })
    }

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
                | CheckKind::LoopClauseOutsideLoop
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

/// Tipo da variável que pode receber a expressão numa atribuição, para tirar a expressão de onde
/// está e pôr numa variável nova. `None` quando a expressão não pode ir sozinha numa atribuição ou
/// quando o tipo de alguma parte dela não é conhecido.
pub(crate) fn expression_type(
    tree: &Tree,
    symbols: &Symbols,
    text: &str,
    context: Context,
    inherited: &Inherited,
    node: NodeId,
) -> Option<crate::symbols::VarType> {
    let sources = sql::Sources::build(tree, symbols, text, &inherited.dictionaries);
    let scope = types::Scope {
        symbols,
        text,
        inherited,
        context,
        sql: &sources,
    };
    types::assignable(tree, &scope, node)
}

/// `inherited` são as funções definidas em outras regras que valem para esta, como a regra da
/// raiz de um formulário SGI.
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
    let first_uses = sql::FirstUses::new(tree, text);
    let objects = objects::Objects::build(&scope, tree, &mut checks);
    let declarations = declarations::Declarations::build(&scope, tree, &mut checks);
    let mut native_sql = sql::NativeSql::default();
    // Só na regra comum, porque na tela e no relatório as regras seguintes podem ler o cursor.
    let mut column_uses = (context == Context::Rule).then(sql::ColumnUses::default);
    let mut stack = vec![tree.root()];
    while let Some(node) = stack.pop() {
        types::check(tree, &scope, context, node, &mut checks);
        match tree.kind(node) {
            BinaryExpr => binary(tree, node, &mut checks),
            Assign => {
                assignment(tree, node, &mut checks);
                objects.assign(node, &mut checks);
            }
            CallExpr => {
                message_text(tree, &scope, node, &mut checks);
                objects.call(node, &mut checks);
                declarations.call(node, &mut checks);
            }
            IndexExpr => objects.index_expr(node, &mut checks),
            TableDef => objects.table_def(node, &mut checks),
            Goto => declarations.goto(node, &mut checks),
            While => endless_loop(tree, symbols, text, inherited, node, &mut checks),
            Break | Continue => loop_clause(tree, text, node, &mut checks),
            CallStmt => {
                unknown_function(tree, symbols, text, context, inherited, node, &mut checks);
                native_sql.visit(tree, text, node);
                sources.check_returned_column(tree, text, node, &mut checks);
                sql::exec_sql(tree, &scope, &first_uses, node, &mut checks);
            }
            KeywordStmt => sql::exec_sql(tree, &scope, &first_uses, node, &mut checks),
            FieldCommand => {
                sql::cursor_sql(tree, &scope, &first_uses, node, &mut checks);
                if let Some(uses) = &mut column_uses {
                    uses.command(tree, symbols, text, node);
                }
            }
            Literal => lowercase_char(tree, text, node, &mut checks),
            FieldExpr => {
                objects.field_expr(node, &mut checks);
                let read = sources.check_cursor_field(tree, symbols, text, node, &mut checks);
                if let (Some(uses), Some((cursor, field))) = (&mut column_uses, read) {
                    uses.read(cursor, field);
                }
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
            FuncImpl => {
                function(tree, node, &mut checks);
                unreachable(tree, node, &mut checks);
            }
            Block | SourceFile => unreachable(tree, node, &mut checks),
            _ => {}
        }
        let first = stack.len();
        stack.extend(tree.children(node));
        // Em ordem de texto, para os diagnósticos saírem na ordem do arquivo.
        stack[first..].reverse();
    }
    native_sql.finish(text, inherited, &mut checks);
    // Os nomes sem uso direto dos parâmetros e, na regra comum, das variáveis e funções são
    // procurados nos textos numa varredura só, que numa regra com muito SQL passa de 1 MB.
    let used = used_definitions(symbols);
    let parameters = unused_parameter_defs(tree, symbols, &used);
    let mut wanted: rustc_hash::FxHashSet<std::string::String> = parameters
        .iter()
        .map(|&i| text[symbols.definitions[i].name].to_ascii_uppercase())
        .collect();
    if context == Context::Rule {
        wanted.extend(unused_names(symbols, text, &used));
    }
    let in_strings = names_in_strings(tree, text, &wanted);
    unused_parameters(symbols, text, &parameters, &in_strings, &mut checks);
    if let Some(uses) = column_uses {
        uses.finish(text, &inherited.dictionaries, &mut checks);
    }
    if context == Context::Rule {
        unused(tree, symbols, text, &used, &in_strings, &mut checks);
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

/// Para cada definição, se algum nome do arquivo a alcança.
fn used_definitions(symbols: &Symbols) -> Vec<bool> {
    let mut used = vec![false; symbols.definitions.len()];
    for target in symbols.references.iter().flat_map(|r| &r.targets) {
        used[*target] = true;
    }
    used
}

/// Parâmetros da `funcao` que nenhum nome do corpo alcança, pelo índice da definição. Os da
/// declaração não contam, porque o corpo é o da `funcao`.
fn unused_parameter_defs(tree: &Tree, symbols: &Symbols, used: &[bool]) -> Vec<usize> {
    let in_impl = |def: &crate::symbols::Definition| {
        tree.parent(def.node)
            .and_then(|list| tree.parent(list))
            .is_some_and(|header| tree.kind(header) == FuncImpl)
    };
    symbols
        .definitions
        .iter()
        .enumerate()
        .filter(|(i, d)| matches!(d.kind, DefKind::Parameter(_)) && !used[*i] && in_impl(d))
        .map(|(i, _)| i)
        .collect()
}

/// Parâmetros da `funcao` que o corpo não usa, dos de `parameters`. Vale em todo tipo de regra,
/// porque o parâmetro só existe dentro da função. O nome citado num texto (`in_strings`) conta como
/// usado, como no [`unused`], porque o SQL lê o parâmetro como `:nome`.
fn unused_parameters(
    symbols: &Symbols,
    text: &str,
    parameters: &[usize],
    in_strings: &rustc_hash::FxHashSet<std::string::String>,
    checks: &mut Vec<Check>,
) {
    for &i in parameters {
        let def = &symbols.definitions[i];
        let name = &text[def.name];
        if !in_strings.contains(&name.to_ascii_uppercase()) {
            checks.push(Check {
                kind: CheckKind::UnusedParameter,
                range: def.name,
                name: None,
                label: Some(name.to_owned()),
            });
        }
    }
}

/// Variável, cursor, lista, tabela ou web service, que o [`unused`] cobra.
fn is_variable(def: &crate::symbols::Definition) -> bool {
    matches!(
        def.kind,
        DefKind::Variable(_)
            | DefKind::Cursor
            | DefKind::List
            | DefKind::Table
            | DefKind::WebService
    )
}

/// As funções do arquivo pelo nome em maiúsculas, cada uma com o `definir funcao` e a `funcao`.
fn functions_by_name(
    symbols: &Symbols,
    text: &str,
) -> std::collections::BTreeMap<std::string::String, Vec<usize>> {
    let mut functions: std::collections::BTreeMap<std::string::String, Vec<usize>> =
        Default::default();
    for (i, def) in symbols.definitions.iter().enumerate() {
        if def.kind == DefKind::Function {
            functions
                .entry(text[def.name].to_ascii_uppercase())
                .or_default()
                .push(i);
        }
    }
    functions
}

/// Nomes, em maiúsculas, das variáveis e funções sem uso direto, que o [`unused`] procura nos
/// textos. A maioria das regras não tem nenhum, e então os textos nem são lidos.
fn unused_names(
    symbols: &Symbols,
    text: &str,
    used: &[bool],
) -> impl Iterator<Item = std::string::String> {
    let variables: Vec<std::string::String> = symbols
        .definitions
        .iter()
        .enumerate()
        .filter(|(i, def)| is_variable(def) && !used[*i])
        .map(|(_, def)| text[def.name].to_ascii_uppercase())
        .collect();
    let functions = functions_by_name(symbols, text)
        .into_iter()
        .filter(|(_, defs)| !defs.iter().any(|&i| used[i]))
        .map(|(upper, _)| upper);
    variables.into_iter().chain(functions)
}

/// Definições sem nenhum uso. Um nome citado dentro de um texto conta como usado, porque o SQL de
/// um cursor ou do `ExecSql` lê a variável como `:nome`, e há regras que passam o nome da
/// variável num texto para uma função preencher.
fn unused(
    tree: &Tree,
    symbols: &Symbols,
    text: &str,
    used: &[bool],
    in_sql: &rustc_hash::FxHashSet<std::string::String>,
    checks: &mut Vec<Check>,
) {
    // A função tem o `definir funcao` e a `funcao`, e qualquer chamada alcança as duas. O aviso
    // sai uma vez, na `funcao` quando ela existe.
    let functions = functions_by_name(symbols, text);
    for (i, def) in symbols.definitions.iter().enumerate() {
        let name = &text[def.name];
        if is_variable(def) && !used[i] && !in_sql.contains(&name.to_ascii_uppercase()) {
            checks.push(Check {
                kind: CheckKind::Unused,
                range: def.name,
                name: None,
                label: Some(name.to_owned()),
            });
        }
    }
    for (upper, defs) in functions {
        if defs.iter().any(|&i| used[i]) || in_sql.contains(&upper) {
            continue;
        }
        let def = defs
            .iter()
            .map(|&i| &symbols.definitions[i])
            .find(|d| tree.kind(d.node) == SyntaxKind::FuncImpl)
            .unwrap_or(&symbols.definitions[defs[0]]);
        checks.push(Check {
            kind: CheckKind::UnusedFunction,
            range: def.name,
            name: None,
            label: Some(text[def.name].to_owned()),
        });
    }
}

/// Dos nomes em `wanted` (em maiúsculas), os que aparecem como palavra em algum texto do arquivo.
///
/// Roda a cada tecla e passa por todos os textos, que numa regra com muito SQL somam mais de 1 MB.
/// Os nomes procurados são poucos, então quase nenhuma palavra tem o tamanho e a primeira letra de
/// um deles, e a palavra é descartada por aí antes de ser copiada e consultada no conjunto.
pub(crate) fn names_in_strings(
    tree: &Tree,
    text: &str,
    wanted: &rustc_hash::FxHashSet<std::string::String>,
) -> rustc_hash::FxHashSet<std::string::String> {
    let mut found = rustc_hash::FxHashSet::default();
    if wanted.is_empty() {
        return found;
    }
    let longest = wanted.iter().map(|w| w.len()).max().unwrap_or(0);
    let mut lengths = vec![false; longest + 1];
    let mut firsts = [false; 256];
    for name in wanted {
        lengths[name.len()] = true;
        if let Some(&b) = name.as_bytes().first() {
            firsts[b as usize] = true;
            firsts[b.to_ascii_lowercase() as usize] = true;
        }
    }
    let tokens = tree.tokens();
    let mut upper = std::string::String::new();
    for i in 0..tokens.len() {
        if tokens.kind(i) != SyntaxKind::String {
            continue;
        }
        for word in words(tokens.text(i, text)) {
            if word.len() > longest || !lengths[word.len()] || !firsts[word.as_bytes()[0] as usize]
            {
                continue;
            }
            upper.clear();
            upper.push_str(word);
            upper.make_ascii_uppercase();
            if wanted.contains(&upper) && !found.contains(&upper) {
                found.insert(upper.clone());
                // Com todos achados, o resto dos textos não muda nada.
                if found.len() == wanted.len() {
                    return found;
                }
            }
        }
    }
    found
}

/// Bytes que formam nome: letras sem acento, dígitos e `_`.
static WORD: [bool; 256] = {
    let mut table = [false; 256];
    let mut b = 0;
    while b < 256 {
        table[b] = (b as u8).is_ascii_alphanumeric() || b as u8 == b'_';
        b += 1;
    }
    table
};

/// Palavras de um texto que podem ser nome, como `nCodEmp` em `"WHERE CODEMP = :nCodEmp"`. A
/// varredura é por bytes. Os de um caractere acentuado nunca são de nome, então separam as
/// palavras como qualquer outro, sem decodificar o texto.
fn words(literal: &str) -> impl Iterator<Item = &str> {
    let bytes = literal.as_bytes();
    let mut at = 0;
    std::iter::from_fn(move || {
        loop {
            let start = at + bytes[at..].iter().position(|&b| WORD[b as usize])?;
            let len = bytes[start..]
                .iter()
                .position(|&b| !WORD[b as usize])
                .unwrap_or(bytes.len() - start);
            at = start + len;
            // Nome não começa com dígito.
            if !bytes[start].is_ascii_digit() {
                // A palavra começa e termina em byte ASCII, que é fronteira de caractere.
                return Some(&literal[start..at]);
            }
        }
    })
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
    if let Some(after) = body.find(|&c| !is_definition(tree, c)) {
        checks.push(Check {
            kind: CheckKind::CodeAfterFunctionBlock,
            range: tree.text_range(after),
            name: Some(name),
            label: None,
        });
    }
}

/// Definições não executam.
fn is_definition(tree: &Tree, node: NodeId) -> bool {
    matches!(
        tree.kind(node),
        VarDef | CursorDef | ListDef | TableDef | FuncDecl | WsDef
    )
}

/// Comando que desvia o fluxo para fora do resto do bloco, com o nome que a mensagem cita.
/// `Cancel` e `Cancelar` valem com e sem parênteses e terminam a regra, e o `Continue` volta ao
/// começo do laço. Todos conferidos no ERP executando a regra.
fn flow_stop(tree: &Tree, node: NodeId) -> Option<&'static str> {
    let name = |kind| match kind {
        CancelKw => Some("Cancel"),
        CancelarKw => Some("Cancelar"),
        _ => None,
    };
    match tree.kind(node) {
        Break => Some("Pare"),
        Continue => Some("Continue"),
        Goto => Some("VaPara"),
        KeywordStmt => operator(tree, node).and_then(name),
        // O `Cancel(...)` é uma chamada, e o nome fica num `NameRef`.
        CallStmt => tree
            .children(node)
            .find(|&c| tree.kind(c) == CallExpr)
            .and_then(|call| tree.children(call).next())
            .filter(|&callee| tree.kind(callee) == NameRef)
            .and_then(|callee| name(tree.tokens().kind(tree.token_range(callee).start))),
        _ => None,
    }
}

/// `Pare` e `Continue` fora de um `para` ou `enquanto`, que o compilador recusa, e o `Continue`
/// num `para`, que volta sem rodar o incremento e repete o laço sem fim quando nada mais muda o
/// contador. Os dois conferidos no ERP. O laço vale só dentro da mesma função.
fn loop_clause(tree: &Tree, text: &str, node: NodeId, checks: &mut Vec<Check>) {
    let kind = tree.kind(node);
    // O laço mais de dentro, subindo pelos pais até a função em volta.
    let mut up = tree.parent(node);
    let inner = loop {
        match up.map(|n| (n, tree.kind(n))) {
            Some((n, While | For)) => break Some(n),
            Some((_, FuncImpl)) | None => break None,
            Some((n, _)) => up = tree.parent(n),
        }
    };
    match inner {
        None => checks.push(Check {
            kind: CheckKind::LoopClauseOutsideLoop,
            range: tree.text_range(node),
            name: None,
            label: Some(if kind == Break { "Pare" } else { "Continue" }.to_owned()),
        }),
        Some(lp) if kind == Continue && tree.kind(lp) == For => {
            let step = tree
                .children(lp)
                .find(|&c| tree.kind(c) == ForStep)
                .map(|s| text[tree.text_range(s)].trim().to_owned())
                .filter(|s| !s.is_empty());
            if let Some(step) = step {
                checks.push(Check {
                    kind: CheckKind::ContinueSkipsStep,
                    range: tree.text_range(node),
                    name: None,
                    label: Some(step),
                });
            }
        }
        Some(_) => {}
    }
}

/// `enquanto ()` com a condição vazia, que o compilador aceita e trata como verdadeira (conferido
/// no ERP executando a regra), e sem nada no bloco que possa sair do laço. Conta como saída o
/// `Pare`, o `VaPara`, o `Cancel` e o `Cancelar` em qualquer ponto do bloco, a `Mensagem` de
/// `Erro` ou `Refaz` e a chamada a uma função da regra ou de outra regra, que pode ter um `Cancel`.
/// O `Pare` de um laço de dentro também conta, então o aviso pode deixar de sair num caso que
/// trava, mas não sai num que termina.
fn endless_loop(
    tree: &Tree,
    symbols: &Symbols,
    text: &str,
    inherited: &Inherited,
    node: NodeId,
    checks: &mut Vec<Check>,
) {
    let mut significant = tree.elements(node).filter(|e| match e {
        Element::Token(t) => !tree.tokens().kind(*t).is_trivia(),
        Element::Node(_) => true,
    });
    let (Some(Element::Token(keyword)), Some(Element::Token(open)), Some(Element::Token(close))) =
        (significant.next(), significant.next(), significant.next())
    else {
        return;
    };
    let kind = |t: usize| tree.tokens().kind(t);
    if kind(open) != LParen || kind(close) != RParen {
        return;
    }
    let Some(body) = tree.children(node).last() else {
        return;
    };
    let user_function = |name: &str| {
        inherited.contains(name)
            || symbols
                .definitions
                .iter()
                .any(|d| d.kind == DefKind::Function && text[d.name].eq_ignore_ascii_case(name))
    };
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        // O `Continue` volta ao começo e não sai do laço.
        if flow_stop(tree, n).is_some_and(|stop| stop != "Continue") {
            return;
        }
        if tree.kind(n) == CallExpr {
            let mut parts = tree.children(n);
            let callee = parts
                .next()
                .map(|c| &text[tree.text_range(c)])
                .unwrap_or_default();
            let first = parts
                .next()
                .and_then(|args| tree.children(args).next())
                .map(|a| text[tree.text_range(a)].trim());
            let stops_rule = callee.eq_ignore_ascii_case("Mensagem")
                && first.is_some_and(|f| {
                    f.eq_ignore_ascii_case("Erro") || f.eq_ignore_ascii_case("Refaz")
                });
            if stops_rule || user_function(callee) {
                return;
            }
        }
        stack.extend(tree.children(n));
    }
    checks.push(Check {
        kind: CheckKind::EndlessLoop,
        range: TextRange::new(
            tree.tokens().range(keyword).start(),
            tree.tokens().range(close).end(),
        ),
        name: None,
        label: None,
    });
}

/// Rótulo em `node` ou abaixo dele (fora de funções), que um `VaPara` pode alcançar.
fn contains_label(tree: &Tree, node: NodeId) -> bool {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        match tree.kind(n) {
            Label => return true,
            FuncImpl => {}
            _ => stack.extend(tree.children(n)),
        }
    }
    false
}

/// Instruções depois de `Pare`, `Continue`, `Cancel`, `Cancelar` ou `VaPara` no mesmo bloco, até um
/// rótulo ou o fim do bloco. O rótulo pode ser alvo de um `VaPara` e volta a tornar o código alcançável. Uma
/// instrução que guarda um rótulo por dentro também encerra o trecho, pelo mesmo motivo. O que
/// vem antes de uma `funcao` não entra, porque o compilador trata esse caso à parte (SEN061).
fn unreachable(tree: &Tree, node: NodeId, checks: &mut Vec<Check>) {
    let mut stopper = None;
    let mut dead: Option<TextRange> = None;
    let mut flush = |stopper: &mut Option<&'static str>, dead: &mut Option<TextRange>| {
        if let (Some(cmd), Some(range)) = (stopper.take(), dead.take()) {
            checks.push(Check {
                kind: CheckKind::Unreachable,
                range,
                name: None,
                label: Some(cmd.to_owned()),
            });
        }
    };
    for child in tree.children(node) {
        if stopper.is_none() {
            stopper = flow_stop(tree, child);
            continue;
        }
        if matches!(tree.kind(child), Label | FuncImpl) || contains_label(tree, child) {
            flush(&mut stopper, &mut dead);
        } else if !is_definition(tree, child) {
            let range = tree.text_range(child);
            dead = Some(dead.map_or(range, |d| d.cover(range)));
        }
    }
    flush(&mut stopper, &mut dead);
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
    fn nome_citado_num_texto_conta_como_uso_sem_diferenca_de_caixa() {
        let text = "definir funcao PeloTexto(); definir funcao Esquecida();\n\
                    definir alfa aTexto; definir numero NMAIUSCULA;\n\
                    aTexto = \"WHERE X = :nmaiuscula AND Y = 'pelotexto'\";\n\
                    Mensagem(Retorna, aTexto);\n\
                    funcao PeloTexto(); { }\n\
                    funcao Esquecida(); { }";
        let found: Vec<_> = checks_all(text)
            .into_iter()
            .filter(|(kind, _)| matches!(kind, CheckKind::Unused | CheckKind::UnusedFunction))
            .collect();
        assert_eq!(found, [(CheckKind::UnusedFunction, "Esquecida")]);
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
        // Caractere acentuado separa as palavras, e a palavra pode encostar nas pontas do texto.
        let found: Vec<_> = words("açãoX é_1 fim").collect();
        assert_eq!(found, ["a", "oX", "_1", "fim"]);
        assert_eq!(
            words("").count() + words("\"\"").count() + words("12 3.4").count(),
            0
        );
    }

    #[test]
    fn nome_no_texto_so_conta_como_palavra_inteira() {
        // Mesmo tamanho ou mesma primeira letra de um nome procurado não bastam, e o nome dentro
        // de outra palavra também não.
        let text = "definir numero nValor; definir numero nOutro; definir alfa aLonga_1;\n\
                    definir alfa aTexto;\n\
                    aTexto = \"nValo1 nValorX XnValor nOutr0 9nOutro aLonga_\";\n\
                    Mensagem(Retorna, aTexto);";
        let unused: Vec<_> = checks(text)
            .into_iter()
            .filter(|(kind, _)| *kind == CheckKind::Unused)
            .map(|(_, name)| name)
            .collect();
        assert_eq!(unused, ["nValor", "nOutro", "aLonga_1"]);

        // Entre pontuação, com acento encostado, em outra caixa e num texto de várias linhas.
        let text = "definir numero nValor; definir numero nOutro; definir alfa aLonga_1;\n\
                    definir alfa aTexto;\n\
                    aTexto = \"(:NVALOR),énoutroé \\\n  =ALONGA_1\";\n\
                    Mensagem(Retorna, aTexto);";
        assert!(
            checks(text)
                .iter()
                .all(|(kind, _)| *kind != CheckKind::Unused)
        );
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
        // A função de exemplo de muitos testes não é chamada nem usa os parâmetros, e não tem a
        // `funcao` quando só a chamada importa. Os cursores de exemplo não têm os campos lidos.
        // Os testes dessas conferências usam `checks_all`.
        .filter(|c| {
            !matches!(
                c.kind,
                CheckKind::UnusedFunction
                    | CheckKind::CallWithoutImplementation
                    | CheckKind::UnusedParameter
                    | CheckKind::CursorColumnUnused
                    | CheckKind::CursorStarUnused
            )
        })
        .map(|c| (c.kind, &text[c.range]))
        .collect()
    }

    pub(super) fn checks_all(text: &str) -> Vec<(CheckKind, &str)> {
        let tree = crate::parse(text).unwrap();
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
    fn parametro_sem_uso() {
        let text = "Definir Funcao F(Numero pA, Numero pB, Alfa pC, Alfa End pD);\n\
                    definir cursor c;\nF(1, 2, \"x\", aX);\n\
                    funcao F(Numero pA, Numero pB, Alfa pC, Alfa End pD); {\n\
                    pA = 1;\n  c.SQL \"SELECT A FROM T WHERE B = :pC\";\n}";
        // O pA é escrito e o pC é lido pelo SQL. Na declaração não há aviso.
        let found: Vec<_> = checks_all(text)
            .into_iter()
            .filter(|(k, _)| *k == UnusedParameter)
            .collect();
        assert_eq!(found, [(UnusedParameter, "pB"), (UnusedParameter, "pD")]);
    }

    #[test]
    fn funcao_sem_chamada() {
        let text = "Definir Funcao Usada();\nDefinir Funcao Sobra();\nDefinir Funcao PeloNome();\n\
                    definir alfa a;\nUsada();\na = \"PeloNome\";\nMensagem(Retorna, a);\n\
                    funcao Usada(); { }\nfuncao Sobra(); { }\nfuncao PeloNome(); { }";
        let sobra: Vec<_> = checks_all(text)
            .into_iter()
            .filter(|(k, _)| *k == CheckKind::UnusedFunction)
            .map(|(_, n)| n)
            .collect();
        assert_eq!(sobra, ["Sobra"]);
    }

    /// Uma soma de milhares de termos tem um nível de árvore por termo. A conferência de tipos
    /// descia por ela com recursão e estourava a pilha, que aqui é pequena de propósito, como a
    /// de 1 MB do Windows num binário de depuração.
    #[test]
    fn soma_longa_nao_estoura_a_pilha() {
        let termos = 30_000;
        let numero = format!("definir numero n;\nn = {};", vec!["1"; termos].join(" + "));
        let texto = format!(
            "definir alfa a;\na = {};",
            vec!["\"x\""; termos].join(" + ")
        );
        let errado = format!(
            "definir alfa a;\na = {} + 1;",
            vec!["\"x\""; termos].join(" + ")
        );
        let resultado = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || {
                let tipo = |text: &str| {
                    let tree = crate::parse(text).unwrap();
                    let symbols = Symbols::build(&tree, text);
                    let mut stack = vec![tree.root()];
                    let mut assign = None;
                    while let Some(n) = stack.pop() {
                        if tree.kind(n) == Assign {
                            assign = Some(n);
                            break;
                        }
                        stack.extend(tree.children(n));
                    }
                    let rhs = tree.children(assign.unwrap()).nth(1).unwrap();
                    expression_type(
                        &tree,
                        &symbols,
                        text,
                        Context::Rule,
                        &Inherited::default(),
                        rhs,
                    )
                };
                (
                    checks(&numero).len(),
                    checks(&texto).len(),
                    checks(&errado).iter().map(|(k, _)| *k).collect::<Vec<_>>(),
                    tipo(&numero),
                    tipo(&texto),
                )
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(
            resultado,
            (
                0,
                0,
                vec![AlfaAndNumber],
                Some(VarType::Numero),
                Some(VarType::Alfa)
            )
        );
    }

    #[test]
    fn enquanto_vazio_sem_saida() {
        let laco = |corpo: &str| format!("definir numero n;\nenquanto () {{ n = n + 1; {corpo} }}");
        assert_eq!(checks(&laco("")), [(EndlessLoop, "enquanto ()")]);
        assert_eq!(
            checks("definir numero n;\nenquanto ( @ nada @ )\n  n = n + 1;"),
            [(EndlessLoop, "enquanto ( @ nada @ )")]
        );
        // Qualquer coisa que possa sair do laço tira o aviso.
        for corpo in [
            "se (n > 3) Pare;",
            "se (n > 3) { VaPara Saida; }",
            "se (n > 3) cancel;",
            "se (n > 3) Cancel(1);",
            "se (n > 3) Mensagem(Erro, \"x\");",
            "se (n > 3) Mensagem(refaz, \"x\");",
            "Verifica();",
        ] {
            let text = format!(
                "definir funcao Verifica();\n{}\nSaida:\nfuncao Verifica(); {{ }}",
                laco(corpo)
            );
            assert!(
                checks_all(&text).iter().all(|(k, _)| *k != EndlessLoop),
                "{corpo}"
            );
        }
        // Mensagem que só avisa não sai, e a condição escrita não é cobrada.
        assert_eq!(
            checks(&laco("Mensagem(Retorna, \"x\");")),
            [(EndlessLoop, "enquanto ()")]
        );
        assert!(checks("definir numero n;\nenquanto (n < 3) { n = n + 1; }").is_empty());
    }

    #[test]
    fn codigo_depois_de_pare_cancel_e_vapara() {
        // A regra conferida no ERP. Rodam o 2, o 4 e o 6, e o 3, o 5 e o 8 não.
        let text = "nI = 0;\n\
                    enquanto (nI < 3) {\n\
                    nI++;\n\
                    se (nI = 2) {\n\
                    Mensagem(Retorna, \"2\");\n\
                    Pare;\n\
                    Mensagem(Retorna, \"3\");\n\
                    }\n\
                    }\n\
                    Mensagem(Retorna, \"4\");\n\
                    vapara pulo;\n\
                    Mensagem(Retorna, \"5\");\n\
                    pulo:\n\
                    Mensagem(Retorna, \"6\");\n\
                    Cancel(1);\n\
                    Mensagem(Retorna, \"8\");";
        assert_eq!(
            checks(text),
            [
                (Unreachable, "Mensagem(Retorna, \"3\");"),
                (Unreachable, "Mensagem(Retorna, \"5\");"),
                (Unreachable, "Mensagem(Retorna, \"8\");"),
            ]
        );
    }

    #[test]
    fn faixa_do_codigo_inalcancavel_vai_ate_o_fim_do_bloco() {
        let text = "Cancel;\na = 1;\n@ comentário @\nb = 2;\nc = 3;";
        assert_eq!(
            checks(text),
            [(Unreachable, "a = 1;\n@ comentário @\nb = 2;\nc = 3;")]
        );
        // Depois do fim do bloco o código volta a executar.
        let text = "enquanto (a < 1) { Pare; x = 1; y = 2; }\nz = 3;";
        assert_eq!(checks(text), [(Unreachable, "x = 1; y = 2;")]);
    }

    #[test]
    fn rotulo_no_meio_volta_a_ser_alcancavel() {
        let text =
            "vapara saida;\nx = 1;\ny = 2;\nmeio:\nz = 3;\nCancel(2);\nw = 4;\nsaida:\nv = 5;";
        assert_eq!(
            checks(text),
            [(Unreachable, "x = 1;\ny = 2;"), (Unreachable, "w = 4;")]
        );
        // Rótulo logo depois do comando, sem nada entre os dois.
        assert!(checks("vapara saida;\nsaida:\nx = 1;").is_empty());
        // Rótulo dentro de um bloco inalcançável ainda pode ser alvo do VaPara de fora.
        assert!(checks("Cancel;\n{ x = 1; alvo: y = 2; }\nvapara alvo;").is_empty());
    }

    #[test]
    fn comando_dentro_do_se_nao_afeta_o_que_vem_depois() {
        let text = "se (a = 1) {\nCancel;\n}\nx = 1;\nse (b = 2) { Cancel(1); }\ny = 2;\n\
                    se (c = 3) { vapara saida; } senao { z = 3; }\nw = 4;\nsaida:";
        assert!(checks(text).is_empty());
        // Dentro do próprio bloco do se, o que vem depois do comando não executa.
        assert_eq!(
            checks("se (a = 1) { Cancel; x = 1; }\ny = 2;"),
            [(Unreachable, "x = 1;")]
        );
    }

    #[test]
    fn bloco_vazio_depois_do_comando() {
        assert!(checks("Cancel;").is_empty());
        assert!(checks("enquanto (a < 1) { Pare; }").is_empty());
        assert!(checks("Cancel(1);\n").is_empty());
        // Só definições depois do comando, que não executam.
        assert_eq!(
            checks("Cancel(1);\ndefinir alfa aX;\naX = \"\";"),
            [(Unreachable, "aX = \"\";")]
        );
        assert!(checks("vapara saida;\nsaida:").is_empty());
    }

    #[test]
    fn cancel_sem_parenteses_e_outros_comandos() {
        assert_eq!(checks("cancel;\nx = 1;"), [(Unreachable, "x = 1;")]);
        assert_eq!(checks("Cancel(1);\nx = 1;"), [(Unreachable, "x = 1;")]);
        // O ExecSql não desvia o fluxo para fora do bloco.
        assert!(checks("ExecSql \"UPDATE T SET A = 1\";\nx = 1;").is_empty());
    }

    #[test]
    fn continue_e_cancelar() {
        // A regra conferida no ERP. O B nunca entrou na mensagem, e o Cancelar terminou a regra
        // com e sem parênteses.
        assert_eq!(
            checks("enquanto (a < 3) { a = a + 1; Continue; b = 1; }"),
            [(Unreachable, "b = 1;")]
        );
        assert_eq!(checks("Cancelar(1);\nx = 1;"), [(Unreachable, "x = 1;")]);
        assert_eq!(checks("Cancelar;\nx = 1;"), [(Unreachable, "x = 1;")]);
        // Com valor sem parênteses o compilador diz que falta o ;.
        let tree = crate::parse("Cancelar 1;").unwrap();
        assert!(!tree.errors().is_empty());
        // O Continue não sai do laço sem fim, e o Cancelar sai.
        assert_eq!(
            checks("enquanto () { Continue; }"),
            [(EndlessLoop, "enquanto ()")]
        );
        assert!(checks("enquanto () { Cancelar; }").is_empty());
    }

    #[test]
    fn pare_e_continue_fora_de_laco_e_continue_no_para() {
        assert_eq!(checks("Pare;"), [(LoopClauseOutsideLoop, "Pare;")]);
        assert_eq!(
            checks("se (a = 1) { Continue; }"),
            [(LoopClauseOutsideLoop, "Continue;")]
        );
        // O laço de fora não vale dentro da função.
        let funcao = "Definir Funcao F();\nenquanto (a < 1) { F(); }\nfuncao F(); { Pare; }";
        assert_eq!(checks(funcao), [(LoopClauseOutsideLoop, "Pare;")]);
        // No para, o Continue volta sem rodar o incremento. No enquanto de dentro do para, não.
        assert_eq!(
            checks(
                "para (n = 1; n <= 3; n++) { se (n = 2) Continue; }\n\
                 para (n = 1; n <= 3; n++) { enquanto (a < 1) { Continue; } Pare; }"
            ),
            [(ContinueSkipsStep, "Continue;")]
        );
    }

    #[test]
    fn codigo_inalcancavel_dentro_de_funcao() {
        let text = "Definir Funcao F();\nF();\nfuncao F(); {\nCancel;\nx = 1;\n}";
        assert_eq!(checks(text), [(Unreachable, "x = 1;")]);
        // O comando antes de uma funcao não faz as funcoes seguintes inalcançáveis.
        let text = "Definir Funcao F();\nF();\nCancel(1);\nfuncao F(); {\nx = 1;\n}";
        assert!(checks(text).is_empty());
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
        // A coluna com apelido não vira campo do cursor, nem pelo apelido nem pelo nome dela.
        let text = "definir cursor c;\nc.SQL \"SELECT T.A, T.B X FROM T\";\nc.AbrirCursor();\n\
                    se (c.Achou) { y = c.a; y = c.X; y = c.B; y = c.Nao; c.Proximo(); }\n\
                    c.FecharCursor();";
        assert_eq!(
            checks(text),
            [
                (CursorFieldNotInSelect, "X"),
                (CursorFieldNotInSelect, "B"),
                (CursorFieldNotInSelect, "Nao")
            ]
        );
        // Com dois SQL, vale o campo de qualquer um.
        let dois = "definir cursor c;\nc.SQL \"SELECT A FROM T\";\nc.SQL \"SELECT B FROM T\";\n\
                    y = c.A + c.B;";
        assert!(checks(dois).is_empty());
        // SQL numa variável, ou * sem dicionário, deixam o cursor sem conferência.
        assert!(checks("definir cursor c;\nc.SQL aSql;\ny = c.Z;").is_empty());
        assert!(checks("definir cursor c;\nc.SQL \"SELECT * FROM T\";\ny = c.Z;").is_empty());
    }

    #[test]
    fn coluna_do_cursor_que_o_compilador_recusa() {
        let cursor = |sql: &str| format!("definir cursor c;\nc.SQL \"{sql}\";");
        // Com algo depois do espaço, o que vem antes precisa da tabela.
        assert_eq!(
            checks(&cursor("SELECT A X, DISTINCT T.B, T.C Y FROM T")),
            [
                (CursorColumnWithoutTable, "A"),
                (CursorColumnWithoutTable, "DISTINCT")
            ]
        );
        // A constante com apelido já sai como constante.
        assert_eq!(
            checks(&cursor("SELECT 'x' AS Z, T.A FROM T")),
            [(CursorConstantColumn, "\"SELECT 'x' AS Z, T.A FROM T\"")]
        );
        // O mesmo nome de outra tabela é recusado, e a mesma coluna de novo não.
        assert_eq!(
            checks(&cursor("SELECT T.A, B, U.A, T.A, A, T.B, U.B FROM T, U")),
            [(CursorRepeatedField, "U.A"), (CursorRepeatedField, "U.B")]
        );
    }

    #[test]
    fn parametro_do_cursor_que_ainda_nao_apareceu() {
        let text = "definir cursor c;\ndefinir numero nDef;\nxUsada = 1;\n\
                    definir funcao f(numero pPar);\n\
                    c.SQL \"SELECT A FROM T WHERE A = :nDef AND B = :XUSADA AND C = :pPar \\\n\
                    AND D = :yDepois AND E = ':zTexto' AND F = :yDepois AND G = :12\";\n\
                    yDepois = 2;\ndefinir numero zTexto;";
        assert_eq!(
            checks(text),
            [
                (CursorUnknownParameter, "yDepois"),
                (CursorUnknownParameter, "yDepois")
            ]
        );
        // Na tela e no relatório outra regra pode ter usado o nome antes.
        let tree = crate::parse(text).unwrap();
        let symbols = Symbols::build(&tree, text);
        for context in [Context::Sgi, Context::Report] {
            let found = validate(&tree, &symbols, text, context, &Inherited::default());
            assert!(
                found.iter().all(|c| c.kind != CursorUnknownParameter),
                "{context:?}"
            );
        }
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
