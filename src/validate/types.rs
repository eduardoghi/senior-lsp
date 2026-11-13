//! Erros de tipo entre alfa e número, pela tabela de símbolos.
//!
//! O tipo de um nome sai do `definir` dele, no arquivo ou numa regra anterior do formulário SGI. Nome
//! sem `definir` tem tipo desconhecido e nunca gera erro. O compilador cria esse nome como número,
//! mas antes pergunta ao ERP, que responde com variáveis próprias de cada lugar, como as `VS` do
//! identificador, os campos da tabela na tela SGI e o `ValStr` do relatório, e essa lista o servidor
//! não tem. Data conta como número. Dentro da função, um parâmetro alfa, com ou sem `end`, também se
//! comporta como número, e só na chamada ele é alfa.
//! O apóstrofo (`'c'`) vale como alfa na atribuição alfa e é o código do caractere numa conta,
//! então fica de fora.
//!
//! Só entram os casos conferidos no compilador. Na atribuição alfa o lado direito é uma lista de
//! operandos alfa ligados por `+`, e número, parênteses ou outro operador dão erro. Na atribuição
//! numérica, string ou variável alfa numa conta com `+ - * / e ou` dão erro, mas entre parênteses
//! ou depois de um `-` compilam (e valem 0). Comparação de alfa com número, `+` de alfa numa
//! comparação, duas strings comparadas e chamada de função com parâmetro de tipo ou número errado
//! também dão erro.

use crate::SyntaxKind::*;
use crate::builtins::{self, Context};
use crate::form::Inherited;
use crate::symbols::{DefKind, Symbols, VarType};
use crate::{NodeId, Tree};
use text_size::TextRange;

use super::{Check, CheckKind, operator_token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    Alfa,
    Num,
    Unknown,
}

/// O que o validador de tipos consulta para saber o tipo de um nome. É a tabela do arquivo e, para
/// nome sem `definir` aqui, as variáveis das regras anteriores do formulário SGI.
pub(super) struct Scope<'a> {
    pub(super) symbols: &'a Symbols,
    pub(super) text: &'a str,
    pub(super) inherited: &'a Inherited,
    pub(super) context: Context,
    /// Colunas do SQL dos cursores, para o tipo de `cur.CAMPO`.
    pub(super) sql: &'a super::sql::Sources,
}

pub(super) fn check(
    tree: &Tree,
    scope: &Scope,
    context: Context,
    node: NodeId,
    checks: &mut Vec<Check>,
) {
    match tree.kind(node) {
        Assign => {
            let mut children = tree.children(node);
            let (Some(lhs), Some(rhs)) = (children.next(), children.next()) else {
                return;
            };
            if tree.kind(lhs) != NameRef {
                return;
            }
            match name_type(tree, scope, lhs) {
                Ty::Alfa => alfa_rhs(tree, scope, rhs, checks),
                Ty::Num => numeric_rhs(tree, scope, rhs, checks),
                Ty::Unknown => false,
            };
        }
        BinaryExpr if is_relational(tree, node) => {
            let mut children = tree.children(node);
            let (Some(l), Some(r)) = (children.next(), children.next()) else {
                return;
            };
            let types = (leaf_type(tree, scope, l), leaf_type(tree, scope, r));
            if is_string(tree, l) && is_string(tree, r) {
                push(tree, checks, CheckKind::TwoStringLiterals, r, None);
            } else if matches!(types, (Ty::Alfa, Ty::Num) | (Ty::Num, Ty::Alfa)) {
                push(tree, checks, CheckKind::NumberAndAlfa, r, None);
            } else if let Some(op) =
                concatenation(tree, scope, l).or_else(|| concatenation(tree, scope, r))
            {
                checks.push(Check {
                    kind: CheckKind::AlfaOperatorInComparison,
                    range: tree.tokens().range(op),
                    name: None,
                    label: None,
                });
            }
        }
        CallExpr => call(tree, scope, context, node, checks),
        _ => {}
    }
}

/// Lado direito de atribuição alfa. Para no primeiro erro, como o compilador.
fn alfa_rhs(tree: &Tree, scope: &Scope, node: NodeId, checks: &mut Vec<Check>) -> bool {
    let mut push = |kind, range| {
        checks.push(Check {
            kind,
            range,
            name: None,
            label: None,
        });
        true
    };
    match tree.kind(node) {
        BinaryExpr => {
            let mut children = tree.children(node);
            let (Some(l), Some(r)) = (children.next(), children.next()) else {
                return false;
            };
            if alfa_rhs(tree, scope, l, checks) {
                return true;
            }
            let Some(op) = operator_token(tree, node) else {
                return false;
            };
            if tree.tokens().kind(op) == Plus {
                return alfa_rhs(tree, scope, r, checks);
            }
            checks.push(Check {
                kind: CheckKind::AlfaOperator,
                range: tree.tokens().range(op),
                name: None,
                label: None,
            });
            true
        }
        ParenExpr => {
            let first = tree.token_range(node).start;
            push(CheckKind::AlfaAndNumber, tree.tokens().range(first))
        }
        Literal | NameRef | FieldExpr if leaf_type(tree, scope, node) == Ty::Num => {
            push(CheckKind::AlfaAndNumber, tree.text_range(node))
        }
        _ => false,
    }
}

