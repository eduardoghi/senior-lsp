//! Conversa com o servidor por uma conexão em memória, como um editor faria.

use std::thread::JoinHandle;
use std::time::Duration;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Exit, Initialized,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest, Initialize,
    PrepareRenameRequest, References, Rename, Request as _, Shutdown,
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
    /// Notificações que chegaram enquanto o editor esperava a resposta de um pedido, como os
    /// diagnósticos que o comando de troca de tipo publica antes de responder.
    pending: std::cell::RefCell<std::collections::VecDeque<Message>>,
}

impl Editor {
    /// Sobe o servidor e faz o `initialize`. Devolve o editor e a codificação que o servidor
    /// escolheu.
    fn start(encodings: Option<Vec<PositionEncodingKind>>) -> (Editor, PositionEncodingKind) {
        Editor::start_with(encodings, json!(null))
    }

    /// Como o `start`, com as opções de inicialização do editor.
    fn start_with(
        encodings: Option<Vec<PositionEncodingKind>>,
        options: serde_json::Value,
    ) -> (Editor, PositionEncodingKind) {
        let (client, server_conn) = Connection::memory();
        let server = std::thread::spawn(move || super::run(server_conn));
        let mut editor = Editor {
            conn: client,
            server: Some(server),
            next_id: 0,
            pending: Default::default(),
        };
        let capabilities = ClientCapabilities {
            general: Some(GeneralClientCapabilities {
                position_encodings: encodings,
                ..GeneralClientCapabilities::default()
            }),
            ..ClientCapabilities::default()
        };
        let mut params = json!({ "capabilities": capabilities, "initializationOptions": options });
        // `workspaceFolders` dentro das opções vira parâmetro do initialize, para os testes da
        // análise da pasta.
        if let Some(folders) = params["initializationOptions"]
            .get("workspaceFolders")
            .cloned()
        {
            params["workspaceFolders"] = folders;
        }
        let response = editor.request(Initialize::METHOD, params);
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
        // Lê direto da conexão, porque o `recv` devolveria de novo as notificações guardadas.
        loop {
            let message = self
                .conn
                .receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("o servidor não respondeu");
            match message {
                Message::Response(r) if r.id == id => return r,
                Message::Notification(n) => self.pending.borrow_mut().push_back(n.into()),
                _ => continue,
            }
        }
    }

    fn notify(&self, method: &str, params: serde_json::Value) {
        let n = Notification::new(method.to_owned(), params);
        self.conn.sender.send(n.into()).unwrap();
    }

    fn recv(&self) -> Message {
        if let Some(message) = self.pending.borrow_mut().pop_front() {
            return message;
        }
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
        self.open_as(uri, "lsp-senior", version, text);
    }

    fn open_as(&self, uri: &str, language: &str, version: i32, text: &str) {
        self.notify(
            DidOpenTextDocument::METHOD,
            json!({ "textDocument": {
                "uri": uri, "languageId": language, "version": version, "text": text } }),
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
    editor.open_as(
        "file:///docs/README.txt",
        "plaintext",
        1,
        "isto não é regra #",
    );
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
    let response = editor.request(Formatting::METHOD, params);
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

    // Função do ERP mostra a declaração que o compilador usa.
    editor.change(RULE, 2, "definir alfa a;\nIntParaAlfa(1, a);");
    editor.diagnostics();
    let r = editor.request(HoverRequest::METHOD, at(1, 3));
    let hover = r.response_result.unwrap();
    let value = hover["contents"]["value"].as_str().unwrap();
    assert!(
        value.contains("Funcao IntParaAlfa(Numero Origem, Alfa end Retorno);"),
        "{value}"
    );
    editor.change(RULE, 3, text);
    editor.diagnostics();

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
fn variavel_dentro_do_sql_vai_para_o_definir() {
    let text = "definir numero nCodEmp;\n\
                definir funcao Busca(numero pCod);\n\
                ExecSql \"UPDATE T SET A = 1 WHERE B = :ncodemp \\\n\
                \x20 AND C = :pCod\";\n\
                funcao Busca(numero pCod); {\n\
                \x20 ExecSql \"UPDATE T SET A = :pcod WHERE B = :nCodEmp AND C = :nNada\";\n\
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

    // O parâmetro vale dentro da própria função e a variável em qualquer lugar.
    let r = editor.request(GotoDefinition::METHOD, at(5, 29));
    assert_eq!(lines(&r.response_result.unwrap()), [1, 4]);
    let r = editor.request(GotoDefinition::METHOD, at(5, 50));
    assert_eq!(lines(&r.response_result.unwrap()), [0]);
    let r = editor.request(GotoDefinition::METHOD, at(2, 41));
    assert_eq!(lines(&r.response_result.unwrap()), [0]);
    // Nome sem `definir`, e o parâmetro de uma função vista de fora dela, não levam a lugar nenhum.
    let r = editor.request(GotoDefinition::METHOD, at(5, 64));
    assert!(lines(&r.response_result.unwrap()).is_empty());
    let r = editor.request(GotoDefinition::METHOD, at(3, 13));
    assert!(lines(&r.response_result.unwrap()).is_empty());

    let r = editor.request(HoverRequest::METHOD, at(2, 41));
    let hover = r.response_result.unwrap();
    let value = hover["contents"]["value"].as_str().unwrap();
    assert!(value.contains("definir numero nCodEmp;"), "{value}");
    assert!(value.ends_with("Variável definida na linha 1"), "{value}");
    // O hover cobre só o nome, sem os dois pontos.
    assert_eq!(hover["range"]["start"]["character"], 39);
    assert_eq!(hover["range"]["end"]["character"], 46);
    let r = editor.request(HoverRequest::METHOD, at(5, 64));
    assert!(r.response_result.unwrap().is_null());
    editor.stop();
}

#[test]
fn coluna_do_cursor_vai_para_o_select() {
    let text = "definir cursor cur;\n\
                cur.SQL \"SELECT E070EMP.CodEmp, \\\n\
                \x20 NOMEMP AS nome, COUNT(*) \\\n\
                \x20 FROM E070EMP\";\n\
                cur.AbrirCursor();\n\
                x = cur.codemp;\n\
                y = cur.Nome;\n\
                z = cur.Inexistente;";
    let (mut editor, _) = Editor::start(None);
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let at = |line: u32, character: u32| json!({ "textDocument": { "uri": RULE }, "position": { "line": line, "character": character } });
    let range = |v: &serde_json::Value| {
        let r = &v.as_array().unwrap()[0]["range"];
        [
            r["start"]["line"].as_u64().unwrap(),
            r["start"]["character"].as_u64().unwrap(),
            r["end"]["character"].as_u64().unwrap(),
        ]
    };

    // O campo de `TABELA.CAMPO` e o apelido, e a caixa não importa.
    let r = editor.request(GotoDefinition::METHOD, at(5, 11));
    assert_eq!(range(&r.response_result.unwrap()), [1, 24, 30]);
    let r = editor.request(GotoDefinition::METHOD, at(6, 10));
    assert_eq!(range(&r.response_result.unwrap()), [2, 12, 16]);
    // Coluna fora da lista do SELECT não leva a lugar nenhum, e o cursor continua indo para o `definir`.
    let r = editor.request(GotoDefinition::METHOD, at(7, 12));
    assert!(r.response_result.unwrap().as_array().unwrap().is_empty());
    let r = editor.request(GotoDefinition::METHOD, at(5, 5));
    assert_eq!(range(&r.response_result.unwrap())[0], 0);
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

#[test]
fn funcao_de_outra_regra_do_formulario_sgi() {
    let base = std::env::temp_dir().join(format!("senior-lsp-sgi-{}", std::process::id()));
    let form = base.join("Formulários SGI/interface/F1");
    let action = form.join("actions/A1");
    std::fs::create_dir_all(&action).unwrap();
    let root_rule = form.join("rule.txt");
    std::fs::write(
        &root_rule,
        "definir funcao Atualiza(numero p);\nx = 1;\nfuncao Atualiza(numero p); { }",
    )
    .unwrap();
    let action_rule = action.join("rule.txt");
    let uri = crate::form::uri_from_path(&action_rule);

    let (mut editor, _) = Editor::start(None);
    editor.open(&uri, 1, "Atualiza(1);\nAtualiza(\"a\");\nOutra();");
    let codes: Vec<_> = editor
        .diagnostics()
        .diagnostics
        .into_iter()
        .map(|d| d.code.unwrap())
        .collect();
    let code = |c: &str| lsp_types::NumberOrString::String(c.to_owned());
    // Tipo errado na função herdada e nome que não existe em lugar nenhum.
    assert_eq!(codes, [code("SEN047"), code("SEN052")]);

    let at = json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 2 } });
    let r = editor.request(GotoDefinition::METHOD, at.clone());
    let locations = r.response_result.unwrap();
    assert_eq!(
        locations[0]["uri"],
        crate::form::uri_from_path(&root_rule).as_str()
    );
    assert_eq!(locations[0]["range"]["start"]["line"], 2);

    let r = editor.request(HoverRequest::METHOD, at);
    let hover = r.response_result.unwrap();
    let value = hover["contents"]["value"].as_str().unwrap();
    assert!(
        value.contains("definir funcao Atualiza(numero p);"),
        "{value}"
    );
    assert!(value.contains("linha 3"), "{value}");
    editor.stop();
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn variavel_e_referencias_entre_regras_do_formulario_sgi() {
    let base = std::env::temp_dir().join(format!("senior-lsp-sgi-ref-{}", std::process::id()));
    let form = base.join("Formulários SGI/interface/F1");
    for d in ["actions/A1", "actions/A2"] {
        std::fs::create_dir_all(form.join(d)).unwrap();
    }
    let root = form.join("rule.txt");
    std::fs::write(&root, "definir alfa aNome;\naNome = \"x\";").unwrap();
    std::fs::write(form.join("actions/A2/rule.txt"), "aNome = \"y\";").unwrap();
    let a1 = form.join("actions/A1/rule.txt");
    std::fs::write(&a1, "aNome = 1;").unwrap();
    let a1_uri = crate::form::uri_from_path(&a1);
    let root_uri = crate::form::uri_from_path(&root);

    let (mut editor, _) = Editor::start(None);
    // O tipo vem da raiz, então número na variável alfa herdada é erro.
    editor.open(&a1_uri, 1, "aNome = 1;");
    let d = editor.diagnostics();
    assert_eq!(
        d.diagnostics[0].code,
        Some(lsp_types::NumberOrString::String("SEN043".to_owned()))
    );

    let at = |uri: &str, line: u32, character: u32| json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } });
    let r = editor.request(GotoDefinition::METHOD, at(&a1_uri, 0, 2));
    let locations = r.response_result.unwrap();
    assert_eq!(locations[0]["uri"], root_uri.as_str());

    let r = editor.request(HoverRequest::METHOD, at(&a1_uri, 0, 2));
    let hover = r.response_result.unwrap();
    let value = hover["contents"]["value"].as_str().unwrap();
    assert!(value.contains("definir alfa aNome;"), "{value}");
    assert!(value.contains("Variável definida em"), "{value}");

    // Da ação vêm a definição e o uso na raiz e os usos nas duas ações.
    let mut params = at(&a1_uri, 0, 2);
    params["context"] = json!({ "includeDeclaration": true });
    let r = editor.request(References::METHOD, params);
    let refs = r.response_result.unwrap();
    let files: Vec<_> = refs
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            l["uri"]
                .as_str()
                .unwrap()
                .rsplit('/')
                .nth(1)
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(files, ["F1", "F1", "A1", "A2"]);

    // Renomear a partir da ação troca na raiz e nas duas ações.
    let params = json!({ "textDocument": { "uri": a1_uri },
                         "position": { "line": 0, "character": 2 }, "newName": "aCliente" });
    let r = editor.request(Rename::METHOD, params);
    let changes = r.response_result.unwrap()["changes"]
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[root_uri.as_str()].as_array().unwrap().len(), 2);

    // Da raiz vêm os usos nela e nas duas ações.
    editor.open(&root_uri, 1, "definir alfa aNome;\naNome = \"x\";");
    editor.diagnostics();
    let mut params = at(&root_uri, 0, 15);
    params["context"] = json!({ "includeDeclaration": false });
    let r = editor.request(References::METHOD, params);
    assert_eq!(r.response_result.unwrap().as_array().unwrap().len(), 3);
    editor.stop();
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn completar_nomes() {
    let text = "definir numero nTotal;\n\
                definir funcao Soma(numero pValor);\n\
                nT\n\
                funcao Soma(numero pValor); {\n\
                \x20 pV\n\
                }\n\
                @ nT dentro do comentario @\n\
                cur.Ab";
    let (mut editor, _) = Editor::start(None);
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let mut complete = |line: u32, character: u32| -> Vec<std::string::String> {
        let params = json!({ "textDocument": { "uri": RULE },
                             "position": { "line": line, "character": character } });
        let r = editor.request(Completion::METHOD, params);
        r.response_result
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["label"].as_str().unwrap().to_owned())
            .collect()
    };
    // Variável do arquivo, sem diferença de caixa.
    assert_eq!(complete(2, 2), ["nTotal"]);
    // Parâmetro só dentro da função dele.
    assert_eq!(complete(4, 4), ["pValor"]);
    // Funções do ERP e palavras da linguagem.
    let int = complete(2, 0);
    assert!(int.contains(&"IntParaAlfa".to_owned()) && int.contains(&"Enquanto".to_owned()));
    assert!(!int.contains(&"pValor".to_owned()));
    // Nada dentro de comentário nem depois de ponto.
    assert!(complete(6, 4).is_empty());
    assert!(complete(7, 6).is_empty());
    editor.stop();
}

