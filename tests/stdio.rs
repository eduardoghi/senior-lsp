//! Roda o executável de verdade e conversa com ele pelo stdin e pelo stdout.
//!
//! Os testes em memória não pegam uma saída acidental no stdout, que corromperia o protocolo.
//! Aqui toda mensagem lida tem que ser um quadro válido, com o cabeçalho `Content-Length`.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn send(stdin: &mut impl Write, message: &Value) {
    let body = message.to_string();
    write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    stdin.flush().unwrap();
}

/// Lê um quadro e falha se vier qualquer coisa fora do formato do protocolo.
fn recv(stdout: &mut impl BufRead) -> Value {
    let mut length = None;
    loop {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "o servidor fechou o stdout");
        let line = line.trim_end_matches("\r\n");
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(": ").expect("cabeçalho fora do formato");
        if name.eq_ignore_ascii_case("Content-Length") {
            length = Some(value.parse::<usize>().unwrap());
        }
    }
    let mut body = vec![0; length.expect("quadro sem Content-Length")];
    stdout.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[test]
fn conversa_pelo_stdio_e_encerra_limpo() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_senior-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                             "params": {"capabilities": {}}}),
    );
    let init = recv(&mut stdout);
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "senior-lsp");

    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    );
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": "file:///r/Rule.txt", "languageId": "lsp-senior",
                                    "version": 1, "text": "x = 1"}}}),
    );
    let diag = recv(&mut stdout);
    assert_eq!(diag["method"], "textDocument/publishDiagnostics");
    assert_eq!(diag["params"]["diagnostics"][0]["code"], "SEN020");

    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
    );
    assert_eq!(recv(&mut stdout)["id"], 2);
    send(&mut stdin, &json!({"jsonrpc": "2.0", "method": "exit"}));

    assert!(child.wait().unwrap().success());
    // Depois do exit, nada mais pode ter sido escrito no stdout.
    let mut resto = Vec::new();
    stdout.read_to_end(&mut resto).unwrap();
    assert!(resto.is_empty(), "saída depois do exit: {resto:?}");
}