/// Lado direito de atribuição numérica, descendo só pelas contas com `+ - * /`.
fn numeric_rhs(tree: &Tree, scope: &Scope, node: NodeId, checks: &mut Vec<Check>) -> bool {
    match tree.kind(node) {
        BinaryExpr => {
            let arithmetic = operator_token(tree, node).is_some_and(|op| {
                matches!(
                    tree.tokens().kind(op),
                    Plus | Minus | Star | Slash | EKw | OuKw
                )
            });
            arithmetic
                && tree
                    .children(node)
                    .any(|c| numeric_rhs(tree, scope, c, checks))
        }
        Literal | NameRef | FieldExpr if leaf_type(tree, scope, node) == Ty::Alfa => {
            push(tree, checks, CheckKind::NumberAndAlfa, node, None);
            true
        }
        _ => false,
    }
}

/// Chamada de função do arquivo, contra os parâmetros do `definir funcao`, ou da `funcao` se
/// não houver `definir funcao`.
fn call(tree: &Tree, scope: &Scope, context: Context, node: NodeId, checks: &mut Vec<Check>) {
    let mut children = tree.children(node);
    let (Some(callee), Some(args)) = (children.next(), children.next()) else {
        return;
    };
    if tree.kind(callee) != NameRef {
        return;
    }
    let name = &scope.text[tree.text_range(callee)];
    // Num modelo de relatório, uma função definida em mais de uma regra deixa em dúvida qual
    // assinatura vale, então a chamada não é conferida.
    if context == Context::Report && scope.inherited.get(name).is_some_and(|d| d.rules > 1) {
        return;
    }
    let Some(reference) = scope.symbols.reference_at(tree.text_range(callee)) else {
        if let Some(builtin) = builtins::lookup(name, context) {
            external_call(tree, scope, &builtin.params, callee, args, checks);
        } else if let Some(function) = scope.inherited.get(name) {
            external_call(tree, scope, &function.params, callee, args, checks);
        }
        return;
    };
    let defs = || {
        reference
            .targets
            .iter()
            .map(|&t| &scope.symbols.definitions[t])
    };
    if !defs().all(|d| d.kind == DefKind::Function) {
        return;
    }
    // Com `definir funcao` e `funcao` em número diferente de parâmetros, não se sabe contra qual
    // o compilador confere, então a chamada fica sem conferência.
    let counts: Vec<usize> = defs()
        .filter_map(|d| tree.children(d.node).find(|&c| tree.kind(c) == ParamList))
        .map(|list| tree.children(list).count())
        .collect();
    if counts.windows(2).any(|w| w[0] != w[1]) {
        return;
    }
    let def = defs()
        .find(|d| tree.kind(d.node) == FuncDecl)
        .or_else(|| defs().next());
    let Some(list) = def.and_then(|d| tree.children(d.node).find(|&c| tree.kind(c) == ParamList))
    else {
        return;
    };
    let params: Vec<NodeId> = tree.children(list).collect();
    let args: Vec<NodeId> = tree.children(args).collect();
    if args.len() > params.len() {
        push(tree, checks, CheckKind::TooManyArguments, callee, None);
        return;
    }
    if args.len() < params.len() {
        push(tree, checks, CheckKind::TooFewArguments, callee, None);
        return;
    }
    for (&arg, &param) in args.iter().zip(&params) {
        let expected = scope.symbols.definitions.iter().find_map(|d| match d.kind {
            DefKind::Parameter(Some(t)) if d.node == param => Some(t),
            _ => None,
        });
        let by_ref = tree
            .elements(param)
            .any(|e| matches!(e, crate::Element::Token(t) if tree.tokens().kind(t) == EndKw));
        let kind = match (expected, leaf_type(tree, scope, arg)) {
            (Some(VarType::Numero | VarType::Data), Ty::Alfa) => CheckKind::AlfaArgument,
            (Some(VarType::Alfa), Ty::Num) => CheckKind::NumberArgument,
            _ if by_ref && tree.kind(arg) == Literal => CheckKind::ConstantInEndParameter,
            _ => continue,
        };
        let name = tree.children(param).find(|&c| tree.kind(c) == Name);
        push(tree, checks, kind, arg, name);
        return;
    }
}

