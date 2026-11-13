//! Servidor de linguagem pelo protocolo LSP.
//!
//! O laço é síncrono. Uma regra média é analisada em microssegundos, então cada mensagem é
//! respondida na hora, sem fila nem thread de trabalho. O editor manda só o trecho alterado de cada
//! mudança (sincronização incremental), e o arquivo é analisado de novo do começo.

mod calls;
mod config;
mod diagnostics;
mod directives;
mod documents;
mod extract;
mod folding;
mod highlight;
mod ignore;
mod implementation;
mod inlay;
mod kind;
mod lens;
mod links;
mod members;
mod navigation;
mod quickfix;
mod related;
mod selection;
mod semantic;
mod signature;
mod sql;
mod sync;
mod webservice;
mod workspace;
mod workspace_symbols;

use std::collections::HashMap;
use std::error::Error;
use std::panic::{AssertUnwindSafe, catch_unwind};

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
    Notification as LspNotification, PublishDiagnostics,
};
use lsp_types::request::{
    CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare,
    CodeActionRequest, CodeLensRequest, Completion, DocumentHighlightRequest, DocumentLinkRequest,
    DocumentSymbolRequest, ExecuteCommand, FoldingRangeRequest, GotoDefinition, GotoImplementation,
    HoverRequest, InlayHintRequest, PrepareRenameRequest, References, Rename,
    Request as LspRequest, SelectionRangeRequest, SemanticTokensFullRequest, SignatureHelpRequest,
    WorkspaceSymbolRequest,
};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    HoverParams, HoverProviderCapability, InitializeResult, OneOf, PositionEncodingKind,
    PublishDiagnosticsParams, ReferenceParams, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};

use crate::builtins::Context;
use crate::line_index::{ColumnEncoding, LineIndex};
use documents::{Documents, is_report, is_report_rule, is_rule};

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// Comando que lê de novo os `.tbs` na hora, depois de uma atualização do ERP ou de uma
/// customização no CBDS, sem esperar a conferência periódica.
const RELOAD_DICTIONARY: &str = "senior-lsp.recarregarDicionario";

/// Atende o editor pela conexão até o `shutdown` e o `exit`.
pub fn run(connection: Connection) -> Result<()> {
    let (id, params) = connection.initialize_start()?;
    // Só os campos de que o servidor precisa são lidos, e um que falte ou venha fora do formato
    // vale como ausente. Um cliente que manda capacidades incompletas (um `semanticTokens` sem
    // `tokenTypes`, por exemplo) não derruba o `initialize`.
    let params = ClientInit::read(&params);
    crate::dictionary::configure(
        dictionary_path(params.options, TBS_PRODUCTION),
        dictionary_path(params.options, TBS_TEST),
    );
    let default_kind = default_kind(params.options);
    let folders = params.folders.clone();
    let analyze_folders = analyze_folders(params.options);
    let encoding = negotiate_encoding(&params);
    let result = InitializeResult {
        capabilities: ServerCapabilities {
            position_encoding: Some(match encoding {
                ColumnEncoding::Utf8 => PositionEncodingKind::UTF8,
                ColumnEncoding::Utf16 => PositionEncodingKind::UTF16,
            }),
            text_document_sync: Some(TextDocumentSyncCapability::Kind(
                TextDocumentSyncKind::INCREMENTAL,
            )),
            definition_provider: Some(OneOf::Left(true)),
            implementation_provider: Some(lsp_types::ImplementationProviderCapability::Simple(
                true,
            )),
            document_highlight_provider: Some(OneOf::Left(true)),
            document_link_provider: Some(lsp_types::DocumentLinkOptions {
                resolve_provider: Some(false),
                work_done_progress_options: Default::default(),
            }),
            selection_range_provider: Some(lsp_types::SelectionRangeProviderCapability::Simple(
                true,
            )),
            call_hierarchy_provider: Some(lsp_types::CallHierarchyServerCapability::Simple(true)),
            code_lens_provider: Some(lsp_types::CodeLensOptions {
                resolve_provider: Some(false),
            }),
            folding_range_provider: Some(lsp_types::FoldingRangeProviderCapability::Simple(true)),
            inlay_hint_provider: Some(OneOf::Left(true)),
            references_provider: Some(OneOf::Left(true)),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            document_symbol_provider: Some(OneOf::Left(true)),
            workspace_symbol_provider: Some(OneOf::Left(true)),
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
                commands: vec![kind::COMMAND.to_owned(), RELOAD_DICTIONARY.to_owned()],
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
    log_option_warnings(&connection, params.options)?;

    let mut server = Server {
        connection: &connection,
        documents: Documents::default(),
        encoding,
        default_kind,
        chosen: HashMap::new(),
        folders,
        analyze_folders,
        index: Default::default(),
        configs: Default::default(),
        published: Default::default(),
    };
    // O `initialize_finish` já esperou o `initialized`, então o editor está pronto.
    server.analyze_workspace()?;
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

/// Opções do servidor, como o editor as manda no `initializationOptions` do `initialize` e, com o
/// servidor rodando, no `settings` do `workspace/didChangeConfiguration`.
#[derive(Clone, Copy)]
struct Options<'a>(Option<&'a serde_json::Value>);

impl<'a> Options<'a> {
    fn get(self, key: &str) -> Option<&'a serde_json::Value> {
        self.0?.get(key)
    }
}

/// Opção e variável de ambiente com o caminho do `.tbs` de produção e do de teste.
const TBS_PRODUCTION: (&str, &str) = ("tbs_producao", "SENIOR_LSP_TBS_PRODUCAO");
const TBS_TEST: (&str, &str) = ("tbs_teste", "SENIOR_LSP_TBS_TESTE");

/// O que o servidor lê do `initialize` do editor.
struct ClientInit<'a> {
    options: Options<'a>,
    /// Pastas do workspace, onde ficam as regras da análise da pasta inteira e da busca de
    /// símbolos. Sem `workspaceFolders`, vale o `rootUri`.
    folders: Vec<std::path::PathBuf>,
    /// O editor aceita colunas em UTF-8.
    utf8: bool,
}

