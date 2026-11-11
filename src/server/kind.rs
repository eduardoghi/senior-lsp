//! Tipo da regra num arquivo fora da exportação.
//!
//! Dentro da exportação a pasta diz o tipo (`Context::from_uri`). Num arquivo solto, como um
//! texto novo que o editor marcou como `lsp-senior`, o tipo é o padrão das opções de
//! inicialização (`tipo_padrao`) até o editor escolher outro pelo comando `senior-lsp.tipoRegra`.
//! As ações de código oferecem esse comando em qualquer editor.

use crate::builtins::Context;

/// Comando do `workspace/executeCommand` que troca o tipo, com o URI e o nome do tipo.
pub(crate) const COMMAND: &str = "senior-lsp.tipoRegra";

/// Pedido que devolve o tipo de um documento aberto, para o editor mostrar.
pub(crate) const REQUEST: &str = "senior-lsp/tipoRegra";

/// Os tipos na ordem em que o editor mostra.
pub(crate) const ALL: [Context; 3] = [Context::Rule, Context::Sgi, Context::Report];

/// Nome do tipo nas opções, no comando e na resposta do pedido.
pub(crate) fn name(context: Context) -> &'static str {
    match context {
        Context::Rule => "comum",
        Context::Sgi => "tela",
        Context::Report => "relatorio",
    }
}

pub(crate) fn parse(name: &str) -> Option<Context> {
    ALL.into_iter().find(|&c| self::name(c) == name)
}

/// Título da ação de código que troca para o tipo.
pub(crate) fn action_title(context: Context) -> &'static str {
    match context {
        Context::Rule => "Tratar como regra comum",
        Context::Sgi => "Tratar como regra de tela SGI",
        Context::Report => "Tratar como regra de modelo de relatório",
    }
}

/// O tipo vem da pasta e não pode ser trocado. A pasta `Formulários SGI` dá tela e a pasta
/// `Modelos` dá relatório (os JSON e as regras separadas em `regras`). As outras valem como regra
/// comum.
pub(crate) fn fixed(uri: &str) -> bool {
    Context::from_uri(uri) != Context::Rule
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nome_e_volta() {
        for context in ALL {
            assert_eq!(parse(name(context)), Some(context));
        }
        assert_eq!(parse("web service"), None);
    }

    #[test]
    fn so_a_pasta_de_tela_fixa_o_tipo() {
        assert!(fixed("file:///x/Formul%C3%A1rios%20SGI/F1/rule.txt"));
        assert!(!fixed("file:///x/Regras/R1.LSP/Rule.txt"));
        assert!(!fixed("untitled:Untitled-1"));
    }
}
