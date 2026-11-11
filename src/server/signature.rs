//! Ajuda de assinatura: a declaração da função cuja chamada está aberta no cursor, com o
//! parâmetro atual.
//!
//! A chamada é achada no texto e não na árvore, porque enquanto a pessoa digita os argumentos a
//! linha ainda não fecha e a árvore tem erro justamente ali.

use lsp_types::{
    ParameterInformation, ParameterLabel, Position, SignatureHelp, SignatureInformation,
};
use text_size::TextSize;

use super::navigation::Context;
use crate::builtins;
use crate::symbols::DefKind;

/// Chamada aberta antes do cursor, com o nome da função e quantas vírgulas já vieram.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct OpenCall<'a> {
    pub(crate) name: &'a str,
    pub(crate) commas: u32,
}

/// Lê o texto até `offset` e devolve a chamada mais interna ainda aberta. Textos e comentários
/// não contam, e `;`, `{` e `}` encerram o comando, então um parêntese esquecido numa linha de
/// cima não prende a ajuda.
pub(crate) fn open_call(text: &str, offset: TextSize) -> Option<OpenCall<'_>> {
    let end = usize::from(offset).min(text.len());
    let bytes = text.as_bytes();
    // Para cada parêntese aberto, o nome antes dele (vazio num parêntese de expressão) e as
    // vírgulas.
    let mut stack: Vec<(&str, u32)> = Vec::new();
    let mut i = 0;
    while i < end {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < end && bytes[i] != b'"' {
                    // A barra escapa as aspas e continua o texto na linha seguinte.
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'@' => {
                i += 1;
                while i < end && bytes[i] != b'@' && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < end && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i += 1;
            }
            b'(' => stack.push((name_before(text, i), 0)),
            b')' => {
                stack.pop();
            }
            b',' => {
                if let Some(top) = stack.last_mut() {
                    top.1 += 1;
                }
            }
            b';' | b'{' | b'}' => stack.clear(),
            _ => {}
        }
        i += 1;
    }
    let (name, commas) = stack.into_iter().rev().find(|(name, _)| !name.is_empty())?;
    Some(OpenCall { name, commas })
}

/// Nome logo antes do parêntese em `paren`, pulando espaços. Vazio quando não há.
fn name_before(text: &str, paren: usize) -> &str {
    let head = text[..paren].trim_end();
    let start = head
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .map_or(0, |p| p + 1);
    let name = &head[start..];
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        ""
    } else {
        name
    }
}

/// Parâmetros de uma declaração como `Funcao F(Alfa A, Numero End B);`, separados pelas vírgulas
/// de dentro dos parênteses.
fn params_of(decl: &str) -> Vec<&str> {
    let (Some(open), Some(close)) = (decl.find('('), decl.rfind(')')) else {
        return Vec::new();
    };
    if close <= open {
        return Vec::new();
    }
    decl[open + 1..close]
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect()
}

impl Context<'_> {
    pub(crate) fn signature_help(&self, position: Position) -> Option<SignatureHelp> {
        let call = open_call(&self.doc.text, self.offset(position))?;
        let decl = self.declaration(call.name)?;
        let params = params_of(&decl);
        let parameters = params
            .iter()
            .map(|p| ParameterInformation {
                label: ParameterLabel::Simple((*p).to_owned()),
                documentation: None,
            })
            .collect();
        let active = (!params.is_empty()).then(|| call.commas.min(params.len() as u32 - 1));
        Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label: decl,
                documentation: None,
                parameters: Some(parameters),
                active_parameter: active,
            }],
            active_signature: Some(0),
            active_parameter: active,
        })
    }

    /// Declaração da função, procurada no arquivo, nas regras de cima da tela ou nas outras do
    /// modelo, e nas funções do ERP, nessa ordem.
    fn declaration(&self, name: &str) -> Option<String> {
        let text = &self.doc.text;
        let local = self
            .doc
            .symbols
            .definitions
            .iter()
            .find(|d| d.kind == DefKind::Function && text[d.name].eq_ignore_ascii_case(name));
        if let Some(def) = local {
            let decl = self.header(def.node);
            return Some(decl.trim_end_matches(';').trim_end().to_owned());
        }
        if let Some(def) = self.doc.inherited.get(name) {
            return Some(def.decl.trim_end_matches(';').trim_end().to_owned());
        }
        builtins::lookup(name, self.doc.context)
            .map(|b| b.decl.trim_end_matches(';').trim_end().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(text: &str) -> Option<(String, u32)> {
        let offset = TextSize::from(text.find('|').unwrap() as u32);
        let text = text.replace('|', "");
        open_call(&text, offset).map(|c| (c.name.to_owned(), c.commas))
    }

    #[test]
    fn acha_a_chamada_aberta_e_conta_as_virgulas() {
        assert_eq!(call("IntParaAlfa(|"), Some(("IntParaAlfa".into(), 0)));
        assert_eq!(call("IntParaAlfa(n, |"), Some(("IntParaAlfa".into(), 1)));
        assert_eq!(call("F(G(1, 2), |"), Some(("F".into(), 1)));
        assert_eq!(call("F(G(1, |"), Some(("G".into(), 1)));
        assert_eq!(call("F((1 + 2), |"), Some(("F".into(), 1)));
    }

    #[test]
    fn ignora_texto_comentario_e_comando_fechado() {
        assert_eq!(call("F(\"a, (b\", |"), Some(("F".into(), 1)));
        assert_eq!(call("F(\"x \\\" , \", |"), Some(("F".into(), 1)));
        assert_eq!(call("F(a @ , ( @, |"), Some(("F".into(), 1)));
        assert_eq!(call("F(a /* , */, |"), Some(("F".into(), 1)));
        assert_eq!(call("F(a;\nx = |"), None);
        assert_eq!(call("F(a);|"), None);
        assert_eq!(call("x = (1 + |"), None);
    }

    #[test]
    fn separa_os_parametros_da_declaracao() {
        assert_eq!(
            params_of("Funcao IntParaAlfa(Numero Valor, Alfa End Retorno)"),
            ["Numero Valor", "Alfa End Retorno"]
        );
        assert!(params_of("Funcao Sem();").is_empty());
    }
}