/// `+` entre dois operandos alfa, que não vale dentro de comparação.
fn concatenation(tree: &Tree, scope: &Scope, node: NodeId) -> Option<usize> {
    if tree.kind(node) != BinaryExpr {
        return None;
    }
    let op = operator_token(tree, node).filter(|&op| tree.tokens().kind(op) == Plus)?;
    tree.children(node)
        .all(|c| leaf_type(tree, scope, c) == Ty::Alfa)
        .then_some(op)
}

/// Chamada de função de fora do arquivo, do ERP ou de outra regra do formulário SGI.
fn external_call(
    tree: &Tree,
    scope: &Scope,
    params: &[builtins::Param],
    callee: NodeId,
    args: NodeId,
    checks: &mut Vec<Check>,
) {
    let args: Vec<NodeId> = tree.children(args).collect();
    if args.len() != params.len() {
        let kind = if args.len() > params.len() {
            CheckKind::TooManyArguments
        } else {
            CheckKind::TooFewArguments
        };
        push(tree, checks, kind, callee, None);
        return;
    }
    for (&arg, param) in args.iter().zip(params) {
        let kind = match (param.ty, leaf_type(tree, scope, arg)) {
            (VarType::Numero | VarType::Data, Ty::Alfa) => CheckKind::AlfaArgument,
            (VarType::Alfa, Ty::Num) => CheckKind::NumberArgument,
            // O parâmetro `end` devolve o valor, então o compilador exige uma variável.
            _ if param.by_ref && tree.kind(arg) == Literal => CheckKind::ConstantInEndParameter,
            _ => continue,
        };
        checks.push(Check {
            kind,
            range: tree.text_range(arg),
            name: None,
            label: Some(param.name.clone()),
        });
        return;
    }
}

fn push(tree: &Tree, checks: &mut Vec<Check>, kind: CheckKind, node: NodeId, name: Option<NodeId>) {
    checks.push(Check {
        kind,
        range: tree.text_range(node),
        name,
        label: None,
    });
}

/// Tipo de uma expressão que pode ir sozinha no lado direito de uma atribuição, ou `None` quando
/// ela não pode ou o tipo de alguma parte não é conhecido.
///
/// A atribuição alfa só aceita textos, variáveis alfa e apóstrofos somados com `+`, sem
/// parênteses. A numérica aceita contas, com parênteses e sinal, e não aceita comparação nem `e`
/// e `ou` soltos. Data conta como número, que é como o compilador trata.
pub(super) fn assignable(tree: &Tree, scope: &Scope, node: NodeId) -> Option<VarType> {
    #[derive(Clone, Copy, PartialEq)]
    enum Part {
        Alfa,
        Num,
        /// Apóstrofo, que vale como texto numa soma de textos e como número numa conta.
        Char,
    }
    fn part(tree: &Tree, scope: &Scope, node: NodeId) -> Option<Part> {
        let numeric = |p: Part| (p != Part::Alfa).then_some(Part::Num);
        match tree.kind(node) {
            Literal => match tree.tokens().kind(tree.token_range(node).start) {
                String => Some(Part::Alfa),
                Number => Some(Part::Num),
                Char => Some(Part::Char),
                _ => None,
            },
            NameRef | FieldExpr => match leaf_type(tree, scope, node) {
                Ty::Alfa => Some(Part::Alfa),
                Ty::Num => Some(Part::Num),
                Ty::Unknown => None,
            },
            ParenExpr | PrefixExpr => numeric(part(tree, scope, tree.children(node).next()?)?),
            BinaryExpr => {
                let op = tree.tokens().kind(operator_token(tree, node)?);
                let mut children = tree.children(node);
                let left = part(tree, scope, children.next()?)?;
                let right = part(tree, scope, children.next()?)?;
                match op {
                    Plus if left == Part::Alfa || right == Part::Alfa => {
                        (left != Part::Num && right != Part::Num).then_some(Part::Alfa)
                    }
                    Plus | Minus | Star | Slash => numeric(left).and(numeric(right)),
                    _ => None,
                }
            }
            _ => None,
        }
    }
    match part(tree, scope, node)? {
        Part::Alfa => Some(VarType::Alfa),
        Part::Num => Some(VarType::Numero),
        Part::Char => None,
    }
}

/// Tipo de uma string, número ou nome, sem olhar expressões.
fn leaf_type(tree: &Tree, scope: &Scope, node: NodeId) -> Ty {
    match tree.kind(node) {
        Literal => match tree.tokens().kind(tree.token_range(node).start) {
            String => Ty::Alfa,
            Number => Ty::Num,
            _ => Ty::Unknown,
        },
        NameRef => name_type(tree, scope, node),
        FieldExpr => {
            let ty = match table_field(tree, scope, node) {
                Some((found, _)) => found.field.ty.var_type(),
                None => scope
                    .sql
                    .cursor_field_type(tree, scope.symbols, scope.text, node),
            };
            match ty {
                Some(VarType::Alfa) => Ty::Alfa,
                Some(_) => Ty::Num,
                None => Ty::Unknown,
            }
        }
        _ => Ty::Unknown,
    }
}

