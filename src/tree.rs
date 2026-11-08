//! Árvore de sintaxe em vetores planos.
//!
//! Os nós ficam em pré-ordem, o pai antes dos filhos, e cada um cobre um intervalo de tokens. O
//! texto não é copiado, e o de um nó é o trecho dos seus tokens no texto original. Os tokens dentro
//! de um nó que não estão em nenhum filho são filhos diretos dele, e a raiz cobre todos os tokens,
//! então a árvore reconstrói o arquivo inteiro.
//!
//! Um nó começa no primeiro token que não é trivia e termina no último, então espaços e
//! comentários entre dois filhos ficam com o pai.

use std::ops::Range;

use text_size::{TextRange, TextSize};

use crate::{SyntaxKind, Tokens};

/// Índice de um nó. Vale só dentro da árvore que o gerou.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// Faltou um token, como em `Cláusula "Se", falta ")"`.
    Expected(SyntaxKind),
    /// Faltou um valor, como em `a +` ou `f(a, )`.
    ExpectedExpression,
    /// Segundo relacional na mesma expressão, como em `a < b < c`. O compilador só aceita um.
    ChainedComparison,
    /// Nada que comece um comando, como `;` solto, `5;` ou `senao` sem `se`.
    ExpectedStatement,
    /// `fim` ou `}` sem bloco aberto.
    UnmatchedEnd,
    /// Aninhamento acima do limite do parser. O resto do arquivo vira um `ErrorNode`.
    TooDeep,
    /// Faltou o tipo, como em `definir 5;` ou num parâmetro de função.
    ExpectedType,
    /// Tipo do `Mensagem` que não é `Erro`, `Refaz` nem `Retorna`.
    InvalidMessageType,
    /// Texto do `Mensagem` que não é string nem variável, como um número ou um campo.
    ExpectedMessageText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub range: TextRange,
}

/// Filho de um nó, que é outro nó ou um token que nenhum filho cobre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Node(NodeId),
    Token(usize),
}

#[derive(Debug, Clone)]
pub struct Tree {
    tokens: Tokens,
    kinds: Vec<SyntaxKind>,
    /// Primeiro token do nó.
    first: Vec<u32>,
    /// Token logo depois do último. Igual a `first` num nó vazio.
    end: Vec<u32>,
    /// Nó que vem depois de toda a subárvore.
    next: Vec<u32>,
    /// Pai do nó, `NO_PARENT` na raiz.
    parent: Vec<u32>,
    errors: Vec<ParseError>,
}

const NO_PARENT: u32 = u32::MAX;

impl Tree {
    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    pub fn tokens(&self) -> &Tokens {
        &self.tokens
    }

    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }

    pub fn kind(&self, node: NodeId) -> SyntaxKind {
        self.kinds[node.0 as usize]
    }

    pub fn parent(&self, node: NodeId) -> Option<NodeId> {
        let p = self.parent[node.0 as usize];
        (p != NO_PARENT).then_some(NodeId(p))
    }

    /// Tokens que o nó cobre.
    pub fn token_range(&self, node: NodeId) -> Range<usize> {
        let i = node.0 as usize;
        self.first[i] as usize..self.end[i] as usize
    }

    /// Trecho do texto que o nó cobre. Num nó vazio, é a posição onde ele estaria.
    pub fn text_range(&self, node: NodeId) -> TextRange {
        let Range { start, end } = self.token_range(node);
        let at = |t: usize| {
            if t < self.tokens.len() {
                self.tokens.range(t).start()
            } else {
                self.text_end()
            }
        };
        if start == end {
            return TextRange::empty(at(start));
        }
        TextRange::new(at(start), self.tokens.range(end - 1).end())
    }

    fn text_end(&self) -> TextSize {
        match self.tokens.len() {
            0 => TextSize::new(0),
            n => self.tokens.range(n - 1).end(),
        }
    }

    /// Filhos que são nós, em ordem.
    pub fn children(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let stop = self.next[node.0 as usize];
        let mut child = node.0 + 1;
        std::iter::from_fn(move || {
            if child >= stop {
                return None;
            }
            let current = child;
            child = self.next[child as usize];
            Some(NodeId(current))
        })
    }

    /// Filhos em ordem, nós e os tokens que nenhum nó filho cobre.
    pub fn elements(&self, node: NodeId) -> impl Iterator<Item = Element> + '_ {
        let mut token = self.first[node.0 as usize] as usize;
        let end = self.end[node.0 as usize] as usize;
        let mut children = self.children(node).peekable();
        std::iter::from_fn(move || {
            if let Some(&child) = children.peek() {
                let child_first = self.first[child.0 as usize] as usize;
                if token >= child_first {
                    children.next();
                    token = token.max(self.end[child.0 as usize] as usize);
                    return Some(Element::Node(child));
                }
            }
            if token < end {
                token += 1;
                return Some(Element::Token(token - 1));
            }
            None
        })
    }
}

