//! Parser de descida recursiva que monta a árvore.
//!
//! O parser aceita um superconjunto da linguagem. Ele monta a mesma árvore para qualquer tipo de
//! nome, e as restrições do compilador que dependem de onde a expressão aparece (relacional no
//! lado direito da atribuição, `-` logo depois de `e`) ficam para um validador que olha a árvore.

// Por enquanto só os testes chamam o parser. Sai quando os comandos entrarem.
#![allow(dead_code)]

use crate::SyntaxKind::{self, *};
use crate::tree::{Builder, ParseErrorKind, Tree};

pub(crate) struct Parser {
    b: Builder,
}

impl Parser {
    pub(crate) fn new(b: Builder) -> Parser {
        Parser { b }
    }

    pub(crate) fn finish(self) -> Tree {
        self.b.finish()
    }

    fn current(&self) -> SyntaxKind {
        self.b.current()
    }

    fn at(&self, kind: SyntaxKind) -> bool {
        self.current() == kind
    }

    fn bump(&mut self) {
        self.b.bump();
    }

    fn error(&mut self, kind: ParseErrorKind) {
        let range = self.b.current_range();
        self.b.error(kind, range);
    }

    /// Consome `kind` se for o token atual, senão registra que ele faltou.
    fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            self.error(ParseErrorKind::Expected(kind));
            false
        }
    }

    /// Expressão completa, com no máximo um relacional.
    pub(crate) fn expr(&mut self) {
        let start = self.b.checkpoint();
        self.additive();
        if !is_relational(self.current()) {
            return;
        }
        self.b.start_node_at(start, BinaryExpr);
        self.bump();
        self.additive();
        self.b.finish_node();
        // O compilador só aceita um relacional. Os seguintes entram na árvore, com erro, para o
        // resto da expressão não virar lixo.
        while is_relational(self.current()) {
            self.error(ParseErrorKind::ChainedComparison);
            self.b.start_node_at(start, BinaryExpr);
            self.bump();
            self.additive();
            self.b.finish_node();
        }
    }

    /// `+`, `-` e `ou`, da esquerda para a direita, como o compilador avalia.
    fn additive(&mut self) {
        let start = self.b.checkpoint();
        self.multiplicative();
        while matches!(self.current(), Plus | Minus | OuKw) {
            self.b.start_node_at(start, BinaryExpr);
            self.bump();
            self.multiplicative();
            self.b.finish_node();
        }
    }

    /// `*`, `/` e `e`.
    fn multiplicative(&mut self) {
        let start = self.b.checkpoint();
        self.prefix();
        while matches!(self.current(), Star | Slash | EKw) {
            self.b.start_node_at(start, BinaryExpr);
            self.bump();
            self.prefix();
            self.b.finish_node();
        }
    }

    /// Sinal na frente do operando. O compilador recusa alguns, como `- -3` e `-(3)` depois de
    /// `*`, mas isso é com o validador.
    fn prefix(&mut self) {
        if matches!(self.current(), Plus | Minus) {
            self.b.start_node(PrefixExpr);
            self.bump();
            self.prefix();
            self.b.finish_node();
        } else {
            self.postfix();
        }
    }

    /// Operando seguido de chamada, índice ou campo, em qualquer combinação.
    fn postfix(&mut self) {
        let start = self.b.checkpoint();
        if !self.primary() {
            return;
        }
        loop {
            match self.current() {
                LParen => {
                    self.b.start_node_at(start, CallExpr);
                    self.arg_list();
                    self.b.finish_node();
                }
                LBracket => {
                    self.b.start_node_at(start, IndexExpr);
                    self.bump();
                    self.expr();
                    self.expect(RBracket);
                    self.b.finish_node();
                }
                Dot => {
                    self.b.start_node_at(start, FieldExpr);
                    self.bump();
                    // Depois do ponto, palavra reservada vale como nome (`cursor.Achou`).
                    if self.at(Ident) || self.current().is_keyword() {
                        self.b.start_node(NameRef);
                        self.bump();
                        self.b.finish_node();
                    } else {
                        self.error(ParseErrorKind::Expected(Ident));
                    }
                    self.b.finish_node();
                }
                _ => break,
            }
        }
    }

    /// Operando simples. Devolve `false` se não havia operando, depois de registrar o erro.
    fn primary(&mut self) -> bool {
        let kind = self.current();
        match kind {
            Ident => self.leaf(NameRef),
            Number | SyntaxKind::String | Char => self.leaf(Literal),
            _ if is_value_keyword(kind) => self.leaf(NameRef),
            LParen => {
                self.b.start_node(ParenExpr);
                self.bump();
                self.expr();
                self.expect(RParen);
                self.b.finish_node();
            }
            _ => {
                self.missing_operand();
                return false;
            }
        }
        true
    }

    fn leaf(&mut self, kind: SyntaxKind) {
        self.b.start_node(kind);
        self.bump();
        self.b.finish_node();
    }

    /// Registra a falta do operando. Um token que não fecha nada, como `#`, é consumido num
    /// `ErrorNode`. Os que fecham uma construção maior ficam para ela, e o operando faltante vira
    /// um nó vazio na posição.
    fn missing_operand(&mut self) {
        self.error(ParseErrorKind::ExpectedExpression);
        self.b.start_node(ErrorNode);
        if !is_expression_end(self.current()) {
            self.bump();
        }
        self.b.finish_node();
    }

    /// `( arg, arg, ... )`.
    fn arg_list(&mut self) {
        self.b.start_node(ArgList);
        self.bump();
        if !self.at(RParen) {
            loop {
                if matches!(self.current(), Comma | RParen) {
                    self.missing_operand();
                } else {
                    self.expr();
                }
                if self.at(Comma) {
                    self.bump();
                    continue;
                }
                // Outro valor sem vírgula no meio, como em `f(1 2)`.
                if starts_expression(self.current()) {
                    self.error(ParseErrorKind::Expected(Comma));
                    continue;
                }
                break;
            }
        }
        self.expect(RParen);
        self.b.finish_node();
    }
}