/// Campo de tabela do dicionário em `TABELA.Campo`, com o trecho do nome da tabela. A base não
/// pode ser uma variável, como cursor, lista ou tabela definidos na regra, porque aí o campo vem
/// do SQL ou da definição dela.
pub(super) fn table_field<'a>(
    tree: &Tree,
    scope: &Scope<'a>,
    node: NodeId,
) -> Option<(crate::dictionary::Found<'a>, TextRange)> {
    let inherited: &'a Inherited = scope.inherited;
    if inherited.dictionaries.is_empty() {
        return None;
    }
    let mut children = tree.children(node);
    let (base, field) = (children.next()?, children.next()?);
    if tree.kind(base) != NameRef || tree.kind(field) != NameRef {
        return None;
    }
    let range = tree.text_range(base);
    if scope.symbols.reference_at(range).is_some() {
        return None;
    }
    let table = &scope.text[range];
    if inherited.variable(table).is_some() {
        return None;
    }
    let found = inherited
        .dictionaries
        .field(table, &scope.text[tree.text_range(field)])?;
    Some((found, range))
}

/// Tipo de um nome, quando todas as definições que ele alcança concordam.
fn name_type(tree: &Tree, scope: &Scope, node: NodeId) -> Ty {
    let range = tree.text_range(node);
    let Some(reference) = scope.symbols.reference_at(range) else {
        // `ValStr` e `ValRet` são palavras reservadas, alfa e número para o compilador.
        match tree.tokens().kind(tree.token_range(node).start) {
            ValStrKw => return Ty::Alfa,
            ValRetKw => return Ty::Num,
            _ => {}
        }
        return match scope.inherited.variable(&scope.text[range]).map(|v| v.kind) {
            Some(DefKind::Variable(Some(VarType::Alfa))) => Ty::Alfa,
            Some(DefKind::Variable(Some(_))) => Ty::Num,
            Some(_) => Ty::Unknown,
            None => match builtins::system_variable(&scope.text[range], scope.context) {
                Some(VarType::Alfa) => Ty::Alfa,
                Some(_) => Ty::Num,
                // Na regra de identificador o ERP acrescenta variáveis próprias de cada
                // identificador, que o servidor não conhece.
                None if scope.context == Context::Rule => Ty::Unknown,
                None => Ty::Num,
            },
        };
    };
    let mut ty = Ty::Unknown;
    for &t in &reference.targets {
        let this = match scope.symbols.definitions[t].kind {
            DefKind::Variable(Some(VarType::Alfa)) => Ty::Alfa,
            DefKind::Variable(Some(_)) | DefKind::Parameter(Some(_)) => Ty::Num,
            _ => return Ty::Unknown,
        };
        if ty != Ty::Unknown && ty != this {
            return Ty::Unknown;
        }
        ty = this;
    }
    ty
}

fn is_string(tree: &Tree, node: NodeId) -> bool {
    tree.kind(node) == Literal && tree.tokens().kind(tree.token_range(node).start) == String
}

fn is_relational(tree: &Tree, node: NodeId) -> bool {
    operator_token(tree, node).is_some_and(|op| {
        matches!(
            tree.tokens().kind(op),
            Eq | Lt | Gt | LtEq | GtEq | Neq | RelRun
        )
    })
}

#[cfg(test)]
mod tests {
    use crate::validate::CheckKind::{self, *};

    /// Sem o aviso de definição sem uso, porque os exemplos definem mais do que usam.
    fn checks(text: &str) -> Vec<(CheckKind, &str)> {
        super::super::tests::checks(text)
            .into_iter()
            .filter(|(kind, _)| *kind != CheckKind::Unused)
            .collect()
    }

    const DEFS: &str = "definir alfa aTxt; definir alfa aB; definir numero n; definir data d;\n";

    fn com_defs(text: &str) -> std::string::String {
        format!("{DEFS}{text}")
    }

    /// Dicionário com `E070EMP.CodEmp` número e `E070EMP.NomEmp` alfa, e `E070EMP.USU_Nova` só
    /// no de teste.
    fn dicionarios() -> crate::dictionary::Dictionaries {
        use crate::tbs::{Dictionary, Field, FieldKind, FieldType, Table};
        let field = |name: &str, kind| Field {
            name: name.to_owned(),
            ty: FieldType {
                kind,
                size: 4,
                decimals: 0,
                enumeration: None,
                base: None,
            },
            ..Field::default()
        };
        let table = |fields| Table {
            name: "E070EMP".to_owned(),
            description: std::string::String::new(),
            fields,
            ..Table::default()
        };
        let prod = vec![
            field("CodEmp", FieldKind::Number),
            field("NomEmp", FieldKind::Text),
        ];
        let mut test = prod.clone();
        test.push(field("USU_Nova", FieldKind::Text));
        crate::dictionary::Dictionaries {
            production: Some(std::sync::Arc::new(Dictionary {
                tables: vec![table(prod)],
                ..Dictionary::default()
            })),
            test: Some(std::sync::Arc::new(Dictionary {
                tables: vec![table(test)],
                ..Dictionary::default()
            })),
        }
    }

