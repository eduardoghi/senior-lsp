//! Servidor de linguagem pelo protocolo LSP.
//!
//! O laço é síncrono. Uma regra média é analisada em microssegundos, então cada mensagem é
//! respondida na hora, sem fila nem thread de trabalho. O editor manda o texto inteiro a cada
//! mudança, e o arquivo é analisado de novo do começo.

mod diagnostics;
mod documents;
mod folding;
mod highlight;
mod implementation;
mod inlay;
mod kind;
mod lens;
mod members;
mod navigation;
mod quickfix;
mod semantic;
mod signature;
mod webservice;

use std::collections::HashMap;
use std::error::Error;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
    Notification as LspNotification, PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, CodeLensRequest, Completion, DocumentHighlightRequest,
    DocumentSymbolRequest, ExecuteCommand, FoldingRangeRequest, GotoDefinition, GotoImplementation,
    HoverRequest, InlayHintRequest, PrepareRenameRequest, References, Rename,
    Request as LspRequest, SemanticTokensFullRequest, SignatureHelpRequest,
};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    HoverParams, HoverProviderCapability, InitializeParams, InitializeResult, OneOf,
    PositionEncodingKind, PublishDiagnosticsParams, ReferenceParams, ServerCapabilities,
    ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};

use crate::builtins::Context;
use crate::line_index::ColumnEncoding;
use documents::{Documents, is_report, is_rule};

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// Atende o editor pela conexão até o `shutdown` e o `exit`.
pub fn run(connection: Connection) -> Result<()> {
    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    configure_dictionaries(&params);
    let default_kind = default_kind(&params);
    let encoding = negotiate_encoding(&params);
    let result = InitializeResult {
        capabilities: ServerCapabilities {
            position_encoding: Some(match encoding {
                ColumnEncoding::Utf8 => PositionEncodingKind::UTF8,
                ColumnEncoding::Utf16 => PositionEncodingKind::UTF16,
            }),
            text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
            definition_provider: Some(OneOf::Left(true)),
            implementation_provider: Some(lsp_types::ImplementationProviderCapability::Simple(
                true,
            )),
            document_highlight_provider: Some(OneOf::Left(true)),
            code_lens_provider: Some(lsp_types::CodeLensOptions {
                resolve_provider: Some(false),
            }),
            folding_range_provider: Some(lsp_types::FoldingRangeProviderCapability::Simple(true)),
            inlay_hint_provider: Some(OneOf::Left(true)),
            references_provider: Some(OneOf::Left(true)),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            document_symbol_provider: Some(OneOf::Left(true)),
            completion_provider: Some(lsp_types::CompletionOptions {
                // Depois do ponto vêm os membros do cursor, da lista e da tabela.
                trigger_characters: Some(vec![".".to_owned()]),
                ..lsp_types::CompletionOptions::default()
            }),
            rename_provider: Some(OneOf::Right(lsp_types::RenameOptions {
                prepare_provider: Some(true),
                work_done_progress_options: Default::default(),
            })),
            semantic_tokens_provider: Some(
                lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(
                    lsp_types::SemanticTokensOptions {
                        legend: semantic::legend(),
                        full: Some(lsp_types::SemanticTokensFullOptions::Bool(true)),
                        range: None,
                        work_done_progress_options: Default::default(),
                    },
                ),
            ),
            signature_help_provider: Some(lsp_types::SignatureHelpOptions {
                trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
                retrigger_characters: None,
                work_done_progress_options: Default::default(),
            }),
            code_action_provider: Some(lsp_types::CodeActionProviderCapability::Simple(true)),
            execute_command_provider: Some(lsp_types::ExecuteCommandOptions {
                commands: vec![kind::COMMAND.to_owned()],
                work_done_progress_options: Default::default(),
            }),
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
        default_kind,
        chosen: HashMap::new(),
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
/// Caminhos do `.tbs` de produção e do de teste, pelas opções de inicialização do editor
/// (`tbs_producao` e `tbs_teste`) ou pelas variáveis `SENIOR_LSP_TBS_PRODUCAO` e
/// `SENIOR_LSP_TBS_TESTE`. As opções do editor valem primeiro.
fn configure_dictionaries(params: &InitializeParams) {
    let option = |key: &str, var: &str| {
        let from_editor = params
            .initialization_options
            .as_ref()
            .and_then(|o| o.get(key))
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        from_editor
            .or_else(|| std::env::var(var).ok())
            .map(|v| v.trim().to_owned())
            // Valor ainda com `${...}` é uma opção do plugin que ninguém preencheu.
            .filter(|v| !v.is_empty() && !v.starts_with("${"))
            .map(std::path::PathBuf::from)
    };
    crate::dictionary::configure(
        option("tbs_producao", "SENIOR_LSP_TBS_PRODUCAO"),
        option("tbs_teste", "SENIOR_LSP_TBS_TESTE"),
    );
}

/// Tipo dos arquivos soltos que não se chamam `Rule.txt`, pela opção `tipo_padrao`. Sem ela, ou
/// com um nome desconhecido, é regra comum.
fn default_kind(params: &InitializeParams) -> Context {
    params
        .initialization_options
        .as_ref()
        .and_then(|o| o.get("tipo_padrao"))
        .and_then(|v| v.as_str())
        .and_then(kind::parse)
        .unwrap_or(Context::Rule)
}

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
    default_kind: Context,
    /// Tipo escolhido pelo editor para arquivos soltos. Fica depois de fechar o documento, para
    /// valer de novo quando ele for reaberto.
    chosen: HashMap<Uri, Context>,
}

impl Server<'_> {
    fn request(&mut self, request: Request) -> Result<()> {
        let id = request.id.clone();
        let result = match request.method.as_str() {
            GotoDefinition::METHOD => {
                let params: GotoDefinitionParams = serde_json::from_value(request.params)?;
                let at = params.text_document_position_params;
                self.with(&at.text_document.uri, Some(at.position), |c| {
                    serde_json::to_value(GotoDefinitionResponse::Array(c.definition(at.position)))
                })
            }
            GotoImplementation::METHOD => {
                let params: lsp_types::request::GotoImplementationParams =
                    serde_json::from_value(request.params)?;
                let at = params.text_document_position_params;
                self.with(&at.text_document.uri, Some(at.position), |c| {
                    serde_json::to_value(GotoDefinitionResponse::Array(
                        c.implementation(at.position),
                    ))
                })
            }
            References::METHOD => {
                let params: ReferenceParams = serde_json::from_value(request.params)?;
                let at = params.text_document_position;
                let declaration = params.context.include_declaration;
                self.with(&at.text_document.uri, Some(at.position), |c| {
                    serde_json::to_value(c.references(at.position, declaration))
                })
            }
            HoverRequest::METHOD => {
                let params: HoverParams = serde_json::from_value(request.params)?;
                let at = params.text_document_position_params;
                self.with(&at.text_document.uri, Some(at.position), |c| {
                    serde_json::to_value(c.hover(at.position))
                })
            }
            Completion::METHOD => {
                let params: lsp_types::CompletionParams = serde_json::from_value(request.params)?;
                let at = params.text_document_position;
                self.with(&at.text_document.uri, Some(at.position), |c| {
                    serde_json::to_value(lsp_types::CompletionResponse::Array(
                        c.completion(at.position),
                    ))
                })
            }
            PrepareRenameRequest::METHOD => {
                let params: lsp_types::TextDocumentPositionParams =
                    serde_json::from_value(request.params)?;
                self.with(&params.text_document.uri, Some(params.position), |c| {
                    let range = c.prepare_rename(params.position);
                    serde_json::to_value(range.map(lsp_types::PrepareRenameResponse::Range))
                })
            }
            Rename::METHOD => {
                let params: lsp_types::RenameParams = serde_json::from_value(request.params)?;
                let at = params.text_document_position;
                let mut failure = None;
                let result = self.with(&at.text_document.uri, Some(at.position), |c| {
                    match c.rename(at.position, &params.new_name) {
                        Ok(edit) => serde_json::to_value(edit),
                        Err(message) => {
                            failure = Some(message);
                            Ok(serde_json::Value::Null)
                        }
                    }
                });
                if let Some(message) = failure {
                    let code = ErrorCode::InvalidParams as i32;
                    let response = Response::new_err(id, code, message);
                    self.connection.sender.send(response.into())?;
                    return Ok(());
                }
                result
            }
            DocumentSymbolRequest::METHOD => {
                let params: DocumentSymbolParams = serde_json::from_value(request.params)?;
                let uri = &params.text_document.uri;
                match self.documents.report(uri) {
                    // Num JSON de modelo, os símbolos de todas as regras dele.
                    Some(report) => {
                        let symbols: Vec<_> = report
                            .rules
                            .iter()
                            .flat_map(|rule| {
                                self.report_context(uri, report, rule).document_symbols()
                            })
                            .collect();
                        Ok(serde_json::to_value(DocumentSymbolResponse::Nested(
                            symbols,
                        ))?)
                    }
                    None => self.with(uri, None, |c| {
                        serde_json::to_value(DocumentSymbolResponse::Nested(c.document_symbols()))
                    }),
                }
            }
            CodeLensRequest::METHOD => {
                let params: lsp_types::CodeLensParams = serde_json::from_value(request.params)?;
                let uri = &params.text_document.uri;
                Ok(match self.documents.get(uri) {
                    Some(doc) => {
                        let context = navigation::Context {
                            doc,
                            uri,
                            encoding: self.encoding,
                            embedding: None,
                        };
                        serde_json::to_value(context.code_lenses())?
                    }
                    None => serde_json::Value::Null,
                })
            }
            InlayHintRequest::METHOD => {
                let params: lsp_types::InlayHintParams = serde_json::from_value(request.params)?;
                let uri = &params.text_document.uri;
                Ok(match self.documents.get(uri) {
                    Some(doc) => {
                        let context = navigation::Context {
                            doc,
                            uri,
                            encoding: self.encoding,
                            embedding: None,
                        };
                        serde_json::to_value(context.inlay_hints(params.range))?
                    }
                    None => serde_json::Value::Null,
                })
            }
            FoldingRangeRequest::METHOD => {
                let params: lsp_types::FoldingRangeParams = serde_json::from_value(request.params)?;
                // Nos JSON de modelo, as linhas da regra não são as linhas do arquivo.
                Ok(match self.documents.get(&params.text_document.uri) {
                    Some(doc) => serde_json::to_value(folding::ranges(doc, self.encoding))?,
                    None => serde_json::Value::Null,
                })
            }
            DocumentHighlightRequest::METHOD => {
                let params: lsp_types::DocumentHighlightParams =
                    serde_json::from_value(request.params)?;
                let at = params.text_document_position_params;
                self.with(&at.text_document.uri, Some(at.position), |c| {
                    serde_json::to_value(c.document_highlight(at.position))
                })
            }
            SemanticTokensFullRequest::METHOD => {
                let params: lsp_types::SemanticTokensParams =
                    serde_json::from_value(request.params)?;
                // Nos JSON de modelo de relatório não há destaque, porque a posição de cada regra
                // dentro do JSON mudaria a conta das colunas.
                Ok(match self.documents.get(&params.text_document.uri) {
                    Some(doc) => serde_json::to_value(semantic::tokens(doc, self.encoding))?,
                    None => serde_json::Value::Null,
                })
            }
            SignatureHelpRequest::METHOD => {
                let params: lsp_types::SignatureHelpParams =
                    serde_json::from_value(request.params)?;
                let at = params.text_document_position_params;
                self.with(&at.text_document.uri, Some(at.position), |c| {
                    serde_json::to_value(c.signature_help(at.position))
                })
            }
            CodeActionRequest::METHOD => {
                let params: lsp_types::CodeActionParams = serde_json::from_value(request.params)?;
                let uri = &params.text_document.uri;
                let mut actions = Vec::new();
                if let Some(doc) = self.documents.get(uri) {
                    let context = navigation::Context {
                        doc,
                        uri,
                        encoding: self.encoding,
                        embedding: None,
                    };
                    actions.extend(
                        context
                            .define_actions(params.range.start)
                            .into_iter()
                            .chain(context.check_fixes(params.range))
                            .map(lsp_types::CodeActionOrCommand::CodeAction),
                    );
                }
                actions.extend(self.kind_actions(uri));
                Ok(serde_json::to_value(actions)?)
            }
            ExecuteCommand::METHOD => {
                let params: lsp_types::ExecuteCommandParams =
                    serde_json::from_value(request.params)?;
                if let Err(message) = self.execute(&params) {
                    let code = ErrorCode::InvalidParams as i32;
                    let response = Response::new_err(id, code, message);
                    self.connection.sender.send(response.into())?;
                    return Ok(());
                }
                Ok(serde_json::Value::Null)
            }
            kind::REQUEST => {
                let uri: Uri = serde_json::from_value(request.params["uri"].clone())?;
                Ok(match self.documents.get(&uri) {
                    Some(doc) => serde_json::json!({
                        "tipo": kind::name(doc.context),
                        "fixo": kind::fixed(uri.as_str()),
                    }),
                    None => serde_json::Value::Null,
                })
            }
            method => {
                let response = Response::new_err(
                    id,
                    ErrorCode::MethodNotFound as i32,
                    format!("método não suportado: {method}"),
                );
                self.connection.sender.send(response.into())?;
                return Ok(());
            }
        };
        let response = Response::new_ok(id, result?);
        self.connection.sender.send(response.into())?;
        Ok(())
    }

    /// Resposta de um pedido sobre o documento, ou `null` se ele não está aberto, como uma regra
    /// que o servidor não analisa. Num JSON de modelo de relatório, a posição escolhe a regra, e
    /// fora de regra a resposta é `null`.
    fn with(
        &self,
        uri: &Uri,
        position: Option<lsp_types::Position>,
        f: impl FnOnce(&navigation::Context) -> serde_json::Result<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        if let Some(doc) = self.documents.get(uri) {
            let context = navigation::Context {
                doc,
                uri,
                encoding: self.encoding,
                embedding: None,
            };
            return Ok(f(&context)?);
        }
        let (Some(report), Some(position)) = (self.documents.report(uri), position) else {
            return Ok(serde_json::Value::Null);
        };
        let lc = crate::line_index::LineCol {
            line: position.line,
            col: position.character,
        };
        let offset = report.index.offset(&report.json, lc, self.encoding);
        match report.rule_at(offset) {
            Some(rule) => Ok(f(&self.report_context(uri, report, rule))?),
            None => Ok(serde_json::Value::Null),
        }
    }

    fn report_context<'a>(
        &self,
        uri: &'a Uri,
        report: &'a documents::ReportDocument,
        rule: &'a documents::ReportRule,
    ) -> navigation::Context<'a> {
        navigation::Context {
            doc: &rule.doc,
            uri,
            encoding: self.encoding,
            embedding: Some(navigation::Embedding {
                rule: &rule.embedded.rule,
                json: &report.json,
                index: &report.index,
            }),
        }
    }

    fn notification(&mut self, notification: Notification) -> Result<()> {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let params: DidOpenTextDocumentParams =
                    serde_json::from_value(notification.params)?;
                let doc = params.text_document;
                if is_report(&doc.uri) {
                    self.documents
                        .set_report(doc.uri.clone(), doc.text, doc.version)?;
                    self.publish(&doc.uri)?;
                } else if is_rule(&doc.uri) || doc.language_id == "lsp-senior" {
                    let context = self.rule_context(&doc.uri);
                    self.documents
                        .set(doc.uri.clone(), doc.text, doc.version, context)?;
                    self.publish(&doc.uri)?;
                }
            }
            DidChangeTextDocument::METHOD => {
                let params: DidChangeTextDocumentParams =
                    serde_json::from_value(notification.params)?;
                let uri = params.text_document.uri;
                let version = params.text_document.version;
                // Com a sincronização completa, a última mudança traz o texto inteiro.
                let Some(change) = params.content_changes.into_iter().last() else {
                    return Ok(());
                };
                // O didChange não traz a linguagem, então vale o que o didOpen aceitou.
                if self.documents.get(&uri).is_some() {
                    let context = self.rule_context(&uri);
                    self.documents
                        .set(uri.clone(), change.text, version, context)?;
                } else if is_report(&uri) {
                    self.documents
                        .set_report(uri.clone(), change.text, version)?;
                } else {
                    return Ok(());
                }
                self.publish(&uri)?;
            }
            DidCloseTextDocument::METHOD => {
                let params: DidCloseTextDocumentParams =
                    serde_json::from_value(notification.params)?;
                let uri = params.text_document.uri;
                if self.documents.remove(&uri) {
                    // Sem isso, os erros do documento fechado continuam no editor.
                    self.send_diagnostics(uri, Vec::new(), None)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Tipo de uma regra em texto. Na pasta de tela é sempre tela. Fora dela vale o que o editor
    /// escolheu, senão regra comum para `Rule.txt` e o tipo padrão para os outros arquivos.
    fn rule_context(&self, uri: &Uri) -> Context {
        if kind::fixed(uri.as_str()) {
            return Context::Sgi;
        }
        if let Some(&context) = self.chosen.get(uri) {
            return context;
        }
        if is_rule(uri) {
            Context::Rule
        } else {
            self.default_kind
        }
    }

    /// Ações que trocam o tipo de um arquivo solto, uma para cada tipo diferente do atual.
    fn kind_actions(&self, uri: &Uri) -> Vec<lsp_types::CodeActionOrCommand> {
        let Some(doc) = self.documents.get(uri) else {
            return Vec::new();
        };
        if kind::fixed(uri.as_str()) {
            return Vec::new();
        }
        kind::ALL
            .into_iter()
            .filter(|&c| c != doc.context)
            .map(|c| {
                lsp_types::CodeActionOrCommand::Command(lsp_types::Command {
                    title: kind::action_title(c).to_owned(),
                    command: kind::COMMAND.to_owned(),
                    arguments: Some(vec![
                        serde_json::to_value(uri).unwrap_or_default(),
                        kind::name(c).into(),
                    ]),
                })
            })
            .collect()
    }

    /// Executa o comando que troca o tipo e analisa o documento de novo. O erro vira resposta de
    /// parâmetro inválido.
    fn execute(
        &mut self,
        params: &lsp_types::ExecuteCommandParams,
    ) -> std::result::Result<(), String> {
        if params.command != kind::COMMAND {
            return Err(format!("comando desconhecido: {}", params.command));
        }
        let (uri, context) = match params.arguments.as_slice() {
            [uri, name] => {
                let uri: Uri = serde_json::from_value(uri.clone()).map_err(|e| e.to_string())?;
                let name = name.as_str().unwrap_or_default();
                let context =
                    kind::parse(name).ok_or_else(|| format!("tipo desconhecido: {name}"))?;
                (uri, context)
            }
            _ => return Err("o comando recebe o URI e o tipo".to_owned()),
        };
        if kind::fixed(uri.as_str()) {
            return Err("o tipo deste arquivo vem da pasta da exportação".to_owned());
        }
        self.chosen.insert(uri.clone(), context);
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(());
        };
        let (text, version) = (doc.text.clone(), doc.version);
        self.documents
            .set(uri.clone(), text, version, context)
            .map_err(|e| e.to_string())?;
        self.publish(&uri).map_err(|e| e.to_string())
    }

    fn publish(&self, uri: &Uri) -> Result<()> {
        if let Some(report) = self.documents.report(uri) {
            let mut all = Vec::new();
            for rule in &report.rules {
                let to_range = |range: text_size::TextRange| {
                    let json_range = rule.embedded.rule.range_to_json(range);
                    let p = |offset| {
                        let lc = report.index.line_col(&report.json, offset, self.encoding);
                        lsp_types::Position::new(lc.line, lc.col)
                    };
                    lsp_types::Range::new(p(json_range.start()), p(json_range.end()))
                };
                let doc = &rule.doc;
                all.extend(diagnostics::diagnostics_with(
                    &doc.text,
                    &doc.tree,
                    &doc.symbols,
                    doc.context,
                    &doc.inherited,
                    &to_range,
                ));
            }
            return self.send_diagnostics(uri.clone(), all, Some(report.version));
        }
        let Some(doc) = self.documents.get(uri) else {
            return Ok(());
        };
        let mut diagnostics = diagnostics::diagnostics(
            &doc.text,
            &doc.tree,
            &doc.symbols,
            doc.context,
            &doc.inherited,
            &doc.index,
            self.encoding,
        );
        // Os parâmetros do web service vêm do Port.json no disco, fora da validação da regra.
        let context = navigation::Context {
            doc,
            uri,
            encoding: self.encoding,
            embedding: None,
        };
        diagnostics.extend(context.webservice_diagnostics());
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
