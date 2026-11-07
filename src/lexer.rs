//! Corta o texto de uma regra em tokens.
//!
//! Para código válido, o lexer classifica cada trecho como o compilador da Senior. Para código
//! inválido ou incompleto, pode recuperar de um jeito mais útil ao editor,
//! desde que preserve todo o texto, registre cada problema com a posição exata e não mude a
//! leitura de construções válidas.
//!
//! Nada do texto é descartado. Espaços, quebras de linha e comentários também viram tokens, então
//! juntar o texto de todos os tokens reproduz a entrada.

use text_size::{TextRange, TextSize};

use crate::SyntaxKind::{self, ErrorToken, Newline, Whitespace};

/// O texto passa de `u32::MAX` bytes, o limite das posições.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextTooLarge;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexErrorKind {
    /// Caractere que não pertence à linguagem fora de string ou comentário.
    InvalidChar,
    UnterminatedString,
    UnterminatedAtComment,
    UnterminatedBlockComment,
    /// Identificador com mais de 101 caracteres.
    IdentTooLong,
    /// Literal de caractere vazio ou com mais de um caractere, como `''` ou `'ab'`.
    MalformedChar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LexError {
    pub kind: LexErrorKind,
    pub range: TextRange,
}

/// Tokens de um texto, com o tipo e a posição inicial de cada um.
///
/// `starts` tem um elemento a mais que `kinds`, o fim do texto, para que o token `i` vá de
/// `starts[i]` a `starts[i + 1]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    kinds: Vec<SyntaxKind>,
    starts: Vec<TextSize>,
    errors: Vec<LexError>,
}

impl Tokens {
    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    pub fn kind(&self, i: usize) -> SyntaxKind {
        self.kinds[i]
    }

    pub fn range(&self, i: usize) -> TextRange {
        TextRange::new(self.starts[i], self.starts[i + 1])
    }

    /// Texto do token `i`. `src` tem que ser o mesmo texto passado para [`lex`].
    pub fn text<'a>(&self, i: usize, src: &'a str) -> &'a str {
        &src[self.range(i)]
    }

    pub fn kinds(&self) -> &[SyntaxKind] {
        &self.kinds
    }

    pub fn errors(&self) -> &[LexError] {
        &self.errors
    }
}

/// Corta `text` em tokens. Erros não interrompem a leitura, vão para [`Tokens::errors`].
pub fn lex(text: &str) -> Result<Tokens, TextTooLarge> {
    let end = TextSize::try_from(text.len()).map_err(|_| TextTooLarge)?;
    let mut lexer = Lexer {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        tokens: Tokens {
            kinds: Vec::new(),
            starts: Vec::new(),
            errors: Vec::new(),
        },
    };
    while lexer.pos < text.len() {
        lexer.next_token();
    }
    lexer.tokens.starts.push(end);
    Ok(lexer.tokens)
}

struct Lexer<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    tokens: Tokens,
}

impl Lexer<'_> {
    fn next_token(&mut self) {
        let start = self.pos;
        let kind = match self.bytes[start] {
            // O compilador troca tab por espaço.
            b' ' | b'\t' => {
                self.eat_while(|b| b == b' ' || b == b'\t');
                Whitespace
            }
            b'\n' => {
                self.pos += 1;
                Newline
            }
            // O compilador também quebra a linha em CR sozinho.
            b'\r' => {
                self.pos += 1;
                if self.bytes.get(self.pos) == Some(&b'\n') {
                    self.pos += 1;
                }
                Newline
            }
            _ => {
                self.pos += self.text[start..].chars().next().map_or(1, char::len_utf8);
                self.error(LexErrorKind::InvalidChar, start);
                ErrorToken
            }
        };
        self.tokens.kinds.push(kind);
        self.tokens.starts.push(offset(start));
    }

    fn eat_while(&mut self, pred: impl Fn(u8) -> bool) {
        while self.bytes.get(self.pos).is_some_and(|&b| pred(b)) {
            self.pos += 1;
        }
    }

    /// Registra um erro de `start` até a posição atual.
    fn error(&mut self, kind: LexErrorKind, start: usize) {
        let range = TextRange::new(offset(start), offset(self.pos));
        self.tokens.errors.push(LexError { kind, range });
    }
}

/// Converte uma posição já validada por [`lex`], que recusa textos acima de `u32::MAX`.
fn offset(pos: usize) -> TextSize {
    TextSize::new(pos as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confere as invariantes que valem para qualquer entrada e devolve os tokens.
    fn lex_ok(text: &str) -> Tokens {
        let tokens = lex(text).unwrap();
        let n = tokens.len();

        assert_eq!(tokens.starts.len(), n + 1);
        assert_eq!(tokens.starts[0], TextSize::new(0));
        assert_eq!(tokens.starts[n], TextSize::of(text));

        let mut juntos = String::new();
        for i in 0..n {
            let range = tokens.range(i);
            assert!(!range.is_empty(), "token {i} vazio em {text:?}");
            assert!(text.is_char_boundary(range.start().into()));
            juntos.push_str(tokens.text(i, text));
        }
        assert_eq!(juntos, text);

        let fim = TextRange::up_to(TextSize::of(text));
        for e in tokens.errors() {
            assert!(fim.contains_range(e.range), "erro fora do texto em {text:?}");
        }
        tokens
    }

    fn kinds(text: &str) -> Vec<SyntaxKind> {
        lex_ok(text).kinds().to_vec()
    }

    #[test]
    fn texto_vazio() {
        let tokens = lex_ok("");
        assert!(tokens.is_empty());
        assert_eq!(tokens.starts, [TextSize::new(0)]);
        assert!(tokens.errors().is_empty());
    }

    #[test]
    fn espacos_e_tabs_viram_um_token() {
        assert_eq!(kinds(" \t  \t"), [Whitespace]);
    }

    #[test]
    fn quebras_de_linha() {
        assert_eq!(kinds("\r\n"), [Newline]);
        assert_eq!(kinds("\n\r"), [Newline, Newline]);
        assert_eq!(kinds("\r\r\n\n"), [Newline, Newline, Newline]);
        assert_eq!(kinds(" \r\n "), [Whitespace, Newline, Whitespace]);
    }

    #[test]
    fn caractere_desconhecido_vira_error_token_com_erro() {
        let tokens = lex_ok("#😀");
        assert_eq!(tokens.kinds(), [ErrorToken, ErrorToken]);
        assert_eq!(tokens.text(1, "#😀"), "😀");
        let erros: Vec<_> = tokens.errors().iter().map(|e| (e.kind, e.range)).collect();
        assert_eq!(
            erros,
            [
                (LexErrorKind::InvalidChar, TextRange::new(0.into(), 1.into())),
                (LexErrorKind::InvalidChar, TextRange::new(1.into(), 5.into())),
            ]
        );
    }

    #[test]
    fn combinacoes_de_caracteres_mantem_as_invariantes() {
        let alfabeto = [
            "", " ", "\t", "\r", "\n", "\r\n", "a", "1", "\"", "'", "@", "/", "*", "\\", "é", "😀",
            "\0",
        ];
        for a in alfabeto {
            for b in alfabeto {
                for c in alfabeto {
                    lex_ok(&format!("{a}{b}{c}"));
                }
            }
        }
    }
}
