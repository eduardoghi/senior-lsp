//! Vazão do lexer e do parser em bytes por segundo.
//!
//! Mede regras sintéticas que ficam no repositório e, se `SENIOR_CORPUS` apontar para uma pasta
//! com regras `Rule.txt`, também todas elas, uma por vez. O grupo `parser` inclui o lexer. Para
//! comparar uma mudança:
//!
//! ```sh
//! cargo bench --bench vazao -- --save-baseline antes
//! # muda o código
//! cargo bench --bench vazao -- --baseline antes
//! ```

use std::hint::black_box;
use std::path::Path;

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, Throughput, criterion_group, criterion_main};

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

/// Regras sintéticas de uns 6 KB, o tamanho médio de uma regra real, somando 1 MB. Um texto
/// único de 1 MB variava demais entre rodadas com o mesmo código.
fn regras_sinteticas() -> Vec<String> {
    let regra = TRECHO.repeat(6_000 / TRECHO.len() + 1);
    let quantas = 1_000_000 / regra.len() + 1;
    vec![regra; quantas]
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
    let sinteticas = regras_sinteticas();
    assert!(
        senior_lsp::lex(&sinteticas[0]).unwrap().errors().is_empty(),
        "o texto sintético tem erro de lexer"
    );

    let tree = senior_lsp::parse(&sinteticas[0]).unwrap();
    assert!(
        tree.errors().is_empty(),
        "o texto sintético tem erro de parser"
    );
    let corpus = regras_do_corpus();

    let mut grupo = c.benchmark_group("lexer");
    medir(&mut grupo, "sintetico", &sinteticas, |r| {
        black_box(senior_lsp::lex(r).unwrap());
    });
    if !corpus.is_empty() {
        medir(&mut grupo, "corpus", &corpus, |r| {
            black_box(senior_lsp::lex(r).unwrap());
        });
    }
    grupo.finish();

    let mut grupo = c.benchmark_group("parser");
    medir(&mut grupo, "sintetico", &sinteticas, |r| {
        black_box(senior_lsp::parse(r).unwrap());
    });
    if !corpus.is_empty() {
        medir(&mut grupo, "corpus", &corpus, |r| {
            black_box(senior_lsp::parse(r).unwrap());
        });
    }
    grupo.finish();
}

fn medir(
    grupo: &mut BenchmarkGroup<'_, WallTime>,
    nome: &str,
    regras: &[String],
    analisar: impl Fn(&str),
) {
    let bytes: usize = regras.iter().map(String::len).sum();
    grupo.throughput(Throughput::Bytes(bytes as u64));
    grupo.bench_function(nome, |b| {
        b.iter(|| {
            for regra in regras {
                analisar(black_box(regra));
            }
        })
    });
}

criterion_group!(benches, lexer);
criterion_main!(benches);