/// Monta a árvore enquanto o parser avança pelos tokens.
///
/// O parser grava eventos e a árvore só é montada no fim, numa passada. Assim um nó aberto depois
/// dos filhos, como a soma em `a + b` que só aparece ao ler o `+`, entra inserindo um evento na
/// posição do checkpoint, sem corrigir índice de nenhum nó.
pub struct Builder {
    tokens: Tokens,
    events: Vec<Event>,
    /// Próximo token ainda não consumido, que pode ser trivia.
    pos: usize,
    /// Token logo depois do último que não é trivia já consumido.
    last_end: u32,
    /// Para cada nó aberto, o índice do evento de início e o primeiro token.
    open: Vec<(usize, u32)>,
    errors: Vec<ParseError>,
}

#[derive(Debug, Clone, Copy)]
enum Event {
    Start { kind: SyntaxKind, token: u32 },
    Finish { token: u32 },
}

/// Posição para abrir um nó depois, com [`Builder::start_node_at`].
#[derive(Debug, Clone, Copy)]
pub struct Checkpoint {
    event: usize,
    token: u32,
}

impl Builder {
    pub fn new(tokens: Tokens) -> Builder {
        Builder {
            tokens,
            events: Vec::new(),
            pos: 0,
            last_end: 0,
            open: Vec::new(),
            errors: Vec::new(),
        }
    }

    pub fn tokens(&self) -> &Tokens {
        &self.tokens
    }

    /// Índice do próximo token que não é trivia, ou o fim dos tokens.
    fn next_significant(&self) -> usize {
        let kinds = self.tokens.kinds();
        let mut i = self.pos;
        while i < kinds.len() && kinds[i].is_trivia() {
            i += 1;
        }
        i
    }

    /// Tipo do próximo token que não é trivia, ou `Eof`.
    pub fn current(&self) -> SyntaxKind {
        let i = self.next_significant();
        self.tokens
            .kinds()
            .get(i)
            .copied()
            .unwrap_or(SyntaxKind::Eof)
    }

    /// Tipo do token significativo depois do atual, ou `Eof`.
    pub fn peek_second(&self) -> SyntaxKind {
        let kinds = self.tokens.kinds();
        let mut i = self.next_significant() + 1;
        while i < kinds.len() && kinds[i].is_trivia() {
            i += 1;
        }
        kinds.get(i).copied().unwrap_or(SyntaxKind::Eof)
    }

    /// Trecho do próximo token significativo, ou a posição vazia no fim do texto.
    pub fn current_range(&self) -> TextRange {
        let i = self.next_significant();
        if i < self.tokens.len() {
            self.tokens.range(i)
        } else {
            let end = match self.tokens.len() {
                0 => TextSize::new(0),
                n => self.tokens.range(n - 1).end(),
            };
            TextRange::empty(end)
        }
    }

    /// Consome o próximo token que não é trivia, com a trivia antes dele.
    /// Posição vazia logo depois do último token consumido que não é trivia, ou o começo do texto
    /// se nada foi consumido. É onde fica um token que faltou.
    pub fn after_last_range(&self) -> TextRange {
        let end = match self.last_end {
            0 => TextSize::new(0),
            n => self.tokens.range(n as usize - 1).end(),
        };
        TextRange::empty(end)
    }

    /// Posição do próximo token não consumido. Serve para conferir se o parser avançou.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Diz se o token logo depois do atual, sem nada no meio, é `kind`. Um rótulo só vale com o
    /// `:` colado no nome.
    pub fn next_is_glued(&self, kind: SyntaxKind) -> bool {
        let i = self.next_significant() + 1;
        self.tokens.kinds().get(i) == Some(&kind)
    }

    pub fn bump(&mut self) {
        let i = self.next_significant();
        if i < self.tokens.len() {
            self.pos = i + 1;
            self.last_end = self.pos as u32;
        }
    }

    pub fn start_node(&mut self, kind: SyntaxKind) {
        let token = self.next_significant() as u32;
        self.open.push((self.events.len(), token));
        self.events.push(Event::Start { kind, token });
    }

