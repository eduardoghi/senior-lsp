//! Language server para a linguagem de regras (LSP) do ERP Senior.

mod delphi_float;
mod lexer;
mod syntax_kind;

pub use lexer::{LexError, LexErrorKind, TextTooLarge, Tokens, lex};
pub use syntax_kind::SyntaxKind;