    fn checks_com_dicionario(text: &str) -> Vec<(CheckKind, &str)> {
        let tree = crate::parse(text).unwrap();
        let symbols = crate::symbols::Symbols::build(&tree, text);
        let mut inherited = crate::form::Inherited::default();
        inherited.dictionaries = dicionarios();
        crate::validate::validate(
            &tree,
            &symbols,
            text,
            crate::builtins::Context::Report,
            &inherited,
        )
        .into_iter()
        .map(|c| (c.kind, &text[c.range]))
        .collect()
    }

    #[test]
    fn campo_de_cursor_pelo_sql_e_pelo_dicionario() {
        let cursor = |sql: &str, uso: &str| {
            format!(
                "definir alfa aTxt; definir numero n; definir cursor c;\nc.SQL \"{sql}\";\n{uso}"
            )
        };
        // Com e sem a tabela na frente e pelo *, o tipo vem do dicionário.
        for sql in [
            "SELECT E070EMP.CodEmp, E070EMP.NomEmp FROM E070EMP",
            "SELECT CodEmp, NomEmp FROM E070EMP",
            "SELECT * FROM E070EMP",
        ] {
            let ok = cursor(sql, "aTxt = c.NomEmp; n = c.CodEmp;");
            assert!(checks_com_dicionario(&ok).is_empty(), "{sql}");
            let erro = cursor(sql, "aTxt = c.CodEmp; n = c.NomEmp;");
            assert_eq!(
                checks_com_dicionario(&erro),
                [(AlfaAndNumber, "c.CodEmp"), (NumberAndAlfa, "c.NomEmp")],
                "{sql}"
            );
        }
        // A coluna sem tabela é da primeira tabela do FROM, e o * também.
        let duas = cursor(
            "SELECT CodEmp, T.NomEmp FROM E070EMP, T",
            "aTxt = c.CodEmp;",
        );
        assert_eq!(
            checks_com_dicionario(&duas),
            [(CursorUnknownTable, "T"), (AlfaAndNumber, "c.CodEmp")]
        );
        // O * junto com o mesmo campo de outra tabela repete o nome.
        assert_eq!(
            checks_com_dicionario(&cursor(
                "SELECT *, T.CodEmp FROM E070EMP, T",
                "n = c.CodEmp;"
            )),
            [(CursorUnknownTable, "T"), (CursorRepeatedField, "T.CodEmp")]
        );
        // Campo fora do SELECT, mesmo existindo na tabela.
        let fora = cursor("SELECT CodEmp FROM E070EMP", "aTxt = c.NomEmp;");
        assert_eq!(
            checks_com_dicionario(&fora),
            [(CursorFieldNotInSelect, "NomEmp")]
        );
    }

    #[test]
    fn colunas_do_cursor_pelo_dicionario() {
        let cursor = |sql: &str| format!("definir cursor c;\nc.SQL \"{sql}\";");
        let checks = |sql: &str| -> Vec<(CheckKind, std::string::String)> {
            checks_com_dicionario(&cursor(sql))
                .into_iter()
                .map(|(kind, text)| (kind, text.to_owned()))
                .collect()
        };
        let um = |kind, text: &str| vec![(kind, text.to_owned())];
        // O que vem antes do ponto é lido como tabela, e apelido e função não são.
        assert_eq!(
            checks("SELECT E.CodEmp, E.NomEmp FROM E070EMP E"),
            um(CursorUnknownTable, "E")
        );
        assert_eq!(
            checks("SELECT MAX(E070EMP.CodEmp) FROM E070EMP"),
            um(CursorUnknownTable, "MAX(E070EMP")
        );
        // A coluna sem tabela é procurada na primeira tabela do FROM, e o erro fica nela.
        assert_eq!(
            checks("SELECT CodEmp FROM E999XXX, E070EMP WHERE 1 = 1"),
            um(CursorUnknownTable, "E999XXX")
        );
        assert_eq!(
            checks("SELECT CodEmp, Nome FROM E070EMP"),
            um(CursorUnknownField, "Nome")
        );
        assert_eq!(
            checks("select e070emp.codemp, e070emp.nome x from e070emp"),
            um(CursorUnknownField, "e070emp.nome")
        );
        assert_eq!(
            checks("SELECT COUNT(*) FROM E070EMP"),
            um(CursorUnknownField, "COUNT(*)")
        );
        assert_eq!(
            checks("SELECT USU_Nova FROM E070EMP"),
            um(OnlyInTest, "USU_Nova")
        );
        // A posição vale com o SQL emendado em várias linhas.
        assert_eq!(
            checks("SELECT CodEmp, \\\n   E070EMP.Nome \\\n FROM E070EMP"),
            um(CursorUnknownField, "E070EMP.Nome")
        );
        // O compilador não confere o *, o WHERE, o ORDER BY nem o que vem depois do espaço.
        for sql in [
            "SELECT * FROM E999XXX",
            "SELECT CodEmp FROM E070EMP WHERE Nada = 1 AND X.Y = 2 ORDER BY Outro",
            "SELECT E070EMP.CodEmp Apelido FROM E070EMP",
            "SELECT CodEmp FROM E070EMP INNER JOIN E999XXX ON 1 = 1",
        ] {
            assert!(checks(sql).is_empty(), "{sql}");
        }
        // Sem dicionário não há o que conferir.
        assert!(super::super::tests::checks(&cursor("SELECT X.Y, Z FROM W")).is_empty());
    }

