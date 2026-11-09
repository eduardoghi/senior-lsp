//! Erros de tipo entre alfa e número, pela tabela de símbolos.
//!
//! O tipo de um nome sai do `definir` dele, no arquivo ou numa regra acima no formulário SGI. Nome
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

use super::{Check, CheckKind, operator_token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    Alfa,
    Num,
    Unknown,
}

/// O que o validador de tipos consulta para saber o tipo de um nome. É a tabela do arquivo e, para
/// nome sem `definir` aqui, as variáveis das regras acima no formulário SGI.
pub(super) struct Scope<'a> {
    pub(super) symbols: &'a Symbols,
    pub(super) text: &'a str,
    pub(super) inherited: &'a Inherited,
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
        Literal | NameRef if leaf_type(tree, scope, node) == Ty::Num => {
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
        Literal | NameRef if leaf_type(tree, scope, node) == Ty::Alfa => {
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

/// Tipo de uma string, número ou nome, sem olhar expressões.
fn leaf_type(tree: &Tree, scope: &Scope, node: NodeId) -> Ty {
    match tree.kind(node) {
        Literal => match tree.tokens().kind(tree.token_range(node).start) {
            String => Ty::Alfa,
            Number => Ty::Num,
            _ => Ty::Unknown,
        },
        NameRef => name_type(tree, scope, node),
        _ => Ty::Unknown,
    }
}

/// Tipo de um nome, quando todas as definições que ele alcança concordam.
fn name_type(tree: &Tree, scope: &Scope, node: NodeId) -> Ty {
    let range = tree.text_range(node);
    let Some(reference) = scope.symbols.reference_at(range) else {
        return match scope.inherited.variable(&scope.text[range]).map(|v| v.kind) {
            Some(DefKind::Variable(Some(VarType::Alfa))) => Ty::Alfa,
            Some(DefKind::Variable(Some(_))) => Ty::Num,
            _ => Ty::Unknown,
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

    fn checks(text: &str) -> Vec<(CheckKind, &str)> {
        super::super::tests::checks(text)
    }

    const DEFS: &str = "definir alfa aTxt; definir alfa aB; definir numero n; definir data d;\n";

    fn com_defs(text: &str) -> std::string::String {
        format!("{DEFS}{text}")
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