#[test]
fn renomear() {
    let text = "definir numero x;\n\
                definir funcao f(numero x);\n\
                X = 1; Mensagem(Erro, \"x\"); @ x @\n\
                IntParaAlfa(x, a);\n\
                funcao f(numero x); { x = 2; }";
    let (mut editor, _) = Editor::start(None);
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let rename = |editor: &mut Editor, line: u32, character: u32, name: &str| {
        let params = json!({ "textDocument": { "uri": RULE },
                             "position": { "line": line, "character": character },
                             "newName": name });
        editor.request(Rename::METHOD, params)
    };
    // A global `x` muda na definição e nos dois usos, com caixa diferente inclusive. O parâmetro,
    // a string e o comentário ficam.
    let r = rename(&mut editor, 0, 15, "nTotal");
    let edit = r.response_result.unwrap();
    let edits = edit["changes"][RULE].as_array().unwrap();
    let mut places: Vec<_> = edits
        .iter()
        .map(|e| {
            (
                e["range"]["start"]["line"].as_u64().unwrap(),
                e["range"]["start"]["character"].as_u64().unwrap(),
            )
        })
        .collect();
    places.sort();
    assert_eq!(places, [(0, 15), (2, 0), (3, 12)]);
    assert!(edits.iter().all(|e| e["newText"] == "nTotal"));

    // O parâmetro muda só dentro da função dele e no `definir funcao`.
    let r = rename(&mut editor, 4, 23, "p");
    let edit = r.response_result.unwrap();
    assert_eq!(edit["changes"][RULE].as_array().unwrap().len(), 3);

    // Recusas.
    for name in ["1x", "se", "f"] {
        let r = rename(&mut editor, 0, 15, name);
        assert!(r.response_result.is_err(), "{name}");
    }
    // Função do ERP não se renomeia.
    let params =
        json!({ "textDocument": { "uri": RULE }, "position": { "line": 3, "character": 2 } });
    let r = editor.request(PrepareRenameRequest::METHOD, params);
    assert!(r.response_result.unwrap().is_null());
    editor.stop();
}