    #[test]
    fn colunas_do_cursor_que_a_regra_nao_le() {
        let regra = |sql: &str, uso: &str| {
            format!(
                "definir cursor c; definir numero n; definir alfa a;\nc.SQL \"{sql}\";\n\
                 c.AbrirCursor();\n{uso}\nc.FecharCursor();"
            )
        };
        let sem_uso = |text: &str, context| -> Vec<(CheckKind, std::string::String)> {
            let tree = crate::parse(text).unwrap();
            let symbols = crate::symbols::Symbols::build(&tree, text);
            let mut inherited = crate::form::Inherited::default();
            inherited.dictionaries = dicionarios();
            crate::validate::validate(&tree, &symbols, text, context, &inherited)
                .into_iter()
                .filter(|c| matches!(c.kind, CursorColumnUnused | CursorStarUnused))
                .map(|c| (c.kind, text[c.range].to_owned()))
                .collect()
        };
        let rule = crate::builtins::Context::Rule;
        let checks = |sql: &str, uso: &str| sem_uso(&regra(sql, uso), rule);
        let um = |kind, text: &str| vec![(kind, text.to_owned())];

        let duas = "SELECT CodEmp, E070EMP.NomEmp FROM E070EMP";
        assert_eq!(
            checks(duas, "n = c.CodEmp;"),
            um(CursorColumnUnused, "E070EMP.NomEmp")
        );
        assert!(checks(duas, "n = c.codemp; a = c.NOMEMP;").is_empty());
        // O nome do campo num texto não é leitura.
        assert_eq!(
            checks(duas, "n = c.CodEmp; a = \"usa c.NomEmp\";"),
            um(CursorColumnUnused, "E070EMP.NomEmp")
        );
        // O campo do ORDER BY fica, porque com o Senior SQL 2 ele tem que estar no SELECT.
        for ordem in ["NomEmp", "E070EMP.NOMEMP DESC, CodEmp"] {
            let sql = format!("{duas} ORDER BY {ordem}");
            assert!(checks(&sql, "n = c.CodEmp;").is_empty(), "{sql}");
        }
        assert_eq!(
            checks(&format!("{duas} ORDER BY NomEmpresa"), "n = c.CodEmp;"),
            um(CursorColumnUnused, "E070EMP.NomEmp")
        );
        // Sem nenhuma leitura, a primeira coluna fica, porque o SELECT precisa de uma.
        let so_achou = "se (c.Achou) { n = 1; }";
        assert!(checks("SELECT CodEmp FROM E070EMP", so_achou).is_empty());
        assert_eq!(
            checks(duas, so_achou),
            um(CursorColumnUnused, "E070EMP.NomEmp")
        );
        // O * conta os campos da tabela no dicionário.
        let tudo = "SELECT * FROM E070EMP";
        assert_eq!(checks(tudo, "n = c.CodEmp;"), um(CursorStarUnused, "*"));
        assert_eq!(checks(tudo, so_achou), um(CursorStarUnused, "*"));
        assert!(checks(tudo, "n = c.CodEmp; a = c.NomEmp;").is_empty());
        assert!(checks("SELECT * FROM E999XXX", "n = c.CodEmp;").is_empty());
        // A coluna com apelido não vira campo do cursor e fica de fora.
        assert!(
            checks(
                "SELECT CodEmp, E070EMP.NomEmp Nome FROM E070EMP",
                "n = c.CodEmp;"
            )
            .is_empty()
        );
        // Na tela e no relatório outra regra pode ler o cursor.
        for context in [
            crate::builtins::Context::Sgi,
            crate::builtins::Context::Report,
        ] {
            assert!(sem_uso(&regra(duas, "n = c.CodEmp;"), context).is_empty());
        }
    }

