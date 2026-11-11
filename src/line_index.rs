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

    /// Posição em bytes da linha e coluna do editor. Coluna depois do fim da linha fica no fim
    /// dela, e linha depois da última fica no fim do texto, como o protocolo pede.
    pub fn offset(&self, text: &str, lc: LineCol, encoding: ColumnEncoding) -> TextSize {
        let Some(&start) = self.line_starts.get(lc.line as usize) else {
            return TextSize::of(text);
        };
        let start = usize::from(start);
        let end = self
            .line_starts
            .get(lc.line as usize + 1)
            .map_or(text.len(), |&e| usize::from(e));
        let line = text[start..end].trim_end_matches(['\r', '\n']);
        let mut col = lc.col as usize;
        let mut bytes = 0;
        for ch in line.chars() {
            let width = match encoding {
                ColumnEncoding::Utf8 => ch.len_utf8(),
                ColumnEncoding::Utf16 => ch.len_utf16(),
            };
            if col < width {
                break;
            }
            col -= width;
            bytes += ch.len_utf8();
        }
        TextSize::new((start + bytes) as u32)
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Linha, a partir de zero, que contém `offset`.
    pub fn line_of(&self, offset: TextSize) -> usize {
        self.line_starts.partition_point(|&start| start <= offset) - 1
    }

    /// Posição em bytes do começo da linha, ou o fim do texto para a linha depois da última.
    pub fn line_start(&self, line: usize, text: &str) -> TextSize {
        self.line_starts
            .get(line)
            .copied()
            .unwrap_or_else(|| TextSize::of(text))
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
    fn linha_de_uma_posicao_e_comeco_da_linha() {
        let text = "a\nbc\r\nd";
        let index = LineIndex::new(text);
        assert_eq!(index.line_of(TextSize::new(0)), 0);
        assert_eq!(index.line_of(TextSize::new(3)), 1);
        assert_eq!(index.line_of(TextSize::new(7)), 2);
        assert_eq!(index.line_start(1, text), TextSize::new(2));
        assert_eq!(index.line_start(2, text), TextSize::new(6));
        assert_eq!(index.line_start(3, text), TextSize::of(text));
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
    fn offset_volta_da_linha_e_coluna() {
        let text = "x = 'é';\r\n\"😀\" #\nfim";
        let index = LineIndex::new(text);
        let off = |line, col, enc| u32::from(index.offset(text, LineCol { line, col }, enc));
        assert_eq!(off(0, 6, Utf16), 7);
        assert_eq!(off(0, 7, Utf8), 7);
        assert_eq!(off(1, 5, Utf16), 18);
        assert_eq!(off(1, 7, Utf8), 18);
        // Coluna no meio do emoji fica antes dele, coluna além da linha fica no fim dela.
        assert_eq!(off(1, 2, Utf16), 12);
        assert_eq!(off(0, 99, Utf16), 9);
        assert_eq!(off(9, 0, Utf16), text.len() as u32);
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