#[test]
fn regras_de_modelo_de_relatorio() {
    let base = std::env::temp_dir().join(format!("senior-lsp-modelo-{}", std::process::id()));
    let model = base.join("Modelos/M1.GER");
    std::fs::create_dir_all(&model).unwrap();
    let functions = r#"{ "function_rule": { "rule": { "prop_name": "SFunctionRule", "lines": [
  "Definir Funcao Soma(Numero p);",
  "Funcao Soma(Numero p); { }"
] } } }"#;
    std::fs::write(model.join("Base_Structure.json"), functions).unwrap();
    let header = r#"{ "children": [ { "rule": { "prop_name": "SOnPrint", "lines": [
  "Definir Alfa aTxt;",
  "Soma(\"x\");",
  "AlteraControle(\"Campo\", \"Descrição\", \"Folha\");",
  "Inexistente(1);"
] } } ] }"#;
    let header_path = model.join("Cabecalho.json");
    std::fs::write(&header_path, header).unwrap();
    let uri = crate::form::uri_from_path(&header_path);

    let (mut editor, _) = Editor::start(None);
    editor.open(&uri, 1, header);
    let d = editor.diagnostics();
    let found: Vec<_> = d
        .diagnostics
        .iter()
        .map(|d| {
            let lsp_types::NumberOrString::String(code) = d.code.clone().unwrap() else {
                unreachable!()
            };
            (code, d.range.start.line, d.range.start.character)
        })
        .collect();
    // O tipo errado na função do modelo e o nome desconhecido, nas linhas do JSON. A função de
    // relatório `AlteraControle` não dá erro.
    assert_eq!(
        found,
        [("SEN047".to_owned(), 2, 8), ("SEN052".to_owned(), 4, 3)]
    );

    let at = |line: u32, character: u32| json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } });
    let r = editor.request(HoverRequest::METHOD, at(3, 5));
    let hover = r.response_result.unwrap();
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("AlteraControle")
    );

    let r = editor.request(GotoDefinition::METHOD, at(2, 4));
    let locations = r.response_result.unwrap();
    assert_eq!(
        locations[0]["uri"],
        crate::form::uri_from_path(&model.join("Base_Structure.json")).as_str()
    );
    assert_eq!(locations[0]["range"]["start"]["line"], 2);

    // Fora de regra, no JSON em si, não há resposta.
    let r = editor.request(HoverRequest::METHOD, at(0, 2));
    assert!(r.response_result.unwrap().is_null());

    // As referências da função atravessam os JSON do modelo.
    let base_uri = crate::form::uri_from_path(&model.join("Base_Structure.json"));
    let mut params = at(2, 4);
    params["context"] = json!({ "includeDeclaration": true });
    let r = editor.request(References::METHOD, params);
    let refs = r.response_result.unwrap();
    let mut found: Vec<_> = refs
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["uri"].as_str().unwrap() == base_uri.as_str(),
                l["range"]["start"]["line"].as_u64().unwrap(),
            )
        })
        .collect();
    found.sort();
    assert_eq!(found, [(false, 2), (true, 1), (true, 2)]);

    // Renomear troca nos dois arquivos e recusa nome que já existe no modelo.
    let rename = |editor: &mut Editor, name: &str| {
        let mut params = at(2, 4);
        params["newName"] = json!(name);
        editor.request(Rename::METHOD, params)
    };
    let r = rename(&mut editor, "Total");
    let changes = r.response_result.unwrap()["changes"]
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(changes[base_uri.as_str()].as_array().unwrap().len(), 2);
    assert_eq!(changes[uri.as_str()].as_array().unwrap().len(), 1);
    let r = rename(&mut editor, "aTxt");
    assert!(r.response_result.is_err());
    // Função do ERP continua sem renomear.
    let r = editor.request(PrepareRenameRequest::METHOD, at(3, 5));
    assert!(r.response_result.unwrap().is_null());
    editor.stop();
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn entradas_de_modelo_de_relatorio() {
    let base = std::env::temp_dir().join(format!("senior-lsp-entradas-{}", std::process::id()));
    let model = base.join("Modelos/M1.GER");
    std::fs::create_dir_all(&model).unwrap();
    let structure = r#"{ "entrance": [
  { "sytem_varName": "ECodEmp", "description": "Empresa", "type_var": 1, "size": 0 }
], "children": [ { "classname": "TSFormula", "name": "FTotal" } ] }"#;
    let structure_path = model.join("Base_Structure.json");
    std::fs::write(&structure_path, structure).unwrap();
    let header = r#"{ "children": [ { "rule": { "prop_name": "SOnPrint", "lines": [
  "Definir Alfa aTxt;",
  "aTxt = ECodEmp;",
  "Se (ECodEmp = 1) { }",
  "Se (FTotal > 0) { }"
] } } ] }"#;
    let header_path = model.join("Cabecalho.json");
    std::fs::write(&header_path, header).unwrap();
    let uri = crate::form::uri_from_path(&header_path);

    let (mut editor, _) = Editor::start(None);
    editor.open(&uri, 1, header);
    let d = editor.diagnostics();
    let lines: Vec<_> = d.diagnostics.iter().map(|d| d.range.start.line).collect();
    // A entrada vale como variável numérica, sem `definir`.
    assert_eq!(lines, [2], "{:?}", d.diagnostics);

    let at = |line: u32, character: u32| json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } });
    let r = editor.request(HoverRequest::METHOD, at(3, 8));
    let hover = r.response_result.unwrap();
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(text.contains("numero ECodEmp"), "{text}");
    assert!(text.contains("Entrada do relatório"), "{text}");

    let r = editor.request(GotoDefinition::METHOD, at(3, 8));
    let locations = r.response_result.unwrap();
    assert_eq!(
        locations[0]["uri"],
        crate::form::uri_from_path(&structure_path).as_str()
    );
    assert_eq!(locations[0]["range"]["start"]["line"], 1);

    let mut params = at(3, 8);
    params["context"] = json!({ "includeDeclaration": true });
    let r = editor.request(References::METHOD, params);
    let refs = r.response_result.unwrap();
    assert_eq!(refs.as_array().unwrap().len(), 3);

    // O nome da entrada é o do cadastro no modelo, então não se renomeia pela regra.
    let r = editor.request(PrepareRenameRequest::METHOD, at(3, 8));
    assert!(r.response_result.unwrap().is_null());

    // O controle de fórmula também vale como variável.
    let r = editor.request(HoverRequest::METHOD, at(4, 8));
    let hover = r.response_result.unwrap();
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(text.contains("Controle de fórmula do relatório"), "{text}");
    assert!(text.contains("numero FTotal"), "{text}");
    let r = editor.request(GotoDefinition::METHOD, at(4, 8));
    assert_eq!(r.response_result.unwrap()[0]["range"]["start"]["line"], 2);
    let r = editor.request(PrepareRenameRequest::METHOD, at(4, 8));
    assert!(r.response_result.unwrap().is_null());
    editor.stop();
    std::fs::remove_dir_all(&base).unwrap();
}

/// Códigos dos diagnósticos publicados, para conferir a troca de tipo pela chamada a uma função
/// que só existe na tela (SEN052 fora dela).
fn codes(params: &PublishDiagnosticsParams) -> Vec<String> {
    params
        .diagnostics
        .iter()
        .filter_map(|d| match &d.code {
            Some(lsp_types::NumberOrString::String(code)) => Some(code.clone()),
            _ => None,
        })
        .collect()
}

const TELA: &str = "AbrirTelaSGI(\"F000\");";

#[test]
fn arquivo_solto_com_a_linguagem_e_analisado_e_troca_de_tipo() {
    let (mut editor, _) = Editor::start(None);
    let solto = "untitled:Untitled-1";
    editor.open(solto, 1, TELA);
    assert_eq!(codes(&editor.diagnostics()), ["SEN052"]);

    let tipo = editor.request("senior-lsp/tipoRegra", json!({ "uri": solto }));
    assert_eq!(
        tipo.response_result.unwrap(),
        json!({ "tipo": "comum", "fixo": false })
    );

    let actions = editor.request(
        lsp_types::request::CodeActionRequest::METHOD,
        json!({ "textDocument": { "uri": solto },
                "range": { "start": { "line": 0, "character": 0 },
                           "end": { "line": 0, "character": 0 } },
                "context": { "diagnostics": [] } }),
    );
    // Só as ações de troca de tipo, que são comandos. As correções vêm como edição.
    let actions: Vec<serde_json::Value> = actions
        .response_result
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["command"].is_string())
        .cloned()
        .collect();
    let titles: Vec<_> = actions
        .iter()
        .map(|a| a["title"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        titles,
        [
            "Tratar como regra de tela SGI",
            "Tratar como regra de modelo de relatório"
        ]
    );

    let tela = actions[0]["arguments"].clone();
    let response = editor.request(
        lsp_types::request::ExecuteCommand::METHOD,
        json!({ "command": "senior-lsp.tipoRegra", "arguments": tela }),
    );
    assert!(response.response_result.is_ok());
    assert!(codes(&editor.diagnostics()).is_empty());

    // A escolha vale nas mudanças e depois de reabrir.
    editor.change(solto, 2, TELA);
    assert!(codes(&editor.diagnostics()).is_empty());
    editor.close(solto);
    editor.diagnostics();
    editor.open(solto, 3, TELA);
    assert!(codes(&editor.diagnostics()).is_empty());
    editor.stop();
}

#[test]
fn tipo_padrao_vale_para_arquivo_solto_e_nao_para_rule_txt() {
    let (editor, _) = Editor::start_with(None, json!({ "tipo_padrao": "tela" }));
    editor.open("untitled:Untitled-1", 1, TELA);
    assert!(codes(&editor.diagnostics()).is_empty());
    editor.open(RULE, 1, TELA);
    assert_eq!(codes(&editor.diagnostics()), ["SEN052"]);
    editor.stop();
}

