//! Reproduz a validação de número do compilador da Senior.
//!
//! O compilador converte cada número com o `StrToFloat` do Delphi e, se a conversão falhar, dá
//! `identificador mal-formado`. O `StrToFloat` usa o x87
//! com precisão de 64 bits e arredondamento para o mais próximo:
//!
//! 1. Acumula a parte inteira e a decimal dígito a dígito, `x = x * 10` e depois `x = x + dígito`,
//!    arredondando a cada operação.
//! 2. Depois de `E`, lê dígitos do expoente até o valor chegar a 500 ou mais. O que sobrar torna o
//!    texto inválido, por isso `1E5000` falha e `1E1000` passa.
//! 3. Multiplica por 10^(e % 32), 10^(32 * k) e 10^(512 * k), nessa ordem, com `e` sendo o
//!    expoente menos o número de casas decimais. Com `e` negativo divide pelas mesmas constantes.
//! 4. Só recusa se a flag de overflow ou de operação inválida acender.
//!
//! Como cada passo arredonda, o mesmo valor pode passar escrito com 20 dígitos e falhar com 22.
//! Os casos dos testes foram conferidos no compilador.

/// Número de ponto flutuante de 80 bits do x87, só com o que o `StrToFloat` usa. Vale
/// `m * 2^q`, com `m` normalizado (bit 63 ligado) ou zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ext {
    m: u64,
    q: i32,
}

/// Maior expoente sem viés de um Extended finito.
const MAX_EXP: i32 = 16383;

/// O valor do expoente para de crescer ao chegar aqui.
const EXP_DIGITS_STOP: u32 = 500;

impl Ext {
    const ZERO: Ext = Ext { m: 0, q: 0 };

    const fn new(m: u64, q: i32) -> Ext {
        Ext { m, q }
    }

    fn from_u64(n: u64) -> Ext {
        if n == 0 {
            return Ext::ZERO;
        }
        let lz = n.leading_zeros();
        Ext::new(n << lz, -(lz as i32))
    }

    /// Arredonda `p * 2^q` para 64 bits de mantissa, para o mais próximo e empate para o par.
    /// `sticky` indica bits não nulos que já ficaram de fora abaixo de `p`. Devolve `None` se o
    /// resultado passar do maior Extended finito.
    fn round(p: u128, mut q: i32, sticky: bool) -> Option<Ext> {
        if p == 0 {
            return Some(Ext::ZERO);
        }
        let top = 127 - p.leading_zeros() as i32;
        let mut m;
        if top <= 63 {
            m = (p << (63 - top)) as u64;
            q -= 63 - top;
        } else {
            let k = (top - 63) as u32;
            m = (p >> k) as u64;
            let rest = p & ((1u128 << k) - 1);
            let half = 1u128 << (k - 1);
            q += k as i32;
            let up = rest > half || (rest == half && (sticky || m & 1 == 1));
            if up {
                m = m.wrapping_add(1);
                if m == 0 {
                    m = 1 << 63;
                    q += 1;
                }
            }
        }
        if q + 63 > MAX_EXP {
            return None;
        }
        Some(Ext::new(m, q))
    }

    fn mul(self, other: Ext) -> Option<Ext> {
        if self.m == 0 || other.m == 0 {
            return Some(Ext::ZERO);
        }
        Ext::round(self.m as u128 * other.m as u128, self.q + other.q, false)
    }

    fn add(self, other: Ext) -> Option<Ext> {
        if self.m == 0 {
            return Some(other);
        }
        if other.m == 0 {
            return Some(self);
        }
        let (hi, lo) = if self.q >= other.q {
            (self, other)
        } else {
            (other, self)
        };
        // Desloca 63 bits e não 64 para a soma caber em u128.
        let d = (hi.q - lo.q) as u32;
        let a = (hi.m as u128) << 63;
        let (b, sticky) = if d <= 63 {
            ((lo.m as u128) << (63 - d), false)
        } else if d - 63 < 64 {
            let s = d - 63;
            ((lo.m >> s) as u128, lo.m & ((1u64 << s) - 1) != 0)
        } else {
            (0, true)
        };
        Ext::round(a + b, hi.q - 63, sticky)
    }
}

