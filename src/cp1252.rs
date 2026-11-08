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
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => C1[(b - 0x80) as usize],
            _ => Some(char::from(b)),
        })
        .collect()
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
        }
    }
}
