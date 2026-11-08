//! Roda o lexer em todas as regras de uma pasta.
//!
//! O teste procura todo arquivo `Rule.txt` abaixo da pasta em `SENIOR_CORPUS`, em cp1252, e
//! espera que nenhum tenha erro de lexer. Use regras que compilam no ERP, assim qualquer erro é
//! do lexer. Para rodar:
//!
//! ```sh
//! SENIOR_CORPUS=/caminho/das/regras cargo test --release --test corpus -- --ignored --nocapture
//! ```

use std::fs;
use std::path::{Path, PathBuf};

#[test]
#[ignore = "precisa de SENIOR_CORPUS apontando para uma pasta com regras"]
fn regras_do_corpus_sem_erro_de_lexer() {
    let raiz =
        std::env::var_os("SENIOR_CORPUS").expect("defina SENIOR_CORPUS com o caminho das regras");
    let mut arquivos = Vec::new();
    coletar_regras(Path::new(&raiz), &mut arquivos);
    arquivos.sort();
    assert!(!arquivos.is_empty(), "nenhum Rule.txt em {raiz:?}");

    let mut sem_decodificar = Vec::new();
    let mut falhas = Vec::new();
    let mut tokens = 0;
    for caminho in &arquivos {
        let bytes = fs::read(caminho).unwrap();
        let Some(texto) = decodificar_cp1252(&bytes) else {
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

/// Junta todo arquivo `Rule.txt` (sem diferenciar maiúsculas) abaixo de `dir`.
fn coletar_regras(dir: &Path, saida: &mut Vec<PathBuf>) {
    for entrada in fs::read_dir(dir).unwrap() {
        let caminho = entrada.unwrap().path();
        if caminho.is_dir() {
            if caminho.file_name().is_some_and(|n| n != ".git") {
                coletar_regras(&caminho, saida);
            }
        } else if caminho
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("rule.txt"))
        {
            saida.push(caminho);
        }
    }
}

/// Os caracteres de 0x80 a 0x9F no cp1252, onde ele difere do Latin-1. `None` são os 5 bytes que
/// não têm caractere.
const CP1252_80_9F: [Option<char>; 32] = [
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

fn decodificar_cp1252(bytes: &[u8]) -> Option<String> {
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => CP1252_80_9F[(b - 0x80) as usize],
            _ => Some(char::from(b)),
        })
        .collect()
}

#[test]
fn decodificacao_cp1252() {
    assert_eq!(decodificar_cp1252(b"a\xe7\xe3o").as_deref(), Some("ação"));
    assert_eq!(
        decodificar_cp1252(b"\x80 \x93x\x94").as_deref(),
        Some("\u{20AC} \u{201C}x\u{201D}")
    );
    assert_eq!(decodificar_cp1252(b"\x81"), None);
}
