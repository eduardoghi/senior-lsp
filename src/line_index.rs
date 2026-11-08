//! Conversão de posição em bytes para linha e coluna.
//!
//! Os tokens e a árvore guardam posições em bytes de UTF-8. O protocolo dos editores usa linha e
//! coluna, e a coluna é contada em UTF-16 por padrão ou em bytes de UTF-8 quando o editor aceita.

use text_size::TextSize;

/// Como a coluna é contada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnEncoding {
    Utf8,
    Utf16,
}

/// Linha e coluna, as duas a partir de zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

/// Começo de cada linha de um texto. Quebra de linha é `\n`, `\r\n` ou `\r` sozinho, como no
/// compilador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    line_starts: Vec<TextSize>,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let bytes = text.as_bytes();
        let mut line_starts = vec![TextSize::new(0)];
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => line_starts.push(TextSize::new(i as u32 + 1)),
                b'\r' => {
                    if bytes.get(i + 1) == Some(&b'\n') {
                        i += 1;
                    }
                    line_starts.push(TextSize::new(i as u32 + 1));
                }
                _ => {}
            }
            i += 1;
        }
        LineIndex { line_starts }
    }

    /// Linha e coluna de `offset`, que tem que ser uma fronteira de caractere de `text`.
    pub fn line_col(&self, text: &str, offset: TextSize, encoding: ColumnEncoding) -> LineCol {
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let start = usize::from(self.line_starts[line]);
        let prefix = &text[start..usize::from(offset)];
        let col = match encoding {
            ColumnEncoding::Utf8 => prefix.len(),
            // Texto ASCII tem a mesma contagem nas duas codificações, e quase toda linha de
            // regra é ASCII.
            ColumnEncoding::Utf16 if prefix.is_ascii() => prefix.len(),
            ColumnEncoding::Utf16 => prefix.encode_utf16().count(),
        };
        LineCol {
            line: line as u32,
            col: col as u32,
        }
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ColumnEncoding::{Utf8, Utf16};

    fn lc(text: &str, offset: u32, encoding: ColumnEncoding) -> (u32, u32) {
        let p = LineIndex::new(text).line_col(text, TextSize::new(offset), encoding);
        (p.line, p.col)
    }

    #[test]
    fn quebras_de_linha() {
        let text = "a\nb\r\nc\rd";
        assert_eq!(LineIndex::new(text).line_count(), 4);
        assert_eq!(lc(text, 0, Utf16), (0, 0));
        assert_eq!(lc(text, 2, Utf16), (1, 0));
        assert_eq!(lc(text, 5, Utf16), (2, 0));
        assert_eq!(lc(text, 7, Utf16), (3, 0));
        // O \r de um \r\n ainda é da linha de cima.
        assert_eq!(lc(text, 3, Utf16), (1, 1));
    }

    #[test]
    fn acento_e_emoji() {
        let text = "x = 'é';\n\"😀\" #";
        // `é` ocupa 2 bytes e 1 unidade de UTF-16.
        assert_eq!(lc(text, 7, Utf8), (0, 7));
        assert_eq!(lc(text, 7, Utf16), (0, 6));
        // O emoji ocupa 4 bytes e 2 unidades de UTF-16. O offset 17 é o `#`.
        assert_eq!(lc(text, 17, Utf8), (1, 7));
        assert_eq!(lc(text, 17, Utf16), (1, 5));
    }

    #[test]
    fn linha_vazia_e_fim_do_texto() {
        let text = "a\n\n";
        assert_eq!(lc(text, 2, Utf16), (1, 0));
        assert_eq!(lc(text, 3, Utf16), (2, 0));
        assert_eq!(lc("", 0, Utf16), (0, 0));
        assert_eq!(lc("abc", 3, Utf16), (0, 3));
    }
}