/// Diz se o `StrToFloat` do compilador aceita o texto de um token de número. O `.` faz o papel
/// do separador decimal, que o compilador troca antes de converter.
pub fn accepts(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut x = Ext::ZERO;

    let digits = |i: &mut usize, x: &mut Ext| -> Option<i32> {
        let mut count = 0;
        while let Some(&b) = bytes.get(*i).filter(|b| b.is_ascii_digit()) {
            *x = x
                .mul(Ext::from_u64(10))?
                .add(Ext::from_u64((b - b'0') as u64))?;
            *i += 1;
            count += 1;
        }
        Some(count)
    };

    let Some(_) = digits(&mut i, &mut x) else {
        return false;
    };
    let mut exp: i32 = 0;
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        let Some(decimals) = digits(&mut i, &mut x) else {
            return false;
        };
        exp = -decimals;
    }
    if i == 0 {
        return false;
    }
    if matches!(bytes.get(i), Some(b'E' | b'e')) {
        i += 1;
        let mut value = 0u32;
        while let Some(&b) = bytes.get(i).filter(|b| b.is_ascii_digit()) {
            value = value * 10 + (b - b'0') as u32;
            i += 1;
            if value >= EXP_DIGITS_STOP {
                break;
            }
        }
        exp += value as i32;
    }
    if i != bytes.len() {
        return false;
    }
    // Com expoente negativo a conversão divide, o que não acende overflow. Underflow e perda de
    // precisão não contam como falha.
    exp <= 0 || pow10_mul(x, exp as u32).is_some()
}

/// `x * 10^e` como o `StrToFloat` faz, para `e` positivo e menor que 5120.
fn pow10_mul(x: Ext, e: u32) -> Option<Ext> {
    let mut x = x.mul(POW10_LOW[(e & 31) as usize])?;
    let mid = (e >> 5) & 15;
    if mid != 0 {
        x = x.mul(POW10_MID[mid as usize])?;
    }
    let high = e >> 9;
    if high != 0 {
        x = x.mul(POW10_HIGH[high as usize])?;
    }
    Some(x)
}

// Potências de 10 que o `StrToFloat` usa, arredondadas para Extended e geradas com aritmética
// exata.

/// 10^k para k de 0 a 31.
const POW10_LOW: [Ext; 32] = [
    Ext::new(0x8000000000000000, -63), // 10^0
    Ext::new(0xa000000000000000, -60), // 10^1
    Ext::new(0xc800000000000000, -57), // 10^2
    Ext::new(0xfa00000000000000, -54), // 10^3
    Ext::new(0x9c40000000000000, -50), // 10^4
    Ext::new(0xc350000000000000, -47), // 10^5
    Ext::new(0xf424000000000000, -44), // 10^6
    Ext::new(0x9896800000000000, -40), // 10^7
    Ext::new(0xbebc200000000000, -37), // 10^8
    Ext::new(0xee6b280000000000, -34), // 10^9
    Ext::new(0x9502f90000000000, -30), // 10^10
    Ext::new(0xba43b74000000000, -27), // 10^11
    Ext::new(0xe8d4a51000000000, -24), // 10^12
    Ext::new(0x9184e72a00000000, -20), // 10^13
    Ext::new(0xb5e620f480000000, -17), // 10^14
    Ext::new(0xe35fa931a0000000, -14), // 10^15
    Ext::new(0x8e1bc9bf04000000, -10), // 10^16
    Ext::new(0xb1a2bc2ec5000000, -7),  // 10^17
    Ext::new(0xde0b6b3a76400000, -4),  // 10^18
    Ext::new(0x8ac7230489e80000, 0),   // 10^19
    Ext::new(0xad78ebc5ac620000, 3),   // 10^20
    Ext::new(0xd8d726b7177a8000, 6),   // 10^21
    Ext::new(0x878678326eac9000, 10),  // 10^22
    Ext::new(0xa968163f0a57b400, 13),  // 10^23
    Ext::new(0xd3c21bcecceda100, 16),  // 10^24
    Ext::new(0x84595161401484a0, 20),  // 10^25
    Ext::new(0xa56fa5b99019a5c8, 23),  // 10^26
    Ext::new(0xcecb8f27f4200f3a, 26),  // 10^27
    Ext::new(0x813f3978f8940984, 30),  // 10^28
    Ext::new(0xa18f07d736b90be5, 33),  // 10^29
    Ext::new(0xc9f2c9cd04674edf, 36),  // 10^30
    Ext::new(0xfc6f7c4045812296, 39),  // 10^31
];

