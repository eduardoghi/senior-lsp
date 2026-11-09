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