#[test]
fn tipo_da_pasta_de_tela_nao_troca() {
    let (mut editor, _) = Editor::start(None);
    let tela = "file:///x/Formul%C3%A1rios%20SGI/F1/rule.txt";
    editor.open(tela, 1, TELA);
    assert!(codes(&editor.diagnostics()).is_empty());
    let actions = editor.request(
        lsp_types::request::CodeActionRequest::METHOD,
        json!({ "textDocument": { "uri": tela },
                "range": { "start": { "line": 0, "character": 0 },
                           "end": { "line": 0, "character": 0 } },
                "context": { "diagnostics": [] } }),
    );
    assert_eq!(actions.response_result.unwrap(), json!([]));
    let response = editor.request(
        lsp_types::request::ExecuteCommand::METHOD,
        json!({ "command": "senior-lsp.tipoRegra", "arguments": [tela, "comum"] }),
    );
    assert_eq!(
        response.response_result.unwrap_err().code,
        ErrorCode::InvalidParams as i32
    );
    editor.stop();
}

#[test]
fn ajuda_de_assinatura_mostra_o_parametro_atual() {
    let (mut editor, _) = Editor::start(None);
    let text = "Definir Funcao Soma(Numero A, Numero B);\nIntParaAlfa(1, \nSoma(1, ";
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let help = |editor: &mut Editor, line: u32, character: u32| {
        editor
            .request(
                lsp_types::request::SignatureHelpRequest::METHOD,
                json!({ "textDocument": { "uri": RULE },
                        "position": { "line": line, "character": character } }),
            )
            .response_result
            .unwrap()
    };
    let erp = help(&mut editor, 1, 15);
    assert!(
        erp["signatures"][0]["label"]
            .as_str()
            .unwrap()
            .contains("IntParaAlfa(")
    );
    assert_eq!(erp["activeParameter"], 1);
    let local = help(&mut editor, 2, 8);
    assert_eq!(
        local["signatures"][0]["label"],
        "Definir Funcao Soma(Numero A, Numero B)"
    );
    assert_eq!(local["signatures"][0]["parameters"][1]["label"], "Numero B");
    assert_eq!(local["activeParameter"], 1);
    assert_eq!(help(&mut editor, 0, 0), json!(null));
    editor.stop();
}

#[test]
fn destaque_semantico_classifica_cada_nome() {
    let (mut editor, _) = Editor::start(None);
    let text = "Definir Funcao Soma(Numero A);\n\
                definir alfa aTxt;\n\
                definir cursor cur;\n\
                IntParaAlfa(DatSis, aTxt);\n\
                Soma(1);\n\
                aTxt = cur.NomEmp;\n\
                funcao Soma(Numero A); { A = 1; }";
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let response = editor.request(
        lsp_types::request::SemanticTokensFullRequest::METHOD,
        json!({ "textDocument": { "uri": RULE } }),
    );
    let data: Vec<u32> =
        serde_json::from_value(response.response_result.unwrap()["data"].clone()).unwrap();
    // Volta do formato relativo para (texto, tipo, modificadores).
    let tipos = [
        "function",
        "variable",
        "parameter",
        "property",
        "struct",
        "class",
    ];
    let lines: Vec<&str> = text.lines().collect();
    let (mut line, mut col) = (0usize, 0usize);
    let mut found = Vec::new();
    for t in data.chunks(5) {
        if t[0] > 0 {
            line += t[0] as usize;
            col = t[1] as usize;
        } else {
            col += t[1] as usize;
        }
        let name = &lines[line][col..col + t[2] as usize];
        found.push(format!("{name} {} {}", tipos[t[3] as usize], t[4]));
    }
    assert_eq!(
        found,
        [
            "Soma function 1",
            "A parameter 1",
            "aTxt variable 1",
            "cur variable 1",
            "IntParaAlfa function 2",
            "DatSis variable 2",
            "aTxt variable 0",
            "Soma function 0",
            "aTxt variable 0",
            "cur variable 0",
            "NomEmp property 0",
            "Soma function 1",
            "A parameter 1",
            "A parameter 0",
        ]
    );
    editor.stop();
}

#[test]
fn destaca_as_ocorrencias_do_nome() {
    let (mut editor, _) = Editor::start(None);
    let text = "Definir Funcao Soma(Numero A);\n\
                definir alfa aTxt;\n\
                IntParaAlfa(1, aTxt);\n\
                IntParaAlfa(2, aTxt);\n\
                Soma(1);\n\
                funcao Soma(Numero A); { A = 1; }";
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let mut at = |line: u32, character: u32| -> Vec<String> {
        let response = editor.request(
            lsp_types::request::DocumentHighlightRequest::METHOD,
            json!({ "textDocument": { "uri": RULE },
                    "position": { "line": line, "character": character } }),
        );
        let list: Vec<lsp_types::DocumentHighlight> =
            serde_json::from_value(response.response_result.unwrap()).unwrap();
        list.iter()
            .map(|h| {
                let kind = match h.kind.unwrap() {
                    lsp_types::DocumentHighlightKind::WRITE => "escrita",
                    lsp_types::DocumentHighlightKind::READ => "leitura",
                    _ => "texto",
                };
                format!("{}:{} {kind}", h.range.start.line, h.range.start.character)
            })
            .collect()
    };
    // Variável, pelo uso.
    assert_eq!(at(2, 16), ["1:13 escrita", "2:15 leitura", "3:15 leitura"]);
    // Função da regra, pela declaração, marca a implementação e a chamada.
    assert_eq!(at(0, 16), ["0:15 escrita", "4:0 leitura", "5:7 escrita"]);
    // Função do ERP, que o arquivo não define.
    assert_eq!(at(2, 3), ["2:0 texto", "3:0 texto"]);
    editor.stop();
}

/// A correção que cria o `definir` no ponto, com o título e o texto inserido.
fn define_fix(editor: &mut Editor, line: u32, character: u32) -> Option<(String, String, u32)> {
    let response = editor.request(
        lsp_types::request::CodeActionRequest::METHOD,
        json!({ "textDocument": { "uri": RULE },
                "range": { "start": { "line": line, "character": character },
                           "end": { "line": line, "character": character } },
                "context": { "diagnostics": [] } }),
    );
    let actions = response.response_result.unwrap();
    let fix = actions
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "quickfix")?;
    let edit = &fix["edit"]["changes"][RULE][0];
    Some((
        fix["title"].as_str().unwrap().to_owned(),
        edit["newText"].as_str().unwrap().to_owned(),
        edit["range"]["start"]["line"].as_u64().unwrap() as u32,
    ))
}

#[test]
fn correcao_cria_o_definir_com_o_tipo_do_uso() {
    let (mut editor, _) = Editor::start(None);
    let text = "definir alfa aA;\n\
                aA = \"x\";\n\
                aTxt = aA + \"y\";\n\
                IntParaAlfa(1, saida);\n\
                nTotal = nTotal + 1;\n\
                dHoje = DatSis;\n\
                VSCodEmp = 1;";
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let fix = |editor: &mut Editor, line| define_fix(editor, line, 1);
    assert_eq!(
        fix(&mut editor, 2),
        Some((
            "definir alfa aTxt;".into(),
            "definir alfa aTxt;\n".into(),
            1
        ))
    );
    assert_eq!(fix(&mut editor, 3), None, "a chamada, não o argumento");
    assert_eq!(
        define_fix(&mut editor, 3, 16).map(|f| f.0),
        Some("definir alfa saida;".into())
    );
    assert_eq!(
        fix(&mut editor, 4).map(|f| f.0),
        Some("definir numero nTotal;".into())
    );
    assert_eq!(
        fix(&mut editor, 5).map(|f| f.0),
        Some("definir data dHoje;".into())
    );
    assert_eq!(fix(&mut editor, 6), None, "variável de identificador");
    assert_eq!(fix(&mut editor, 1), None, "já definida");
    editor.stop();
}

