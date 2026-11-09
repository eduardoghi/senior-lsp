//! Language server para a linguagem de regras (LSP) do ERP Senior.

pub mod builtins;
pub mod cp1252;
mod delphi_float;
pub mod dictionary;
pub mod form;
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
