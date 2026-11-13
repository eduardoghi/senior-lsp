//! Decodificação de cp1252, a codificação dos fontes de regra do ERP.

/// Os caracteres de 0x80 a 0x9F, onde o cp1252 difere do Latin-1. `None` são os 5 bytes que não
/// têm caractere.
const C1: [Option<char>; 32] = [
    Some('\u{20AC}'),
    None,
    Some('\u{201A}'),
    Some('\u{0192}'),
    Some('\u{201E}'),
    Some('\u{2026}'),
    Some('\u{2020}'),
    Some('\u{2021}'),
    Some('\u{02C6}'),
    Some('\u{2030}'),
    Some('\u{0160}'),
    Some('\u{2039}'),
    Some('\u{0152}'),
    None,
    Some('\u{017D}'),
    None,
    None,
    Some('\u{2018}'),
    Some('\u{2019}'),
    Some('\u{201C}'),
    Some('\u{201D}'),
    Some('\u{2022}'),
    Some('\u{2013}'),
    Some('\u{2014}'),
    Some('\u{02DC}'),
    Some('\u{2122}'),
    Some('\u{0161}'),
    Some('\u{203A}'),
    Some('\u{0153}'),
    None,
    Some('\u{017E}'),
    Some('\u{0178}'),
];

/// Converte bytes em cp1252 para texto. Devolve `None` se aparecer um dos 5 bytes sem caractere
/// (0x81, 0x8D, 0x8F, 0x90 e 0x9D).
pub fn decode(bytes: &[u8]) -> Option<String> {
    // Texto sem acento é igual nas duas codificações, e vai numa cópia só.
    if bytes.is_ascii() {
        return String::from_utf8(bytes.to_vec()).ok();
    }
    // Cada byte acentuado vira dois ou três em UTF-8. A folga cobre o texto comum sem o `String`
    // precisar crescer.
    let mut out = String::with_capacity(bytes.len() + bytes.len() / 8);
    let mut rest = bytes;
    while !rest.is_empty() {
        // O trecho sem acento vai de uma vez, e só o byte acentuado é convertido.
        let ascii = rest
            .iter()
            .position(|b| !b.is_ascii())
            .unwrap_or(rest.len());
        out.push_str(std::str::from_utf8(&rest[..ascii]).ok()?);
        let Some(&b) = rest.get(ascii) else {
            break;
        };
        out.push(match b {
            0x80..=0x9F => C1[(b - 0x80) as usize]?,
            _ => char::from(b),
        });
        rest = &rest[ascii + 1..];
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodifica() {
        assert_eq!(decode(b"a\xe7\xe3o").as_deref(), Some("ação"));
        assert_eq!(
            decode(b"\x80 \x93x\x94").as_deref(),
            Some("\u{20AC} \u{201C}x\u{201D}")
        );
    }

    #[test]
    fn recusa_os_bytes_sem_caractere() {
        for b in [0x81, 0x8D, 0x8F, 0x90, 0x9D] {
            assert_eq!(decode(&[b]), None, "{b:#x}");
            // No começo, no meio e no fim de um texto com e sem acento.
            for text in [&b"abc"[..], b"a\xe7\xe3o", b""] {
                let around = [text, &[b], text].concat();
                assert_eq!(decode(&around), None, "{b:#x}");
                assert_eq!(decode(&[&[b], text].concat()), None, "{b:#x}");
                assert_eq!(decode(&[text, &[b]].concat()), None, "{b:#x}");
            }
        }
    }

    /// A conversão um byte por vez, que o `decode` tem que repetir.
    fn simples(bytes: &[u8]) -> Option<String> {
        bytes
            .iter()
            .map(|&b| match b {
                0x80..=0x9F => C1[(b - 0x80) as usize],
                _ => Some(char::from(b)),
            })
            .collect()
    }

    #[test]
    fn da_o_mesmo_que_a_conversao_byte_a_byte() {
        // Cada byte sozinho e cercado de texto sem acento.
        for b in 0..=255u8 {
            assert_eq!(decode(&[b]), simples(&[b]), "{b:#x}");
            let around = [b"abc", &[b][..], b"xyz"].concat();
            assert_eq!(decode(&around), simples(&around), "{b:#x}");
        }
        // Vazio, só ASCII, só acentos, acento nas pontas e seguidos, e um texto comprido.
        let long = [&b"definir alfa aTexto; "[..], b"@ a\xe7\xe3o \x80 @\r\n"]
            .concat()
            .repeat(50);
        for text in [
            &b""[..],
            b"definir numero n;\r\n",
            b"\xe7\xe3\xe9\x80\x99",
            b"\xe9 no come\xe7o e no fim \xe1",
            b"\x93aspas\x94\x85",
            &long,
        ] {
            assert_eq!(decode(text), simples(text), "{text:?}");
            assert!(decode(text).is_some());
        }
    }
}
