//! Executável do servidor. O editor conversa com ele pelo stdin e pelo stdout, então todo log vai
//! para o stderr.

use std::process::ExitCode;

use lsp_server::Connection;

fn main() -> ExitCode {
    eprintln!("senior-lsp {} iniciado", env!("CARGO_PKG_VERSION"));
    let (connection, io_threads) = Connection::stdio();
    let result = senior_lsp::server::run(connection);
    let joined = io_threads.join();
    match result
        .map_err(|e| e.to_string())
        .and(joined.map_err(|e| e.to_string()))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("senior-lsp: {e}");
            ExitCode::FAILURE
        }
    }
}
