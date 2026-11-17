//! Documentos abertos no editor.
//!
//! O texto vem sempre do editor, pelo `didOpen` e pelo `didChange`, que o servidor aplica ao texto
//! guardado. Assim o que ainda não foi salvo também é analisado. A análise é refeita a cada mudança
//! e fica guardada para os pedidos do editor, como ir para a definição. As regras vizinhas, como as
//! de cima num formulário SGI e as outras de um modelo de relatório, são lidas do disco.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::sync::Arc;

use lsp_types::Uri;

use crate::builtins::Context;
use crate::form::Inherited;
use crate::line_index::LineIndex;
use crate::symbols::Symbols;
use crate::validate::Check;
use crate::{TextTooLarge, Tree};

use super::directives;

pub(crate) struct Document {
    pub(crate) text: String,
    pub(crate) version: i32,
    pub(crate) tree: Tree,
    pub(crate) symbols: Symbols,
    pub(crate) index: LineIndex,
    pub(crate) context: Context,
    /// Funções e variáveis de outras regras que valem para esta, como as de cima num formulário
    /// SGI ou as das outras regras de um modelo de relatório.
    pub(crate) inherited: Arc<Inherited>,
    /// Resultado de cada etapa que só depende do texto e que mais de um pedido do editor usa,
    /// calculado na primeira vez que alguém pede.
    checks: OnceCell<Vec<Check>>,
    scan: OnceCell<directives::Scan>,
    code_lines: OnceCell<Vec<usize>>,
    implicit: OnceCell<Vec<text_size::TextRange>>,
    /// Diagnósticos montados para as ações de código, com os níveis que valeram para eles.
    shown: std::cell::RefCell<Option<(super::config::Levels, Shown)>>,
}

impl Document {
    pub(crate) fn new(
        text: String,
        version: i32,
        tree: Tree,
        symbols: Symbols,
        index: LineIndex,
        context: Context,
        inherited: Arc<Inherited>,
    ) -> Document {
        Document {
            text,
            version,
            tree,
            symbols,
            index,
            context,
            inherited,
            checks: OnceCell::new(),
            scan: OnceCell::new(),
            code_lines: OnceCell::new(),
            implicit: OnceCell::new(),
            shown: Default::default(),
        }
    }

    /// Avisos do validador, calculados uma vez por texto. Os diagnósticos e as correções rápidas
    /// de um mesmo texto usam a mesma lista.
    pub(crate) fn checks(&self) -> &[Check] {
        self.checks.get_or_init(|| {
            crate::validate::validate(
                &self.tree,
                &self.symbols,
                &self.text,
                self.context,
                &self.inherited,
            )
        })
    }

    /// Diretivas dos comentários.
    pub(crate) fn scan(&self) -> &directives::Scan {
        self.scan
            .get_or_init(|| directives::scan(&self.text, &self.tree))
    }

    /// Primeiro uso de cada nome sem `definir`, para a dica de tipo e a correção de definir. A
    /// dica é pedida a cada rolagem da tela, e o texto não muda entre elas.
    pub(crate) fn implicit_names(&self) -> &[text_size::TextRange] {
        self.implicit
            .get_or_init(|| super::inlay::implicit_names(self))
    }

    /// Diagnósticos do texto com estes níveis, para as ações de código. O editor pede ações a cada
    /// movimento do cursor, e montar as mensagens de novo custava uns 5 µs por diagnóstico. Com
    /// outros níveis, a lista é montada de novo.
    pub(crate) fn shown(
        &self,
        levels: &super::config::Levels,
        make: impl FnOnce() -> Vec<lsp_types::Diagnostic>,
    ) -> Shown {
        if let Some((cached, shown)) = self.shown.borrow().as_ref()
            && cached == levels
        {
            return shown.clone();
        }
        let shown = Shown {
            diagnostics: Arc::new(make()),
            fix_all: Default::default(),
        };
        *self.shown.borrow_mut() = Some((levels.clone(), shown.clone()));
        shown
    }

    /// Descarta a lista das ações de código, quando os diagnósticos são publicados de novo por
    /// algo de fora do texto, como o `Port.json` do web service.
    pub(crate) fn forget_shown(&self) {
        self.shown.borrow_mut().take();
    }

    /// Linhas com código, para as diretivas de próxima linha.
    pub(crate) fn code_lines(&self) -> &[usize] {
        self.code_lines
            .get_or_init(|| directives::code_lines(&self.tree, &self.index))
    }
}

