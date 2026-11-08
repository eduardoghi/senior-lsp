//! Busca das regras para o teste e o benchmark do corpus.

use std::fs;
use std::path::{Path, PathBuf};

/// Junta todo arquivo `Rule.txt` (sem diferenciar maiúsculas) abaixo de `dir`, em ordem.
pub fn regras(dir: &Path) -> Vec<PathBuf> {
    let mut saida = Vec::new();
    coletar(dir, &mut saida);
    saida.sort();
    saida
}

fn coletar(dir: &Path, saida: &mut Vec<PathBuf>) {
    for entrada in fs::read_dir(dir).unwrap() {
        let caminho = entrada.unwrap().path();
        if caminho.is_dir() {
            if caminho.file_name().is_some_and(|n| n != ".git") {
                coletar(&caminho, saida);
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
