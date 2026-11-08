//! Roda o lexer e o parser em todas as regras de uma pasta.
//!
//! O teste procura todo arquivo `Rule.txt` abaixo da pasta em `SENIOR_CORPUS`, em cp1252, e
//! espera que nenhum tenha erro de lexer ou de parser nem trecho que o parser não entendeu. Use
//! regras que compilam no ERP, assim qualquer erro é do lexer ou do parser. Para rodar:
//!
//! ```sh
//! SENIOR_CORPUS=/caminho/das/regras cargo test --release --test corpus -- --ignored --nocapture
//! ```

mod support;

use std::fs;
use std::path::Path;

use senior_lsp::{Element, SyntaxKind, Tree};

#[test]
#[ignore = "precisa de SENIOR_CORPUS apontando para uma pasta com regras"]
fn regras_do_corpus_sem_erro() {
    let raiz =
        std::env::var_os("SENIOR_CORPUS").expect("defina SENIOR_CORPUS com o caminho das regras");
    let arquivos = support::regras(Path::new(&raiz));
    assert!(!arquivos.is_empty(), "nenhum Rule.txt em {raiz:?}");

    let mut sem_decodificar = Vec::new();
    let mut falhas = Vec::new();
    let (mut tokens, mut nos) = (0, 0);
    for caminho in &arquivos {
        let bytes = fs::read(caminho).unwrap();
        let Some(texto) = senior_lsp::cp1252::decode(&bytes) else {
            sem_decodificar.push(caminho.display().to_string());
            continue;
        };
        let tree = senior_lsp::parse(&texto).unwrap();
        tokens += tree.tokens().len();

        let linha = |inicio: usize| texto[..inicio].matches('\n').count() + 1;
        for erro in tree.tokens().errors() {
            let l = linha(erro.range.start().into());
            falhas.push(format!("{}:{l}: lexer {:?}", caminho.display(), erro.kind));
        }
        for erro in tree.errors() {
            let l = linha(erro.range.start().into());
            falhas.push(format!("{}:{l}: parser {:?}", caminho.display(), erro.kind));
        }

        let (reconstruido, contagem, nos_de_erro) = percorrer(&tree, &texto);
        nos += contagem;
        if reconstruido != texto {
            falhas.push(format!("{}: a reconstrução diverge", caminho.display()));
        }
        for inicio in nos_de_erro {
            let l = linha(inicio);
            falhas.push(format!("{}:{l}: trecho não entendido", caminho.display()));
        }
    }

    eprintln!(
        "{} arquivos, {} lidos, {} sem decodificar, {tokens} tokens, {nos} nós",
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

/// Percorre a árvore com uma pilha, sem recursão, e devolve o texto reconstruído, o número de nós
/// e onde começa cada `ErrorNode`.
fn percorrer(tree: &Tree, texto: &str) -> (String, usize, Vec<usize>) {
    let mut saida = String::with_capacity(texto.len());
    let mut nos = 0;
    let mut nos_de_erro = Vec::new();
    // Os elementos de cada nó entram na pilha de trás para frente, para sair na ordem do texto.
    let mut pilha = vec![Element::Node(tree.root())];
    while let Some(elemento) = pilha.pop() {
        match elemento {
            Element::Token(i) => saida.push_str(tree.tokens().text(i, texto)),
            Element::Node(no) => {
                nos += 1;
                if tree.kind(no) == SyntaxKind::ErrorNode {
                    nos_de_erro.push(tree.text_range(no).start().into());
                }
                let filhos: Vec<_> = tree.elements(no).collect();
                pilha.extend(filhos.into_iter().rev());
            }
        }
    }
    (saida, nos, nos_de_erro)
}
