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

use crate::SyntaxKind::{
    self, Colon, Comma, Dot, Eq, ErrorToken, Gt, GtEq, Ident, LBrace, LBracket, LParen, Lt, LtEq,
    Minus, MinusMinus, Neq, Newline, Number, Plus, PlusPlus, RBrace, RBracket, RParen, RelRun,
    Semicolon, Slash, Star, Whitespace,
};
use crate::delphi_float;

/// Tamanho máximo de um identificador no compilador (`identificador muito extenso`).
///
/// O compilador aceita 100 caracteres e dá o erro a partir de 101.
const MAX_IDENT_CHARS: usize = 100;

/// O compilador corta o número em 101 caracteres, contando o ponto, e o resto começa outro token.
const MAX_NUMBER_CHARS: usize = 101;

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
    /// Identificador com mais de 100 caracteres.
    IdentTooLong,
    /// Literal de caractere vazio ou com mais de um caractere, como `''` ou `'ab'`.
    MalformedChar,
    /// Número que o compilador recusa (`identificador mal-formado`), como `12abc` ou `1E4933`.
    MalformedNumber,
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
            b'0'..=b'9' => self.number(start),
            b'(' => self.single(LParen),
            b')' => self.single(RParen),
            b'[' => self.single(LBracket),
            b']' => self.single(RBracket),
            b',' => self.single(Comma),
            b';' => self.single(Semicolon),
            b':' => self.single(Colon),
            b'.' => self.single(Dot),
            b'*' => self.single(Star),
            b'/' => self.single(Slash),
            b'+' => self.doubled(b'+', Plus, PlusPlus),
            b'-' => self.doubled(b'-', Minus, MinusMinus),
            b'<' | b'>' | b'=' => self.relational(),
            b'{' => self.brace(start, LBrace),
            b'}' => self.brace(start, RBrace),
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

    /// Lê um número. Ele continua com os mesmos caracteres de um identificador, então `12abc` é um
    /// token só, e o primeiro `.` seguido de dígito vira a parte decimal.
    fn number(&mut self, start: usize) -> SyntaxKind {
        let first_error = self.tokens.errors.len();
        let mut chars = 0;
        let mut decimal = false;
        while chars < MAX_NUMBER_CHARS {
            let Some(&b) = self.bytes.get(self.pos) else {
                break;
            };
            if b == b'.' {
                let next_is_digit = self.bytes.get(self.pos + 1).is_some_and(u8::is_ascii_digit);
                if decimal || !next_is_digit {
                    break;
                }
                decimal = true;
                self.pos += 1;
            } else if b.is_ascii_alphanumeric() || b == b'_' || b == b'\'' {
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

        // Com letra acentuada o compilador já parou no InvalidChar, então não acumula outro erro.
        let has_accent = self.tokens.errors.len() > first_error;
        if !has_accent && !delphi_float::accepts(&self.text[start..self.pos]) {
            self.error(LexErrorKind::MalformedNumber, start);
        }
        Number
    }

    fn single(&mut self, kind: SyntaxKind) -> SyntaxKind {
        self.pos += 1;
        kind
    }

    /// `+` e `-` formam `++` e `--` só com o mesmo caractere logo depois.
    fn doubled(&mut self, ch: u8, one: SyntaxKind, two: SyntaxKind) -> SyntaxKind {
        self.pos += 1;
        if self.bytes.get(self.pos) == Some(&ch) {
            self.pos += 1;
            return two;
        }
        one
    }

    /// O compilador junta `<`, `>` ou `=` com mais um desses e para, então `<<=` vira `<<` e `=`.
    /// Só `<=`, `>=` e `<>` são operadores válidos. As outras duplas o parser recusa.
    fn relational(&mut self) -> SyntaxKind {
        let first = self.bytes[self.pos];
        self.pos += 1;
        let Some(&second) = self
            .bytes
            .get(self.pos)
            .filter(|b| matches!(b, b'<' | b'>' | b'='))
        else {
            return match first {
                b'<' => Lt,
                b'>' => Gt,
                _ => Eq,
            };
        };
        self.pos += 1;
        match (first, second) {
            (b'<', b'=') => LtEq,
            (b'>', b'=') => GtEq,
            (b'<', b'>') => Neq,
            _ => RelRun,
        }
    }

    /// O compilador só reconhece `{` e `}` no começo de um token. Colados no fim de um nome ou
    /// número, eles dão "O Caractere "{" é inválido". O token continua sendo chave para o parser
    /// não perder o bloco.
    fn brace(&mut self, start: usize, kind: SyntaxKind) -> SyntaxKind {
        self.pos += 1;
        let glued = self
            .tokens
            .kinds
            .last()
            .is_some_and(|&k| k == Ident || k == Number || k.is_keyword());
        if glued {
            self.error(LexErrorKind::InvalidChar, start);
        }
        kind
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
    use crate::SyntaxKind::{EKw, GravarNlEolKw, SeKw, SenaoKw};
    use LexErrorKind::{InvalidChar, MalformedNumber};

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
    fn limite_de_100_caracteres() {
        assert!(erros(&"a".repeat(100)).is_empty());
        assert_eq!(
            erros(&"a".repeat(101)),
            [(LexErrorKind::IdentTooLong, 0, 101)]
        );

        // O acento conta como um caractere, não como os dois bytes do UTF-8.
        let com_acento = format!("{}é", "a".repeat(99));
        assert_eq!(erros(&com_acento), [(LexErrorKind::InvalidChar, 99, 101)]);

        let longo_com_acento = format!("é{}", "a".repeat(100));
        assert_eq!(
            erros(&longo_com_acento),
            [
                (LexErrorKind::IdentTooLong, 0, 102),
                (LexErrorKind::InvalidChar, 0, 2)
            ]
        );
    }

    #[test]
    fn numeros_validos() {
        for n in ["0", "10", "007", "3.14", "1E5", "1.5e3", "1E"] {
            let tokens = lex_ok(n);
            assert_eq!(tokens.kinds(), [Number], "{n}");
            assert!(tokens.errors().is_empty(), "{n}");
        }
    }

    #[test]
    fn numero_continua_com_letras_e_vira_um_token_so() {
        assert_eq!(kinds("12abc"), [Number]);
        assert_eq!(erros("12abc"), [(MalformedNumber, 0, 5)]);
        assert_eq!(erros("1_5"), [(MalformedNumber, 0, 3)]);
        assert_eq!(erros("12'3"), [(MalformedNumber, 0, 4)]);
        assert_eq!(erros("1E4933"), [(MalformedNumber, 0, 6)]);
    }

    #[test]
    fn ponto_so_entra_no_numero_se_vier_digito_depois() {
        assert_eq!(kinds("2."), [Number, Dot]);
        assert_eq!(kinds("1.5.3"), [Number, Dot, Number]);
        assert_eq!(kinds("1E-3"), [Number, Minus, Number]);
        assert_eq!(lex_ok("1.5.3").range(0), TextRange::new(0.into(), 3.into()));
    }

    #[test]
    fn numero_cortado_em_101_caracteres() {
        let inteiro = format!("1{}", "0".repeat(101));
        let tokens = lex_ok(&inteiro);
        assert_eq!(tokens.kinds(), [Number, Number]);
        assert_eq!(tokens.range(0), TextRange::new(0.into(), 101.into()));
        assert!(tokens.errors().is_empty());

        let decimal = format!("1.{}", "0".repeat(100));
        assert_eq!(
            lex_ok(&decimal).range(0),
            TextRange::new(0.into(), 101.into())
        );
    }

    #[test]
    fn numero_com_acento_so_leva_o_erro_do_acento() {
        assert_eq!(kinds("1é"), [Number]);
        assert_eq!(erros("1é"), [(InvalidChar, 1, 3)]);
    }

    #[test]
    fn simbolos_de_um_caractere() {
        assert_eq!(
            kinds("()[],;:.*/"),
            [
                LParen, RParen, LBracket, RBracket, Comma, Semicolon, Colon, Dot, Star, Slash
            ]
        );
        assert!(lex_ok("()[],;:.*/").errors().is_empty());
    }

    #[test]
    fn mais_e_menos_dobrados() {
        assert_eq!(kinds("++"), [PlusPlus]);
        assert_eq!(kinds("--"), [MinusMinus]);
        assert_eq!(kinds("+++"), [PlusPlus, Plus]);
        assert_eq!(kinds("+-"), [Plus, Minus]);
        assert_eq!(kinds("+ +"), [Plus, Whitespace, Plus]);
    }

    #[test]
    fn relacionais_juntam_no_maximo_dois() {
        assert_eq!(kinds("<"), [Lt]);
        assert_eq!(kinds(">"), [Gt]);
        assert_eq!(kinds("="), [Eq]);
        assert_eq!(kinds("<="), [LtEq]);
        assert_eq!(kinds(">="), [GtEq]);
        assert_eq!(kinds("<>"), [Neq]);
        for dupla in ["==", "=<", "=>", "><", "<<", ">>"] {
            assert_eq!(kinds(dupla), [RelRun], "{dupla}");
        }
        assert_eq!(kinds("<<="), [RelRun, Eq]);
        assert_eq!(kinds("a<b"), [Ident, Lt, Ident]);
    }

    #[test]
    fn chave_colada_no_fim_de_nome_ou_numero_tem_erro() {
        assert_eq!(kinds("senao{"), [SenaoKw, LBrace]);
        assert_eq!(erros("senao{"), [(InvalidChar, 5, 6)]);
        assert_eq!(kinds("2}"), [Number, RBrace]);
        assert_eq!(erros("2}"), [(InvalidChar, 1, 2)]);
        assert_eq!(erros("a{"), [(InvalidChar, 1, 2)]);
    }

    #[test]
    fn chave_depois_de_espaco_ou_simbolo_nao_tem_erro() {
        for texto in ["senao {", "){", "}{", ";}", "{", "x = 1; }"] {
            assert!(lex_ok(texto).errors().is_empty(), "{texto}");
        }
    }

    #[test]
    fn combinacoes_de_caracteres_mantem_as_invariantes() {
        let alfabeto = [
            "", " ", "\t", "\r", "\n", "\r\n", "a", "1", "\"", "'", "@", "/", "*", "\\", "é", "😀",
            "\0", "E", "_", "x'y", "{", "}", "<", "=", "+", ".",
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
