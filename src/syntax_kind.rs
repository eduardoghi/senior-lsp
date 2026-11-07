//! Tipos dos tokens produzidos pelo lexer.

/// Tipo de um token. Os nós da árvore sintática vão entrar neste mesmo enum.
///
/// `repr(u8)` mantém o tipo em 1 byte, o que limita o enum a 256 variantes.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SyntaxKind {
    // Trivia. O compilador descarta, o editor precisa.
    Whitespace,
    Newline,
    AtComment,
    BlockComment,

    Ident,
    Number,
    String,
    Char,

    // Palavras reservadas, na ordem da tabela do compilador.
    InicioKw,
    FimKw,
    DefinirKw,
    ParaKw,
    EnquantoKw,
    VaParaKw,
    RegraKw,
    SeKw,
    SenaoKw,
    CancelKw,
    CancelarKw,
    AlfaKw,
    NumeroKw,
    TabelaKw,
    FuncaoKw,
    ContinueKw,
    PareKw,
    EndKw,
    EKw,
    OuKw,
    AbrirKw,
    FecharKw,
    LerKw,
    LerNlKw,
    GravarNlKw,
    FormatarKw,
    InsirirKw,
    ListaKw,
    MensagemKw,
    HabilitarCampoKw,
    DesabilitarCampoKw,
    EsconderCampoKw,
    HabilitarTodosKw,
    DesabilitarTodosKw,
    VaParaCampoKw,
    VaParaPaginaKw,
    HabilitarBotaoKw,
    DesabilitarBotaoKw,
    ChamarFuncaoKw,
    AtualizarCamposKw,
    IniciarTransacaoKw,
    FinalizarTransacaoKw,
    DesfazerTransacaoKw,
    ValRetKw,
    ValStrKw,
    AtualizarKw,
    InserirKw,
    ExcluirKw,
    CursorKw,
    SelectKw,
    UnicoKw,
    IntKw,
    DataKw,
    DecimalKw,
    ExecSqlKw,
    UsarKw,
    GravarKw,
    ExecSqlExKw,
    WebServiceKw,
    GravarNlEolKw,

    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Semicolon,
    Colon,
    Dot,
    Plus,
    Minus,
    Star,
    Slash,
    Lt,
    Gt,
    Eq,

    PlusPlus,
    MinusMinus,
    LtEq,
    GtEq,
    Neq,
    /// Outra sequência de `<`, `>` e `=`, como `==` ou `=<`. O compilador lê qualquer sequência
    /// desses caracteres como um token só.
    RelRun,

    /// Texto que não forma nenhum token, como `é` fora de string.
    ErrorToken,

    /// Fim do arquivo. Só o parser usa, o lexer nunca emite.
    Eof,
}

use SyntaxKind::*;

