//! Executável do servidor. O editor conversa com ele pelo stdin e pelo stdout, então todo log vai
//! para o stderr.

use std::process::ExitCode;

use lsp_server::Connection;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "tbs") {
        return tbs(&args[1..]);
    }
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

/// `senior-lsp tbs <arquivo>` imprime o dicionário de dados do ERP em texto, uma linha por tabela
/// e por campo. Com dois arquivos, imprime as diferenças do primeiro para o segundo.
fn tbs(paths: &[std::ffi::OsString]) -> ExitCode {
    let read = |path: &std::ffi::OsString| {
        std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|b| senior_lsp::tbs::Dictionary::read(&b).map_err(|e| e.to_string()))
            .map_err(|e| eprintln!("senior-lsp: {}: {e}", path.to_string_lossy()))
    };
    let text = match paths {
        [one] => read(one).map(|d| d.to_text()),
        [first, second] => read(first).and_then(|a| read(second).map(|b| a.diff(&b))),
        _ => {
            eprintln!("uso: senior-lsp tbs <arquivo.tbs> [outro.tbs]");
            return ExitCode::FAILURE;
        }
    };
    let Ok(text) = text else {
        return ExitCode::FAILURE;
    };
    use std::io::Write;
    match std::io::stdout().lock().write_all(text.as_bytes()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