/// Diagnósticos que o editor mostra, para as ações de código, e o "Corrigir o arquivo", que só
/// depende deles e do texto, e não da posição do cursor.
#[derive(Clone)]
pub(crate) struct Shown {
    pub(crate) diagnostics: Arc<Vec<lsp_types::Diagnostic>>,
    pub(crate) fix_all: std::rc::Rc<OnceCell<Option<lsp_types::CodeAction>>>,
}

/// JSON de um modelo de relatório, com cada regra dele analisada como um documento.
pub(crate) struct ReportDocument {
    pub(crate) version: i32,
    pub(crate) json: Arc<str>,
    /// Montado no primeiro uso. Na análise da pasta, quase nenhum JSON tem diagnóstico, e o índice
    /// de um JSON grande custa uma varredura do arquivo inteiro.
    index: OnceCell<LineIndex>,
    pub(crate) rules: Vec<ReportRule>,
}

pub(crate) struct ReportRule {
    pub(crate) embedded: Arc<crate::report::Embedded>,
    pub(crate) doc: Document,
}

impl ReportDocument {
    /// Começo de cada linha do JSON, para converter posição em linha e coluna.
    pub(crate) fn index(&self) -> &LineIndex {
        self.index.get_or_init(|| LineIndex::new(&self.json))
    }

    /// Regra em que cai a posição do JSON, com a posição dentro dela.
    pub(crate) fn rule_at(&self, offset: text_size::TextSize) -> Option<&ReportRule> {
        self.rules
            .iter()
            .find(|r| r.embedded.rule.from_json(offset).is_some())
    }
}

#[derive(Default)]
pub(crate) struct Documents {
    open: HashMap<Uri, Document>,
    reports: HashMap<Uri, ReportDocument>,
}

impl Documents {
    /// Guarda uma regra em texto. O contexto vem do servidor, porque num arquivo fora da
    /// exportação ele é o que o editor escolheu.
    pub(crate) fn set(
        &mut self,
        uri: Uri,
        text: String,
        version: i32,
        context: Context,
    ) -> Result<(), TextTooLarge> {
        let inherited = match (context, crate::form::path_from_uri(uri.as_str())) {
            // As regras de cima valem como estão no disco, e o que foi lido delas fica guardado.
            (Context::Sgi, Some(path)) => crate::form::inherited_functions(&path),
            // Regra de relatório separada do JSON. As regras e as entradas do modelo valem nela,
            // a própria inclusive, como no JSON, para uma função definida em mais de uma regra
            // ficar em dúvida do mesmo jeito.
            (Context::Report, Some(path)) if crate::report::is_rule_file(&path) => {
                model_inherited(&crate::report::model(&path, &text))
            }
            (_, Some(path)) => Arc::new(Inherited::of_rule(&path)),
            _ => Arc::new(Inherited::with_dictionaries(crate::dictionary::current())),
        };
        self.set_with(uri, text, version, context, inherited)
    }

    /// Guarda uma regra em texto com as definições de fora já montadas, como as de um modelo de
    /// relatório analisado de uma vez.
    pub(crate) fn set_with(
        &mut self,
        uri: Uri,
        text: String,
        version: i32,
        context: Context,
        inherited: Arc<Inherited>,
    ) -> Result<(), TextTooLarge> {
        let tree = crate::parse(&text)?;
        let symbols = Symbols::build(&tree, &text);
        let index = LineIndex::new(&text);
        let doc = Document::new(text, version, tree, symbols, index, context, inherited);
        self.open.insert(uri, doc);
        Ok(())
    }

    /// Guarda o JSON de um modelo de relatório. As outras regras do modelo, nos outros JSON da
    /// pasta, valem como estão no disco.
    pub(crate) fn set_report(
        &mut self,
        uri: Uri,
        json: String,
        version: i32,
    ) -> Result<(), TextTooLarge> {
        let path = crate::form::path_from_uri(uri.as_str()).unwrap_or_default();
        let model = crate::report::model(&path, &json);
        let inherited = model_inherited(&model);
        self.set_report_in(uri, json, version, &model, inherited)
    }

