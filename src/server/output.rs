//! Saída do servidor para o editor.
//!
//! No processo de verdade, cada resposta e cada notificação sai do resultado direto para o texto
//! do JSON, que a thread principal escreve no stdout. O `lsp-server` só aceita a mensagem como
//! árvore (`serde_json::Value`), escreve numa thread e libera em outra. Uma árvore grande montada
//! numa thread e liberada em outra custa caro no alocador da glibc. A lista de símbolos de uma
//! regra com 650 definições levava 4 ms, quase tudo aí, e o cálculo dela leva 0,05 ms.
//!
//! Nos testes o servidor conversa por canais em memória, e a mensagem vai como árvore pelo canal,
//! que é o que o teste lê.

use std::io::Write;

use lsp_server::{Connection, ErrorCode, Message, Notification, RequestId, Response};

use super::Result;

/// Para onde vão as respostas e as notificações.
#[derive(Clone, Copy)]
pub(super) enum Output<'a> {
    /// Canal do `lsp-server`, com a mensagem em árvore.
    Channel(&'a Connection),
    /// Stdout do processo, com o JSON escrito direto do resultado.
    Stdout,
}

/// Resultado de um pedido, na forma em que a saída manda.
pub(super) enum Json {
    Tree(serde_json::Value),
    Text(Vec<u8>),
}

impl Output<'_> {
    /// Resultado de um pedido a partir do valor tipado.
    pub(super) fn json<T: serde::Serialize>(&self, result: &T) -> serde_json::Result<Json> {
        Ok(match self {
            Output::Channel(_) => Json::Tree(serde_json::to_value(result)?),
            Output::Stdout => Json::Text(serde_json::to_vec(result)?),
        })
    }

    /// Resultado `null`, como o de um pedido sobre um documento que não está aberto.
    pub(super) fn null(&self) -> Json {
        match self {
            Output::Channel(_) => Json::Tree(serde_json::Value::Null),
            Output::Stdout => Json::Text(b"null".to_vec()),
        }
    }

    pub(super) fn respond(&self, id: RequestId, result: Json) -> Result<()> {
        match self {
            Output::Channel(connection) => {
                let value = match result {
                    Json::Tree(value) => value,
                    // Não acontece, porque o resultado sai do `json` e do `null` da mesma saída.
                    Json::Text(text) => serde_json::from_slice(&text)?,
                };
                // O `Response::new_ok` passaria o resultado por `serde_json::to_value` de novo.
                let response = Response {
                    id,
                    response_result: Ok(value),
                };
                connection.sender.send(response.into())?;
            }
            Output::Stdout => {
                let result = match result {
                    Json::Text(text) => text,
                    Json::Tree(value) => serde_json::to_vec(&value)?,
                };
                let mut body = Vec::with_capacity(result.len() + 48);
                body.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":");
                serde_json::to_writer(&mut body, &id)?;
                body.extend_from_slice(b",\"result\":");
                body.extend_from_slice(&result);
                body.push(b'}');
                write_frame(&body)?;
            }
        }
        Ok(())
    }

    /// Resposta de erro a um pedido.
    pub(super) fn fail(&self, id: RequestId, code: ErrorCode, message: String) -> Result<()> {
        match self {
            Output::Channel(connection) => {
                let response = Response::new_err(id, code as i32, message);
                connection.sender.send(response.into())?;
            }
            Output::Stdout => {
                let body = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": code as i32, "message": message },
                });
                write_frame(&serde_json::to_vec(&body)?)?;
            }
        }
        Ok(())
    }

    pub(super) fn notify<P: serde::Serialize>(&self, method: &str, params: &P) -> Result<()> {
        match self {
            Output::Channel(connection) => {
                let notification = Notification::new(method.to_owned(), params);
                connection.sender.send(notification.into())?;
            }
            Output::Stdout => {
                let mut body = Vec::with_capacity(256);
                body.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"method\":");
                serde_json::to_writer(&mut body, method)?;
                body.extend_from_slice(b",\"params\":");
                serde_json::to_writer(&mut body, params)?;
                body.push(b'}');
                write_frame(&body)?;
            }
        }
        Ok(())
    }
}

/// Escreve uma mensagem inteira no stdout, com o cabeçalho do protocolo. A trava do stdout fica
/// presa até o fim da mensagem, para ela não se misturar com outra escrita.
fn write_frame(body: &[u8]) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(body)?;
    out.flush()
}

/// Threads que ligam o stdin e o stdout do processo aos canais do servidor.
pub(super) struct Threads {
    reader: std::thread::JoinHandle<std::io::Result<()>>,
    writer: std::thread::JoinHandle<std::io::Result<()>>,
}

impl Threads {
    /// Espera as duas threads terminarem. A de escrita termina quando o servidor larga a conexão,
    /// e a de leitura no `exit` ou no fim do stdin.
    pub(super) fn join(self) -> Result<()> {
        for thread in [self.writer, self.reader] {
            match thread.join() {
                Ok(result) => result?,
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
        Ok(())
    }
}

/// Conexão do servidor com o stdin e o stdout do processo.
///
/// O que chega pelo stdin vai para o canal de entrada, como no `lsp-server`. O canal de saída só
/// leva o que o próprio `lsp-server` manda, a resposta do `initialize` e a do `shutdown`, e as
/// outras mensagens o servidor escreve com [`Output::Stdout`]. As duas escritas não se cruzam,
/// porque o servidor só escreve depois que o editor recebeu a resposta do `initialize` e não
/// escreve mais nada depois de mandar a do `shutdown`.
pub(super) fn stdio() -> (Connection, Threads) {
    let (server, editor) = Connection::memory();
    let Connection { sender, receiver } = editor;
    let reader = std::thread::Builder::new()
        .name("senior-lsp-leitura".to_owned())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            while let Some(message) = Message::read(&mut stdin)? {
                let exit = matches!(&message, Message::Notification(n) if n.method == "exit");
                if sender.send(message).is_err() || exit {
                    break;
                }
            }
            Ok(())
        })
        .expect("thread de leitura");
    let writer = std::thread::Builder::new()
        .name("senior-lsp-escrita".to_owned())
        .spawn(move || {
            for message in receiver {
                message.write(&mut std::io::stdout().lock())?;
            }
            Ok(())
        })
        .expect("thread de escrita");
    (server, Threads { reader, writer })
}