/// Palavras reservadas em maiúsculas, como na tabela do compilador.
///
/// Nomes como `ACHOU` e `RETORNA` só têm sentido em certos lugares (`cursor.Achou`,
/// `Mensagem(Retorna, ...)`) e não são reservadas.
pub const KEYWORDS: &[(&str, SyntaxKind)] = &[
    ("INICIO", InicioKw),
    ("FIM", FimKw),
    ("DEFINIR", DefinirKw),
    ("PARA", ParaKw),
    ("ENQUANTO", EnquantoKw),
    ("VAPARA", VaParaKw),
    ("REGRA", RegraKw),
    ("SE", SeKw),
    ("SENAO", SenaoKw),
    ("CANCEL", CancelKw),
    ("CANCELAR", CancelarKw),
    ("ALFA", AlfaKw),
    ("NUMERO", NumeroKw),
    ("TABELA", TabelaKw),
    ("FUNCAO", FuncaoKw),
    ("CONTINUE", ContinueKw),
    ("PARE", PareKw),
    ("END", EndKw),
    ("E", EKw),
    ("OU", OuKw),
    ("ABRIR", AbrirKw),
    ("FECHAR", FecharKw),
    ("LER", LerKw),
    ("LERNL", LerNlKw),
    ("GRAVARNL", GravarNlKw),
    ("FORMATAR", FormatarKw),
    ("INSIRIR", InsirirKw),
    ("LISTA", ListaKw),
    ("MENSAGEM", MensagemKw),
    ("HABILITARCAMPO", HabilitarCampoKw),
    ("DESABILITARCAMPO", DesabilitarCampoKw),
    ("ESCONDERCAMPO", EsconderCampoKw),
    ("HABILITARTODOS", HabilitarTodosKw),
    ("DESABILITARTODOS", DesabilitarTodosKw),
    ("VAPARACAMPO", VaParaCampoKw),
    ("VAPARAPAGINA", VaParaPaginaKw),
    ("HABILITARBOTAO", HabilitarBotaoKw),
    ("DESABILITARBOTAO", DesabilitarBotaoKw),
    ("CHAMARFUNCAO", ChamarFuncaoKw),
    ("ATUALIZARCAMPOS", AtualizarCamposKw),
    ("INICIARTRANSACAO", IniciarTransacaoKw),
    ("FINALIZARTRANSACAO", FinalizarTransacaoKw),
    ("DESFAZERTRANSACAO", DesfazerTransacaoKw),
    ("VALRET", ValRetKw),
    ("VALSTR", ValStrKw),
    ("ATUALIZAR", AtualizarKw),
    ("INSERIR", InserirKw),
    ("EXCLUIR", ExcluirKw),
    ("CURSOR", CursorKw),
    ("SELECT", SelectKw),
    ("UNICO", UnicoKw),
    ("INT", IntKw),
    ("DATA", DataKw),
    ("DECIMAL", DecimalKw),
    ("EXECSQL", ExecSqlKw),
    ("USAR", UsarKw),
    ("GRAVAR", GravarKw),
    ("EXECSQLEX", ExecSqlExKw),
    ("WEBSERVICE", WebServiceKw),
    ("GRAVARNLEOL", GravarNlEolKw),
];

impl SyntaxKind {
    /// Devolve o tipo da palavra reservada, sem diferenciar maiúsculas, ou `None` se o nome não for
    /// reservado. Recebe o identificador inteiro, então `E075PRO` não casa com `E`.
    pub fn keyword(ident: &str) -> Option<SyntaxKind> {
        KEYWORDS
            .iter()
            .find(|(text, _)| text.eq_ignore_ascii_case(ident))
            .map(|&(_, kind)| kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cabe_em_um_byte() {
        assert_eq!(size_of::<SyntaxKind>(), 1);
    }

    #[test]
    fn tabela_tem_as_60_palavras_do_compilador_sem_repetir() {
        assert_eq!(KEYWORDS.len(), 60);
        for (i, (a, ka)) in KEYWORDS.iter().enumerate() {
            assert_eq!(*a, a.to_ascii_uppercase());
            for (b, kb) in &KEYWORDS[i + 1..] {
                assert_ne!(a, b);
                assert_ne!(ka, kb);
            }
        }
    }

    #[test]
    fn tabela_segue_a_ordem_do_enum() {
        for (i, (_, kind)) in KEYWORDS.iter().enumerate() {
            assert_eq!(*kind as u8, InicioKw as u8 + i as u8);
        }
    }

    #[test]
    fn reconhece_sem_diferenciar_maiusculas() {
        assert_eq!(SyntaxKind::keyword("se"), Some(SeKw));
        assert_eq!(SyntaxKind::keyword("Senao"), Some(SenaoKw));
        assert_eq!(SyntaxKind::keyword("e"), Some(EKw));
        assert_eq!(SyntaxKind::keyword("GravarNlEol"), Some(GravarNlEolKw));
    }

    #[test]
    fn nomes_que_nao_sao_reservados() {
        for nome in [
            "E075PRO", "Endereco", "Achou", "Retorna", "NomeTela", "Inserir_", "",
        ] {
            assert_eq!(SyntaxKind::keyword(nome), None, "{nome}");
        }
    }
}
