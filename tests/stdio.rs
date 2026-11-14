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

type Running = (
    std::process::Child,
    std::process::ChildStdin,
    BufReader<std::process::ChildStdout>,
);

/// Sobe o executável e faz o `initialize`.
fn start() -> Running {
    start_with(json!({}))
}

/// Sobe o executável e faz o `initialize` com as opções de inicialização.
fn start_with(options: Value) -> Running {
    start_in(options, Value::Null)
}

/// Como o `start_with`, com as pastas do workspace.
fn start_in(options: Value, folders: Value) -> Running {
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
                "params": {"capabilities": {}, "initializationOptions": options,
                           "workspaceFolders": folders}}),
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

/// Próxima mensagem que atende `wanted`, descartando as outras, como avisos do servidor.
fn recv_until(stdout: &mut impl BufRead, wanted: impl Fn(&Value) -> bool) -> Value {
    loop {
        let message = recv(stdout);
        if wanted(&message) {
            return message;
        }
    }
}

fn diagnostic_codes(message: &Value) -> Vec<String> {
    message["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// O dicionário configurado por caminho, lido do arquivo como no editor, com o dicionário
/// fictício de `tests/fixtures`.
#[test]
fn dicionario_lido_do_arquivo_configurado() {
    let tbs = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/dicionario-ficticio.tbs"
    );
    let (mut child, mut stdin, mut stdout) = start_with(json!({"tbs_producao": tbs}));
    let uri = "file:///r/Rule.txt";
    let text = "definir cursor c;\ndefinir numero n;\n\
                c.SQL \"SELECT CodEmp, NomEmpp FROM E070EMP\";\n\
                n = c.CodEmp;\n\
                c.SQL \"SELECT E070FIL. FROM E070FIL\";\n";
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": {"textDocument": {"uri": uri, "languageId": "lsp-senior",
                                            "version": 1, "text": text}}}),
    );
    let is_diagnostics = |m: &Value| m["method"] == "textDocument/publishDiagnostics";
    let diag = recv_until(&mut stdout, is_diagnostics);
    let field = diag["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "SEN075")
        .expect("sem o SEN075 do campo que a tabela não tem");
    let message = field["message"].as_str().unwrap();
    assert!(message.ends_with("Você quis dizer NomEmp?"), "{message}");

    // Completar dentro do SQL, logo depois de `E070FIL.`.
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
                "params": {"textDocument": {"uri": uri},
                           "position": {"line": 4, "character": 22}}}),
    );
    let reply = recv_until(&mut stdout, |m| m["id"] == 2);
    let items = reply["result"].as_array().unwrap();
    let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["CodEmp", "CodFil", "NomFil"]);
    let detail = items[1]["detail"].as_str().unwrap();
    assert!(detail.contains("Código da filial"), "{detail}");

    // Sem o dicionário, a conferência dos campos some sem reiniciar o servidor.
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "workspace/didChangeConfiguration",
                "params": {"settings": {"tbs_producao": ""}}}),
    );
    let diag = recv_until(&mut stdout, is_diagnostics);
    assert!(
        !diagnostic_codes(&diag).contains(&"SEN075".to_owned()),
        "{diag}"
    );
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

/// Referências de um campo do dicionário procuradas nas regras da pasta, numa regra fechada e na
/// regra de um JSON de modelo. Fica aqui porque o dicionário configurado vale para o processo
/// inteiro, e cada teste daqui roda o servidor num processo próprio.
#[test]
fn referencias_do_campo_do_dicionario_nas_regras_da_pasta() {
    let base = std::env::temp_dir().join(format!("senior-lsp-campo-{}", std::process::id()));
    let aberta = base.join("regras/R1.LSP/Rule.txt");
    let fechada = base.join("regras/R2.LSP/Rule.txt");
    let modelo = base.join("Modelos/M1.GER/Detalhe.json");
    for path in [&aberta, &fechada, &modelo] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    }
    let texto = "definir alfa a;\na = E070EMP.NomEmp;\n";
    std::fs::write(&aberta, texto).unwrap();
    std::fs::write(
        &fechada,
        "definir cursor c;\nc.SQL \"SELECT NomEmp FROM E070EMP WHERE SigEmp = 'X'\";\n\
         c.SQL \"SELECT NomFil FROM E070FIL\";\n",
    )
    .unwrap();
    std::fs::write(
        &modelo,
        r#"{"rule": {"prop_name": "SOnPrint", "lines": ["definir cursor c;", "c.SQL \"SELECT E.NomEmp FROM E070EMP E\";"]}}"#,
    )
    .unwrap();
    let tbs = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/dicionario-ficticio.tbs"
    );
    let folder = senior_lsp::form::uri_from_path(&base);
    let (mut child, mut stdin, mut stdout) = start_in(
        json!({"tbs_producao": tbs}),
        json!([{"uri": folder, "name": "x"}]),
    );
    let uri = senior_lsp::form::uri_from_path(&aberta);
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": {"textDocument": {"uri": uri, "languageId": "lsp-senior",
                                            "version": 1, "text": texto}}}),
    );
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/references",
                "params": {"textDocument": {"uri": uri},
                           "position": {"line": 1, "character": 14},
                           "context": {"includeDeclaration": true}}}),
    );
    let reply = recv_until(&mut stdout, |m| m["id"] == 2);
    let found: Vec<(String, u64, u64)> = reply["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            let path = senior_lsp::form::path_from_uri(l["uri"].as_str().unwrap()).unwrap();
            let name = path
                .strip_prefix(&base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let start = &l["range"]["start"];
            (
                name,
                start["line"].as_u64().unwrap(),
                start["character"].as_u64().unwrap(),
            )
        })
        .collect();
    drop(stdin);
    assert!(child.wait().unwrap().success());
    std::fs::remove_dir_all(&base).unwrap();
    // A regra aberta, o SQL da fechada e a regra do JSON, com a posição no JSON.
    assert_eq!(
        found,
        [
            ("Modelos/M1.GER/Detalhe.json".to_owned(), 0, 84),
            ("regras/R1.LSP/Rule.txt".to_owned(), 1, 12),
            ("regras/R2.LSP/Rule.txt".to_owned(), 1, 14),
        ]
    );
}