    #[test]
    fn senior_sql_2_so_vale_na_regra_que_o_usa() {
        let text = "definir cursor c;\nc.SQL \"SELECT CodEmp FROM E070EMP, E999XXX \\\n\
                    WHERE Nada = 1 ORDER BY NomEmp\";";
        let checks = |senior_sql2| -> Vec<(CheckKind, std::string::String)> {
            let tree = crate::parse(text).unwrap();
            let symbols = crate::symbols::Symbols::build(&tree, text);
            let mut inherited = crate::form::Inherited::default();
            inherited.dictionaries = dicionarios();
            inherited.senior_sql2 = senior_sql2;
            let context = crate::builtins::Context::Report;
            crate::validate::validate(&tree, &symbols, text, context, &inherited)
                .into_iter()
                .map(|c| (c.kind, text[c.range].to_owned()))
                .collect()
        };
        assert!(checks(None).is_empty());
        assert!(checks(Some(false)).is_empty());
        // O nome sem tabela não é cobrado enquanto uma tabela do FROM é desconhecida.
        assert_eq!(
            checks(Some(true)),
            [
                (Sql2UnknownTable, "E999XXX".to_owned()),
                (Sql2OrderBy, "NomEmp".to_owned())
            ]
        );
        // No ExecSql o SELECT tem as colunas conferidas, e o ORDER BY não.
        let exec = "ExecSql \"SELECT Nada FROM E070EMP ORDER BY NomEmp\";";
        let tree = crate::parse(exec).unwrap();
        let symbols = crate::symbols::Symbols::build(&tree, exec);
        let mut inherited = crate::form::Inherited::default();
        inherited.dictionaries = dicionarios();
        inherited.senior_sql2 = Some(true);
        let context = crate::builtins::Context::Report;
        let found: Vec<_> = crate::validate::validate(&tree, &symbols, exec, context, &inherited)
            .into_iter()
            .map(|c| (c.kind, &exec[c.range]))
            .collect();
        assert_eq!(found, [(Sql2UnknownSymbol, "Nada")]);
    }

    #[test]
    fn execsql_com_e_sem_senior_sql_2() {
        let text = "definir numero nErr; definir alfa aMsg;\n\
                    ExecSql \"UPDATE E070EMP SET Nada = :nErr WHERE CodEmp = :nDepois\";\n\
                    ExecSqlEx(\"DELETE FROM E999XXX WHERE A = :nSolto\", nErr, aMsg);\n\
                    ExecSql \"SELECT Nada FROM E999XXX\";\nExecSql aMsg;\nnDepois = 1;";
        let checks = |senior_sql2, context| -> Vec<(CheckKind, std::string::String)> {
            let tree = crate::parse(text).unwrap();
            let symbols = crate::symbols::Symbols::build(&tree, text);
            let mut inherited = crate::form::Inherited::default();
            inherited.dictionaries = dicionarios();
            inherited.senior_sql2 = senior_sql2;
            crate::validate::validate(&tree, &symbols, text, context, &inherited)
                .into_iter()
                .filter(|c| c.kind != Unused)
                .map(|c| (c.kind, text[c.range].to_owned()))
                .collect()
        };
        let (rule, sgi) = (
            crate::builtins::Context::Rule,
            crate::builtins::Context::Sgi,
        );
        // No SQL nativo só o parâmetro do ExecSql é conferido, e só na regra comum.
        let parametro = vec![(CursorUnknownParameter, "nDepois".to_owned())];
        assert_eq!(checks(None, rule), parametro);
        assert_eq!(checks(Some(false), rule), parametro);
        assert!(checks(Some(false), sgi).is_empty());
        // Com o Senior SQL 2 o comando dos dois é lido, e o SELECT também.
        assert_eq!(
            checks(Some(true), sgi),
            [
                (Sql2UnknownField, "Nada".to_owned()),
                (Sql2UnknownTable, "E999XXX".to_owned()),
                (Sql2UnknownTable, "E999XXX".to_owned())
            ]
        );
    }

    #[test]
    fn campo_de_tabela_pelo_dicionario() {
        let ok = com_defs("aTxt = e070emp.NOMEMP; n = E070EMP.CodEmp + 1;");
        assert!(checks_com_dicionario(&ok).is_empty());
        let erro = com_defs("aTxt = E070EMP.CodEmp;");
        assert_eq!(
            checks_com_dicionario(&erro),
            [(AlfaAndNumber, "E070EMP.CodEmp")]
        );
        // Cursor definido na regra tem os campos do SQL, não do dicionário.
        let cursor = "definir alfa aTxt; definir cursor E070EMP;\naTxt = E070EMP.CodEmp;";
        assert!(checks_com_dicionario(cursor).is_empty());
        let teste = com_defs("aTxt = E070EMP.USU_Nova;");
        assert_eq!(
            checks_com_dicionario(&teste),
            [(OnlyInTest, "E070EMP.USU_Nova")]
        );
    }

