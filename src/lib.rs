//! Language server para a linguagem de regras (LSP) do ERP Senior.

pub mod builtins;
pub mod cp1252;
mod delphi_float;
pub mod dictionary;
pub mod form;
pub mod format;
mod lexer;
pub mod line_index;
mod parser;
pub mod report;
mod scan;
pub mod server;
pub mod symbols;
mod syntax_kind;
pub mod tbs;
pub mod tbs_text;
mod tree;
pub mod validate;

pub use lexer::{LexError, LexErrorKind, TextTooLarge, Tokens, lex};
pub use parser::parse;
pub use syntax_kind::SyntaxKind;
pub use tree::{Builder, Checkpoint, Element, NodeId, ParseError, ParseErrorKind, Tree};

/// Chama `f` com o nome em maiúsculas, como `to_uppercase`. Nome ASCII curto, que é quase todo
/// nome de regra, vai num buffer da pilha, e as buscas por nome a cada tecla deixam de alocar.
pub fn with_upper<R>(name: &str, f: impl FnOnce(&str) -> R) -> R {
    let mut buffer = [0u8; 64];
    match buffer.get_mut(..name.len()) {
        Some(bytes) if name.is_ascii() => {
            bytes.copy_from_slice(name.as_bytes());
            bytes.make_ascii_uppercase();
            // ASCII continua ASCII depois da troca de caixa.
            f(std::str::from_utf8(bytes).unwrap_or_default())
        }
        _ => f(&name.to_uppercase()),
    }
}
