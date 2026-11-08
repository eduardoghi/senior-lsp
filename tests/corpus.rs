//! Roda o lexer em todas as regras de uma pasta.
//!
//! O teste procura todo arquivo `Rule.txt` abaixo da pasta em `SENIOR_CORPUS`, em cp1252, e
//! espera que nenhum tenha erro de lexer. Use regras que compilam no ERP, assim qualquer erro é
//! do lexer. Para rodar:
//!
//! ```sh
//! SENIOR_CORPUS=/caminho/das/regras cargo test --release --test corpus -- --ignored --nocapture
//! ```

mod support;

use std::fs;
use std::path::Path;

#[test]
#[ignore = "precisa de SENIOR_CORPUS apontando para uma pasta com regras"]
fn regras_do_corpus_sem_erro_de_lexer() {
    let raiz =
        std::env::var_os("SENIOR_CORPUS").expect("defina SENIOR_CORPUS com o caminho das regras");
    let arquivos = support::regras(Path::new(&raiz));
    assert!(!arquivos.is_empty(), "nenhum Rule.txt em {raiz:?}");

    let mut sem_decodificar = Vec::new();
    let mut falhas = Vec::new();
    let mut tokens = 0;
    for caminho in &arquivos {
        let bytes = fs::read(caminho).unwrap();
        let Some(texto) = senior_lsp::cp1252::decode(&bytes) else {
            sem_decodificar.push(caminho.display().to_string());
            continue;
        };
        let lexado = senior_lsp::lex(&texto).unwrap();
        tokens += lexado.len();

        let junto: String = (0..lexado.len()).map(|i| lexado.text(i, &texto)).collect();
        if junto != texto {
            falhas.push(format!("{}: a reconstrução diverge", caminho.display()));
        }
        for erro in lexado.errors() {
            let inicio: usize = erro.range.start().into();
            let linha = texto[..inicio].matches('\n').count() + 1;
            falhas.push(format!("{}:{linha}: {:?}", caminho.display(), erro.kind));
        }
    }

    eprintln!(
        "{} arquivos, {} lidos, {} sem decodificar, {tokens} tokens",
        arquivos.len(),
        arquivos.len() - sem_decodificar.len(),
        sem_decodificar.len(),
    );
    assert!(
        sem_decodificar.is_empty(),
        "fora do cp1252:\n{}",
        sem_decodificar.join("\n")
    );
    assert!(
        falhas.is_empty(),
        "{} falhas, as primeiras:\n{}",
        falhas.len(),
        falhas[..falhas.len().min(20)].join("\n")
    );
}
