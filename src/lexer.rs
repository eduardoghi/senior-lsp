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

use crate::SyntaxKind::{self, ErrorToken, Ident, Newline, Whitespace};

/// Tamanho máximo de um identificador no compilador (`identificador muito extenso`).
const MAX_IDENT_CHARS: usize = 101;

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
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.ident(start),
            _ => {
                let ch = self.char_at(start);
                if ch.is_alphabetic() {
                    // O compilador para no primeiro caractere acentuado. Aqui o nome inteiro vira
                    // um identificador, com erro em cada caractere, para não quebrar o resto da
                    // linha.
                    self.ident(start)
                } else {
                    self.pos += ch.len_utf8();
                    self.error(LexErrorKind::InvalidChar, start);
                    ErrorToken
                }
            }
        };
        self.tokens.kinds.push(kind);
        self.tokens.starts.push(offset(start));
    }

    /// Lê um identificador e devolve `Ident` ou o tipo da palavra reservada.
    fn ident(&mut self, start: usize) -> SyntaxKind {
        let first_error = self.tokens.errors.len();
        let mut chars = 0;
        while let Some(&b) = self.bytes.get(self.pos) {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'\'' {
                self.pos += 1;
            } else if b.is_ascii() {
                break;
            } else {
                let ch = self.char_at(self.pos);
                if !ch.is_alphabetic() {
                    break;
                }
                let ch_start = self.pos;
                self.pos += ch.len_utf8();
                self.error(LexErrorKind::InvalidChar, ch_start);
            }
            chars += 1;
        }

        // O compilador conta em cp1252, um byte por caractere, então aqui a conta é em caracteres.
        // Palavras reservadas não passam pela checagem, mas nenhuma chega perto do limite.
        if chars > MAX_IDENT_CHARS {
            let range = TextRange::new(offset(start), offset(self.pos));
            let error = LexError {
                kind: LexErrorKind::IdentTooLong,
                range,
            };
            // Antes dos erros de acento do mesmo nome, para os erros ficarem em ordem de posição.
            self.tokens.errors.insert(first_error, error);
            return Ident;
        }
        SyntaxKind::keyword(&self.text[start..self.pos]).unwrap_or(Ident)
    }

    /// Caractere que começa em `pos`, que tem que ser uma fronteira de caractere.
    fn char_at(&self, pos: usize) -> char {
        self.text[pos..].chars().next().unwrap_or('\0')
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
    use crate::SyntaxKind::{EKw, GravarNlEolKw, SeKw};

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
            assert!(
                fim.contains_range(e.range),
                "erro fora do texto em {text:?}"
            );
        }
        for par in tokens.errors().windows(2) {
            assert!(
                par[0].range.start() <= par[1].range.start(),
                "erros fora de ordem em {text:?}"
            );
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
                (
                    LexErrorKind::InvalidChar,
                    TextRange::new(0.into(), 1.into())
                ),
                (
                    LexErrorKind::InvalidChar,
                    TextRange::new(1.into(), 5.into())
                ),
            ]
        );
    }

    fn erros(text: &str) -> Vec<(LexErrorKind, u32, u32)> {
        let tokens = lex_ok(text);
        tokens
            .errors()
            .iter()
            .map(|e| (e.kind, e.range.start().into(), e.range.end().into()))
            .collect()
    }

    #[test]
    fn palavras_reservadas_sem_diferenciar_maiusculas() {
        assert_eq!(
            kinds("se SE Se"),
            [SeKw, Whitespace, SeKw, Whitespace, SeKw]
        );
        assert_eq!(kinds("gravarnleol"), [GravarNlEolKw]);
    }

    #[test]
    fn palavra_reservada_so_com_o_nome_inteiro() {
        assert_eq!(
            kinds("E075PRO Endereco E e"),
            [Ident, Whitespace, Ident, Whitespace, EKw, Whitespace, EKw]
        );
        assert_eq!(kinds("SeAchou"), [Ident]);
    }

    #[test]
    fn nomes_validos() {
        for nome in [
            "x'y", "_", "_a1", "a_b'c1", "a''", "Achou", "Retorna", "NomeTela",
        ] {
            let tokens = lex_ok(nome);
            assert_eq!(tokens.kinds(), [Ident], "{nome}");
            assert!(tokens.errors().is_empty(), "{nome}");
        }
    }

    #[test]
    fn nome_termina_em_espaco_aspas_ou_simbolo() {
        assert_eq!(kinds("a b"), [Ident, Whitespace, Ident]);
        assert_eq!(kinds("a\r\nb"), [Ident, Newline, Ident]);
        assert_eq!(kinds("a\"").first(), Some(&Ident));
        assert_eq!(kinds("a(").first(), Some(&Ident));
    }

    #[test]
    fn nome_acentuado_vira_um_token_com_erro_em_cada_acento() {
        assert_eq!(kinds("ação"), [Ident]);
        use LexErrorKind::InvalidChar;
        assert_eq!(erros("ação"), [(InvalidChar, 1, 3), (InvalidChar, 3, 5)]);
        assert_eq!(erros("ção"), [(InvalidChar, 0, 2), (InvalidChar, 2, 4)]);
        assert_eq!(erros("José"), [(InvalidChar, 3, 5)]);
    }

    #[test]
    fn caractere_que_nao_e_letra_corta_o_nome() {
        assert_eq!(kinds("a#b"), [Ident, ErrorToken, Ident]);
        assert_eq!(kinds("a😀b"), [Ident, ErrorToken, Ident]);
    }

    #[test]
    fn limite_de_101_caracteres() {
        assert!(erros(&"a".repeat(101)).is_empty());
        assert_eq!(
            erros(&"a".repeat(102)),
            [(LexErrorKind::IdentTooLong, 0, 102)]
        );

        // O acento conta como um caractere, não como os dois bytes do UTF-8.
        let com_acento = format!("{}é", "a".repeat(100));
        assert_eq!(erros(&com_acento), [(LexErrorKind::InvalidChar, 100, 102)]);

        let longo_com_acento = format!("é{}", "a".repeat(101));
        assert_eq!(
            erros(&longo_com_acento),
            [
                (LexErrorKind::IdentTooLong, 0, 103),
                (LexErrorKind::InvalidChar, 0, 2)
            ]
        );
    }

    #[test]
    fn combinacoes_de_caracteres_mantem_as_invariantes() {
        let alfabeto = [
            "", " ", "\t", "\r", "\n", "\r\n", "a", "1", "\"", "'", "@", "/", "*", "\\", "é", "😀",
            "\0", "E", "_", "x'y",
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
