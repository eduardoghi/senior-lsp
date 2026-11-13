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
pub(crate) use sql::cursor_sql_literals;

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
    /// Instruções depois de `Pare`, `Cancel` ou `VaPara` no mesmo bloco, que nunca executam.
    Unreachable,
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
    let mut native_sql = sql::NativeSql::default();
    // Só na regra comum, porque na tela e no relatório as regras seguintes podem ler o cursor.
    let mut column_uses = (context == Context::Rule).then(sql::ColumnUses::default);
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
    native_sql.finish(&mut checks);
    if let Some(uses) = column_uses {
        uses.finish(text, &inherited.dictionaries, &mut checks);
    }
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
    // A função tem o `definir funcao` e a `funcao`, e qualquer chamada alcança as duas. O aviso
    // sai uma vez, na `funcao` quando ela existe.
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
    let is_variable = |def: &crate::symbols::Definition| {
        matches!(
            def.kind,
            DefKind::Variable(_)
                | DefKind::Cursor
                | DefKind::List
                | DefKind::Table
                | DefKind::WebService
        )
    };
    // Só os nomes sem uso direto precisam ser procurados nos textos, e a maioria das regras não
    // tem nenhum, então o texto das strings nem é lido.
    let mut wanted = rustc_hash::FxHashSet::default();
    for (i, def) in symbols.definitions.iter().enumerate() {
        if is_variable(def) && !used[i] {
            wanted.insert(text[def.name].to_ascii_uppercase());
        }
    }
    for (upper, defs) in &functions {
        if !defs.iter().any(|&i| used[i]) {
            wanted.insert(upper.clone());
        }
    }
    let in_sql = names_in_strings(tree, text, &wanted);
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
fn names_in_strings(
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
/// `Cancel` vale com e sem parênteses. `Continue` e `Cancelar` ainda não foram conferidos no
/// compilador e ficam de fora.
fn flow_stop(tree: &Tree, node: NodeId) -> Option<&'static str> {
    match tree.kind(node) {
        Break => Some("Pare"),
        Goto => Some("VaPara"),
        KeywordStmt if operator(tree, node) == Some(CancelKw) => Some("Cancel"),
        // O `Cancel(...)` é uma chamada, e o nome fica num `NameRef`.
        CallStmt => tree
            .children(node)
            .find(|&c| tree.kind(c) == CallExpr)
            .and_then(|call| tree.children(call).next())
            .filter(|&name| {
                tree.kind(name) == NameRef
                    && tree.tokens().kind(tree.token_range(name).start) == CancelKw
            })
            .map(|_| "Cancel"),
        _ => None,
    }
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

/// Instruções depois de `Pare`, `Cancel` ou `VaPara` no mesmo bloco, até um rótulo ou o fim do
/// bloco. O rótulo pode ser alvo de um `VaPara` e volta a tornar o código alcançável. Uma
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
        // A função de exemplo de muitos testes não é chamada, e os cursores de exemplo não têm
        // os campos lidos. Os testes da dica de função sem uso usam `checks_all`.
        .filter(|c| {
            !matches!(
                c.kind,
                CheckKind::UnusedFunction
                    | CheckKind::CursorColumnUnused
                    | CheckKind::CursorStarUnused
            )
        })
        .map(|c| (c.kind, &text[c.range]))
        .collect()
    }

    fn checks_all(text: &str) -> Vec<(CheckKind, &str)> {
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
        let text = "Pare;\na = 1;\n@ comentário @\nb = 2;\nc = 3;";
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
        assert!(checks("Pare;\n{ x = 1; alvo: y = 2; }\nvapara alvo;").is_empty());
    }

    #[test]
    fn comando_dentro_do_se_nao_afeta_o_que_vem_depois() {
        let text = "se (a = 1) {\nPare;\n}\nx = 1;\nse (b = 2) { Cancel(1); }\ny = 2;\n\
                    se (c = 3) { vapara saida; } senao { z = 3; }\nw = 4;\nsaida:";
        assert!(checks(text).is_empty());
        // Dentro do próprio bloco do se, o que vem depois do comando não executa.
        assert_eq!(
            checks("se (a = 1) { Pare; x = 1; }\ny = 2;"),
            [(Unreachable, "x = 1;")]
        );
    }

    #[test]
    fn bloco_vazio_depois_do_comando() {
        assert!(checks("Pare;").is_empty());
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
        // O retorna e o ExecSql não desviam o fluxo para fora do bloco nesta verificação, e o
        // Continue e o Cancelar não foram conferidos.
        assert!(checks("ExecSql \"UPDATE T SET A = 1\";\nx = 1;").is_empty());
        assert!(checks("enquanto (a < 1) { continue; x = 1; }").is_empty());
        assert!(checks("Cancelar(1);\nx = 1;").is_empty());
    }

    #[test]
    fn codigo_inalcancavel_dentro_de_funcao() {
        let text = "Definir Funcao F();\nF();\nfuncao F(); {\nPare;\nx = 1;\n}";
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
