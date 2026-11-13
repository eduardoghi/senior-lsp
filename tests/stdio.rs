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

/// Sobe o executável e faz o `initialize`.
fn start() -> (
    std::process::Child,
    std::process::ChildStdin,
    BufReader<std::process::ChildStdout>,
) {
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
    assert_eq!(recv(&mut stdout)["id"], 1);
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    );
    (child, stdin, stdout)
}

#[test]
fn respostas_erros_e_notificacoes_saem_como_quadros_validos() {
    let (mut child, mut stdin, mut stdout) = start();
    let uri = "file:///r/Rule.txt";
    // Texto com acento, para o tamanho do quadro ser contado em bytes.
    let text = "definir alfa aAção;\naAção = \"çé😀\";\nMensagem(Retorna, aAção);\nx = ;";
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": {"textDocument": {"uri": uri, "languageId": "lsp-senior",
                                            "version": 1, "text": text}}}),
    );
    let diag = recv(&mut stdout);
    assert_eq!(diag["jsonrpc"], "2.0");
    assert_eq!(diag["method"], "textDocument/publishDiagnostics");
    assert_eq!(diag["params"]["uri"], uri);
    assert_eq!(diag["params"]["version"], 1);
    assert!(!diag["params"]["diagnostics"].as_array().unwrap().is_empty());

    // Resultado com conteúdo, com id numérico.
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/hover",
                "params": {"textDocument": {"uri": uri},
                           "position": {"line": 1, "character": 1}}}),
    );
    let hover = recv(&mut stdout);
    assert_eq!(hover["jsonrpc"], "2.0");
    assert_eq!(hover["id"], 2);
    let value = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(value.contains("definir alfa aAção;"), "{value}");
    assert!(hover.get("error").is_none());

    // Resultado nulo, com id em texto, que volta igual.
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": "a-\"1\"", "method": "textDocument/hover",
                "params": {"textDocument": {"uri": "file:///r/fechado.txt"},
                           "position": {"line": 0, "character": 0}}}),
    );
    let closed = recv(&mut stdout);
    assert_eq!(closed["id"], "a-\"1\"");
    assert!(closed["result"].is_null() && closed.get("error").is_none());
    assert!(closed.as_object().unwrap().contains_key("result"));

    // Erros: parâmetros fora do formato e método que não existe.
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 3, "method": "textDocument/hover", "params": {}}),
    );
    let invalid = recv(&mut stdout);
    assert_eq!(invalid["id"], 3);
    assert_eq!(invalid["error"]["code"], -32602);
    assert!(
        invalid["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("parâmetros inválidos")
    );
    assert!(invalid.get("result").is_none());
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 4, "method": "textDocument/formatting", "params": {}}),
    );
    let unknown = recv(&mut stdout);
    assert_eq!(unknown["id"], 4);
    assert_eq!(unknown["error"]["code"], -32601);

    // O comando publica os diagnósticos antes de responder, e a ordem se mantém na saída.
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 5, "method": "workspace/executeCommand",
                "params": {"command": "senior-lsp.tipoRegra", "arguments": [uri, "tela"]}}),
    );
    assert_eq!(
        recv(&mut stdout)["method"],
        "textDocument/publishDiagnostics"
    );
    let done = recv(&mut stdout);
    assert_eq!(done["id"], 5);
    assert!(done["result"].is_null());

    // Resposta grande, bem maior do que o buffer do stdout e do que o pipe.
    let mut big = String::new();
    for n in 0..4000 {
        big.push_str(&format!("definir numero nVariavelDeNomeComprido{n};\n"));
    }
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "textDocument/didChange",
                "params": {"textDocument": {"uri": uri, "version": 2},
                           "contentChanges": [{"text": big}]}}),
    );
    assert_eq!(recv(&mut stdout)["params"]["version"], 2);
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 6, "method": "textDocument/documentSymbol",
                "params": {"textDocument": {"uri": uri}}}),
    );
    let symbols = recv(&mut stdout);
    assert_eq!(symbols["id"], 6);
    let symbols = symbols["result"].as_array().unwrap();
    assert_eq!(symbols.len(), 4000);
    assert_eq!(symbols[3999]["name"], "nVariavelDeNomeComprido3999");
    assert_eq!(symbols[3999]["range"]["start"]["line"], 3999);

    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 7, "method": "shutdown"}),
    );
    let shutdown = recv(&mut stdout);
    assert_eq!(shutdown["id"], 7);
    send(&mut stdin, &json!({"jsonrpc": "2.0", "method": "exit"}));
    assert!(child.wait().unwrap().success());
    let mut resto = Vec::new();
    stdout.read_to_end(&mut resto).unwrap();
    assert!(resto.is_empty(), "saída depois do exit: {resto:?}");
}

#[test]
fn stdin_fechado_encerra_o_servidor() {
    let (mut child, stdin, _stdout) = start();
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