#[test]
fn nomes_dos_parametros_na_chamada() {
    let (mut editor, _) = Editor::start(None);
    let text = "Definir Funcao Soma(Numero A, Numero B);\n\
                definir alfa aTxt; definir numero Origem;\n\
                IntParaAlfa(1, aTxt);\n\
                IntParaAlfa(Origem, aTxt);\n\
                Soma(1, 2);\n\
                AlfaParaInt(aTxt, Origem);";
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let response = editor.request(
        lsp_types::request::InlayHintRequest::METHOD,
        json!({ "textDocument": { "uri": RULE },
                "range": { "start": { "line": 0, "character": 0 },
                           "end": { "line": 99, "character": 0 } } }),
    );
    let hints: Vec<lsp_types::InlayHint> =
        serde_json::from_value(response.response_result.unwrap()).unwrap();
    let found: Vec<String> = hints
        .iter()
        .map(|h| {
            let lsp_types::InlayHintLabel::String(label) = &h.label else {
                unreachable!()
            };
            format!("{}:{} {label}", h.position.line, h.position.character)
        })
        .collect();
    // O argumento com o mesmo nome do parâmetro fica sem dica.
    assert_eq!(
        found,
        [
            "2:12 Origem:",
            "2:15 end Retorno:",
            "3:20 end Retorno:",
            "4:5 A:",
            "4:8 B:",
            "5:12 Origem:",
            "5:18 end Retorno:",
        ]
    );
    editor.stop();
}

#[test]
fn trechos_para_recolher() {
    let (mut editor, _) = Editor::start(None);
    let text = "Definir Funcao F();\n\
                definir alfa a;\n\
                definir numero n;\n\
                /* comentário\n\
                   de duas linhas */\n\
                se (n = 1) {\n\
                  a = \"x\";\n\
                }\n\
                funcao F(); {\n\
                  n = 2;\n\
                }\n";
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let response = editor.request(
        lsp_types::request::FoldingRangeRequest::METHOD,
        json!({ "textDocument": { "uri": RULE } }),
    );
    let ranges: Vec<lsp_types::FoldingRange> =
        serde_json::from_value(response.response_result.unwrap()).unwrap();
    let found: Vec<String> = ranges
        .iter()
        .map(|r| format!("{}-{} {:?}", r.start_line, r.end_line, r.kind))
        .collect();
    assert_eq!(
        found,
        [
            "0-2 Some(Imports)",
            "3-4 Some(Comment)",
            "5-6 None",
            "8-10 None",
            "8-9 None",
        ]
    );
    editor.stop();
}

/// Rótulos do completar no ponto.
fn completion_labels(editor: &mut Editor, line: u32, character: u32) -> Vec<String> {
    let response = editor.request(
        lsp_types::request::Completion::METHOD,
        json!({ "textDocument": { "uri": RULE },
                "position": { "line": line, "character": character } }),
    );
    let items: Vec<lsp_types::CompletionItem> =
        serde_json::from_value(response.response_result.unwrap()).unwrap();
    let mut labels: Vec<String> = items.into_iter().map(|i| i.label).collect();
    labels.sort();
    labels
}

#[test]
fn completar_depois_do_ponto() {
    let (mut editor, _) = Editor::start(None);
    let text = "definir cursor cur;\n\
                definir lista lst;\n\
                cur.SQL \"SELECT E070EMP.CodEmp, E070EMP.NomEmp FROM E070EMP\";\n\
                lst.AdicionarCampo(\"Email\", alfa);\n\
                cur.\n\
                lst.Pri\n\
                lst.Em\n\
                x.";
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let cursor = completion_labels(&mut editor, 4, 4);
    for esperado in [
        "AbrirCursor",
        "Achou",
        "CODEMP",
        "NOMEMP",
        "SQL",
        "UsaAbrangencia",
    ] {
        assert!(
            cursor.contains(&esperado.to_owned()),
            "{esperado} em {cursor:?}"
        );
    }
    assert!(!cursor.contains(&"AdicionarCampo".to_owned()));
    assert_eq!(completion_labels(&mut editor, 5, 7), ["Primeiro"]);
    assert_eq!(completion_labels(&mut editor, 6, 6), ["Email"]);
    assert!(completion_labels(&mut editor, 7, 2).is_empty());
    editor.stop();
}

/// Aplica a ação de título começando com `title` no ponto e devolve o texto resultante.
fn apply_fix(editor: &mut Editor, text: &str, line: u32, character: u32, title: &str) -> String {
    let response = editor.request(
        lsp_types::request::CodeActionRequest::METHOD,
        json!({ "textDocument": { "uri": RULE },
                "range": { "start": { "line": line, "character": character },
                           "end": { "line": line, "character": character } },
                "context": { "diagnostics": [] } }),
    );
    let actions: Vec<lsp_types::CodeActionOrCommand> =
        serde_json::from_value(response.response_result.unwrap()).unwrap();
    let actions: Vec<lsp_types::CodeAction> = actions
        .into_iter()
        .filter_map(|a| match a {
            lsp_types::CodeActionOrCommand::CodeAction(a) => Some(a),
            lsp_types::CodeActionOrCommand::Command(_) => None,
        })
        .collect();
    let action = actions
        .iter()
        .find(|a| a.title.starts_with(title))
        .unwrap_or_else(|| {
            panic!(
                "sem \"{title}\" em {:?}",
                actions.iter().map(|a| &a.title).collect::<Vec<_>>()
            )
        });
    let mut edits = action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(RULE)].clone();
    let offset = |p: lsp_types::Position| {
        text.split_inclusive('\n')
            .take(p.line as usize)
            .map(str::len)
            .sum::<usize>()
            + p.character as usize
    };
    edits.sort_by_key(|e| std::cmp::Reverse(offset(e.range.start)));
    let mut out = text.to_owned();
    for e in edits {
        out.replace_range(offset(e.range.start)..offset(e.range.end), &e.new_text);
    }
    out
}

#[test]
fn correcoes_dos_avisos() {
    let (mut editor, _) = Editor::start(None);
    let text = "definir alfa aUsada;\n\
                definir alfa aSobra; definir numero n;\n\
                n = aUsada > 1;\n\
                n = 'a';\n\
                definir cursor cur;\n  \
                SQL_UsarSQLSenior2(cur, 0);\n\
                Calcula(aUsada, 2);\n";
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let fixed = |editor: &mut Editor, line, character, title| {
        apply_fix(editor, text, line, character, title)
    };
    assert!(
        fixed(&mut editor, 1, 15, "Apagar a definição de aSobra")
            .contains("\ndefinir numero n;\nn = aUsada")
    );
    assert!(fixed(&mut editor, 2, 11, "Pôr a comparação").contains("n = (aUsada > 1);"));
    assert!(fixed(&mut editor, 3, 5, "Trocar por 'A'").contains("n = 'A';"));
    assert!(
        fixed(&mut editor, 5, 4, "Acrescentar SQL_UsarAbrangencia")
            .contains("  SQL_UsarAbrangencia(cur, 0);\n  SQL_UsarSQLSenior2(cur, 0);")
    );
    let declarada = fixed(&mut editor, 6, 2, "Declarar a função Calcula");
    assert!(
        declarada.contains("definir funcao Calcula(alfa aUsada, numero Param2);\n"),
        "{declarada}"
    );
    assert!(
        declarada
            .ends_with("Calcula(aUsada, 2);\n\nfuncao Calcula(alfa aUsada, numero Param2); {\n}\n"),
        "{declarada}"
    );
    editor.stop();
}

#[test]
fn contagem_de_referencias_das_funcoes() {
    let (mut editor, _) = Editor::start(None);
    let text = "Definir Funcao Soma(Numero A);\n\
                Definir Funcao SoDeclarada();\n\
                Soma(1);\n\
                Soma(2);\n\
                funcao Soma(Numero A); { A = 1; }\n\
                funcao SemUso(); { }";
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let response = editor.request(
        lsp_types::request::CodeLensRequest::METHOD,
        json!({ "textDocument": { "uri": RULE } }),
    );
    let lenses: Vec<lsp_types::CodeLens> =
        serde_json::from_value(response.response_result.unwrap()).unwrap();
    let found: Vec<String> = lenses
        .iter()
        .map(|l| {
            let command = l.command.as_ref().unwrap();
            assert_eq!(command.command, "senior-lsp.mostrarReferencias");
            format!("{} {}", l.range.start.line, command.title)
        })
        .collect();
    assert_eq!(
        found,
        ["1 0 referências", "4 2 referências", "5 0 referências"]
    );
    editor.stop();
}