    #[test]
    fn atribuicao_alfa() {
        let ok = com_defs("aTxt = \"x\" + aB + 'C'; aTxt = aB; aTxt = aSemDefinir + \"x\";");
        assert!(checks(&ok).is_empty());
        let casos = [
            ("aTxt = \"x\" + n;", AlfaAndNumber, "n"),
            ("aTxt = n;", AlfaAndNumber, "n"),
            ("aTxt = 5;", AlfaAndNumber, "5"),
            ("aTxt = d;", AlfaAndNumber, "d"),
            ("aTxt = (\"a\" + aB);", AlfaAndNumber, "("),
            ("aTxt = aB - \"x\";", AlfaOperator, "-"),
        ];
        for (text, kind, onde) in casos {
            let text = com_defs(text);
            assert_eq!(checks(&text), [(kind, onde)], "{text}");
        }
    }

    #[test]
    fn atribuicao_numerica() {
        assert!(
            checks(&com_defs(
                "n = d; n = 1 + n * 2; n = (aTxt = aB); n = (aTxt); n = -aTxt;"
            ))
            .is_empty()
        );
        let casos = [
            ("n = aTxt;", "aTxt"),
            ("n = 1 + aTxt;", "aTxt"),
            ("n = \"5\";", "\"5\""),
            ("n = 1 e aTxt;", "aTxt"),
            ("n = 1 ou aTxt;", "aTxt"),
        ];
        for (text, onde) in casos {
            let text = com_defs(text);
            assert_eq!(checks(&text), [(NumberAndAlfa, onde)], "{text}");
        }
    }

    #[test]
    fn comparacoes() {
        assert!(checks(&com_defs("se (aTxt = aB) { n = 1; }")).is_empty());
        let casos = [
            ("se (aTxt = 1) { n = 1; }", "1"),
            ("se (1 = aTxt) { n = 1; }", "aTxt"),
            ("se (n = aTxt) { n = 1; }", "aTxt"),
            ("se (aTxt = d) { n = 1; }", "d"),
        ];
        for (text, onde) in casos {
            let text = com_defs(text);
            assert_eq!(checks(&text), [(NumberAndAlfa, onde)], "{text}");
        }
        assert!(
            checks(&com_defs(
                "se (\"a\" = aTxt) { n = 1; } se (aTxt = 'A') { n = 1; }"
            ))
            .is_empty()
        );
        assert_eq!(
            checks(&com_defs("se (aTxt + \"x\" = aB) { n = 1; }")),
            [(AlfaOperatorInComparison, "+")]
        );
        assert_eq!(
            checks(&com_defs("se (\"a\" = \"b\") { n = 1; }")),
            [(TwoStringLiterals, "\"b\"")]
        );
    }

    #[test]
    fn chamada_de_funcao() {
        let base = "definir alfa aTxt; definir funcao f(numero p);\n";
        let tail = "\nfuncao f(numero p); { }";
        let caso = |chamada: &str| format!("{base}{chamada}{tail}");
        assert!(checks(&caso("f(1); f(aSemDefinir);")).is_empty());
        assert_eq!(checks(&caso("f(\"a\");")), [(AlfaArgument, "\"a\"")]);
        assert_eq!(checks(&caso("f(aTxt);")), [(AlfaArgument, "aTxt")]);
        assert_eq!(checks(&caso("f(1, 2);")), [(TooManyArguments, "f")]);
        assert_eq!(checks(&caso("f();")), [(TooFewArguments, "f")]);
    }

    #[test]
    fn declaracao_e_implementacao_diferentes_ficam_sem_conferencia() {
        let text =
            "definir funcao f(numero a);\nf(1, 2, 3);\nfuncao f(numero a, numero b, numero c); { }";
        assert!(checks(text).is_empty());
    }

    #[test]
    fn constante_no_parametro_end() {
        let text = "definir alfa a;\nAgluCtbContas(1, a); AgluCtbContas(1, \"x\");";
        assert_eq!(checks(text), [(ConstantInEndParameter, "\"x\"")]);
        let text = "definir numero x; definir funcao Dobra(numero n, numero end r);\n\
                    Dobra(5, x); Dobra(5, 3);\n\
                    funcao Dobra(numero n, numero end r); { r = n * 2; }";
        assert_eq!(checks(text), [(ConstantInEndParameter, "3")]);
    }

    #[test]
    fn parametro_alfa() {
        let text = "definir alfa aB; definir numero n; definir funcao g(alfa end pa);\n\
                    g(5); g(n); g(aB);\n\
                    funcao g(alfa end pa); { aB = pa; se (pa = \"a\") { n = 1; } n = pa; }";
        assert_eq!(
            checks(text),
            [
                (NumberArgument, "5"),
                (NumberArgument, "n"),
                (AlfaAndNumber, "pa"),
                (NumberAndAlfa, "\"a\""),
            ]
        );
    }
}