fn is_relational(kind: SyntaxKind) -> bool {
    matches!(kind, Eq | Lt | Gt | LtEq | GtEq | Neq | RelRun)
}

/// Palavras reservadas que podem aparecer como valor, como `x = Mensagem(Retorna, aTexto)`.
fn is_value_keyword(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        ValRetKw
            | ValStrKw
            | MensagemKw
            | AbrirKw
            | FecharKw
            | LerKw
            | LerNlKw
            | GravarKw
            | GravarNlKw
            | GravarNlEolKw
            | FormatarKw
            | ChamarFuncaoKw
    )
}

fn starts_expression(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        Ident | Number | SyntaxKind::String | Char | LParen | Plus | Minus
    ) || is_value_keyword(kind)
}

/// Tokens que encerram uma expressão e pertencem a quem está em volta dela.
fn is_expression_end(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        Eof | Comma | RParen | RBracket | Semicolon | RBrace | LBrace | FimKw | InicioKw
    )
}

/// Analisa o texto como uma expressão solta, para testes. O que sobrar depois dela vira um
/// `ErrorNode`.
#[cfg(test)]
pub(crate) fn parse_expression(text: &str) -> Tree {
    let mut p = Parser::new(Builder::new(crate::lex(text).unwrap()));
    p.expr();
    if !p.at(Eof) {
        p.error(ParseErrorKind::Expected(Eof));
        p.b.start_node(ErrorNode);
        while !p.at(Eof) {
            p.bump();
        }
        p.b.finish_node();
    }
    p.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::tests::check;
    use crate::tree::{NodeId, ParseError};

    /// A árvore em forma de lista, com o texto só nas folhas, como em `(BinaryExpr a b)`.
    fn sexp(text: &str) -> std::string::String {
        let tree = parse_expression(text);
        check(&tree, text);
        fn no(tree: &Tree, node: NodeId, text: &str) -> std::string::String {
            let kind = tree.kind(node);
            if matches!(kind, NameRef | Literal) {
                return text[tree.text_range(node)].to_owned();
            }
            let mut s = format!("({kind:?}");
            for child in tree.children(node) {
                s.push(' ');
                s.push_str(&no(tree, child, text));
            }
            s.push(')');
            s
        }
        let raiz = tree.root();
        let filhos: Vec<_> = tree.children(raiz).map(|c| no(&tree, c, text)).collect();
        filhos.join(" ")
    }

    fn erros(text: &str) -> Vec<ParseErrorKind> {
        let tree = parse_expression(text);
        check(&tree, text);
        tree.errors().iter().map(|e: &ParseError| e.kind).collect()
    }

    #[test]
    fn precedencia() {
        assert_eq!(sexp("a + b * c"), "(BinaryExpr a (BinaryExpr b c))");
        assert_eq!(sexp("a * b + c"), "(BinaryExpr (BinaryExpr a b) c)");
        assert_eq!(sexp("a ou b e c"), "(BinaryExpr a (BinaryExpr b c))");
        assert_eq!(sexp("a < b + c"), "(BinaryExpr a (BinaryExpr b c))");
        assert_eq!(
            sexp("(a = 1) e (b = 2)"),
            "(BinaryExpr (ParenExpr (BinaryExpr a 1)) (ParenExpr (BinaryExpr b 2)))"
        );
    }

    #[test]
    fn associacao_da_esquerda() {
        assert_eq!(sexp("a - b - c"), "(BinaryExpr (BinaryExpr a b) c)");
        assert_eq!(sexp("a / b * c"), "(BinaryExpr (BinaryExpr a b) c)");
        assert_eq!(sexp("a - b + c"), "(BinaryExpr (BinaryExpr a b) c)");
    }

    #[test]
    fn prefixo_e_parenteses() {
        assert_eq!(sexp("-a * b"), "(BinaryExpr (PrefixExpr a) b)");
        assert_eq!(
            sexp("(a + b) * c"),
            "(BinaryExpr (ParenExpr (BinaryExpr a b)) c)"
        );
        assert_eq!(sexp("- -3"), "(PrefixExpr (PrefixExpr 3))");
        // O compilador estraga a conta seguinte, mas a árvore precisa existir para o validador.
        assert_eq!(sexp("a e -b"), "(BinaryExpr a (PrefixExpr b))");
        assert!(erros("a e -b").is_empty());
    }

    #[test]
    fn um_relacional_so() {
        assert_eq!(sexp("a < b < c"), "(BinaryExpr (BinaryExpr a b) c)");
        assert_eq!(erros("a < b < c"), [ParseErrorKind::ChainedComparison]);
        assert_eq!(erros("a = 1 e b = 2"), [ParseErrorKind::ChainedComparison]);
        // Duplas como == entram como relacional, quem acusa é o validador.
        assert_eq!(sexp("a == b"), "(BinaryExpr a b)");
        assert!(erros("a == b").is_empty());
    }

    #[test]
    fn chamada_indice_e_campo() {
        assert_eq!(sexp("a.b[i]"), "(IndexExpr (FieldExpr a b) i)");
        assert_eq!(
            sexp("f(a, b + c)"),
            "(CallExpr f (ArgList a (BinaryExpr b c)))"
        );
        assert_eq!(sexp("f()"), "(CallExpr f (ArgList))");
        assert_eq!(sexp("cur.Achou"), "(FieldExpr cur Achou)");
        assert_eq!(sexp("x.Se"), "(FieldExpr x Se)");
        assert_eq!(
            sexp("lst.Primeiro()"),
            "(CallExpr (FieldExpr lst Primeiro) (ArgList))"
        );
        assert_eq!(
            sexp("Mensagem(Retorna, a)"),
            "(CallExpr Mensagem (ArgList Retorna a))"
        );
    }

    #[test]
    fn literais() {
        assert_eq!(sexp("1.5E3"), "1.5E3");
        assert_eq!(sexp("\"x\" + 'y'"), "(BinaryExpr \"x\" 'y')");
    }

    #[test]
    fn operando_faltando() {
        assert_eq!(erros("a +"), [ParseErrorKind::ExpectedExpression]);
        assert_eq!(sexp("a +"), "(BinaryExpr a (ErrorNode))");
        assert_eq!(
            erros("a + # b"),
            [
                ParseErrorKind::ExpectedExpression,
                ParseErrorKind::Expected(Eof)
            ]
        );
    }

    #[test]
    fn argumentos_com_erro() {
        assert_eq!(erros("f(a, )"), [ParseErrorKind::ExpectedExpression]);
        assert_eq!(sexp("f(a, )"), "(CallExpr f (ArgList a (ErrorNode)))");
        assert_eq!(erros("f(1 2)"), [ParseErrorKind::Expected(Comma)]);
        assert_eq!(sexp("f(1 2)"), "(CallExpr f (ArgList 1 2))");
        assert_eq!(erros("f(a"), [ParseErrorKind::Expected(RParen)]);
        assert_eq!(erros("(a"), [ParseErrorKind::Expected(RParen)]);
        assert_eq!(erros("a[1"), [ParseErrorKind::Expected(RBracket)]);
    }

    #[test]
    fn nunca_trava_nem_quebra_a_arvore() {
        let pecas = [
            "a", "1", "\"s\"", "'c'", "(", ")", "[", "]", ",", ".", "+", "-", "*", "/", "e", "ou",
            "=", "<", "==", "f(", "x.", "#", ";", "{", "}", " ", "\n", "se",
        ];
        let mut estado: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut proximo = move || {
            estado ^= estado << 13;
            estado ^= estado >> 7;
            estado ^= estado << 17;
            estado
        };
        for _ in 0..20_000 {
            let n = (proximo() % 12) as usize;
            let text: std::string::String = (0..n)
                .map(|_| pecas[(proximo() % pecas.len() as u64) as usize])
                .collect();
            let tree = parse_expression(&text);
            check(&tree, &text);
        }
    }

    #[test]
    fn operando_faltando_antes_da_quebra_de_linha_fica_dentro_do_pai() {
        let text = "af(f()ou1=\n,\n";
        check(&parse_expression(text), text);
    }
}