    /// Guarda o JSON de um modelo já montado, para analisar vários JSON do mesmo modelo sem ler
    /// a pasta de novo para cada um.
    pub(crate) fn set_report_in(
        &mut self,
        uri: Uri,
        json: impl Into<Arc<str>>,
        version: i32,
        model: &crate::report::Model,
        inherited: Arc<Inherited>,
    ) -> Result<(), TextTooLarge> {
        let path = crate::form::path_from_uri(uri.as_str()).unwrap_or_default();
        let json: Arc<str> = json.into();
        let mut rules = Vec::new();
        for (rule_path, embedded) in &model.rules {
            if *rule_path != path {
                continue;
            }
            let embedded = embedded.clone();
            let text = embedded.rule.text.clone();
            let tree = crate::parse(&text)?;
            let symbols = Symbols::build(&tree, &text);
            let index = LineIndex::new(&text);
            let doc = Document::new(
                text,
                version,
                tree,
                symbols,
                index,
                Context::Report,
                inherited.clone(),
            );
            rules.push(ReportRule { embedded, doc });
        }
        self.reports.insert(
            uri,
            ReportDocument {
                version,
                json,
                index: OnceCell::new(),
                rules,
            },
        );
        Ok(())
    }

    pub(crate) fn get(&self, uri: &Uri) -> Option<&Document> {
        self.open.get(uri)
    }

    /// Tira a regra aberta para o texto dela ser editado sem cópia. Quem chama guarda de volta
    /// com [`Documents::set`].
    pub(crate) fn take(&mut self, uri: &Uri) -> Option<Document> {
        self.open.remove(uri)
    }

    /// Tira o JSON de modelo aberto, como [`Documents::take`] faz com a regra.
    pub(crate) fn take_report(&mut self, uri: &Uri) -> Option<ReportDocument> {
        self.reports.remove(uri)
    }

    /// Regras em texto abertas no editor. Os JSON de modelo ficam de fora.
    pub(crate) fn open_rules(&self) -> impl Iterator<Item = (&Uri, &Document)> {
        self.open.iter()
    }

    pub(crate) fn report(&self, uri: &Uri) -> Option<&ReportDocument> {
        self.reports.get(uri)
    }

    /// Endereço de cada documento aberto, regra ou JSON de modelo.
    pub(crate) fn uris(&self) -> Vec<Uri> {
        self.open
            .keys()
            .chain(self.reports.keys())
            .cloned()
            .collect()
    }

    /// Tira o documento, regra ou JSON de modelo, e diz se ele estava aberto.
    pub(crate) fn remove(&mut self, uri: &Uri) -> bool {
        self.open.remove(uri).is_some() | self.reports.remove(uri).is_some()
    }
}

/// Funções, variáveis e entradas de todas as regras do modelo, que valem em cada uma delas.
pub(crate) fn model_inherited(model: &crate::report::Model) -> Arc<Inherited> {
    let mut inherited = Inherited::from_model(model);
    inherited.dictionaries = crate::dictionary::current();
    inherited.senior_sql2 = model.senior_sql2;
    Arc::new(inherited)
}

/// Diz se o endereço é de um JSON de modelo de relatório, na pasta `Modelos`.
pub(crate) fn is_report(uri: &Uri) -> bool {
    Context::from_uri(uri.as_str()) == Context::Report && !is_report_rule(uri)
}

/// Diz se o endereço é de uma regra de relatório separada do JSON, em `Modelos/<modelo>/regras`.
pub(crate) fn is_report_rule(uri: &Uri) -> bool {
    crate::form::path_from_uri(uri.as_str()).is_some_and(|p| crate::report::is_rule_file(&p))
}

/// Diz se o endereço é de uma regra pelo nome, `Rule.txt` sem diferenciar maiúsculas. O Claude
/// Code associa o servidor pela extensão, e `.txt` pega todo arquivo de texto. Os outros arquivos
/// só são analisados quando o editor manda a linguagem `lsp-senior`.
pub(crate) fn is_rule(uri: &Uri) -> bool {
    let s = uri.as_str();
    let end = s.find(['?', '#']).unwrap_or(s.len());
    let path = &s[..end];
    let name = path.rsplit('/').next().unwrap_or(path);
    name.eq_ignore_ascii_case("rule.txt")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    #[test]
    fn so_rule_txt_e_analisado() {
        assert!(is_rule(&uri("file:///home/u/regras/EXEMPLO.LSP/Rule.txt")));
        assert!(is_rule(&uri("file:///c%3A/x/rule.TXT")));
        assert!(!is_rule(&uri("file:///home/u/README.txt")));
        assert!(!is_rule(&uri("file:///home/u/Rule.txt.bak")));
        assert!(!is_rule(&uri("file:///home/u/MyRule.txt")));
    }
}
