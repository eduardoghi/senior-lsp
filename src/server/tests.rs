//! Conversa com o servidor por uma conexão em memória, como um editor faria.

use std::thread::JoinHandle;
use std::time::Duration;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Exit, Initialized,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, Initialize, References,
    Request as _, Shutdown,
};
use lsp_types::{
    ClientCapabilities, GeneralClientCapabilities, InitializeResult, PositionEncodingKind,
    PublishDiagnosticsParams, Uri,
};
use serde_json::json;

struct Editor {
    conn: Connection,
    server: Option<JoinHandle<super::Result<()>>>,
    next_id: i32,
}

impl Editor {
    /// Sobe o servidor e faz o `initialize`. Devolve o editor e a codificação que o servidor
    /// escolheu.
    fn start(encodings: Option<Vec<PositionEncodingKind>>) -> (Editor, PositionEncodingKind) {
        let (client, server_conn) = Connection::memory();
        let server = std::thread::spawn(move || super::run(server_conn));
        let mut editor = Editor {
            conn: client,
            server: Some(server),
            next_id: 0,
        };
        let capabilities = ClientCapabilities {
            general: Some(GeneralClientCapabilities {
                position_encodings: encodings,
                ..GeneralClientCapabilities::default()
            }),
            ..ClientCapabilities::default()
        };
        let response = editor.request(Initialize::METHOD, json!({ "capabilities": capabilities }));
        let result: InitializeResult =
            serde_json::from_value(response.response_result.unwrap()).unwrap();
        editor.notify(Initialized::METHOD, json!({}));
        (editor, result.capabilities.position_encoding.unwrap())
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> Response {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        let request = Request::new(id.clone(), method.to_owned(), params);
        self.conn.sender.send(request.into()).unwrap();
        loop {
            match self.recv() {
                Message::Response(r) if r.id == id => return r,
                _ => continue,
            }
        }
    }

    fn notify(&self, method: &str, params: serde_json::Value) {
        let n = Notification::new(method.to_owned(), params);
        self.conn.sender.send(n.into()).unwrap();
    }

    fn recv(&self) -> Message {
        self.conn
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("o servidor não respondeu")
    }

    /// Próxima publicação de diagnósticos.
    fn diagnostics(&self) -> PublishDiagnosticsParams {
        loop {
            match self.recv() {
                Message::Notification(n) if n.method == PublishDiagnostics::METHOD => {
                    return serde_json::from_value(n.params).unwrap();
                }
                _ => continue,
            }
        }
    }

    fn open(&self, uri: &str, version: i32, text: &str) {
        self.notify(
            DidOpenTextDocument::METHOD,
            json!({ "textDocument": {
                "uri": uri, "languageId": "lsp-senior", "version": version, "text": text } }),
        );
    }

    fn change(&self, uri: &str, version: i32, text: &str) {
        self.notify(
            DidChangeTextDocument::METHOD,
            json!({ "textDocument": { "uri": uri, "version": version },
                    "contentChanges": [{ "text": text }] }),
        );
    }

    fn close(&self, uri: &str) {
        self.notify(
            DidCloseTextDocument::METHOD,
            json!({ "textDocument": { "uri": uri } }),
        );
    }

    /// `shutdown` e `exit`, e espera o servidor terminar sem erro.
    fn stop(mut self) {
        let response = self.request(Shutdown::METHOD, json!(null));
        assert!(response.response_result.is_ok());
        self.notify(Exit::METHOD, json!(null));
        self.server.take().unwrap().join().unwrap().unwrap();
    }

    /// Confere que nenhuma mensagem chegou, para os casos em que o servidor deve ficar quieto.
    fn quiet(&self) {
        assert!(
            self.conn
                .receiver
                .recv_timeout(Duration::from_millis(200))
                .is_err(),
            "o servidor mandou uma mensagem"
        );
    }
}

const RULE: &str = "file:///regras/EXEMPLO.LSP/Rule.txt";

fn uri(s: &str) -> Uri {
    s.parse().unwrap()
}

#[test]
fn sem_codificacao_oferecida_usa_utf16() {
    let (editor, encoding) = Editor::start(None);
    assert_eq!(encoding, PositionEncodingKind::UTF16);
    editor.stop();
}

#[test]
fn com_utf8_oferecido_usa_utf8() {
    let offered = vec![PositionEncodingKind::UTF16, PositionEncodingKind::UTF8];
    let (editor, encoding) = Editor::start(Some(offered));
    assert_eq!(encoding, PositionEncodingKind::UTF8);
    editor.stop();
}

#[test]
fn abrir_corrigir_e_fechar() {
    let (editor, _) = Editor::start(None);

    editor.open(RULE, 1, "x = 1;\ny = #;");
    let d = editor.diagnostics();
    assert_eq!(d.uri, uri(RULE));
    assert_eq!(d.version, Some(1));
    assert_eq!(d.diagnostics.len(), 2);
    assert_eq!(d.diagnostics[0].range.start.line, 1);

    editor.change(RULE, 2, "x = 1;\ny = 2;");
    let d = editor.diagnostics();
    assert_eq!(d.version, Some(2));
    assert!(d.diagnostics.is_empty());

    editor.change(RULE, 3, "x = 1");
    assert_eq!(editor.diagnostics().diagnostics.len(), 1);

    // Fechar limpa os erros que estavam no editor.
    editor.close(RULE);
    let d = editor.diagnostics();
    assert!(d.diagnostics.is_empty());
    assert_eq!(d.version, None);
    editor.stop();
}

#[test]
fn colunas_em_utf16_e_utf8() {
    let text = "x = \"ção😀\"";
    let (editor, _) = Editor::start(None);
    editor.open(RULE, 1, text);
    assert_eq!(
        editor.diagnostics().diagnostics[0].range.start.character,
        11
    );
    editor.stop();

    let (editor, _) = Editor::start(Some(vec![PositionEncodingKind::UTF8]));
    editor.open(RULE, 1, text);
    assert_eq!(
        editor.diagnostics().diagnostics[0].range.start.character,
        15
    );
    editor.stop();
}

#[test]
fn so_analisa_rule_txt() {
    let (editor, _) = Editor::start(None);
    editor.open("file:///docs/README.txt", 1, "isto não é regra #");
    editor.change("file:///docs/README.txt", 2, "outra coisa #");
    editor.close("file:///docs/README.txt");
    editor.quiet();
    editor.open("file:///x/rule.TXT", 1, "#");
    assert_eq!(editor.diagnostics().diagnostics.len(), 2);
    editor.stop();
}

#[test]
fn pedido_desconhecido_nao_derruba_o_servidor() {
    let (mut editor, _) = Editor::start(None);
    let params =
        json!({ "textDocument": { "uri": RULE }, "position": { "line": 0, "character": 0 } });
    let response = editor.request(Completion::METHOD, params);
    assert_eq!(
        response.response_result.unwrap_err().code,
        ErrorCode::MethodNotFound as i32
    );
    // O servidor continua atendendo depois.
    editor.open(RULE, 1, "x = 1;");
    assert!(editor.diagnostics().diagnostics.is_empty());
    editor.stop();
}

#[test]
fn definicao_referencias_hover_e_simbolos() {
    let text = "definir numero nTotal;\n\
                definir funcao Soma(numero p);\n\
                nTotal = 0;\n\
                Soma(2);\n\
                funcao Soma(numero p); {\n\
                \x20 definir alfa aLocal;\n\
                \x20 ntotal = nTotal + p;\n\
                }";
    let (mut editor, _) = Editor::start(None);
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let at = |line: u32, character: u32| json!({ "textDocument": { "uri": RULE }, "position": { "line": line, "character": character } });
    let lines = |v: &serde_json::Value| -> Vec<u64> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|l| l["range"]["start"]["line"].as_u64().unwrap())
            .collect()
    };

    // `ntotal`, em minúsculas na linha 6, vai para o `definir` da linha 0.
    let r = editor.request(GotoDefinition::METHOD, at(6, 3));
    assert_eq!(lines(&r.response_result.unwrap()), [0]);

    let mut params = at(0, 16);
    params["context"] = json!({ "includeDeclaration": true });
    let r = editor.request(References::METHOD, params);
    assert_eq!(lines(&r.response_result.unwrap()), [0, 2, 6, 6]);

    // A chamada alcança o `definir funcao` e a `funcao`.
    let r = editor.request(GotoDefinition::METHOD, at(3, 1));
    assert_eq!(lines(&r.response_result.unwrap()), [1, 4]);

    let r = editor.request(HoverRequest::METHOD, at(6, 20));
    let hover = r.response_result.unwrap();
    let value = hover["contents"]["value"].as_str().unwrap();
    assert!(value.contains("funcao Soma(numero p);"), "{value}");
    assert!(
        value.ends_with("Parâmetro definido na linha 2, também na linha 5"),
        "{value}"
    );

    // Hover fora de nome é `null`.
    let r = editor.request(HoverRequest::METHOD, at(2, 7));
    assert!(r.response_result.unwrap().is_null());

    let r = editor.request(
        DocumentSymbolRequest::METHOD,
        json!({ "textDocument": { "uri": RULE } }),
    );
    let symbols = r.response_result.unwrap();
    let names: Vec<_> = symbols
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["nTotal", "Soma"]);
    assert_eq!(symbols[1]["children"][0]["name"], "aLocal");
    editor.stop();
}

#[test]
fn pedido_em_documento_fechado_responde_null() {
    let (mut editor, _) = Editor::start(None);
    let params =
        json!({ "textDocument": { "uri": RULE }, "position": { "line": 0, "character": 0 } });
    let r = editor.request(GotoDefinition::METHOD, params);
    assert!(r.response_result.unwrap().is_null());
    editor.stop();
}