/// 10^(32k) para k de 0 a 15. A posição 0 nunca é usada.
const POW10_MID: [Ext; 16] = [
    Ext::new(0x8000000000000000, -63),  // 10^0
    Ext::new(0x9dc5ada82b70b59e, 43),   // 10^32
    Ext::new(0xc2781f49ffcfa6d5, 149),  // 10^64
    Ext::new(0xefb3ab16c59b14a3, 255),  // 10^96
    Ext::new(0x93ba47c980e98ce0, 362),  // 10^128
    Ext::new(0xb616a12b7fe617aa, 468),  // 10^160
    Ext::new(0xe070f78d3927556b, 574),  // 10^192
    Ext::new(0x8a5296ffe33cc930, 681),  // 10^224
    Ext::new(0xaa7eebfb9df9de8e, 787),  // 10^256
    Ext::new(0xd226fc195c6a2f8c, 893),  // 10^288
    Ext::new(0x81842f29f2cce376, 1000), // 10^320
    Ext::new(0x9fa42700db900ad2, 1106), // 10^352
    Ext::new(0xc4c5e310aef8aa17, 1212), // 10^384
    Ext::new(0xf28a9c07e9b09c59, 1318), // 10^416
    Ext::new(0x957a4ae1ebf7f3d4, 1425), // 10^448
    Ext::new(0xb83ed8dc0795a262, 1531), // 10^480
];

/// 10^(512k) para k de 0 a 9. A posição 0 nunca é usada.
const POW10_HIGH: [Ext; 10] = [
    Ext::new(0x8000000000000000, -63),   // 10^0
    Ext::new(0xe319a0aea60e91c7, 1637),  // 10^512
    Ext::new(0xc976758681750c17, 3338),  // 10^1024
    Ext::new(0xb2b8353b3993a7e4, 5039),  // 10^1536
    Ext::new(0x9e8b3b5dc53d5de5, 6740),  // 10^2048
    Ext::new(0x8ca554c020a1f0a6, 8441),  // 10^2560
    Ext::new(0xf9895d25d88b5a8b, 10141), // 10^3072
    Ext::new(0xdd5dc8a2bf27f3f8, 11842), // 10^3584
    Ext::new(0xc46052028a20979b, 13543), // 10^4096
    Ext::new(0xae3511626ed559f0, 15244), // 10^4608
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabela_baixa_bate_com_10_elevado_a_k() {
        for (k, &esperado) in POW10_LOW.iter().enumerate() {
            let exato = 10u128.pow(k as u32);
            assert_eq!(Ext::round(exato, 0, false), Some(esperado), "10^{k}");
        }
    }

    #[test]
    fn arredonda_para_o_par_no_empate() {
        // 2^64 + 1 precisa de 65 bits e fica no meio entre 2^64 e 2^64 + 2. Vai para o par.
        let meio = (1u128 << 64) + 1;
        assert_eq!(Ext::round(meio, 0, false), Some(Ext::new(1 << 63, 1)));
        // Com bits não nulos abaixo, deixa de ser empate e sobe.
        assert_eq!(Ext::round(meio, 0, true), Some(Ext::new((1 << 63) + 1, 1)));
    }

    /// Casos rodados no compilador, com o resultado que ele deu.
    const CASOS_DO_ERP: &[(&str, bool)] = &[
        ("1.5", true),
        ("1E5", true),
        ("1E", true),
        ("1.5E", true),
        ("1.5E3", true),
        ("2e2", true),
        ("007", true),
        ("1E309", true),
        ("1E4932", true),
        ("1.1E4932", true),
        ("12abc", false),
        ("1_5", false),
        ("12'3", false),
        ("1EE", false),
        ("1.2E4932", false),
        ("1E4933", false),
        ("1E5000", false),
        ("1.189731495357231765E4932", true),
        ("1.18973149535723176502E4932", true),
        ("1.18973149535723176503E4932", true),
        ("1.189731495357231766E4932", false),
        ("1.18973149535723177E4932", false),
        ("11.8973149535723176502E4931", true),
        ("0.118973149535723176502E4933", true),
        ("118973149535723176502E4912", true),
        ("0011.8973149535723176502E4931", true),
        ("1.1897314953572317651E4932", true),
        ("1.18973149535723176509E4932", true),
        ("1.18973149535723176599999999E4932", false),
        ("1.1897314953572317652E4932", false),
        ("1.189731495357231765099E4932", true),
        ("1.189731495357231765100E4932", false),
        ("1.189731495357231765099999E4932", true),
        ("1.189731495357231765100000E4932", false),
        ("1E500", true),
        ("1E0499", true),
        ("1E1000", true),
        ("1E10000", false),
        ("1E0005000", false),
        ("1E4999", false),
        ("0.0001E4936", true),
        ("1E00000000004932", true),
    ];

    #[test]
    fn mesmo_resultado_que_o_compilador() {
        for &(texto, esperado) in CASOS_DO_ERP {
            assert_eq!(accepts(texto), esperado, "{texto}");
        }
    }

    #[test]
    fn numeros_de_101_digitos_que_o_erp_aceitou() {
        assert!(accepts(&format!("1{}", "0".repeat(100))));
        assert!(accepts(&format!("1.{}", "0".repeat(99))));
    }
}
