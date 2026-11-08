//! Parser de descida recursiva que monta a árvore.
//!
//! O parser aceita um superconjunto da linguagem. Ele monta a mesma árvore para qualquer tipo de
//! nome, e as restrições do compilador que dependem de onde a expressão aparece (relacional no
//! lado direito da atribuição, `-` logo depois de `e`) ficam para um validador que olha a árvore.

use crate::SyntaxKind::{self, *};
use crate::tree::{Builder, ParseErrorKind, Tree};
use crate::{TextTooLarge, lex};

/// Analisa o texto de uma regra. Só falha se o texto passar do limite das posições, os erros de
/// sintaxe ficam em [`Tree::errors`] e os do lexer nos tokens.
pub fn parse(text: &str) -> Result<Tree, TextTooLarge> {
    let mut p = Parser::new(Builder::new(lex(text)?), text);
    while !p.at(Eof) {
        p.statement_or_skip();
    }
    Ok(p.finish())
}

/// Maior aninhamento de comandos ou de expressões. Sem limite, um arquivo com milhares de `(`
/// ou `{` seguidos estoura a pilha da recursão e derruba o servidor. O compilador também tem um
/// limite (`O Comando é muito complexo`).
const MAX_DEPTH: u32 = 256;

pub(crate) struct Parser<'a> {
    b: Builder,
    text: &'a str,
    depth: u32,
    /// Blocos `{ }` ou `inicio fim` abertos. O compilador conta isso de forma global, atravessando
    /// as funções, então uma `funcao` pode começar com um bloco de outra ainda aberto.
    open_blocks: u32,
    /// Passou do limite de aninhamento. O resto do texto já foi consumido e nada mais é cobrado.
    gave_up: bool,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(b: Builder, text: &'a str) -> Parser<'a> {
        Parser {
            b,
            text,
            depth: 0,
            open_blocks: 0,
            gave_up: false,
        }
    }

    /// Entra num nível de aninhamento. Acima do limite, registra o erro, consome o resto do
    /// texto num `ErrorNode` e devolve `false` para quem chamou não descer mais.
    fn enter(&mut self) -> bool {
        if self.gave_up {
            return false;
        }
        if self.depth >= MAX_DEPTH {
            self.error(ParseErrorKind::TooDeep);
            self.gave_up = true;
            self.b.start_node(ErrorNode);
            while !self.at(Eof) {
                self.bump();
            }
            self.b.finish_node();
            return false;
        }
        self.depth += 1;
        true
    }

    fn leave(&mut self) {
        self.depth -= 1;
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
        if self.gave_up {
            return;
        }
        let range = self.b.current_range();
        self.b.error(kind, range);
    }

    /// Consome `kind` se for o token atual, senão registra que ele faltou. O erro fica logo depois
    /// do token anterior, e não no próximo, que pode estar em outra linha ou no fim do arquivo.
    fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump();
            return true;
        }
        if !self.gave_up {
            let range = self.b.after_last_range();
            self.b.error(ParseErrorKind::Expected(kind), range);
        }
        false
    }

    /// Analisa um comando e garante que o parser avançou. Se nada foi consumido, o token atual
    /// não começa comando nenhum e é consumido num `ErrorNode`, para o laço nunca travar.
    fn statement_or_skip(&mut self) {
        if !self.enter() {
            return;
        }
        let before = self.b.position();
        self.statement();
        if self.b.position() == before && !self.at(Eof) {
            self.error(ParseErrorKind::ExpectedStatement);
            self.b.start_node(ErrorNode);
            self.bump();
            self.b.finish_node();
        }
        self.leave();
    }

    fn statement(&mut self) {
        match self.current() {
            LBrace | InicioKw => self.block(),
            RBrace | FimKw => {
                self.error(ParseErrorKind::UnmatchedEnd);
                self.skip_one_in_error();
            }
            SeKw => self.if_statement(),
            EnquantoKw => self.while_statement(),
            ParaKw => self.for_statement(),
            VaParaKw => {
                self.b.start_node(Goto);
                self.bump();
                if self.at(Ident) {
                    self.leaf(NameRef);
                } else {
                    self.error(ParseErrorKind::Expected(Ident));
                }
                self.expect(Semicolon);
                self.b.finish_node();
            }
            ExecSqlKw | InsirirKw | CancelKw | CancelarKw if self.b.peek_second() != LParen => {
                self.keyword_with_value();
            }
            DefinirKw => self.definition(),
            FuncaoKw => self.func_impl(),
            PareKw => self.keyword_statement(Break),
            ContinueKw => self.keyword_statement(Continue),
            Ident if self.b.next_is_glued(Colon) => {
                self.b.start_node(Label);
                self.bump();
                self.bump();
                if self.at(Semicolon) {
                    self.bump();
                }
                self.b.finish_node();
            }
            SenaoKw => {
                self.error(ParseErrorKind::ExpectedStatement);
                self.skip_one_in_error();
            }
            _ if self.at_name_start() => self.simple_statement(),
            _ => {
                // `;` solto, `5;`, `(a);`, `senao` sem `se` e o que ainda não é analisado.
                self.error(ParseErrorKind::ExpectedStatement);
                self.recover_statement();
            }
        }
    }

    /// Começo de um comando ou designador por nome. Uma palavra reservada só vale se pode ser
    /// valor ou se vem seguida de `(`, como `Mensagem(...)` e `HabilitarCampo(...)`.
    fn at_name_start(&self) -> bool {
        let kind = self.current();
        kind == Ident
            || is_value_keyword(kind)
            || kind.is_keyword() && self.b.peek_second() == LParen
    }

    /// `ExecSql "UPDATE ...";`, `ExecSql aSql;`, `insirir "..."` e `cancel;`. O valor é
    /// opcional na árvore, e quem confere se o comando exige um é o validador.
    fn keyword_with_value(&mut self) {
        self.b.start_node(KeywordStmt);
        self.bump();
        if !self.at(Semicolon) && starts_expression(self.current()) {
            self.expr();
        }
        self.expect(Semicolon);
        self.b.finish_node();
    }

    /// `definir ...;`, despachado pela palavra depois do `definir`.
    fn definition(&mut self) {
        let start = self.b.checkpoint();
        self.bump();
        let kind = match self.current() {
            AlfaKw | NumeroKw | DataKw => {
                self.bump();
                self.name();
                self.optional_size();
                VarDef
            }
            CursorKw => {
                self.bump();
                self.name();
                if self.at(UnicoKw) {
                    self.bump();
                }
                CursorDef
            }
            // Sem o `;`, o compilador engole o token seguinte e o erro aparece longe. Aqui o erro
            // fica no lugar do `;`.
            ListaKw => {
                self.bump();
                self.name();
                ListDef
            }
            TabelaKw => {
                self.bump();
                self.name();
                self.optional_size();
                self.expect(Eq);
                self.table_fields();
                TableDef
            }
            FuncaoKw => {
                self.bump();
                self.name();
                self.param_list();
                FuncDecl
            }
            Ident => {
                self.qualified_name();
                self.name();
                WsDef
            }
            _ => {
                self.error(ParseErrorKind::ExpectedType);
                self.b.start_node_at(start, ErrorNode);
                self.recover_statement();
                self.b.finish_node();
                return;
            }
        };
        self.b.start_node_at(start, kind);
        self.expect_definition_end();
        self.b.finish_node();
    }

    /// `;` do fim de uma definição. Se faltar, o resto até o `;` vai para um `ErrorNode` sem outro
    /// erro, como em `definir alfa a, b;`. O pulo para numa palavra que começa comando, então a
    /// lista sem `;` não leva junto o `definir` de baixo.
    fn expect_definition_end(&mut self) {
        if self.expect(Semicolon) {
            return;
        }
        let kind = self.current();
        if kind == Eof || starts_statement(kind) || is_block_end(kind) {
            return;
        }
        self.recover_statement();
    }

    fn name(&mut self) {
        if self.at(Ident) {
            self.leaf(Name);
        } else {
            self.error(ParseErrorKind::Expected(Ident));
        }
    }

    /// `[N]` depois do nome. O compilador pede um número, mas qualquer expressão entra na árvore.
    fn optional_size(&mut self) {
        if self.at(LBracket) {
            self.bump();
            self.expr();
            self.expect(RBracket);
        }
    }

    /// `{ tipo nome [ [N] ]; ... }` de `definir tabela`.
    fn table_fields(&mut self) {
        if !self.expect(LBrace) {
            return;
        }
        while matches!(self.current(), AlfaKw | NumeroKw | DataKw) {
            self.b.start_node(VarDef);
            self.bump();
            self.name();
            self.optional_size();
            self.expect(Semicolon);
            self.b.finish_node();
        }
        self.expect(RBrace);
    }

    /// `Interno.com.x.Servico`. Palavra reservada depois de ponto vale como parte do nome.
    fn qualified_name(&mut self) {
        self.b.start_node(QualifiedName);
        self.bump();
        while self.at(Dot) {
            self.bump();
            if self.at(Ident) || self.current().is_keyword() {
                self.bump();
            } else {
                self.error(ParseErrorKind::Expected(Ident));
                break;
            }
        }
        self.b.finish_node();
    }

    /// `( tipo [end] nome [ [N] ], ... )`.
    fn param_list(&mut self) {
        self.b.start_node(ParamList);
        if self.expect(LParen) && !self.at(RParen) {
            loop {
                self.param();
                if self.at(Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        self.expect(RParen);
        self.b.finish_node();
    }

    fn param(&mut self) {
        self.b.start_node(Param);
        if matches!(self.current(), AlfaKw | NumeroKw | DataKw) {
            self.bump();
        } else {
            self.error(ParseErrorKind::ExpectedType);
        }
        if self.at(EndKw) {
            self.bump();
        }
        self.name();
        self.optional_size();
        self.b.finish_node();
    }

    /// `funcao nome(...);` seguido do corpo. O compilador põe um retorno antes de cada
    /// implementação, então o corpo vai até a próxima `funcao` ou o fim da regra, e não só até o
    /// fim do bloco seguinte.
    fn func_impl(&mut self) {
        self.b.start_node(FuncImpl);
        self.bump();
        self.name();
        self.param_list();
        self.expect(Semicolon);
        // Com um bloco aberto em volta, o `}` é dele e encerra também esta função.
        while !matches!(self.current(), FuncaoKw | Eof)
            && !(self.open_blocks > 0 && matches!(self.current(), RBrace | FimKw))
        {
            self.statement_or_skip();
        }
        self.b.finish_node();
    }

    /// `pare;` e `continue;`.
    fn keyword_statement(&mut self, kind: SyntaxKind) {
        self.b.start_node(kind);
        self.bump();
        self.expect(Semicolon);
        self.b.finish_node();
    }

    /// `{ ... }` ou `inicio ... fim`, que o compilador trata como o mesmo token. O `;` depois do
    /// fim é opcional, um só.
    fn block(&mut self) {
        self.b.start_node(Block);
        self.bump();
        self.open_blocks += 1;
        while !matches!(self.current(), RBrace | FimKw | Eof) {
            self.statement_or_skip();
        }
        self.open_blocks -= 1;
        if matches!(self.current(), RBrace | FimKw) {
            self.bump();
            if self.at(Semicolon) {
                self.bump();
            }
        } else {
            self.error(ParseErrorKind::Expected(RBrace));
        }
        self.b.finish_node();
    }

    /// `se ( condição ) comando [ senao comando ]`.
    fn if_statement(&mut self) {
        self.b.start_node(If);
        self.bump();
        self.condition(true);
        self.statement_or_skip();
        if self.at(SenaoKw) {
            self.b.start_node(ElseBranch);
            self.bump();
            self.statement_or_skip();
            self.b.finish_node();
        }
        self.b.finish_node();
    }

    /// `enquanto ( condição ) comando`. A condição pode ser vazia.
    fn while_statement(&mut self) {
        self.b.start_node(While);
        self.bump();
        self.condition(false);
        self.statement_or_skip();
        self.b.finish_node();
    }

    /// `( condição )`. No `se` o compilador exige a condição, no `enquanto` não.
    fn condition(&mut self, required: bool) {
        if !self.expect(LParen) {
            return;
        }
        if self.at(RParen) {
            if required {
                self.missing_operand();
            }
        } else {
            self.expr();
        }
        self.expect(RParen);
    }

    /// `para ( atribuições ; condição ; atribuições ) comando`, com cada parte opcional e as
    /// atribuições separadas por vírgula.
    fn for_statement(&mut self) {
        self.b.start_node(For);
        self.bump();
        if self.expect(LParen) {
            self.for_assignments(ForInit, Semicolon);
            self.expect(Semicolon);
            if !self.at(Semicolon) {
                self.expr();
            }
            self.expect(Semicolon);
            self.for_assignments(ForStep, RParen);
            self.expect(RParen);
        }
        self.statement_or_skip();
        self.b.finish_node();
    }

    fn for_assignments(&mut self, kind: SyntaxKind, end: SyntaxKind) {
        self.b.start_node(kind);
        if !self.at(end) {
            loop {
                self.assignment_or_call(false);
                if self.at(Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
            // O compilador espera mais uma atribuição depois de vírgula ou o fim da lista, como
            // em `para (...; i++;)`.
            if !self.at(end) && !self.at(Eof) {
                self.error(ParseErrorKind::Expected(Comma));
                self.b.start_node(ErrorNode);
                while !matches!(self.current(), RParen | Eof) && !self.at(end) {
                    self.bump();
                }
                self.b.finish_node();
            }
        }
        self.b.finish_node();
    }

    /// Comando que começa com nome, decidido pelo que vem depois do nome e dos seus campos e
    /// índices.
    fn simple_statement(&mut self) {
        self.assignment_or_call(true);
    }

    /// `x = expr`, `x++`, `x--`, chamada `f(...)` ou `cur.SQL "..."`. Com `terminated`, cobra
    /// o `;` do fim. Chamada de campo, como `cur.AbrirCursor()`, aceita ficar sem `;`, porque o
    /// compilador dispensa nos comandos de cursor e só os tipos dizem se é cursor.
    fn assignment_or_call(&mut self, terminated: bool) {
        let start = self.b.checkpoint();
        let designator = self.designator();
        match self.current() {
            Eq => {
                self.b.start_node_at(start, Assign);
                self.bump();
                self.expr();
                if terminated {
                    self.expect(Semicolon);
                }
                self.b.finish_node();
            }
            PlusPlus | MinusMinus => {
                self.b.start_node_at(start, Assign);
                self.bump();
                if terminated {
                    self.expect(Semicolon);
                }
                self.b.finish_node();
            }
            _ if designator == Some(Call) || designator == Some(FieldCall) => {
                self.b.start_node_at(start, CallStmt);
                if terminated {
                    if designator == Some(FieldCall) {
                        if self.at(Semicolon) {
                            self.bump();
                        }
                    } else {
                        self.expect(Semicolon);
                    }
                }
                self.b.finish_node();
            }
            kind if designator == Some(Field) && starts_expression(kind) => {
                self.b.start_node_at(start, FieldCommand);
                self.expr();
                if terminated && self.at(Semicolon) {
                    self.bump();
                }
                self.b.finish_node();
            }
            _ => {
                // Nome sozinho, como `a;` ou `lbl :`. O compilador lê como começo de atribuição.
                self.error(ParseErrorKind::Expected(Eq));
                if terminated {
                    self.recover_after_progress();
                }
            }
        }
    }

    /// Nome seguido de campos, índices e chamadas. Diz qual foi o último sufixo, para o comando
    /// saber se é chamada.
    fn designator(&mut self) -> Option<DesignatorEnd> {
        let start = self.b.checkpoint();
        let simple_args = self.at(MensagemKw);
        if self.at_name_start() {
            self.leaf(NameRef);
        } else {
            self.missing_operand();
            return None;
        }
        let mut last = None;
        loop {
            match self.current() {
                LParen => {
                    self.b.start_node_at(start, CallExpr);
                    self.call_args(simple_args && last.is_none());
                    self.b.finish_node();
                    last = Some(if last == Some(Field) { FieldCall } else { Call });
                }
                LBracket => {
                    self.b.start_node_at(start, IndexExpr);
                    self.bump();
                    self.expr();
                    self.expect(RBracket);
                    self.b.finish_node();
                    last = Some(Index);
                }
                Dot => {
                    self.b.start_node_at(start, FieldExpr);
                    self.bump();
                    if self.at(Ident) || self.current().is_keyword() {
                        self.leaf(NameRef);
                    } else {
                        self.error(ParseErrorKind::Expected(Ident));
                    }
                    self.b.finish_node();
                    last = Some(Field);
                }
                _ => return last,
            }
        }
    }

    /// Pula até um ponto seguro dentro de um `ErrorNode`. O `;` que encerra o trecho entra no
    /// nó. Fim de bloco, `senao` e palavras que só começam comando ficam para quem está em volta.
    fn recover_statement(&mut self) {
        self.recover(true);
    }

    /// Recuperação depois de um trecho que já consumiu tokens. Um comando que começa logo em
    /// seguida, como a `funcao` depois de uma linha incompleta, fica para ele.
    fn recover_after_progress(&mut self) {
        self.recover(false);
    }

    /// Com `force`, consome ao menos um token, para o parser avançar mesmo parado num token que
    /// começa comando.
    fn recover(&mut self, force: bool) {
        self.b.start_node(ErrorNode);
        let mut first = force;
        loop {
            let kind = self.current();
            if kind == Eof || (!first && (starts_statement(kind) || is_block_end(kind))) {
                break;
            }
            self.bump();
            first = false;
            if kind == Semicolon {
                break;
            }
        }
        self.b.finish_node();
    }

    fn skip_one_in_error(&mut self) {
        self.b.start_node(ErrorNode);
        self.bump();
        self.b.finish_node();
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
        // Toda expressão aninhada passa por aqui, inclusive a de dentro de parênteses e de
        // argumentos, então o limite fica neste ponto.
        if !self.enter() {
            return;
        }
        if matches!(self.current(), Plus | Minus) {
            self.b.start_node(PrefixExpr);
            self.bump();
            self.prefix();
            self.b.finish_node();
        } else {
            self.postfix();
        }
        self.leave();
    }

    /// Operando seguido de chamada, índice ou campo, em qualquer combinação.
    fn postfix(&mut self) {
        let start = self.b.checkpoint();
        let mut simple_args = self.at(MensagemKw);
        if !self.primary() {
            return;
        }
        loop {
            match self.current() {
                LParen => {
                    self.b.start_node_at(start, CallExpr);
                    self.call_args(simple_args);
                    self.b.finish_node();
                    simple_args = false;
                }
                LBracket => {
                    self.b.start_node_at(start, IndexExpr);
                    self.bump();
                    self.expr();
                    self.expect(RBracket);
                    self.b.finish_node();
                    simple_args = false;
                }
                Dot => {
                    simple_args = false;
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
            // Tipo como valor, nas funções de lista, como em `Lst.AdicionarCampo("CodRep", numero)`.
            AlfaKw | NumeroKw | DataKw => self.leaf(NameRef),
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

    fn call_args(&mut self, mensagem: bool) {
        if mensagem {
            self.mensagem_args();
        } else {
            self.arg_list();
        }
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

    /// `(tipo, texto)` do `Mensagem`. O tipo é `Erro`, `Refaz` ou `Retorna`, e o texto só pode
    /// ser uma string ou uma variável. Número, apóstrofo, parênteses, campo e concatenação não
    /// compilam. Uma variável numérica também não, mas isso depende do tipo dela e fica para quem
    /// conhece as declarações.
    fn mensagem_args(&mut self) {
        self.b.start_node(ArgList);
        self.bump();
        if self.at(Ident) && is_message_type(&self.text[self.b.current_range()]) {
            self.leaf(NameRef);
        } else {
            self.error(ParseErrorKind::InvalidMessageType);
            if self.at(Ident) {
                self.leaf(NameRef);
            } else if !matches!(self.current(), Comma | RParen)
                && !is_expression_end(self.current())
            {
                self.b.start_node(ErrorNode);
                self.bump();
                self.b.finish_node();
            }
        }
        self.expect(Comma);
        let text_ok = match self.current() {
            SyntaxKind::String => {
                self.leaf(Literal);
                true
            }
            Ident if !matches!(self.b.peek_second(), Dot | LParen | LBracket) => {
                self.leaf(NameRef);
                true
            }
            _ => {
                self.error(ParseErrorKind::ExpectedMessageText);
                false
            }
        };
        // Depois de um erro, o resto até o `)` vai para um `ErrorNode`, para o `;` que vem depois
        // não gerar um segundo erro. Um `+` de concatenação dá `falta ")"`, como no compilador.
        if !text_ok || !self.expect(RParen) {
            self.skip_to_rparen();
        }
        self.b.finish_node();
    }

    /// Consome num `ErrorNode` o que sobrou até o `)`, inclusive. Para antes de `;`, de fim de
    /// bloco e de palavra que começa comando, que ficam para quem está em volta.
    fn skip_to_rparen(&mut self) {
        self.b.start_node(ErrorNode);
        let mut depth = 0u32;
        loop {
            match self.current() {
                Eof | Semicolon => break,
                kind if is_block_end(kind) || starts_statement(kind) => break,
                LParen => depth += 1,
                RParen if depth == 0 => {
                    self.bump();
                    break;
                }
                RParen => depth -= 1,
                _ => {}
            }
            self.bump();
        }
        self.b.finish_node();
    }
}

/// Último sufixo de um designador.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesignatorEnd {
    Call,
    /// Chamada logo depois de um campo, como `cur.AbrirCursor()`.
    FieldCall,
    Index,
    Field,
}

use DesignatorEnd::{Call, Field, FieldCall, Index};

/// Palavras que começam comando e servem de ponto seguro para a recuperação. `e` e `ou` não
/// entram, porque aparecem no meio de expressão.
fn starts_statement(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SeKw | SenaoKw
            | EnquantoKw
            | ParaKw
            | VaParaKw
            | PareKw
            | ContinueKw
            | DefinirKw
            | FuncaoKw
            | InicioKw
            | LBrace
    )
}

fn is_block_end(kind: SyntaxKind) -> bool {
    matches!(kind, RBrace | FimKw | SenaoKw)
}

fn is_relational(kind: SyntaxKind) -> bool {
    matches!(kind, Eq | Lt | Gt | LtEq | GtEq | Neq | RelRun)
}

/// Tipo do `Mensagem`, sem diferença entre maiúsculas e minúsculas.
fn is_message_type(text: &str) -> bool {
    ["erro", "refaz", "retorna"]
        .iter()
        .any(|t| text.eq_ignore_ascii_case(t))
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
        Ident
            | Number
            | SyntaxKind::String
            | Char
            | LParen
            | Plus
            | Minus
            | AlfaKw
            | NumeroKw
            | DataKw
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
    let mut p = Parser::new(Builder::new(lex(text).unwrap()), text);
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
    fn forma(tree: &Tree, text: &str) -> std::string::String {
        check(tree, text);
        fn no(tree: &Tree, node: NodeId, text: &str) -> std::string::String {
            let kind = tree.kind(node);
            if matches!(kind, NameRef | Literal | Name | QualifiedName) {
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
        let filhos: Vec<_> = tree
            .children(tree.root())
            .map(|c| no(tree, c, text))
            .collect();
        filhos.join(" ")
    }

    fn sexp(text: &str) -> std::string::String {
        forma(&parse_expression(text), text)
    }

    /// A árvore de uma regra inteira.
    fn regra(text: &str) -> std::string::String {
        forma(&parse(text).unwrap(), text)
    }

    fn erros(text: &str) -> Vec<ParseErrorKind> {
        let tree = parse_expression(text);
        check(&tree, text);
        tree.errors().iter().map(|e: &ParseError| e.kind).collect()
    }

    fn erros_regra(text: &str) -> Vec<ParseErrorKind> {
        let tree = parse(text).unwrap();
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
    fn tipo_como_argumento() {
        assert_eq!(
            sexp("Lst.AdicionarCampo(\"CodPro\", Alfa, 14)"),
            "(CallExpr (FieldExpr Lst AdicionarCampo) (ArgList \"CodPro\" Alfa 14))"
        );
        assert!(erros("l.AdicionarCampo(\"c\", numero)").is_empty());
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

    use ParseErrorKind::{Expected, ExpectedExpression, ExpectedStatement, UnmatchedEnd};

    #[test]
    fn atribuicao_e_chamada() {
        assert_eq!(regra("x = a + 1;"), "(Assign x (BinaryExpr a 1))");
        assert_eq!(regra("x++; y--;"), "(Assign x) (Assign y)");
        assert_eq!(
            regra("v[i].c = 1;"),
            "(Assign (FieldExpr (IndexExpr v i) c) 1)"
        );
        assert_eq!(regra("f(1);"), "(CallStmt (CallExpr f (ArgList 1)))");
        assert_eq!(
            regra("Mensagem(Erro, aMsg);"),
            "(CallStmt (CallExpr Mensagem (ArgList Erro aMsg)))"
        );
        assert!(erros_regra("x = a + 1; f(1); Mensagem(Erro, a);").is_empty());
        assert_eq!(erros_regra("f(1)"), [Expected(Semicolon)]);
        assert_eq!(erros_regra("x = 1 y = 2;"), [Expected(Semicolon)]);
    }

    #[test]
    fn comandos_de_campo_dispensam_o_ponto_e_virgula() {
        let text = "cur.SQL \"SELECT 1\"\ncur.AbrirCursor()\ncur.FecharCursor();";
        assert_eq!(
            regra(text),
            "(FieldCommand (FieldExpr cur SQL) \"SELECT 1\") \
             (CallStmt (CallExpr (FieldExpr cur AbrirCursor) (ArgList))) \
             (CallStmt (CallExpr (FieldExpr cur FecharCursor) (ArgList)))"
        );
        assert!(erros_regra(text).is_empty());
    }

    #[test]
    fn nao_existe_comando_vazio_nem_expressao_como_comando() {
        assert_eq!(erros_regra("x = 1;;"), [ExpectedStatement]);
        assert_eq!(regra("x = 1;;"), "(Assign x 1) (ErrorNode)");
        assert_eq!(erros_regra("5;"), [ExpectedStatement]);
        assert_eq!(erros_regra("(a);"), [ExpectedStatement]);
        // O compilador lê nome sozinho como começo de atribuição.
        assert_eq!(erros_regra("a;"), [Expected(Eq)]);
        assert_eq!(erros_regra("lbl :"), [Expected(Eq)]);
    }

    #[test]
    fn blocos() {
        assert_eq!(regra("{ }"), "(Block)");
        assert_eq!(regra("inicio x = 1; fim"), "(Block (Assign x 1))");
        assert!(erros_regra("{ x = 1; };").is_empty());
        assert_eq!(erros_regra("{ x = 1; };;"), [ExpectedStatement]);
        assert_eq!(erros_regra("{ x = 1;"), [Expected(RBrace)]);
        assert_eq!(erros_regra("x = 1; }"), [UnmatchedEnd]);
        assert_eq!(regra("{ { x = 1; } }"), "(Block (Block (Assign x 1)))");
    }

    #[test]
    fn se_e_senao() {
        assert_eq!(
            regra("se (a = 1) x = 1; senao x = 2;"),
            "(If (BinaryExpr a 1) (Assign x 1) (ElseBranch (Assign x 2)))"
        );
        assert_eq!(
            regra("se (a = 9) { x = 1; } senao se (a = 1) { x = 2; } senao { x = 3; }"),
            "(If (BinaryExpr a 9) (Block (Assign x 1)) (ElseBranch (If (BinaryExpr a 1) \
             (Block (Assign x 2)) (ElseBranch (Block (Assign x 3))))))"
        );
        assert_eq!(erros_regra("se () { x = 1; }"), [ExpectedExpression]);
        assert_eq!(
            erros_regra("se (a = 1 e b = 2) { }"),
            [ParseErrorKind::ChainedComparison]
        );
        // O senao solto vira erro e o comando dele é analisado normalmente.
        assert_eq!(erros_regra("senao x = 1;"), [ExpectedStatement]);
        assert_eq!(regra("senao x = 1;"), "(ErrorNode) (Assign x 1)");
    }

    #[test]
    fn enquanto_e_para() {
        assert_eq!(regra("enquanto () { pare; }"), "(While (Block (Break)))");
        assert!(erros_regra("enquanto () { pare; }").is_empty());
        assert_eq!(
            regra("para (i = 0, j = 0; i < 3; i++, j++) { continue; }"),
            "(For (ForInit (Assign i 0) (Assign j 0)) (BinaryExpr i 3) \
             (ForStep (Assign i) (Assign j)) (Block (Continue)))"
        );
        assert_eq!(regra("para (;;) { }"), "(For (ForInit) (ForStep) (Block))");
        assert!(erros_regra("para (;;) { }").is_empty());
        assert_eq!(
            erros_regra("para (i = 0; i < 3; i++;) { }"),
            [Expected(Comma)]
        );
    }

    #[test]
    fn vapara_e_rotulo() {
        assert_eq!(
            regra("vapara fim2; x = 1; fim2:"),
            "(Goto fim2) (Assign x 1) (Label)"
        );
        assert!(erros_regra("vapara L; L:;").is_empty());
        assert_eq!(erros_regra("vapara ;"), [Expected(Ident)]);
    }

    #[test]
    fn recuperacao_continua_depois_do_erro() {
        let text = "se (a = 1 { x = 1; } y = 2;";
        assert_eq!(erros_regra(text), [Expected(RParen)]);
        assert_eq!(
            regra(text),
            "(If (BinaryExpr a 1) (Block (Assign x 1))) (Assign y 2)"
        );
        // Um erro dentro do bloco não leva o senao embora.
        let text = "se (a) { # } senao x = 2;";
        assert_eq!(
            regra(text),
            "(If a (Block (ErrorNode)) (ElseBranch (Assign x 2)))"
        );
    }

    #[test]
    fn definicoes_de_variavel() {
        assert_eq!(
            regra("definir alfa a; definir numero n; definir data d;"),
            "(VarDef a) (VarDef n) (VarDef d)"
        );
        assert_eq!(regra("definir alfa a[10];"), "(VarDef a 10)");
        assert_eq!(regra("definir numero v[3];"), "(VarDef v 3)");
        assert!(erros_regra("definir alfa a[10]; definir numero v[3];").is_empty());
        assert_eq!(erros_regra("definir alfa a, b;"), [Expected(Semicolon)]);
        assert_eq!(erros_regra("definir 5;"), [ParseErrorKind::ExpectedType]);
    }

    #[test]
    fn cursor_lista_tabela_e_web_service() {
        assert_eq!(
            regra("definir cursor c; definir cursor u unico; definir lista l;"),
            "(CursorDef c) (CursorDef u) (ListDef l)"
        );
        assert_eq!(
            regra("Definir Tabela Indice[2000] = {Numero VlrInd; alfa Nome[5];};"),
            "(TableDef Indice 2000 (VarDef VlrInd) (VarDef Nome 5))"
        );
        assert_eq!(
            regra("Definir Interno.com.empresa.Produtos.Estoque.Transferir Ws_Transf;"),
            "(WsDef Interno.com.empresa.Produtos.Estoque.Transferir Ws_Transf)"
        );
        assert!(erros_regra("definir Interno.com.senior.g5.co.mcm.Servico ws;").is_empty());
    }

    #[test]
    fn lista_sem_ponto_e_virgula_acusa_no_lugar_certo() {
        // O compilador engole o definir de baixo e acusa o numero. Aqui o erro fica na lista.
        let text = "definir lista l\ndefinir numero n;";
        assert_eq!(erros_regra(text), [Expected(Semicolon)]);
        assert_eq!(regra(text), "(ListDef l) (VarDef n)");
    }

    #[test]
    fn declaracao_de_funcao() {
        assert_eq!(
            regra("definir funcao f(numero end p, alfa a);"),
            "(FuncDecl f (ParamList (Param p) (Param a)))"
        );
        assert_eq!(regra("definir funcao g();"), "(FuncDecl g (ParamList))");
        assert_eq!(
            erros_regra("definir funcao f(p);"),
            [ParseErrorKind::ExpectedType]
        );
    }

    #[test]
    fn corpo_da_funcao_vai_ate_a_proxima_funcao() {
        let text = "x = 0;\nfuncao f(); { x = 1; }\nx = 5;\nfuncao g(numero end p); { p = 9; }";
        assert_eq!(
            regra(text),
            "(Assign x 0) \
             (FuncImpl f (ParamList) (Block (Assign x 1)) (Assign x 5)) \
             (FuncImpl g (ParamList (Param p)) (Block (Assign p 9)))"
        );
        assert!(erros_regra(text).is_empty());
        assert_eq!(erros_regra("funcao f() { }"), [Expected(Semicolon)]);
    }

    #[test]
    fn funcao_dentro_de_bloco_ainda_aberto() {
        // Os blocos são contados no arquivo todo, então o bloco de `a` só fecha no
        // fim, depois de outras funções.
        let text = "funcao a(); { x = 1;\n\
                    funcao b(); { y = 2; }\n\
                    funcao c();\n z = 3;\n}\n\
                    funcao d(); { }";
        assert!(erros_regra(text).is_empty());
        assert_eq!(
            regra(text),
            "(FuncImpl a (ParamList) (Block (Assign x 1) \
             (FuncImpl b (ParamList) (Block (Assign y 2))) \
             (FuncImpl c (ParamList) (Assign z 3)))) \
             (FuncImpl d (ParamList) (Block))"
        );
        // Sem bloco aberto, o fim sobrando continua sendo erro.
        assert_eq!(erros_regra("funcao f();\n x = 7;\n}"), [UnmatchedEnd]);
    }

    #[test]
    fn comandos_de_palavra_reservada() {
        assert_eq!(
            regra("ExecSQL \"DELETE FROM T\"; ExecSql aSql; cancel; cancel(1);"),
            "(KeywordStmt \"DELETE FROM T\") (KeywordStmt aSql) (KeywordStmt) \
             (CallStmt (CallExpr cancel (ArgList 1)))"
        );
        assert_eq!(
            regra("Mensagem(Erro, aMsg); IniciarTransacao(); ExecSqlEx(aSql, nErro, aErro);"),
            "(CallStmt (CallExpr Mensagem (ArgList Erro aMsg))) \
             (CallStmt (CallExpr IniciarTransacao (ArgList))) \
             (CallStmt (CallExpr ExecSqlEx (ArgList aSql nErro aErro)))"
        );
        assert_eq!(erros_regra("ExecSql \"x\""), [Expected(Semicolon)]);
        let text = "ExecSql \"UPDATE T \\\n SET A = 1\";";
        assert!(erros_regra(text).is_empty());
    }

    #[test]
    fn argumentos_do_mensagem() {
        use ParseErrorKind::{Expected, ExpectedMessageText, InvalidMessageType};
        let ok = [
            "Mensagem(Erro, \"a\");",
            "mensagem(retorna, aTxt);",
            "Mensagem(REFAZ, aTxt);",
            "x = Mensagem(Retorna, \"a\");",
        ];
        for text in ok {
            assert!(erros_regra(text).is_empty(), "{text}");
        }
        // Cada caso dá um erro só, e o `;` depois do `)` não gera outro.
        let casos = [
            ("Mensagem(Erro, \"a\" + aTxt);", Expected(RParen)),
            ("x = Mensagem(Retorna, aTxt + \"b\");", Expected(RParen)),
            ("Mensagem(Retorna, 5);", ExpectedMessageText),
            ("Mensagem(Retorna, 'a');", ExpectedMessageText),
            ("Mensagem(Retorna, (\"abc\"));", ExpectedMessageText),
            ("Mensagem(Retorna, Cur_Emp.NomEmp);", ExpectedMessageText),
            ("Mensagem(Retorna, f(1));", ExpectedMessageText),
            ("Mensagem(Aviso, \"abc\");", InvalidMessageType),
            ("Mensagem(x, \"abc\");", InvalidMessageType),
            ("Mensagem(Retorna \"abc\");", Expected(Comma)),
            ("Mensagem(Retorna, \"abc\"\n;", Expected(RParen)),
        ];
        for (text, erro) in casos {
            assert_eq!(erros_regra(&format!("{text} y = 1;")), [erro], "{text}");
        }
        // As outras chamadas continuam aceitando expressão.
        assert!(erros_regra("f(\"a\" + aTxt, 1 * 2); x = f(\"a\" + aTxt);").is_empty());
    }

    #[test]
    fn linha_incompleta_nao_engole_o_comando_seguinte() {
        assert_eq!(
            regra("nT\nfuncao f(); { x = 1; }"),
            "nT (ErrorNode) (FuncImpl f (ParamList) (Block (Assign x 1)))"
        );
        assert_eq!(erros_regra("nT\nse (a = 1) { }"), [Expected(Eq)]);
    }

    #[test]
    fn token_que_falta_fica_logo_depois_do_anterior() {
        let posicao = |text: &str| -> u32 { parse(text).unwrap().errors()[0].range.start().into() };
        // Logo depois do `1`, e não na linha seguinte nem no fim do arquivo.
        assert_eq!(posicao("x = 1\r\n\r\n"), 5);
        assert_eq!(posicao("x = 1\ny = 2;"), 5);
        assert_eq!(posicao("se (a = 1 { }"), 9);
    }

    #[test]
    fn aninhamento_enorme_nao_estoura_a_pilha() {
        for text in [
            format!("x = {}", "(".repeat(100_000)),
            "{".repeat(100_000),
            format!("x = {}1;", "- ".repeat(100_000)),
            "se (a) ".repeat(100_000),
            format!("x = {};", "f(".repeat(100_000)),
        ] {
            let tree = parse(&text).unwrap();
            check(&tree, &text);
            let erros: Vec<_> = tree.errors().iter().map(|e| e.kind).collect();
            assert!(erros.contains(&ParseErrorKind::TooDeep), "{}", &text[..20]);
            // Depois de desistir, nenhum erro a mais por cada nível que ficou aberto.
            assert!(erros.len() <= 2, "{} erros", erros.len());
        }
    }

    #[test]
    fn aninhamento_normal_nao_e_afetado() {
        let text = format!("x = {}1{};", "(".repeat(100), ")".repeat(100));
        assert!(erros_regra(&text).is_empty());
    }

    #[test]
    fn regra_aleatoria_nunca_trava_nem_quebra_a_arvore() {
        let pecas = [
            "x", "1", "\"s\"", "(", ")", "[", "]", ",", ".", "+", "-", "=", "<", "++", ";", "{",
            "}", "se", "senao", "enquanto", "para", "vapara", "pare", "lbl:", "f(", "inicio",
            "fim", "e", "#", " ", "\n", "definir", "alfa", "funcao", "lista", "tabela", "end",
        ];
        let mut estado: u64 = 0x2545_f491_4f6c_dd1d;
        let mut proximo = move || {
            estado ^= estado << 13;
            estado ^= estado >> 7;
            estado ^= estado << 17;
            estado
        };
        for _ in 0..20_000 {
            let n = (proximo() % 16) as usize;
            let text: std::string::String = (0..n)
                .map(|_| pecas[(proximo() % pecas.len() as u64) as usize])
                .collect();
            check(&parse(&text).unwrap(), &text);
        }
    }
}
