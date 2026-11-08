//! Verificações sobre a árvore pronta.
//!
//! O parser aceita um superconjunto da linguagem. Aqui ficam as restrições do compilador que
//! dependem de onde uma construção aparece e os avisos de código que compila mas faz outra coisa
//! do que parece. Cada verificação foi conferida no compilador.

use text_size::TextRange;

use crate::SyntaxKind::{self, *};
use crate::symbols::Symbols;
use crate::{Element, NodeId, Tree};

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
}

impl CheckKind {
    /// Erro de compilação. As outras verificações são avisos.
    pub fn is_error(self) -> bool {
        matches!(
            self,
            CheckKind::MinusTakesOneToken | CheckKind::ComparisonInAssignment
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Check {
    pub kind: CheckKind,
    pub range: TextRange,
    /// Nome da função, nas verificações de função.
    pub function: Option<NodeId>,
}

pub fn validate(tree: &Tree, symbols: &Symbols) -> Vec<Check> {
    let mut checks = Vec::new();
    let mut stack = vec![tree.root()];
    while let Some(node) = stack.pop() {
        match tree.kind(node) {
            BinaryExpr => binary(tree, node, &mut checks),
            Assign => assignment(tree, node, &mut checks),
            FuncImpl => function(tree, node, &mut checks),
            _ => {}
        }
        let first = stack.len();
        stack.extend(tree.children(node));
        // Em ordem de texto, para os diagnósticos saírem na ordem do arquivo.
        stack[first..].reverse();
    }
    checks.extend(
        symbols
            .references
            .iter()
            .filter(|r| r.foreign)
            .map(|r| Check {
                kind: CheckKind::ForeignParameter,
                range: r.range,
                function: None,
            }),
    );
    checks.sort_by_key(|c| c.range.start());
    checks
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
                function: None,
            });
        }
    } else if matches!(op, EKw | OuKw) {
        checks.push(Check {
            kind: CheckKind::LogicalMinus,
            range: tree.text_range(rhs),
            function: None,
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
            function: None,
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
                function: Some(name),
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
            function: Some(name),
        });
    }
}

/// Primeiro token entre os filhos que não é trivia, que numa expressão é o operador.
fn operator_token(tree: &Tree, node: NodeId) -> Option<usize> {
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

    fn checks(text: &str) -> Vec<(CheckKind, &str)> {
        let tree = crate::parse(text).unwrap();
        assert!(tree.errors().is_empty(), "{text}");
        validate(&tree, &Symbols::build(&tree, text))
            .into_iter()
            .map(|c| (c.kind, &text[c.range]))
            .collect()
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
        assert!(checks("x = (a > 0); se (a > 1) { f(a > 1); } x++;").is_empty());
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
    fn parametro_fora_da_funcao() {
        let text = "definir funcao f(numero p);\ndefinir funcao g();\n\
                    funcao f(numero p); { }\nfuncao g(); { r = p; }";
        assert_eq!(checks(text), [(ForeignParameter, "p")]);
    }
}
