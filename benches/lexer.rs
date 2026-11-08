//! Vazão do lexer em bytes por segundo.
//!
//! Mede um texto sintético que fica no repositório e, se `SENIOR_CORPUS` apontar para uma pasta
//! com regras `Rule.txt`, também todas elas, uma por vez. Para comparar uma mudança:
//!
//! ```sh
//! cargo bench --bench lexer -- --save-baseline antes
//! # muda o código
//! cargo bench --bench lexer -- --baseline antes
//! ```

use std::hint::black_box;
use std::path::Path;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};

#[path = "../tests/support/mod.rs"]
mod support;

/// Um trecho com o que aparece numa regra, como declarações, cursor com SQL em várias linhas,
/// condições, strings, comentários, números e literais de caractere.
const TRECHO: &str = concat!(
    "@ Soma os itens do pedido e calcula o desconto @\r\n",
    "Definir Alfa aMsg;\r\n",
    "Definir Alfa aCodPro;\r\n",
    "Definir Numero nTotal;\r\n",
    "Definir Numero nQtd;\r\n",
    "Definir Cursor Cur_Itens;\r\n",
    "Definir Funcao CalculaDesconto(Numero End nValor);\r\n",
    "\r\n",
    "nTotal = 0;\r\n",
    "Cur_Itens.Sql \"SELECT CODPRO, QTDPED, PREUNI \\\r\n",
    "                 FROM E120IPD \\\r\n",
    "                WHERE CODEMP = :CodEmp AND NUMPED = :NumPed\";\r\n",
    "Cur_Itens.AbrirCursor();\r\n",
    "Enquanto (Cur_Itens.Achou) {\r\n",
    "    nQtd = Cur_Itens.QtdPed;\r\n",
    "    Se ((nQtd > 0) e (Cur_Itens.PreUni <> 0)) {\r\n",
    "        nTotal = nTotal + nQtd * Cur_Itens.PreUni;\r\n",
    "    } Senao {\r\n",
    "        aCodPro = Cur_Itens.CodPro;\r\n",
    "        aMsg = \"Item sem quantidade: \" + aCodPro;\r\n",
    "    }\r\n",
    "    /* O desconto depende da faixa do total,\r\n",
    "       calculada a cada item */\r\n",
    "    CalculaDesconto(nTotal);\r\n",
    "    Cur_Itens.Proximo();\r\n",
    "}\r\n",
    "Cur_Itens.FecharCursor();\r\n",
    "Se ((nTotal >= 1.5E3) ou (aMsg <> \"\")) {\r\n",
    "    VSAltPrz = 'S';\r\n",
    "}\r\n",
    "\r\n",
);

/// Repete o trecho até passar de 1 MB.
fn texto_sintetico() -> String {
    let repeticoes = 1_000_000 / TRECHO.len() + 1;
    TRECHO.repeat(repeticoes)
}

fn regras_do_corpus() -> Vec<String> {
    let Some(raiz) = std::env::var_os("SENIOR_CORPUS") else {
        return Vec::new();
    };
    support::regras(Path::new(&raiz))
        .iter()
        .filter_map(|c| senior_lsp::cp1252::decode(&std::fs::read(c).unwrap()))
        .collect()
}

fn lexer(c: &mut Criterion) {
    let sintetico = texto_sintetico();
    let tokens = senior_lsp::lex(&sintetico).unwrap();
    assert!(
        tokens.errors().is_empty(),
        "o texto sintético tem erro de lexer"
    );

    let mut grupo = c.benchmark_group("lexer");
    grupo.throughput(Throughput::Bytes(sintetico.len() as u64));
    grupo.bench_function("sintetico", |b| {
        b.iter(|| senior_lsp::lex(black_box(&sintetico)).unwrap())
    });

    let regras = regras_do_corpus();
    if !regras.is_empty() {
        let bytes: usize = regras.iter().map(String::len).sum();
        grupo.throughput(Throughput::Bytes(bytes as u64));
        grupo.bench_function("corpus", |b| {
            b.iter(|| {
                for regra in &regras {
                    black_box(senior_lsp::lex(black_box(regra)).unwrap());
                }
            })
        });
    }
    grupo.finish();
}

criterion_group!(benches, lexer);
criterion_main!(benches);
