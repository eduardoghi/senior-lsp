//! Rótulos, funções e variáveis declaradas, e as chamadas de função.
//!
//! O ERP guarda os nomes de rótulo cortados em 30 caracteres e os de função em 31, sem diferença
//! de caixa, e rótulos e funções ficam em tabelas separadas. Cada caso foi conferido compilando ou
//! executando a regra no ERP.

use rustc_hash::{FxHashMap, FxHashSet};

use super::types::{self, Scope};
use super::{Check, CheckKind};
use crate::SyntaxKind::*;
use crate::builtins;
use crate::symbols::{DefKind, VarType, var_type};
use crate::{Element, NodeId, Tree};

const LABEL_LIMIT: usize = 30;
const FUNCTION_LIMIT: usize = 31;

/// Nome em maiúsculas, cortado como o ERP corta.
fn key(name: &str, limit: usize) -> std::string::String {
    name.chars()
        .take(limit)
        .flat_map(char::to_uppercase)
        .collect()
}

pub(super) struct Declarations<'a> {
    tree: &'a Tree,
    scope: &'a Scope<'a>,
    /// Rótulos do arquivo, já cortados.
    labels: FxHashSet<std::string::String>,
}

/// Tipo e `end` de cada parâmetro, que é o que diferencia duas declarações da mesma função.
fn signature(tree: &Tree, function: NodeId) -> Vec<(Option<VarType>, bool)> {
    let Some(list) = tree.children(function).find(|&c| tree.kind(c) == ParamList) else {
        return Vec::new();
    };
    tree.children(list)
        .map(|param| {
            let by_ref = tree
                .elements(param)
                .any(|e| matches!(e, Element::Token(t) if tree.tokens().kind(t) == EndKw));
            (var_type(tree, param), by_ref)
        })
        .collect()
}

fn type_name(ty: VarType) -> &'static str {
    match ty {
        VarType::Alfa => "alfa",
        VarType::Numero => "número",
        VarType::Data => "data",
    }
}

impl<'a> Declarations<'a> {
    pub(super) fn build(scope: &'a Scope<'a>, tree: &'a Tree, checks: &mut Vec<Check>) -> Self {
        let mut declarations = Declarations {
            tree,
            scope,
            labels: FxHashSet::default(),
        };
        declarations.labels(checks);
        declarations.functions(checks);
        declarations.variables(checks);
        declarations
    }