    /// Fecha o nó aberto mais recente. Um nó em que nada foi consumido fica logo depois do
    /// último token consumido, sem passar do começo do pai. No próximo token ele poderia cair
    /// depois de uma quebra de linha e ficar fora de um pai que termina antes dela.
    pub fn finish_node(&mut self) {
        let (event, start) = self.open.pop().expect("finish_node sem start_node");
        if self.last_end > start {
            self.events.push(Event::Finish {
                token: self.last_end,
            });
            return;
        }
        let parent_start = self.open.last().map_or(0, |&(_, t)| t);
        let token = self.last_end.max(parent_start);
        // Os descendentes também são vazios e vão junto, senão um filho fechado antes ficaria
        // fora do pai.
        for e in &mut self.events[event..] {
            match e {
                Event::Start { token: t, .. } | Event::Finish { token: t } => *t = token,
            }
        }
        self.events.push(Event::Finish { token });
    }

    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            event: self.events.len(),
            token: self.next_significant() as u32,
        }
    }

    /// Abre um nó na posição do checkpoint, envolvendo o que foi montado desde ele. Os eventos
    /// depois do checkpoint são de nós já fechados, então inserir antes deles não muda nada além
    /// da ordem.
    pub fn start_node_at(&mut self, checkpoint: Checkpoint, kind: SyntaxKind) {
        let event = Event::Start {
            kind,
            token: checkpoint.token,
        };
        // Os nós ainda abertos começaram antes do checkpoint, então o evento de início deles não
        // muda de posição com a inserção.
        debug_assert!(self.open.iter().all(|&(e, _)| e < checkpoint.event));
        self.events.insert(checkpoint.event, event);
        self.open.push((checkpoint.event, checkpoint.token));
    }

    pub fn error(&mut self, kind: ParseErrorKind, range: TextRange) {
        self.errors.push(ParseError { kind, range });
    }

    /// Fecha os nós que ficaram abertos e monta a árvore, com a raiz `SourceFile` cobrindo todos
    /// os tokens.
    pub fn finish(mut self) -> Tree {
        while !self.open.is_empty() {
            self.finish_node();
        }
        let n = 1 + self.events.len() / 2;
        let token_count = self.tokens.len() as u32;
        let mut tree = Tree {
            kinds: Vec::with_capacity(n),
            first: Vec::with_capacity(n),
            end: Vec::with_capacity(n),
            next: Vec::with_capacity(n),
            parent: Vec::with_capacity(n),
            tokens: self.tokens,
            errors: self.errors,
        };
        tree.kinds.push(SyntaxKind::SourceFile);
        tree.first.push(0);
        tree.end.push(token_count);
        tree.next.push(0);
        tree.parent.push(NO_PARENT);

        let mut stack = vec![0u32];
        for event in self.events {
            match event {
                Event::Start { kind, token } => {
                    let index = tree.kinds.len() as u32;
                    tree.kinds.push(kind);
                    tree.first.push(token);
                    tree.end.push(token);
                    tree.next.push(0);
                    tree.parent.push(*stack.last().unwrap());
                    stack.push(index);
                }
                Event::Finish { token } => {
                    let index = stack.pop().unwrap() as usize;
                    tree.end[index] = token;
                    tree.next[index] = tree.kinds.len() as u32;
                }
            }
        }
        tree.next[0] = tree.kinds.len() as u32;
        tree
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::SyntaxKind::{
        BinaryExpr, BlockComment, Eof, ErrorNode, NameRef, ParenExpr, SourceFile, Whitespace,
    };
    use crate::lex;

    /// Confere as invariantes da árvore e que ela reconstrói o texto.
    pub(crate) fn check(tree: &Tree, text: &str) {
        let n = tree.kinds.len();
        let t = tree.tokens.len() as u32;
        assert!(n > 0);
        for v in [&tree.first, &tree.end, &tree.next, &tree.parent] {
            assert_eq!(v.len(), n);
        }
        assert_eq!(tree.kinds[0], SourceFile);
        assert_eq!((tree.first[0], tree.end[0]), (0, t));
        assert_eq!(tree.next[0], n as u32);
        assert_eq!(tree.parent[0], NO_PARENT);

        for i in 1..n {
            let (first, end, next) = (tree.first[i], tree.end[i], tree.next[i]);
            assert!(first <= end && end <= t, "nó {i}");
            assert!(next > i as u32 && next <= n as u32, "nó {i}");
            let p = tree.parent[i] as usize;
            assert!(p < i, "nó {i}");
            assert!(
                tree.first[p] <= first && end <= tree.end[p],
                "nó {i} fora do pai"
            );
            assert!(next <= tree.next[p], "subárvore do nó {i} passa da do pai");
            assert!(!tree.kinds[i].is_trivia(), "nó {i}");
        }
        // Irmãos em ordem e sem sobreposição.
        for i in 0..n {
            let mut fim_anterior = tree.first[i];
            for child in tree.children(NodeId(i as u32)) {
                let c = child.0 as usize;
                assert_eq!(tree.parent[c], i as u32);
                assert!(tree.first[c] >= fim_anterior, "irmãos sobrepostos em {i}");
                fim_anterior = tree.end[c];
            }
        }

        assert_eq!(reconstruir(tree, tree.root(), text), text);
    }

    fn reconstruir(tree: &Tree, node: NodeId, text: &str) -> String {
        tree.elements(node)
            .map(|e| match e {
                Element::Node(child) => reconstruir(tree, child, text),
                Element::Token(i) => tree.tokens.text(i, text).to_owned(),
            })
            .collect()
    }

    /// Árvore em texto, um nó por linha com a indentação da profundidade.
    pub(crate) fn desenho(tree: &Tree, text: &str) -> String {
        fn nivel(tree: &Tree, node: NodeId, text: &str, prof: usize, saida: &mut String) {
            let range = tree.text_range(node);
            saida.push_str(&format!(
                "{}{:?} {:?}\n",
                "  ".repeat(prof),
                tree.kind(node),
                &text[range]
            ));
            for child in tree.children(node) {
                nivel(tree, child, text, prof + 1, saida);
            }
        }
        let mut saida = String::new();
        nivel(tree, tree.root(), text, 0, &mut saida);
        saida
    }

    fn builder(text: &str) -> Builder {
        Builder::new(lex(text).unwrap())
    }

    #[test]
    fn documento_vazio() {
        let tree = builder("").finish();
        check(&tree, "");
        assert_eq!(tree.token_range(tree.root()), 0..0);
        assert_eq!(tree.text_range(tree.root()), TextRange::empty(0.into()));
    }

    #[test]
    fn so_trivia_fica_na_raiz() {
        let text = " @ c @\r\n";
        let mut b = builder(text);
        assert_eq!(b.current(), Eof);
        b.bump();
        let tree = b.finish();
        check(&tree, text);
        assert_eq!(tree.children(tree.root()).count(), 0);
        assert_eq!(tree.elements(tree.root()).count(), 3);
    }

    #[test]
    fn no_unico_e_irmaos() {
        let text = "a b";
        let mut b = builder(text);
        for _ in 0..2 {
            b.start_node(NameRef);
            b.bump();
            b.finish_node();
        }
        let tree = b.finish();
        check(&tree, text);
        assert_eq!(
            desenho(&tree, text),
            "SourceFile \"a b\"\n  NameRef \"a\"\n  NameRef \"b\"\n"
        );
        let filhos: Vec<_> = tree.children(tree.root()).collect();
        assert_eq!(tree.parent(filhos[1]), Some(tree.root()));
        assert_eq!(tree.next, [3, 2, 3]);
    }

    #[test]
    fn soma_e_produto_com_checkpoint() {
        // a + b * c, montado como o parser de expressões faria.
        let text = "a + b * c";
        let mut b = builder(text);
        let soma = b.checkpoint();
        b.start_node(NameRef);
        b.bump();
        b.finish_node();
        b.bump(); // +
        let produto = b.checkpoint();
        b.start_node(NameRef);
        b.bump();
        b.finish_node();
        b.start_node_at(produto, BinaryExpr);
        b.bump(); // *
        b.start_node(NameRef);
        b.bump();
        b.finish_node();
        b.finish_node();
        b.start_node_at(soma, BinaryExpr);
        b.finish_node();
        let tree = b.finish();
        check(&tree, text);
        assert_eq!(
            desenho(&tree, text),
            "SourceFile \"a + b * c\"\n\
             \x20 BinaryExpr \"a + b * c\"\n\
             \x20   NameRef \"a\"\n\
             \x20   BinaryExpr \"b * c\"\n\
             \x20     NameRef \"b\"\n\
             \x20     NameRef \"c\"\n"
        );
        assert_eq!(tree.next, [6, 6, 3, 6, 5, 6]);
        assert_eq!(tree.parent, [NO_PARENT, 0, 1, 1, 3, 3]);
    }

    #[test]
    fn parenteses_mudam_o_agrupamento() {
        let text = "(a + b) * c";
        let mut b = builder(text);
        let produto = b.checkpoint();
        b.start_node(ParenExpr);
        b.bump(); // (
        let soma = b.checkpoint();
        b.start_node(NameRef);
        b.bump();
        b.finish_node();
        b.start_node_at(soma, BinaryExpr);
        b.bump(); // +
        b.start_node(NameRef);
        b.bump();
        b.finish_node();
        b.finish_node();
        b.bump(); // )
        b.finish_node();
        b.start_node_at(produto, BinaryExpr);
        b.bump(); // *
        b.start_node(NameRef);
        b.bump();
        b.finish_node();
        b.finish_node();
        let tree = b.finish();
        check(&tree, text);
        assert_eq!(
            desenho(&tree, text),
            "SourceFile \"(a + b) * c\"\n\
             \x20 BinaryExpr \"(a + b) * c\"\n\
             \x20   ParenExpr \"(a + b)\"\n\
             \x20     BinaryExpr \"a + b\"\n\
             \x20       NameRef \"a\"\n\
             \x20       NameRef \"b\"\n\
             \x20   NameRef \"c\"\n"
        );
    }

    #[test]
    fn trivia_entre_nos_fica_com_o_pai() {
        let text = "  a /* c */ b  ";
        let mut b = builder(text);
        b.start_node(BinaryExpr);
        b.start_node(NameRef);
        b.bump();
        b.finish_node();
        b.start_node(NameRef);
        b.bump();
        b.finish_node();
        b.finish_node();
        let tree = b.finish();
        check(&tree, text);
        let expr = tree.children(tree.root()).next().unwrap();
        // O nó começa no a e termina no b, sem os espaços das pontas.
        assert_eq!(&text[tree.text_range(expr)], "a /* c */ b");
        let diretos: Vec<_> = tree
            .elements(expr)
            .filter_map(|e| match e {
                Element::Token(i) => Some(tree.tokens().kind(i)),
                Element::Node(_) => None,
            })
            .collect();
        assert_eq!(diretos, [Whitespace, BlockComment, Whitespace]);
    }

    #[test]
    fn no_vazio_fica_na_posicao_onde_estaria() {
        let text = "( )";
        let mut b = builder(text);
        b.start_node(ParenExpr);
        b.bump(); // (
        b.start_node(ErrorNode);
        b.finish_node();
        b.bump(); // )
        b.finish_node();
        let tree = b.finish();
        check(&tree, text);
        let paren = tree.children(tree.root()).next().unwrap();
        let vazio = tree.children(paren).next().unwrap();
        // Logo depois do `(`, o último token consumido.
        assert_eq!(tree.token_range(vazio), 1..1);
        assert_eq!(tree.text_range(vazio), TextRange::empty(1.into()));
    }

    #[test]
    fn no_de_erro_guarda_os_tokens() {
        let text = "a # b";
        let mut b = builder(text);
        b.start_node(ErrorNode);
        b.bump();
        b.bump();
        b.bump();
        b.finish_node();
        let tree = b.finish();
        check(&tree, text);
        let erro = tree.children(tree.root()).next().unwrap();
        assert_eq!(tree.kind(erro), ErrorNode);
        assert_eq!(&text[tree.text_range(erro)], "a # b");
    }

    #[test]
    fn nos_abertos_fecham_no_fim() {
        let text = "a";
        let mut b = builder(text);
        b.start_node(BinaryExpr);
        b.start_node(NameRef);
        b.bump();
        let tree = b.finish();
        check(&tree, text);
        assert_eq!(tree.kinds, [SourceFile, BinaryExpr, NameRef]);
    }

    #[test]
    fn tres_niveis() {
        let text = "((a))";
        let mut b = builder(text);
        b.start_node(ParenExpr);
        b.bump();
        b.start_node(ParenExpr);
        b.bump();
        b.start_node(NameRef);
        b.bump();
        b.finish_node();
        b.bump();
        b.finish_node();
        b.bump();
        b.finish_node();
        let tree = b.finish();
        check(&tree, text);
        assert_eq!(tree.parent, [NO_PARENT, 0, 1, 2]);
        assert_eq!(tree.next, [4, 4, 4, 4]);
    }
}
