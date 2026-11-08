//! Conversa com o servidor por uma conexão em memória, como um editor faria.

use std::thread::JoinHandle;
use std::time::Duration;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Exit, Initialized,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{HoverRequest, Initialize, Request as _, Shutdown};
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
    let response = editor.request(HoverRequest::METHOD, params);
    assert_eq!(
        response.response_result.unwrap_err().code,
        ErrorCode::MethodNotFound as i32
    );
    // O servidor continua atendendo depois.
    editor.open(RULE, 1, "x = 1;");
    assert!(editor.diagnostics().diagnostics.is_empty());
    editor.stop();
}