    fn text(&self, range: text_size::TextRange) -> &'a str {
        &self.scope.text[range]
    }

    fn labels(&mut self, checks: &mut Vec<Check>) {
        let symbols = self.scope.symbols;
        let mut full: FxHashSet<std::string::String> = FxHashSet::default();
        // Rótulo cortado pelo primeiro nome inteiro que o teve.
        let mut first_by_key: FxHashMap<std::string::String, std::string::String> =
            FxHashMap::default();
        for def in symbols
            .definitions
            .iter()
            .filter(|d| d.kind == DefKind::Label)
        {
            let name = self.text(def.name);
            let upper = name.to_uppercase();
            let cut = key(name, LABEL_LIMIT);
            let mut push = |kind, label: std::string::String| {
                checks.push(Check {
                    kind,
                    range: def.name,
                    name: None,
                    label: Some(label),
                });
            };
            if full.contains(&upper) {
                push(CheckKind::RepeatedLabel, name.to_owned());
            } else if let Some(other) = first_by_key.get(&cut).filter(|o| **o != upper) {
                push(
                    CheckKind::LabelsSameStart,
                    format!("{name}|{}", self.original_name(other)),
                );
            }
            full.insert(upper.clone());
            first_by_key.entry(cut.clone()).or_insert(upper);
            self.labels.insert(cut);
        }
    }

    /// O nome como foi escrito no arquivo, a partir dele em maiúsculas.
    fn original_name(&self, upper: &str) -> &'a str {
        self.scope
            .symbols
            .definitions
            .iter()
            .filter(|d| d.kind == DefKind::Label)
            .map(|d| self.text(d.name))
            .find(|n| n.to_uppercase() == upper)
            .unwrap_or_default()
    }

    fn functions(&self, checks: &mut Vec<Check>) {
        let (tree, symbols) = (self.tree, self.scope.symbols);
        let mut by_name: FxHashMap<std::string::String, Vec<usize>> = FxHashMap::default();
        for (i, def) in symbols
            .definitions
            .iter()
            .enumerate()
            .filter(|(_, d)| d.kind == DefKind::Function)
        {
            by_name
                .entry(key(self.text(def.name), FUNCTION_LIMIT))
                .or_default()
                .push(i);
            self.repeated_parameters(def.node, checks);
        }
        for defs in by_name.values() {
            let decls: Vec<_> = defs
                .iter()
                .map(|&i| &symbols.definitions[i])
                .filter(|d| tree.kind(d.node) == FuncDecl)
                .collect();
            for def in defs
                .iter()
                .map(|&i| &symbols.definitions[i])
                .filter(|d| tree.kind(d.node) == FuncImpl)
            {
                let name = self.text(def.name);
                let declared = decls.iter().any(|d| d.name.start() < def.name.start());
                if !declared && !self.scope.inherited.contains(name) {
                    checks.push(Check {
                        kind: CheckKind::UndeclaredFunction,
                        range: def.name,
                        name: None,
                        label: Some(name.to_owned()),
                    });
                }
            }
            if let Some((first, repeated)) = decls.split_first() {
                let wanted = signature(tree, first.node);
                for def in repeated
                    .iter()
                    .filter(|d| signature(tree, d.node) != wanted)
                {
                    checks.push(Check {
                        kind: CheckKind::RepeatedDeclaration,
                        range: def.name,
                        name: None,
                        label: Some(self.text(def.name).to_owned()),
                    });
                }
            }
        }
    }

    fn repeated_parameters(&self, function: NodeId, checks: &mut Vec<Check>) {
        let tree = self.tree;
        let Some(list) = tree.children(function).find(|&c| tree.kind(c) == ParamList) else {
            return;
        };
        let mut seen = FxHashSet::default();
        for name in tree
            .children(list)
            .filter_map(|p| tree.children(p).find(|&c| tree.kind(c) == Name))
        {
            let text = self.text(tree.text_range(name));
            if !seen.insert(text.to_uppercase()) {
                checks.push(Check {
                    kind: CheckKind::RepeatedParameter,
                    range: tree.text_range(name),
                    name: None,
                    label: Some(text.to_owned()),
                });
            }
        }
    }

    /// `Definir` do mesmo nome com outro tipo, contra a primeira definição. O mesmo tipo de novo
    /// compila.
    fn variables(&self, checks: &mut Vec<Check>) {
        let mut first: FxHashMap<std::string::String, VarType> = FxHashMap::default();
        for def in &self.scope.symbols.definitions {
            let DefKind::Variable(Some(ty)) = def.kind else {
                continue;
            };
            let name = self.text(def.name);
            match first.get(&name.to_uppercase()) {
                None => {
                    first.insert(name.to_uppercase(), ty);
                }
                Some(&before) if before != ty => checks.push(Check {
                    kind: CheckKind::ConflictingDefinition,
                    range: def.name,
                    name: None,
                    label: Some(format!("{name}|{}", type_name(before))),
                }),
                Some(_) => {}
            }
        }
    }

    /// `VaPara` para um nome que não é rótulo do arquivo.
    pub(super) fn goto(&self, node: NodeId, checks: &mut Vec<Check>) {
        let tree = self.tree;
        let Some(target) = tree
            .children(node)
            .next()
            .filter(|&t| tree.kind(t) == NameRef)
        else {
            return;
        };
        let name = self.text(tree.text_range(target));
        if !self.labels.contains(&key(name, LABEL_LIMIT)) {
            checks.push(Check {
                kind: CheckKind::UnknownLabel,
                range: tree.text_range(target),
                name: None,
                label: Some(name.to_owned()),
            });
        }
    }

    pub(super) fn call(&self, node: NodeId, checks: &mut Vec<Check>) {
        let (tree, scope) = (self.tree, self.scope);
        let mut parts = tree.children(node);
        let (Some(callee), Some(args)) = (parts.next(), parts.next()) else {
            return;
        };
        if tree.kind(callee) != NameRef
            || tree.kind(args) != ArgList
            || tree.tokens().kind(tree.token_range(callee).start) != Ident
        {
            return;
        }
        let arguments: Vec<NodeId> = tree.children(args).collect();
        for &arg in &arguments {
            if types::is_alfa_concatenation(tree, scope, arg) {
                checks.push(Check {
                    kind: CheckKind::AlfaConcatenationInArgument,
                    range: tree.text_range(arg),
                    name: None,
                    label: None,
                });
            }
        }
        let range = tree.text_range(callee);
        let name = self.text(range);
        let Some(reference) = scope.symbols.reference_at(range) else {
            // Função de outra regra da tela, que não é do ERP. Num modelo de relatório com a função
            // em mais de uma regra, não se sabe qual assinatura vale.
            if let Some(function) = scope.inherited.get(name)
                && builtins::lookup(name, scope.context).is_none()
                && !(scope.context == crate::builtins::Context::Report && function.rules > 1)
                && !self.long_name(name, range, checks)
            {
                let params = function
                    .params
                    .iter()
                    .map(|p| (p.ty, p.by_ref, p.name.as_str()));
                self.literal_in_alfa_parameter(params, &arguments, checks);
            }
            return;
        };
        let defs: Vec<_> = reference
            .targets
            .iter()
            .map(|&t| &scope.symbols.definitions[t])
            .collect();
        if defs.is_empty()
            || !defs.iter().all(|d| d.kind == DefKind::Function)
            || self.long_name(name, range, checks)
        {
            return;
        }
        let implemented = defs.iter().any(|d| tree.kind(d.node) == FuncImpl);
        if !implemented
            && !scope.inherited.contains(name)
            && builtins::lookup(name, scope.context).is_none()
        {
            checks.push(Check {
                kind: CheckKind::CallWithoutImplementation,
                range,
                name: None,
                label: Some(name.to_owned()),
            });
        }
        let def = defs
            .iter()
            .find(|d| tree.kind(d.node) == FuncDecl)
            .or_else(|| defs.first());
        let Some(list) =
            def.and_then(|d| tree.children(d.node).find(|&c| tree.kind(c) == ParamList))
        else {
            return;
        };
        let params: Vec<_> = tree
            .children(list)
            .map(|param| {
                let by_ref = tree
                    .elements(param)
                    .any(|e| matches!(e, Element::Token(t) if tree.tokens().kind(t) == EndKw));
                let name = tree
                    .children(param)
                    .find(|&c| tree.kind(c) == Name)
                    .map_or("", |n| self.text(tree.text_range(n)));
                (
                    var_type(tree, param).unwrap_or(VarType::Numero),
                    by_ref,
                    name,
                )
            })
            .collect();
        self.literal_in_alfa_parameter(params.into_iter(), &arguments, checks);
    }

    /// Nome com 31 ou mais caracteres, que o ERP corta em 31. Devolve se a chamada foi acusada.
    fn long_name(&self, name: &str, range: text_size::TextRange, checks: &mut Vec<Check>) -> bool {
        let length = name.chars().count();
        let (kind, label) = match length {
            0..FUNCTION_LIMIT => return false,
            FUNCTION_LIMIT => (CheckKind::CallToLongFunctionName, name.to_owned()),
            _ => (CheckKind::FunctionNameTooLong, format!("{name}|{length}")),
        };
        checks.push(Check {
            kind,
            range,
            name: None,
            label: Some(label),
        });
        true
    }

    /// Texto literal num parâmetro alfa de função da regra, com o tipo, o `end` e o nome de cada
    /// parâmetro. O `end` com constante já é recusado como outro erro (SEN053).
    fn literal_in_alfa_parameter<'p>(
        &self,
        params: impl ExactSizeIterator<Item = (VarType, bool, &'p str)>,
        arguments: &[NodeId],
        checks: &mut Vec<Check>,
    ) {
        let tree = self.tree;
        if params.len() != arguments.len() {
            return;
        }
        for (&arg, (ty, by_ref, name)) in arguments.iter().zip(params) {
            let is_text = tree.kind(arg) == Literal
                && tree.tokens().kind(tree.token_range(arg).start) == String;
            if is_text && !by_ref && ty == VarType::Alfa {
                checks.push(Check {
                    kind: CheckKind::LiteralInAlfaParameter,
                    range: tree.text_range(arg),
                    name: None,
                    label: Some(name.to_owned()),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::validate::CheckKind::{self, *};

    /// Sem as dicas de definição sem uso, porque os exemplos definem mais do que usam.
    fn found(text: &str) -> Vec<(CheckKind, &str)> {
        crate::validate::tests::checks_all(text)
            .into_iter()
            .filter(|(kind, _)| !matches!(kind, Unused | UnusedFunction | UnusedParameter))
            .collect()
    }

    /// Com as funções de outra regra da tela, que a regra do exemplo enxerga.
    fn com_herdadas<'a>(text: &'a str, outra: &str) -> Vec<(CheckKind, &'a str)> {
        let tree = crate::parse(text).unwrap();
        let inherited = crate::form::Inherited::from_rules([(
            std::path::PathBuf::from("raiz/rule.txt"),
            outra.to_owned(),
        )]);
        crate::validate::validate(
            &tree,
            &crate::symbols::Symbols::build(&tree, text),
            text,
            crate::builtins::Context::Sgi,
            &inherited,
        )
        .into_iter()
        .filter(|c| !matches!(c.kind, Unused | UnusedFunction | UnusedParameter))
        .map(|c| (c.kind, &text[c.range]))
        .collect()
    }

    #[test]
    fn vapara_para_rotulo_que_nao_existe() {
        assert_eq!(
            found("x = 1;\nse (x = 1) { VaPara Saida; }\nx = 2;"),
            [(UnknownLabel, "Saida")]
        );
        // Rótulo e função ficam em tabelas separadas.
        let funcao = "Definir Funcao f();\nse (x = 1) { VaPara f; }\nFuncao f();\nInicio\nFim;";
        assert_eq!(found(funcao), [(UnknownLabel, "f")]);
        // O rótulo vale sem diferença de caixa.
        assert!(found("se (x = 1) { VaPara SAIDA; }\nx = 1;\nsaida:\nx = 2;").is_empty());
        // O nome é cortado em 30 caracteres, então o 31º não conta.
        let longo = "rotuloComTrintaCaracteresAbcde";
        assert_eq!(longo.len(), 30);
        let texto = format!("se (x = 1) {{ VaPara {longo}Z; }}\nx = 1;\n{longo}Y:\nx = 2;");
        assert!(found(&texto).is_empty());
        let outro = format!(
            "se (x = 1) {{ VaPara {longo}Z; }}\nx = 1;\nrotuloComOutroInicioAbcdefghijk:\nx = 2;"
        );
        assert_eq!(found(&outro).len(), 1);
    }

    #[test]
    fn rotulo_repetido_e_com_o_mesmo_comeco() {
        let texto = "x = 1;\nr:\nx = 2;\nR:\nx = 3;\nse (x = 1) { VaPara r; }";
        assert_eq!(found(texto), [(RepeatedLabel, "R")]);
        let base = "rotuloComTrintaCaracteresAbcde";
        let texto =
            format!("se (x = 1) {{ VaPara {base}A; }}\nx = 1;\n{base}A:\nx = 2;\n{base}B:\nx = 3;");
        assert_eq!(
            found(&texto),
            [(LabelsSameStart, format!("{base}B").as_str())]
        );
        assert!(found("x = 1;\nr1:\nx = 2;\nr2:\nx = 3;").is_empty());
    }

    #[test]
    fn funcao_sem_declaracao_antes() {
        assert_eq!(
            found("Funcao f();\nInicio\nFim;"),
            [(UndeclaredFunction, "f")]
        );
        // A declaração depois da implementação não vale.
        assert_eq!(
            found("Funcao f();\nInicio\nFim;\nDefinir Funcao f();"),
            [(UndeclaredFunction, "f")]
        );
        assert!(found("Definir Funcao f();\nf();\nFuncao F();\nInicio\nFim;").is_empty());
        // Repetida igual depois da implementação compila.
        let texto = "Definir Funcao f();\nFuncao f();\nInicio\nFim;\nDefinir Funcao f();";
        assert!(found(texto).is_empty());
        // Declarada na regra da raiz da tela.
        let impl_ = "Funcao fRaiz();\nInicio\nFim;";
        assert!(com_herdadas(impl_, "Definir Funcao fRaiz();").is_empty());
        assert_eq!(com_herdadas(impl_, ""), [(UndeclaredFunction, "fRaiz")]);
    }

    #[test]
    fn nome_de_funcao_com_31_ou_mais_caracteres() {
        let regra = |nome: &str| {
            format!("Definir Funcao {nome}();\n{nome}();\nFuncao {nome}();\nInicio\nFim;")
        };
        let com = |n: usize| format!("f{}", "a".repeat(n - 1));
        assert!(found(&regra(&com(30))).is_empty());
        let nome31 = com(31);
        assert_eq!(
            found(&regra(&nome31)),
            [(CallToLongFunctionName, nome31.as_str())]
        );
        let nome32 = com(32);
        assert_eq!(
            found(&regra(&nome32)),
            [(FunctionNameTooLong, nome32.as_str())]
        );
        // Declarada e nunca chamada não dá nada.
        let sem_chamada = format!("Definir Funcao {nome32}();\nFuncao {nome32}();\nInicio\nFim;");
        assert!(found(&sem_chamada).is_empty());
    }

    const FUNCOES: &str = "Funcao f(Numero a, Alfa p);\nInicio\np = p;\nFim;\n\
                           Funcao g(Alfa End p);\nInicio\nn = 1;\nFim;\n\
                           Funcao h(Alfa p);\nInicio\nn = 1;\nFim;\n";
    const DEFINIR: &str = "Definir Alfa aTxt; Definir Numero n; Definir Numero nTam;\n\
                           Definir Funcao f(Numero a, Alfa p);\nDefinir Funcao g(Alfa End p);\n\
                           Definir Funcao h(Alfa p);\n";

    /// Os comandos primeiro, e as funções no fim, que o corpo de uma funcao vai até o fim do
    /// arquivo.
    fn com_funcoes(corpo: &str) -> std::string::String {
        format!("{DEFINIR}{corpo}\n{FUNCOES}")
    }

    #[test]
    fn concatenacao_de_alfa_em_argumento() {
        assert_eq!(
            found(&com_funcoes("nTam = 0;\nTamanhoStr(aTxt + \"b\", nTam);")),
            [(AlfaConcatenationInArgument, "aTxt + \"b\"")]
        );
        assert_eq!(
            found(&com_funcoes("h(\"x\" + aTxt + \"y\");")),
            [(AlfaConcatenationInArgument, "\"x\" + aTxt + \"y\"")]
        );
        // Conta numérica no argumento numérico, texto sozinho e variável compilam.
        let texto =
            com_funcoes("nTam = 0;\nTamanhoStr(aTxt, nTam); n = Abs(n + 1);\nf(n + 1, aTxt);");
        assert!(found(&texto).is_empty());
    }

    #[test]
    fn definir_com_outro_tipo() {
        assert_eq!(
            found("Definir Numero n;\nDefinir Alfa N;\nn = 1;"),
            [(ConflictingDefinition, "N")]
        );
        // O mesmo tipo de novo é inofensivo.
        assert!(found("Definir Numero n;\nDefinir Numero n;\nn = 1;").is_empty());
        // Cada definição de tipo diferente do primeiro é apontada.
        assert_eq!(
            found("Definir Alfa a;\nDefinir Numero a;\nDefinir Data a;\na = \"x\";").len(),
            2
        );
    }

    #[test]
    fn chamada_a_funcao_sem_implementacao() {
        let declarada = "Definir Funcao f();\nf();";
        assert_eq!(found(declarada), [(CallWithoutImplementation, "f")]);
        // Declarada e nunca chamada, ou implementada, não dá nada.
        assert!(found("Definir Funcao f();").is_empty());
        assert!(found("Definir Funcao f();\nf();\nFuncao f();\nInicio\nFim;").is_empty());
        // Implementada em outra regra da tela.
        assert!(com_herdadas(declarada, "Funcao f();\nInicio\nFim;").is_empty());
        assert_eq!(
            com_herdadas(declarada, ""),
            [(CallWithoutImplementation, "f")]
        );
    }

    #[test]
    fn texto_literal_em_parametro_alfa() {
        assert_eq!(
            found(&com_funcoes("f(1, \"a\");")),
            [(LiteralInAlfaParameter, "\"a\"")]
        );
        assert!(found(&com_funcoes("f(1, aTxt); g(aTxt); f(n, aTxt); h(aTxt);")).is_empty());
        // Na função do ERP o texto literal é normal.
        assert!(found(&com_funcoes("nTam = 0;\nTamanhoStr(\"abc\", nTam);")).is_empty());
        // No parâmetro `end` a constante é outro erro.
        assert_eq!(
            found(&com_funcoes("g(\"a\");")),
            [(ConstantInEndParameter, "\"a\"")]
        );
    }

    #[test]
    fn funcao_de_outra_regra_da_tela() {
        let raiz = "Funcao fRaiz(Alfa p);\nInicio\nn = 1;\nFim;";
        assert_eq!(
            com_herdadas("fRaiz(\"a\");", raiz),
            [(LiteralInAlfaParameter, "\"a\"")]
        );
        assert!(com_herdadas("Definir Alfa aTxt;\nfRaiz(aTxt);", raiz).is_empty());
        let longa = format!("f{}", "a".repeat(31));
        let raiz = format!("Funcao {longa}();\nInicio\nn = 1;\nFim;");
        let texto = format!("{longa}();");
        assert_eq!(
            com_herdadas(&texto, &raiz),
            [(FunctionNameTooLong, longa.as_str())]
        );
    }

    #[test]
    fn declaracao_repetida_com_outra_assinatura() {
        let defs = "Definir Funcao fRep(Numero pA);\nDefinir Funcao fRep(Numero pA, Numero pB);\n";
        let impl_ = "Funcao fRep(Numero pA);\nInicio\npA = 1;\nFim;";
        assert_eq!(
            found(&format!("{defs}fRep(1);\n{impl_}")),
            [(RepeatedDeclaration, "fRep")]
        );
        // A chamada que só casa com a segunda é conferida contra a primeira.
        let texto = format!("{defs}fRep(1, 2);\n{impl_}");
        let found = found(&texto);
        assert_eq!(found[0], (RepeatedDeclaration, "fRep"));
        assert_eq!(found[1], (TooManyArguments, "fRep"));
        // Igual à primeira não dá nada, e o tipo diferente conta como outra assinatura.
        let igual = "Definir Funcao f(Numero p);\nDefinir Funcao f(Numero q);\nf(1);\n\
                     Funcao f(Numero p);\nInicio\np = 1;\nFim;";
        assert!(super::tests::found(igual).is_empty());
        let tipo = "Definir Funcao f(Numero p);\nDefinir Funcao f(Alfa p);\nf(1);\n\
                    Funcao f(Numero p);\nInicio\np = 1;\nFim;";
        assert_eq!(super::tests::found(tipo), [(RepeatedDeclaration, "f")]);
    }

    #[test]
    fn parametro_repetido() {
        let texto = "Definir Funcao f(Numero pA, Numero pa);\nf(7, 8);\n\
                     Funcao f(Numero pA, Numero PA);\nInicio\npA = 1;\nFim;";
        let found: Vec<_> = found(texto)
            .into_iter()
            .filter(|(kind, _)| *kind == RepeatedParameter)
            .collect();
        assert_eq!(
            found,
            [(RepeatedParameter, "pa"), (RepeatedParameter, "PA")]
        );
        let sem = "Definir Funcao f(Numero pA, Numero pB);\nf(7, 8);\n\
                   Funcao f(Numero pA, Numero pB);\nInicio\npA = pB;\nFim;";
        assert!(super::tests::found(sem).is_empty());
    }
}