impl<'a> ClientInit<'a> {
    fn read(params: &'a serde_json::Value) -> ClientInit<'a> {
        let uris: Vec<&str> = match params.get("workspaceFolders").and_then(|f| f.as_array()) {
            Some(folders) => folders
                .iter()
                .filter_map(|f| f.get("uri")?.as_str())
                .collect(),
            None => params
                .get("rootUri")
                .and_then(|u| u.as_str())
                .into_iter()
                .collect(),
        };
        let utf8 = params
            .pointer("/capabilities/general/positionEncodings")
            .and_then(|e| e.as_array())
            .is_some_and(|kinds| kinds.iter().any(|k| k.as_str() == Some("utf-8")));
        ClientInit {
            options: Options(params.get("initializationOptions")),
            folders: uris
                .into_iter()
                .filter_map(crate::form::path_from_uri)
                .collect(),
            utf8,
        }
    }
}

/// Avisos sobre as opções que o servidor ignorou por estarem fora do formato, com nome
/// desconhecido ou com um tipo de regra que não existe.
fn option_warnings(options: Options) -> Vec<String> {
    let Some(options) = options.0.filter(|o| !o.is_null()) else {
        return Vec::new();
    };
    let Some(options) = options.as_object() else {
        return vec!["senior-lsp: as opções não são um objeto JSON e foram ignoradas.".to_owned()];
    };
    let mut warnings = Vec::new();
    for (key, value) in options {
        if value.is_null() {
            continue;
        }
        let problem = match key.as_str() {
            "tbs_producao" | "tbs_teste" if !value.is_string() => Some("deveria ser texto"),
            "analisar_pasta" if !value.is_boolean() => Some("deveria ser true ou false"),
            "tipo_padrao" => match value.as_str() {
                None => Some("deveria ser texto"),
                Some(name) if kind::parse(name).is_none() => {
                    Some("não é comum, tela nem relatorio, e vale regra comum")
                }
                Some(_) => None,
            },
            "tbs_producao" | "tbs_teste" | "analisar_pasta" => None,
            _ => Some("não é uma opção do senior-lsp"),
        };
        if let Some(problem) = problem {
            warnings.push(format!("senior-lsp: a opção {key} = {value} {problem}."));
        }
    }
    warnings
}

/// Manda os avisos das opções para o log do editor. As opções são escritas à mão na configuração
/// do editor. Uma fora do formato é ignorada como as capacidades, mas sem aviso a pessoa não
/// saberia por que o dicionário ou a análise da pasta não ligaram.
fn log_option_warnings(connection: &Connection, options: Options) -> Result<()> {
    for message in option_warnings(options) {
        let params = lsp_types::LogMessageParams {
            typ: lsp_types::MessageType::WARNING,
            message,
        };
        connection.sender.send(
            Notification::new(
                lsp_types::notification::LogMessage::METHOD.to_owned(),
                params,
            )
            .into(),
        )?;
    }
    Ok(())
}

/// Caminho de um `.tbs`, pela opção do editor (`tbs_producao` ou `tbs_teste`) ou pela variável de
/// ambiente (`SENIOR_LSP_TBS_PRODUCAO` ou `SENIOR_LSP_TBS_TESTE`). A opção do editor vale primeiro.
fn dictionary_path(options: Options, (key, var): (&str, &str)) -> Option<std::path::PathBuf> {
    let from_editor = options.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    from_editor
        .or_else(|| std::env::var(var).ok())
        .map(|v| v.trim().to_owned())
        // Valor ainda com `${...}` é uma opção do plugin que ninguém preencheu.
        .filter(|v| !v.is_empty() && !v.starts_with("${"))
        .map(std::path::PathBuf::from)
}

/// Tipo dos arquivos soltos que não se chamam `Rule.txt`, pela opção `tipo_padrao`. Sem ela, ou
/// com um nome desconhecido, é regra comum.
fn default_kind(options: Options) -> Context {
    options
        .get("tipo_padrao")
        .and_then(|v| v.as_str())
        .and_then(kind::parse)
        .unwrap_or(Context::Rule)
}

/// Opção `analisar_pasta`, que liga os diagnósticos da pasta inteira.
fn analyze_folders(options: Options) -> bool {
    options
        .get("analisar_pasta")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// UTF-8 se o editor anunciar, porque dispensa a conversão das colunas. Senão UTF-16, que é o
/// padrão do protocolo.
fn negotiate_encoding(params: &ClientInit) -> ColumnEncoding {
    if params.utf8 {
        ColumnEncoding::Utf8
    } else {
        ColumnEncoding::Utf16
    }
}

/// Erro que vai na resposta de um pedido, como o renomear para um nome inválido.
#[derive(Debug)]
struct Failure {
    code: ErrorCode,
    message: String,
}

impl Failure {
    fn boxed(code: ErrorCode, message: String) -> Box<dyn Error + Send + Sync> {
        Box::new(Failure { code, message })
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for Failure {}

/// Texto de um pânico, que o `panic!` guarda como `&str` ou `String`.
fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("pânico sem mensagem")
}

struct Server<'a> {
    connection: &'a Connection,
    documents: Documents,
    encoding: ColumnEncoding,
    default_kind: Context,
    /// Pastas do workspace.
    folders: Vec<std::path::PathBuf>,
    /// Opção `analisar_pasta`: analisa as regras das pastas sem abrir cada arquivo.
    analyze_folders: bool,
    /// Definições das regras das pastas, para a busca de símbolos do workspace.
    index: workspace_symbols::SymbolIndex,
    /// Tipo escolhido pelo editor para arquivos soltos. Fica depois de fechar o documento, para
    /// valer de novo quando ele for reaberto.
    chosen: HashMap<Uri, Context>,
    /// Arquivos `senior-lsp.json` já lidos.
    configs: std::cell::RefCell<config::Configs>,
    /// Arquivos para os quais o servidor publicou algum diagnóstico, para limpar a lista deles
    /// quando a análise da pasta ou a configuração passa a não achar nada.
    published: std::cell::RefCell<std::collections::HashSet<Uri>>,
}

impl Server<'_> {
    /// Responde o pedido. Parâmetros fora do formato e um pânico no meio do atendimento viram
    /// resposta de erro, e o servidor segue atendendo os outros pedidos.
    fn request(&mut self, request: Request) -> Result<()> {
        let id = request.id.clone();
        let outcome = catch_unwind(AssertUnwindSafe(|| self.answer(request)));
        let response = match outcome {
            // O `Response::new_ok` passa o resultado por `serde_json::to_value`, que copia de novo
            // uma árvore que já é JSON. Nos pedidos grandes, como os símbolos do documento, a
            // cópia pesa.
            Ok(Ok(result)) => Response {
                id,
                response_result: Ok(result),
            },
            Ok(Err(error)) => {
                let (code, message) = match error.downcast::<Failure>() {
                    Ok(failure) => (failure.code, failure.message),
                    // Fora o `Failure`, o atendimento só falha ao ler os parâmetros do pedido.
                    Err(error) if error.is::<serde_json::Error>() => (
                        ErrorCode::InvalidParams,
                        format!("parâmetros inválidos: {error}"),
                    ),
                    Err(error) => (ErrorCode::InternalError, error.to_string()),
                };
                Response::new_err(id, code as i32, message)
            }
            Err(panic) => Response::new_err(
                id,
                ErrorCode::InternalError as i32,
                format!("erro interno do senior-lsp: {}", panic_message(&*panic)),
            ),
        };
        self.connection.sender.send(response.into())?;
        Ok(())
    }

    /// Resultado do pedido, ou o erro que vai na resposta.
    fn answer(&mut self, request: Request) -> Result<serde_json::Value> {
        match request.method.as_str() {
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
                    return Err(Failure::boxed(ErrorCode::InvalidParams, message));
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
            WorkspaceSymbolRequest::METHOD => {
                let params: lsp_types::WorkspaceSymbolParams =
                    serde_json::from_value(request.params)?;
                let symbols = self.workspace_symbols(&params.query);
                Ok(serde_json::to_value(
                    lsp_types::WorkspaceSymbolResponse::Flat(symbols),
                )?)
            }
            CallHierarchyPrepare::METHOD => {
                let params: lsp_types::CallHierarchyPrepareParams =
                    serde_json::from_value(request.params)?;
                let at = params.text_document_position_params;
                self.with(&at.text_document.uri, Some(at.position), |c| {
                    serde_json::to_value(c.prepare_call_hierarchy(at.position))
                })
            }
            CallHierarchyIncomingCalls::METHOD => {
                let params: lsp_types::CallHierarchyIncomingCallsParams =
                    serde_json::from_value(request.params)?;
                let item = params.item;
                self.with(&item.uri.clone(), Some(item.selection_range.start), |c| {
                    serde_json::to_value(c.incoming_calls(&item))
                })
            }
            CallHierarchyOutgoingCalls::METHOD => {
                let params: lsp_types::CallHierarchyOutgoingCallsParams =
                    serde_json::from_value(request.params)?;
                let item = params.item;
                self.with(&item.uri.clone(), Some(item.selection_range.start), |c| {
                    serde_json::to_value(c.outgoing_calls(&item))
                })
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
            DocumentLinkRequest::METHOD => {
                let params: lsp_types::DocumentLinkParams = serde_json::from_value(request.params)?;
                let uri = &params.text_document.uri;
                Ok(match self.documents.report(uri) {
                    Some(report) => serde_json::to_value(links::links(report, uri, self.encoding))?,
                    None => serde_json::Value::Null,
                })
            }
            SelectionRangeRequest::METHOD => {
                let params: lsp_types::SelectionRangeParams =
                    serde_json::from_value(request.params)?;
                let uri = &params.text_document.uri;
                Ok(match self.documents.get(uri) {
                    Some(doc) => {
                        let context = navigation::Context {
                            doc,
                            uri,
                            encoding: self.encoding,
                            embedding: None,
                        };
                        serde_json::to_value(context.selection_ranges(&params.positions))?
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
                // O cliente pode pedir só alguns tipos de ação, como `refactor` no menu de
                // refatorar.
                let extract = params.context.only.as_ref().is_none_or(|only| {
                    only.iter()
                        .any(|k| matches!(k.as_str(), "" | "refactor" | "refactor.extract"))
                });
                if let Some(doc) = self.documents.get(uri) {
                    let context = navigation::Context {
                        doc,
                        uri,
                        encoding: self.encoding,
                        embedding: None,
                    };
                    let shown = self
                        .diagnostics_of(&self.documents, uri)
                        .map(|(diagnostics, _)| diagnostics)
                        .unwrap_or_default();
                    actions.extend(
                        context
                            .define_actions(params.range.start)
                            .into_iter()
                            .chain(context.check_fixes(params.range))
                            .chain(
                                extract
                                    .then(|| context.extract_actions(params.range))
                                    .into_iter()
                                    .flatten(),
                            )
                            .chain(context.ignore_actions(params.range, &shown))
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
                    return Err(Failure::boxed(ErrorCode::InvalidParams, message));
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
            #[cfg(test)]
            tests::PANIC => panic!("pânico de teste"),
            method => Err(Failure::boxed(
                ErrorCode::MethodNotFound,
                format!("método não suportado: {method}"),
            )),
        }
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
        let offset = report.index().offset(&report.json, lc, self.encoding);
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
                index: report.index(),
            }),
        }
    }

    /// Trata a notificação. Uma fora do formato, ou que dá pânico, é registrada no log e
    /// ignorada, porque notificação não tem resposta. O documento que estava sendo analisado pode
    /// ficar sem análise até ser reaberto. Só a conexão fechada encerra o servidor.
    fn notification(&mut self, notification: Notification) -> Result<()> {
        let method = notification.method.clone();
        match catch_unwind(AssertUnwindSafe(|| self.handle(notification))) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error))
                if error.is::<serde_json::Error>() || error.is::<crate::TextTooLarge>() =>
            {
                eprintln!("senior-lsp: {method} ignorado ({error})");
                Ok(())
            }
            Ok(Err(error)) => Err(error),
            Err(panic) => {
                eprintln!(
                    "senior-lsp: erro interno em {method}: {}",
                    panic_message(&*panic)
                );
                Ok(())
            }
        }
    }

    fn handle(&mut self, notification: Notification) -> Result<()> {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let params: DidOpenTextDocumentParams =
                    serde_json::from_value(notification.params)?;
                let doc = params.text_document;
                // Abrir um arquivo é a chance de o editor sem aviso de arquivo (como o Kate) ver um
                // `senior-lsp.json` novo ou alterado.
                self.configs.borrow_mut().clear();
                if is_report(&doc.uri) {
                    self.documents
                        .set_report(doc.uri.clone(), doc.text, doc.version)?;
                    self.publish(&doc.uri)?;
                } else if is_rule(&doc.uri)
                    || is_report_rule(&doc.uri)
                    || doc.language_id == "lsp-senior"
                {
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
                let changes = params.content_changes;
                if changes.is_empty() {
                    return Ok(());
                }
                // O didChange não traz a linguagem, então vale o que o didOpen aceitou.
                if let Some(doc) = self.documents.take(&uri) {
                    let context = self.rule_context(&uri);
                    let text = sync::apply(doc.text, &doc.index, changes, self.encoding);
                    self.documents.set(uri.clone(), text, version, context)?;
                } else if let Some(report) = self.documents.take_report(&uri) {
                    let text = sync::apply(
                        report.json.to_string(),
                        report.index(),
                        changes,
                        self.encoding,
                    );
                    self.documents.set_report(uri.clone(), text, version)?;
                } else if is_report(&uri) && changes.iter().any(|c| c.range.is_none()) {
                    // JSON de modelo que o servidor não tem. Só o texto inteiro serve de base.
                    let text =
                        sync::apply(String::new(), &LineIndex::new(""), changes, self.encoding);
                    self.documents.set_report(uri.clone(), text, version)?;
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
                    // Com a pasta inteira analisada, volta a valer o arquivo do disco. Senão, os
                    // erros do documento fechado continuariam no editor.
                    match crate::form::path_from_uri(uri.as_str()) {
                        Some(path) if self.analyzes(&path) && path.is_file() => {
                            self.analyze_disk(&path)?;
                        }
                        _ => self.send_diagnostics(uri, Vec::new(), None)?,
                    }
                }
            }
            lsp_types::notification::DidChangeWatchedFiles::METHOD => {
                let params: lsp_types::DidChangeWatchedFilesParams =
                    serde_json::from_value(notification.params)?;
                let mut config_changed = false;
                for change in params.changes {
                    let Some(path) = crate::form::path_from_uri(change.uri.as_str()) else {
                        continue;
                    };
                    if path.file_name().is_some_and(|n| n == config::FILE_NAME) {
                        config_changed = true;
                        continue;
                    }
                    let deleted = change.typ == lsp_types::FileChangeType::DELETED;
                    self.update_symbols(&path, deleted);
                    if !self.analyzes(&path) {
                        continue;
                    }
                    if deleted {
                        if self.documents.get(&change.uri).is_none() {
                            self.send_diagnostics(change.uri, Vec::new(), None)?;
                        }
                    } else {
                        self.analyze_disk(&path)?;
                    }
                }
                if config_changed {
                    self.reload_config()?;
                }
            }
            DidChangeConfiguration::METHOD => {
                // Lido sem o tipo do protocolo, para um aviso sem `settings` não virar erro.
                if let Some(settings) = notification.params.get("settings") {
                    self.configure(Options(Some(settings)))?;
                }
            }
            #[cfg(test)]
            tests::PANIC => panic!("pânico de teste"),
            _ => {}
        }
        Ok(())
    }

    /// Tipo de uma regra em texto. Na pasta de tela é sempre tela. Fora dela vale o que o editor
    /// escolheu, senão regra comum para `Rule.txt` e o tipo padrão para os outros arquivos.
    fn rule_context(&self, uri: &Uri) -> Context {
        if is_report_rule(uri) {
            return Context::Report;
        }
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
        if params.command == RELOAD_DICTIONARY {
            crate::dictionary::reload();
            return self.reanalyze_open().map_err(|e| e.to_string());
        }
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
        match self.diagnostics_of(&self.documents, uri) {
            Some((diagnostics, version)) => {
                self.send_diagnostics(uri.clone(), diagnostics, version)
            }
            None => Ok(()),
        }
    }

    /// Diagnósticos de um documento guardado em `documents`, que pode ser o dos abertos ou um
    /// montado só para analisar um arquivo do disco.
    fn diagnostics_of(
        &self,
        documents: &Documents,
        uri: &Uri,
    ) -> Option<(Vec<lsp_types::Diagnostic>, Option<i32>)> {
        let levels = self.levels_of(uri);
        if let Some(report) = documents.report(uri) {
            let mut all = Vec::new();
            for rule in &report.rules {
                let to_range = |range: text_size::TextRange| {
                    let json_range = rule.embedded.rule.range_to_json(range);
                    let p = |offset| {
                        let lc = report.index().line_col(&report.json, offset, self.encoding);
                        lsp_types::Position::new(lc.line, lc.col)
                    };
                    lsp_types::Range::new(p(json_range.start()), p(json_range.end()))
                };
                all.extend(diagnostics::diagnostics(
                    &rule.doc,
                    Vec::new(),
                    &levels,
                    &to_range,
                ));
            }
            return Some((all, Some(report.version)));
        }
        let doc = documents.get(uri)?;
        // Os parâmetros do web service vêm do Port.json no disco, fora da validação da regra.
        let context = navigation::Context {
            doc,
            uri,
            encoding: self.encoding,
            embedding: None,
        };
        let mut diagnostics = diagnostics::diagnostics(
            doc,
            context.webservice_diagnostics(),
            &levels,
            &diagnostics::lsp_range(doc, self.encoding),
        );
        context.add_related(&mut diagnostics);
        Some((diagnostics, Some(doc.version)))
    }

    /// Níveis do `senior-lsp.json` que vale para o arquivo. O que o arquivo tem de errado vai ao
    /// editor como aviso, uma vez a cada leitura.
    fn levels_of(&self, uri: &Uri) -> config::Levels {
        let Some(path) = crate::form::path_from_uri(uri.as_str()) else {
            return config::Levels::default();
        };
        let mut warnings = Vec::new();
        let levels = self.configs.borrow_mut().levels(&path, &mut warnings);
        for message in warnings {
            let params = lsp_types::ShowMessageParams {
                typ: lsp_types::MessageType::WARNING,
                message,
            };
            let notification = Notification::new(
                lsp_types::notification::ShowMessage::METHOD.to_owned(),
                params,
            );
            // Sem conexão o servidor já está terminando, e o aviso não faz falta.
            let _ = self.connection.sender.send(notification.into());
        }
        levels
    }

    /// Aplica as opções que o editor mandou com o servidor rodando. Só as que vieram contam, e a
    /// que falta continua como está. Cada uma refaz só o que depende dela. O `.tbs` novo analisa de
    /// novo os documentos abertos e a pasta, o tipo padrão só os abertos, e a análise da pasta roda
    /// ao ligar e limpa os diagnósticos dos arquivos fechados ao desligar.
    fn configure(&mut self, options: Options) -> Result<()> {
        log_option_warnings(self.connection, options)?;
        let mut dictionaries = false;
        for (test, option) in [(false, TBS_PRODUCTION), (true, TBS_TEST)] {
            if options.get(option.0).is_some() {
                let path = dictionary_path(options, option);
                dictionaries |= crate::dictionary::set_path(test, path);
            }
        }
        let kind = options
            .get("tipo_padrao")
            .map(|_| default_kind(options))
            .filter(|&kind| kind != self.default_kind);
        let folders = options
            .get("analisar_pasta")
            .map(|_| analyze_folders(options))
            .filter(|&on| on != self.analyze_folders);
        self.default_kind = kind.unwrap_or(self.default_kind);
        self.analyze_folders = folders.unwrap_or(self.analyze_folders);
        if dictionaries {
            self.reanalyze_open()?;
        } else if kind.is_some() {
            self.reanalyze_documents()?;
        }
        match folders {
            Some(true) if !dictionaries => self.analyze_workspace(),
            Some(false) => self.clear_closed(),
            _ => Ok(()),
        }
    }

    /// Limpa os diagnósticos dos arquivos fechados, que vieram da análise da pasta.
    fn clear_closed(&self) -> Result<()> {
        let published: Vec<Uri> = self.published.borrow().iter().cloned().collect();
        for uri in published {
            if self.documents.get(&uri).is_none() && self.documents.report(&uri).is_none() {
                self.send_diagnostics(uri, Vec::new(), None)?;
            }
        }
        Ok(())
    }

    /// Analisa de novo os documentos abertos, que guardam o dicionário e o tipo de quando foram
    /// analisados.
    fn reanalyze_documents(&mut self) -> Result<()> {
        for uri in self.documents.uris() {
            if let Some(doc) = self.documents.get(&uri) {
                let (text, version) = (doc.text.clone(), doc.version);
                let context = self.rule_context(&uri);
                self.documents.set(uri.clone(), text, version, context)?;
            } else if let Some(report) = self.documents.report(&uri) {
                let (json, version) = (report.json.to_string(), report.version);
                self.documents.set_report(uri.clone(), json, version)?;
            }
            self.publish(&uri)?;
        }
        Ok(())
    }

    /// Analisa de novo os documentos abertos e a pasta, se a análise dela estiver ligada.
    fn reanalyze_open(&mut self) -> Result<()> {
        self.reanalyze_documents()?;
        self.analyze_workspace()
    }

    /// Lê os `senior-lsp.json` de novo e publica outra vez os diagnósticos dos arquivos abertos e,
    /// com a pasta inteira analisada, dos arquivos do disco.
    fn reload_config(&mut self) -> Result<()> {
        self.configs.borrow_mut().reload();
        for uri in self.documents.uris() {
            self.publish(&uri)?;
        }
        self.analyze_workspace()
    }

    fn send_diagnostics(
        &self,
        uri: Uri,
        diagnostics: Vec<lsp_types::Diagnostic>,
        version: Option<i32>,
    ) -> Result<()> {
        if diagnostics.is_empty() {
            self.published.borrow_mut().remove(&uri);
        } else {
            self.published.borrow_mut().insert(uri.clone());
        }
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
