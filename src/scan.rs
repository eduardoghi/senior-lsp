//! Busca de bytes de 8 em 8, com operações de bits num `u64` (SWAR).

const ONES: u64 = 0x0101_0101_0101_0101;
const HIGHS: u64 = 0x8080_8080_8080_8080;

/// Liga o bit alto de cada byte zero de `word`. Um byte logo acima de um zero também pode ligar,
/// por causa do empréstimo da subtração, mas o primeiro zero sempre fica certo, e é só ele que a
/// busca usa.
#[inline]
fn zero_bytes(word: u64) -> u64 {
    word.wrapping_sub(ONES) & !word & HIGHS
}

/// Posição do primeiro byte de `bytes` igual a um dos `needles`.
///
/// Juntar os resultados de vários `needles` com `|` continua certo, porque um falso positivo de
/// um deles sempre fica depois de um acerto verdadeiro do mesmo.
#[inline]
pub fn find_any<const N: usize>(bytes: &[u8], needles: [u8; N]) -> Option<usize> {
    let (chunks, remainder) = bytes.as_chunks::<8>();
    let mut offset = 0;
    for chunk in chunks {
        let word = u64::from_le_bytes(*chunk);
        let mut hits = 0;
        for needle in needles {
            hits |= zero_bytes(word ^ (ONES * u64::from(needle)));
        }
        if hits != 0 {
            return Some(offset + (hits.trailing_zeros() / 8) as usize);
        }
        offset += 8;
    }
    remainder
        .iter()
        .position(|b| needles.contains(b))
        .map(|i| offset + i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simples(bytes: &[u8], needles: &[u8]) -> Option<usize> {
        bytes.iter().position(|b| needles.contains(b))
    }

    #[test]
    fn acha_em_cada_posicao_e_tamanho() {
        for len in 0..40 {
            for pos in 0..=len {
                let mut bytes = vec![b'a'; len];
                if pos < len {
                    bytes[pos] = b'"';
                }
                let esperado = (pos < len).then_some(pos);
                assert_eq!(find_any(&bytes, *b"\"\\"), esperado, "{len} {pos}");
            }
        }
    }

    #[test]
    fn primeiro_acerto_com_falsos_positivos_por_perto() {
        // 0x01 logo depois do alvo é o caso em que o empréstimo liga um byte a mais.
        let bytes = [b'x', b'"', 0x01, b'"', 0x21, 0x23, 0x01, 0x00, b'\\', b'\n'];
        assert_eq!(find_any(&bytes, *b"\""), Some(1));
        assert_eq!(find_any(&bytes, *b"\\\n"), Some(8));
        assert_eq!(find_any(&bytes, [0x00]), Some(7));
    }

    #[test]
    fn igual_a_busca_simples_em_bytes_aleatorios() {
        // Gerador xorshift, para não depender de crate.
        let mut estado: u64 = 0x2545_f491_4f6c_dd1d;
        let mut proximo = move || {
            estado ^= estado << 13;
            estado ^= estado >> 7;
            estado ^= estado << 17;
            estado
        };
        let needles = *b"\"\\\n\r";
        for _ in 0..20_000 {
            let len = (proximo() % 70) as usize;
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    let r = proximo();
                    // Metade das vezes um byte perto dos procurados, metade qualquer um.
                    if r & 1 == 0 {
                        needles[(r >> 8) as usize % 4].wrapping_add((r >> 16) as u8 % 3)
                    } else {
                        (r >> 8) as u8
                    }
                })
                .collect();
            assert_eq!(
                find_any(&bytes, needles),
                simples(&bytes, &needles),
                "{bytes:?}"
            );
        }
    }
}