#[test]
fn ir_para_a_regra_da_porta_do_web_service() {
    let base = std::env::temp_dir().join(format!("senior-lsp-ws-{}", std::process::id()));
    let porta = base.join("Web Services/interno/com.empresa.vendas/Calcular");
    std::fs::create_dir_all(&porta).unwrap();
    std::fs::write(porta.join("Rule.txt"), "").unwrap();
    let regra = base.join("regras/R1.LSP/Rule.txt");
    std::fs::create_dir_all(regra.parent().unwrap()).unwrap();
    let regra_uri = crate::form::uri_from_path(&regra);

    let (mut editor, _) = Editor::start(None);
    editor.open(&regra_uri, 1, "Definir interno.com.empresa.vendas.Calcular ws;\nDefinir interno.com.empresa.vendas.Outra ws2;");
    editor.diagnostics();
    let mut go = |line: u32, character: u32| {
        let response = editor.request(
            GotoDefinition::METHOD,
            json!({ "textDocument": { "uri": regra_uri },
                    "position": { "line": line, "character": character } }),
        );
        response.response_result.unwrap()
    };
    let found = go(0, 20);
    assert_eq!(
        found[0]["uri"],
        crate::form::uri_from_path(&porta.join("Rule.txt"))
    );
    // Porta que não está na exportação.
    assert_eq!(go(1, 20), json!([]));
    editor.stop();
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn ir_para_a_implementacao_e_de_volta() {
    let (mut editor, _) = Editor::start(None);
    let text = "Definir Funcao Soma(Numero A);\n\
                Definir Funcao SemChamada();\n\
                Soma(1);\n\
                funcao Soma(Numero A); { A = 1; }\n\
                funcao SemChamada(); { }";
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let mut go = |line: u32, character: u32| -> Vec<u32> {
        let response = editor.request(
            lsp_types::request::GotoImplementation::METHOD,
            json!({ "textDocument": { "uri": RULE },
                    "position": { "line": line, "character": character } }),
        );
        let locations: Vec<lsp_types::Location> =
            serde_json::from_value(response.response_result.unwrap()).unwrap();
        locations.iter().map(|l| l.range.start.line).collect()
    };
    assert_eq!(go(0, 16), [3], "da declaração para a funcao");
    assert_eq!(go(2, 1), [3], "da chamada para a funcao");
    assert_eq!(go(3, 8), [0], "da funcao de volta");
    assert_eq!(go(1, 16), [4], "sem chamada nenhuma");
    editor.stop();
}

#[test]
fn parametros_da_porta_do_web_service() {
    let base = std::env::temp_dir().join(format!("senior-lsp-ws-port-{}", std::process::id()));
    let porta = base.join("Web Services/interno/com.empresa.vendas/Consultar");
    std::fs::create_dir_all(&porta).unwrap();
    std::fs::write(porta.join("Rule.txt"), "").unwrap();
    // O gerenciador grava o Port.json em cp1252, por isso o "ç" vai como um byte só.
    let json = b"{\"name\":\"Consultar\",\"description\":\"Consulta pre\xe7o\",\"params\":[\
        {\"name\":\"CodEmp\",\"kind\":\"pkIn\",\"type\":\"ptInteger\",\"help\":\"Empresa\"},\
        {\"name\":\"tbItens\",\"kind\":\"pkOut\",\"type\":\"ptComplexSet\",\"columns\":[\
          {\"name\":\"CodPro\",\"kind\":\"pkOut\",\"type\":\"ptString\"}]},\
        {\"name\":\"PerCom\",\"kind\":\"pkOut\",\"type\":\"ptDouble\"}]}";
    std::fs::write(porta.join("Port.json"), json).unwrap();
    let regra = base.join("regras/R1.LSP/Rule.txt");
    std::fs::create_dir_all(regra.parent().unwrap()).unwrap();
    let regra_uri = crate::form::uri_from_path(&regra);

    let (mut editor, _) = Editor::start(None);
    let text = "Definir interno.com.empresa.vendas.Consultar ws;\nws.\nws.tbItens[1].\n";
    editor.open(&regra_uri, 1, text);
    editor.diagnostics();
    let mut labels = |line: u32, character: u32| -> Vec<String> {
        let response = editor.request(
            lsp_types::request::Completion::METHOD,
            json!({ "textDocument": { "uri": regra_uri },
                    "position": { "line": line, "character": character } }),
        );
        let items: Vec<lsp_types::CompletionItem> =
            serde_json::from_value(response.response_result.unwrap()).unwrap();
        let mut l: Vec<String> = items.into_iter().map(|i| i.label).collect();
        l.sort();
        l
    };
    assert_eq!(
        labels(1, 3),
        [
            "CodEmp",
            "ErroExecucao",
            "Executar",
            "ModoExecucao",
            "PerCom",
            "Senha",
            "Token",
            "Usuario",
            "tbItens"
        ]
    );
    assert_eq!(
        labels(2, 14),
        [
            "CodPro",
            "CriarLinha",
            "ExcluirLinha",
            "LinhaAtual",
            "QtdLinhas"
        ]
    );
    let hover = editor.request(
        HoverRequest::METHOD,
        json!({ "textDocument": { "uri": regra_uri },
                "position": { "line": 0, "character": 20 } }),
    );
    let value = hover.response_result.unwrap()["contents"]["value"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(value.contains("Consulta preço"), "{value}");
    assert!(
        value.contains("Entrada\n\n- `CodEmp` (numero, Empresa)"),
        "{value}"
    );
    assert!(value.contains("- `tbItens` (conjunto)"), "{value}");
    editor.stop();
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn parametro_que_nao_esta_na_porta() {
    let base = std::env::temp_dir().join(format!("senior-lsp-ws-diag-{}", std::process::id()));
    let porta = base.join("Web Services/interno/com.empresa.vendas/Consultar");
    std::fs::create_dir_all(&porta).unwrap();
    std::fs::write(
        porta.join("Port.json"),
        "{\"params\":[{\"name\":\"CodEmp\",\"kind\":\"pkIn\",\"type\":\"ptInteger\"},\
         {\"name\":\"tbItens\",\"kind\":\"pkOut\",\"type\":\"ptComplexSet\",\"columns\":[\
         {\"name\":\"CodPro\",\"kind\":\"pkOut\",\"type\":\"ptString\"}]}]}",
    )
    .unwrap();
    let regra = base.join("regras/R1.LSP/Rule.txt");
    std::fs::create_dir_all(regra.parent().unwrap()).unwrap();
    let regra_uri = crate::form::uri_from_path(&regra);

    let (editor, _) = Editor::start(None);
    let text = "Definir interno.com.empresa.vendas.Consultar ws;\n\
                definir alfa a;\n\
                ws.codemp = 1;\n\
                ws.ModoExecucao = 1;\n\
                ws.Executar();\n\
                se (ws.ErroExecucao <> \"\") { a = \"x\"; }\n\
                ws.CodRep = 2;\n\
                a = ws.tbItens[1].CodPro;\n\
                a = ws.tbItens[1].Nada;\n\
                Mensagem(Retorna, a);";
    editor.open(&regra_uri, 1, text);
    let found: Vec<String> = editor
        .diagnostics()
        .diagnostics
        .iter()
        .filter(|d| d.code == Some(lsp_types::NumberOrString::String("SEN069".into())))
        .map(|d| format!("{} {}", d.range.start.line, d.message))
        .collect();
    assert_eq!(
        found,
        [
            "6 CodRep não é parâmetro da porta no Port.json da exportação.",
            "8 Nada não é parâmetro do conjunto tbItens no Port.json da exportação.",
        ]
    );
    editor.stop();
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn hierarquia_de_chamadas() {
    let (mut editor, _) = Editor::start(None);
    let text = "Definir Funcao A();\n\
                Definir Funcao B();\n\
                Definir Funcao C();\n\
                A();\n\
                funcao A(); { B(); C(); B(); }\n\
                funcao B(); { C(); IntParaAlfa(1, x); }\n\
                funcao C(); { }";
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let prepare = editor.request(
        lsp_types::request::CallHierarchyPrepare::METHOD,
        json!({ "textDocument": { "uri": RULE },
                "position": { "line": 2, "character": 15 } }),
    );
    let items: Vec<lsp_types::CallHierarchyItem> =
        serde_json::from_value(prepare.response_result.unwrap()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].selection_range.start.line, 6,
        "vai para a funcao C"
    );
    let c = items[0].clone();

    let incoming = editor.request(
        lsp_types::request::CallHierarchyIncomingCalls::METHOD,
        json!({ "item": c }),
    );
    let incoming: Vec<lsp_types::CallHierarchyIncomingCall> =
        serde_json::from_value(incoming.response_result.unwrap()).unwrap();
    let callers: Vec<_> = incoming
        .iter()
        .map(|i| (i.from.name.clone(), i.from_ranges.len()))
        .collect();
    assert_eq!(callers, [("A".to_owned(), 1), ("B".to_owned(), 1)]);

    let a = incoming[0].from.clone();
    let outgoing = editor.request(
        lsp_types::request::CallHierarchyOutgoingCalls::METHOD,
        json!({ "item": a }),
    );
    let outgoing: Vec<lsp_types::CallHierarchyOutgoingCall> =
        serde_json::from_value(outgoing.response_result.unwrap()).unwrap();
    let called: Vec<_> = outgoing
        .iter()
        .map(|o| (o.to.name.clone(), o.from_ranges.len()))
        .collect();
    assert_eq!(called, [("B".to_owned(), 2), ("C".to_owned(), 1)]);

    // Quem chama A é o corpo principal da regra.
    let incoming = editor.request(
        lsp_types::request::CallHierarchyIncomingCalls::METHOD,
        json!({ "item": a }),
    );
    let incoming: Vec<lsp_types::CallHierarchyIncomingCall> =
        serde_json::from_value(incoming.response_result.unwrap()).unwrap();
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0].from.kind, lsp_types::SymbolKind::FILE);
    editor.stop();
}

#[test]
fn analisa_a_pasta_inteira_quando_ligado() {
    let base = std::env::temp_dir().join(format!("senior-lsp-pasta-{}", std::process::id()));
    let com_erro = base.join("regras/R1.LSP/Rule.txt");
    let certa = base.join("regras/R2.LSP/Rule.txt");
    std::fs::create_dir_all(com_erro.parent().unwrap()).unwrap();
    std::fs::create_dir_all(certa.parent().unwrap()).unwrap();
    std::fs::write(&com_erro, "x = ;").unwrap();
    std::fs::write(&certa, "definir alfa a;\na = \"x\";\nMensagem(Retorna, a);").unwrap();
    std::fs::write(base.join("regras/LEIAME.txt"), "x = ;").unwrap();
    let folder = crate::form::uri_from_path(&base);
    let erro_uri = crate::form::uri_from_path(&com_erro);

    // Desligado, nada é publicado sem abrir.
    let (editor, _) = Editor::start_with(
        None,
        json!({ "workspaceFolders": [{ "uri": folder, "name": "x" }] }),
    );
    editor.quiet();
    editor.stop();

    let (editor, _) = Editor::start_with(
        None,
        json!({ "analisar_pasta": true, "workspaceFolders": [{ "uri": folder, "name": "x" }] }),
    );
    // Só o arquivo com problema é publicado. O que não tem nada não manda lista vazia.
    let d = editor.diagnostics();
    assert_eq!(d.uri.as_str(), erro_uri);
    assert!(!d.diagnostics.is_empty());
    editor.quiet();

    // Fechar um arquivo aberto volta aos diagnósticos do disco.
    editor.open(&erro_uri, 1, "x = 1;");
    assert!(editor.diagnostics().diagnostics.is_empty());
    editor.close(&erro_uri);
    assert!(!editor.diagnostics().diagnostics.is_empty());

    // O arquivo mudou no disco.
    std::fs::write(&com_erro, "x = 1;").unwrap();
    editor.notify(
        lsp_types::notification::DidChangeWatchedFiles::METHOD,
        json!({ "changes": [{ "uri": erro_uri, "type": 2 }] }),
    );
    assert!(editor.diagnostics().diagnostics.is_empty());
    editor.stop();
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn regras_de_relatorio_separadas_do_json() {
    let base = std::env::temp_dir().join(format!("senior-lsp-separadas-{}", std::process::id()));
    let model = base.join("Modelos/M1.GER");
    let regras = model.join("regras");
    std::fs::create_dir_all(&regras).unwrap();
    std::fs::write(
        model.join("Base_Structure.json"),
        r#"{ "entrance": [
  { "sytem_varName": "ECodEmp", "description": "Empresa", "type_var": 1, "size": 0 }
], "function_rule": { "rule": { "prop_name": "SFunctionRule", "lines": "regras/Base_Structure.SFunctionRule.txt" } } }"#,
    )
    .unwrap();
    let funcoes = regras.join("Base_Structure.SFunctionRule.txt");
    std::fs::write(
        &funcoes,
        "Definir Funcao Dobro(Numero end N);\r\nfuncao Dobro(Numero end N); { N = N * 2; }\r\n",
    )
    .unwrap();
    std::fs::write(
        model.join("Cabecalho.json"),
        r#"{ "children": [ { "name": "Campo01", "rule": { "prop_name": "SOnPrint", "lines": "regras/Cabecalho.Campo01.SOnPrint.txt" } } ] }"#,
    )
    .unwrap();
    let campo = regras.join("Cabecalho.Campo01.SOnPrint.txt");
    let texto = "Definir Numero nValor;\r\nnValor = ECodEmp;\r\nDobro(nValor);\r\nNaoExiste();\r\n";
    std::fs::write(&campo, texto).unwrap();
    let uri = crate::form::uri_from_path(&campo);

    let (mut editor, _) = Editor::start(None);
    editor.open(&uri, 1, texto);
    // Só a função inexistente. A entrada e a função da outra regra do modelo valem aqui.
    let codes: Vec<_> = codes(&editor.diagnostics());
    assert_eq!(codes, ["SEN052"]);

    let r = editor.request(
        GotoDefinition::METHOD,
        json!({ "textDocument": { "uri": uri }, "position": { "line": 2, "character": 1 } }),
    );
    let locations = r.response_result.unwrap();
    assert_eq!(
        locations[0]["uri"],
        crate::form::uri_from_path(&funcoes).as_str()
    );

    // O tipo vem da pasta e não troca.
    let tipo = editor.request("senior-lsp/tipoRegra", json!({ "uri": uri }));
    assert_eq!(
        tipo.response_result.unwrap(),
        json!({ "tipo": "relatorio", "fixo": true })
    );
    editor.stop();
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn selecao_expandida() {
    let (mut editor, _) = Editor::start(None);
    let text = "definir numero n;\nse (n > 1) {\n    n = n + 1;\n}";
    editor.open(RULE, 1, text);
    editor.diagnostics();
    let response = editor.request(
        lsp_types::request::SelectionRangeRequest::METHOD,
        json!({ "textDocument": { "uri": RULE },
                "positions": [{ "line": 2, "character": 9 }] }),
    );
    let ranges: Vec<lsp_types::SelectionRange> =
        serde_json::from_value(response.response_result.unwrap()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let mut steps = Vec::new();
    let mut current = Some(&ranges[0]);
    while let Some(r) = current {
        let (s, e) = (r.range.start, r.range.end);
        let piece = if s.line == e.line {
            lines[s.line as usize][s.character as usize..e.character as usize].to_owned()
        } else {
            format!("linhas {}-{}", s.line, e.line)
        };
        steps.push(piece);
        current = r.parent.as_deref();
    }
    assert_eq!(steps[0], "n");
    assert_eq!(steps[1], "n + 1");
    assert!(
        steps.contains(&"n = n + 1;".to_owned()) || steps.contains(&"n = n + 1".to_owned()),
        "{steps:?}"
    );
    assert!(steps.iter().any(|s| s == "linhas 1-3"), "{steps:?}");
    editor.stop();
}

#[test]
fn link_do_json_para_a_regra_separada() {
    let base = std::env::temp_dir().join(format!("senior-lsp-link-{}", std::process::id()));
    let model = base.join("Modelos/M1.GER");
    std::fs::create_dir_all(model.join("regras")).unwrap();
    let json = "{ \"before_print\": { \"rule\": {\n    \"prop_name\": \"SOnBeforePrint\",\n    \"lines\": \"regras/Cabecalho.SOnBeforePrint.txt\"\n} } }";
    let path = model.join("Cabecalho.json");
    std::fs::write(&path, json).unwrap();
    let uri = crate::form::uri_from_path(&path);
    let (mut editor, _) = Editor::start(None);
    editor.open_as(&uri, "json", 1, json);
    editor.diagnostics();
    let response = editor.request(
        lsp_types::request::DocumentLinkRequest::METHOD,
        json!({ "textDocument": { "uri": uri } }),
    );
    let links: Vec<lsp_types::DocumentLink> =
        serde_json::from_value(response.response_result.unwrap()).unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].range.start, lsp_types::Position::new(2, 14));
    assert_eq!(
        links[0].target.as_ref().unwrap().as_str(),
        crate::form::uri_from_path(&model.join("regras/Cabecalho.SOnBeforePrint.txt"))
    );
    editor.stop();
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn informacao_relacionada_dos_avisos() {
    let (editor, _) = Editor::start(None);
    let text = "Definir Funcao F(Numero p);\n\
                definir numero x;\n\
                x = p;\n\
                F(1);\n\
                funcao F(Numero p); { x = p; }\n\
                x = 2;";
    editor.open(RULE, 1, text);
    let d = editor.diagnostics();
    let related = |code: &str| {
        d.diagnostics
            .iter()
            .find(|d| d.code == Some(lsp_types::NumberOrString::String(code.into())))
            .and_then(|d| d.related_information.clone())
            .map(|r| (r[0].message.clone(), r[0].location.range.start.line))
    };
    assert_eq!(related("SEN063"), Some(("Parâmetro p".into(), 0)));
    assert_eq!(related("SEN061"), Some(("Função F".into(), 4)));
    editor.stop();
}

/// Nomes, tipos e contêineres devolvidos pela busca de símbolos do workspace.
fn workspace_symbols(
    editor: &mut Editor,
    query: &str,
) -> Vec<(String, lsp_types::SymbolKind, String)> {
    let response = editor.request("workspace/symbol", json!({ "query": query }));
    let symbols: Vec<lsp_types::SymbolInformation> =
        serde_json::from_value(response.response_result.unwrap()).unwrap();
    symbols
        .into_iter()
        .map(|s| (s.name, s.kind, s.container_name.unwrap_or_default()))
        .collect()
}

#[test]
fn busca_simbolos_nas_regras_da_pasta() {
    use lsp_types::SymbolKind as K;
    let base = std::env::temp_dir().join(format!("senior-lsp-simbolos-{}", std::process::id()));
    let r1 = base.join("regras/CALC001.LSP/Rule.txt");
    let r2 = base.join("regras/OUTRA.LSP/Rule.txt");
    let tela = base.join("Formulários SGI/interface/I070EMP/rule.txt");
    let relatorio = base.join("Modelos/M1.GER/regras/Cabecalho.SOnPrint.txt");
    for path in [&r1, &r2, &tela, &relatorio] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    }
    std::fs::write(
        &r1,
        "definir numero nTotal;\r\nfuncao CalculaIcms(Numero nBase); {\r\n    definir alfa aLocal;\r\n    Rotulo1:\r\n    nTotal = nBase;\r\n}\r\n",
    )
    .unwrap();
    std::fs::write(
        &r2,
        "Definir Funcao Dobro(Numero n);\nfuncao Dobro(Numero n); { n = n * 2; }\n",
    )
    .unwrap();
    std::fs::write(&tela, "definir numero nTelaTotal;\n").unwrap();
    std::fs::write(&relatorio, "definir numero nRelTotal;\n").unwrap();
    // Fora das regras, o arquivo e o JSON não entram.
    std::fs::write(base.join("regras/LEIAME.txt"), "definir numero nLeiame;\n").unwrap();
    std::fs::write(
        base.join("regras/M.json"),
        "{ \"definir numero nJson;\": 1 }",
    )
    .unwrap();
    let folder = crate::form::uri_from_path(&base);
    let r1_uri = crate::form::uri_from_path(&r1);

    // Com a análise da pasta desligada, a primeira busca lê o disco.
    let options = json!({ "workspaceFolders": [{ "uri": folder, "name": "x" }] });
    let (mut editor, _) = Editor::start_with(None, options);
    let found = workspace_symbols(&mut editor, "total");
    let names: Vec<_> = found.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(names, ["nTotal", "nRelTotal", "nTelaTotal"], "{found:?}");
    assert_eq!(found[0], ("nTotal".into(), K::VARIABLE, "CALC001".into()));
    assert_eq!(found[1].2, "Cabecalho.SOnPrint");
    assert_eq!(found[2].2, "I070EMP");

    // Funções, declaração e implementação, variável de dentro da função e rótulo.
    assert_eq!(
        workspace_symbols(&mut editor, "dobro"),
        [
            ("Dobro".into(), K::FUNCTION, "OUTRA".into()),
            ("Dobro".into(), K::FUNCTION, "OUTRA".into())
        ]
    );
    assert_eq!(workspace_symbols(&mut editor, "alocal").len(), 1);
    assert_eq!(
        workspace_symbols(&mut editor, "rotulo1"),
        [("Rotulo1".into(), K::KEY, "CALC001".into())]
    );
    // Subsequência, sem diferenciar caixa, e o parâmetro não entra.
    assert_eq!(workspace_symbols(&mut editor, "clcicm")[0].0, "CalculaIcms");
    assert!(workspace_symbols(&mut editor, "nbase").is_empty());
    assert!(workspace_symbols(&mut editor, "nleiame").is_empty());
    assert!(workspace_symbols(&mut editor, "njson").is_empty());
    // A posição do nome.
    let response = editor.request("workspace/symbol", json!({ "query": "calculaicms" }));
    let symbols: Vec<lsp_types::SymbolInformation> =
        serde_json::from_value(response.response_result.unwrap()).unwrap();
    assert_eq!(symbols[0].location.uri.as_str(), r1_uri);
    assert_eq!(
        symbols[0].location.range,
        lsp_types::Range::new(
            lsp_types::Position::new(1, 7),
            lsp_types::Position::new(1, 18)
        )
    );

    // Arquivo alterado, criado e apagado no disco.
    std::fs::write(&r2, "definir numero nNovaTotal;\n").unwrap();
    let nova = base.join("regras/NOVA.LSP/Rule.txt");
    std::fs::create_dir_all(nova.parent().unwrap()).unwrap();
    std::fs::write(&nova, "definir numero nOutraTotal;\n").unwrap();
    std::fs::remove_file(&tela).unwrap();
    editor.notify(
        lsp_types::notification::DidChangeWatchedFiles::METHOD,
        json!({ "changes": [
            { "uri": crate::form::uri_from_path(&r2), "type": 2 },
            { "uri": crate::form::uri_from_path(&nova), "type": 1 },
            { "uri": crate::form::uri_from_path(&tela), "type": 3 },
        ] }),
    );
    let names: Vec<_> = workspace_symbols(&mut editor, "total")
        .into_iter()
        .map(|s| s.0)
        .collect();
    assert_eq!(names, ["nTotal", "nRelTotal", "nNovaTotal", "nOutraTotal"]);
    assert!(workspace_symbols(&mut editor, "dobro").is_empty());

    // O texto do editor vale no lugar do disco enquanto o arquivo está aberto.
    editor.open(&r1_uri, 1, "definir numero nSoNoEditor;\n");
    editor.diagnostics();
    assert_eq!(
        workspace_symbols(&mut editor, "nsoneditor"),
        [("nSoNoEditor".into(), K::VARIABLE, "CALC001".into())]
    );
    assert!(workspace_symbols(&mut editor, "calculaicms").is_empty());
    editor.close(&r1_uri);
    assert_eq!(workspace_symbols(&mut editor, "calculaicms").len(), 1);
    editor.stop();

    // Ligada a análise da pasta, o índice sai da leitura do início.
    let options =
        json!({ "analisar_pasta": true, "workspaceFolders": [{ "uri": folder, "name": "x" }] });
    let (mut editor, _) = Editor::start_with(None, options);
    let names: Vec<_> = workspace_symbols(&mut editor, "total")
        .into_iter()
        .map(|s| s.0)
        .collect();
    assert_eq!(names, ["nTotal", "nRelTotal", "nNovaTotal", "nOutraTotal"]);
    editor.stop();
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn busca_de_simbolos_limita_o_resultado() {
    let base = std::env::temp_dir().join(format!("senior-lsp-limite-{}", std::process::id()));
    let rule = base.join("regras/GRANDE.LSP/Rule.txt");
    std::fs::create_dir_all(rule.parent().unwrap()).unwrap();
    let text: String = (0..800)
        .map(|i| format!("definir numero nVar{i};\n"))
        .collect();
    std::fs::write(&rule, text).unwrap();
    let folder = crate::form::uri_from_path(&base);
    let (mut editor, _) = Editor::start_with(
        None,
        json!({ "workspaceFolders": [{ "uri": folder, "name": "x" }] }),
    );
    assert_eq!(workspace_symbols(&mut editor, "nvar").len(), 500);
    assert_eq!(workspace_symbols(&mut editor, "").len(), 500);
    // Os nomes mais curtos vêm primeiro.
    assert_eq!(workspace_symbols(&mut editor, "nvar1")[0].0, "nVar1");
    editor.stop();
    std::fs::remove_dir_all(&base).unwrap();
}
