//! Servidor de linguagem pelo protocolo LSP.
//!
//! O laço é síncrono. Uma regra média é analisada em microssegundos, então cada mensagem é
//! respondida na hora, sem fila nem thread de trabalho. O editor manda o texto inteiro a cada
//! mudança, e o arquivo é analisado de novo do começo.

mod diagnostics;
mod documents;

use std::error::Error;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
    Notification as LspNotification, PublishDiagnostics,
};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    InitializeParams, InitializeResult, PositionEncodingKind, PublishDiagnosticsParams,
    ServerCapabilities, ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};

use crate::line_index::{ColumnEncoding, LineIndex};
use documents::{Documents, is_rule};

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// Atende o editor pela conexão até o `shutdown` e o `exit`.
pub fn run(connection: Connection) -> Result<()> {
    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let encoding = negotiate_encoding(&params);
    let result = InitializeResult {
        capabilities: ServerCapabilities {
            position_encoding: Some(match encoding {
                ColumnEncoding::Utf8 => PositionEncodingKind::UTF8,
                ColumnEncoding::Utf16 => PositionEncodingKind::UTF16,
            }),
            text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
            ..ServerCapabilities::default()
        },
        server_info: Some(ServerInfo {
            name: "senior-lsp".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
    };
    connection.initialize_finish(id, serde_json::to_value(result)?)?;

    let mut server = Server {
        connection: &connection,
        documents: Documents::default(),
        encoding,
    };
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    return Ok(());
                }
                server.request(request)?;
            }
            Message::Notification(notification) => server.notification(notification)?,
            Message::Response(_) => {}
        }
    }
    Ok(())
}

/// UTF-8 se o editor anunciar, porque dispensa a conversão das colunas. Senão UTF-16, que é o
/// padrão do protocolo.
fn negotiate_encoding(params: &InitializeParams) -> ColumnEncoding {
    let offered = params
        .capabilities
        .general
        .as_ref()
        .and_then(|g| g.position_encodings.as_ref());
    match offered {
        Some(kinds) if kinds.contains(&PositionEncodingKind::UTF8) => ColumnEncoding::Utf8,
        _ => ColumnEncoding::Utf16,
    }
}

struct Server<'a> {
    connection: &'a Connection,
    documents: Documents,
    encoding: ColumnEncoding,
}

impl Server<'_> {
    fn request(&mut self, request: Request) -> Result<()> {
        let response = Response::new_err(
            request.id,
            ErrorCode::MethodNotFound as i32,
            format!("método não suportado: {}", request.method),
        );
        self.connection.sender.send(response.into())?;
        Ok(())
    }

    fn notification(&mut self, notification: Notification) -> Result<()> {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let params: DidOpenTextDocumentParams =
                    serde_json::from_value(notification.params)?;
                let doc = params.text_document;
                if is_rule(&doc.uri) {
                    self.documents.set(doc.uri.clone(), doc.text, doc.version);
                    self.publish(&doc.uri)?;
                }
            }
            DidChangeTextDocument::METHOD => {
                let params: DidChangeTextDocumentParams =
                    serde_json::from_value(notification.params)?;
                let uri = params.text_document.uri;
                if !is_rule(&uri) {
                    return Ok(());
                }
                // Com a sincronização completa, a última mudança traz o texto inteiro.
                if let Some(change) = params.content_changes.into_iter().last() {
                    self.documents
                        .set(uri.clone(), change.text, params.text_document.version);
                    self.publish(&uri)?;
                }
            }
            DidCloseTextDocument::METHOD => {
                let params: DidCloseTextDocumentParams =
                    serde_json::from_value(notification.params)?;
                let uri = params.text_document.uri;
                if self.documents.get(&uri).is_some() {
                    self.documents.remove(&uri);
                    // Sem isso, os erros do documento fechado continuam no editor.
                    self.send_diagnostics(uri, Vec::new(), None)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn publish(&self, uri: &Uri) -> Result<()> {
        let Some(doc) = self.documents.get(uri) else {
            return Ok(());
        };
        let tree = crate::parse(&doc.text)?;
        let index = LineIndex::new(&doc.text);
        let diagnostics = diagnostics::diagnostics(&doc.text, &tree, &index, self.encoding);
        self.send_diagnostics(uri.clone(), diagnostics, Some(doc.version))
    }

    fn send_diagnostics(
        &self,
        uri: Uri,
        diagnostics: Vec<lsp_types::Diagnostic>,
        version: Option<i32>,
    ) -> Result<()> {
        let params = PublishDiagnosticsParams {
            uri,
            diagnostics,
            version,
        };
        let notification = Notification::new(PublishDiagnostics::METHOD.to_owned(), params);
        self.connection.sender.send(notification.into())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
