//! Language server para a linguagem de regras (LSP) do ERP Senior.

pub mod cp1252;
mod delphi_float;
mod lexer;
pub mod line_index;
mod parser;
mod scan;
pub mod server;
mod syntax_kind;
mod tree;
pub mod validate;

pub use lexer::{LexError, LexErrorKind, TextTooLarge, Tokens, lex};
pub use parser::parse;
pub use syntax_kind::SyntaxKind;
pub use tree::{Builder, Checkpoint, Element, NodeId, ParseError, ParseErrorKind, Tree};
