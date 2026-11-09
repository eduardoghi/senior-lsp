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

/// `senior-lsp tbs <arquivo>` imprime as tabelas e os campos do dicionário de dados do ERP, uma
/// linha por tabela e por campo. Com `--pasta <dir>`, grava o dicionário inteiro em arquivos de
/// texto nessa pasta, que precisa estar vazia ou não existir. Com dois arquivos, imprime as
/// diferenças do primeiro para o segundo.
fn tbs(args: &[std::ffi::OsString]) -> ExitCode {
    use senior_lsp::tbs_text;
    let read = |path: &std::ffi::OsString| {
        std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|b| senior_lsp::tbs::Dictionary::read(&b).map_err(|e| e.to_string()))
            .map_err(|e| eprintln!("senior-lsp: {}: {e}", path.to_string_lossy()))
    };
    let text = match args {
        [one] => read(one).map(|d| tbs_text::tables(&d)),
        [one, flag, dir] if flag == "--pasta" => {
            let Ok(dictionary) = read(one) else {
                return ExitCode::FAILURE;
            };
            return match write_folder(std::path::Path::new(dir), &tbs_text::export(&dictionary)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("senior-lsp: {}: {e}", dir.to_string_lossy());
                    ExitCode::FAILURE
                }
            };
        }
        [first, second] => read(first).and_then(|a| read(second).map(|b| tbs_text::diff(&a, &b))),
        _ => {
            eprintln!(
                "uso: senior-lsp tbs <arquivo.tbs> [outro.tbs]\n     senior-lsp tbs <arquivo.tbs> --pasta <dir>"
            );
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

/// Grava os arquivos em `dir`. Uma pasta com conteúdo é recusada, para não misturar o dicionário
/// novo com arquivos que não existem mais nele.
fn write_folder(
    dir: &std::path::Path,
    files: &std::collections::BTreeMap<String, String>,
) -> std::io::Result<()> {
    if std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some()) {
        return Err(std::io::Error::other("a pasta não está vazia"));
    }
    for (path, text) in files {
        let path = dir.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)?;
    }
    Ok(())
}
