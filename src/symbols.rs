//! Tabela de símbolos: as definições do arquivo e a definição de cada uso de nome.
//!
//! O compilador resolve os nomes pelo arquivo inteiro, sem diferença de caixa. Um `definir` vale
//! antes dele no texto e dentro de qualquer função, porque toda variável é global, mesmo a
//! definida dentro de uma função. A exceção é o parâmetro, que esconde a global de mesmo nome
//! dentro da sua função. Fora dela o nome do parâmetro ainda compila, mas na execução ele não
//! existe e o valor lido é lixo, por isso a referência fica marcada em [`Reference::foreign`].

use std::collections::HashMap;

use text_size::{TextRange, TextSize};

use crate::SyntaxKind::{
    AlfaKw, CallExpr, CursorDef, DataKw, FieldExpr, FuncDecl, FuncImpl, Goto, Ident, Label,
    ListDef, Name, NameRef, NumeroKw, ParamList, TableDef, VarDef, WsDef,
};
use crate::{Element, NodeId, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarType {
    Alfa,
    Numero,
    Data,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefKind {
    Variable(Option<VarType>),
    Parameter(Option<VarType>),
    Cursor,
    List,
    Table,
    WebService,
    Function,
    Label,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Definition {
    pub kind: DefKind,
    /// Trecho do nome.
    pub name: TextRange,
    /// Nó da definição inteira, como o `VarDef` ou a `FuncImpl`.
    pub node: NodeId,
    /// Nome da função do parâmetro, em maiúsculas pelo índice em [`Symbols::function_key`].
    function: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub range: TextRange,
    /// Definições que o nome alcança. Mais de uma quando o nome foi definido de novo ou quando
    /// a função tem `definir funcao` e `funcao`.
    pub targets: Vec<usize>,
    /// O nome só existe como parâmetro de outra função.
    pub foreign: bool,
}

#[derive(Debug, Default)]
pub struct Symbols {
    pub definitions: Vec<Definition>,
    pub references: Vec<Reference>,
    function_keys: Vec<String>,
}

/// Onde o nome é procurado.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Namespace {
    Value,
    Function,
    Label,
}

impl Symbols {
    pub fn build(tree: &Tree, text: &str) -> Symbols {
        let mut symbols = Symbols::default();
        let mut uses = Vec::new();
        let mut stack = vec![(tree.root(), None::<u32>)];
        while let Some((node, mut function)) = stack.pop() {
            symbols.visit(tree, node, function, text, &mut uses);
            // O `visit` já registrou a função, então o índice existe.
            if tree.kind(node) == FuncImpl {
                function = symbols.function_index(tree, text, node);
            }
            let first = stack.len();
            stack.extend(tree.children(node).map(|c| (c, function)));
            stack[first..].reverse();
        }
        symbols.resolve(tree, text, uses);
        symbols
    }

    /// Definições e usos de nome do nó, sem descer nos filhos.
    fn visit(
        &mut self,
        tree: &Tree,
        node: NodeId,
        function: Option<u32>,
        text: &str,
        uses: &mut Vec<(NodeId, Namespace, Option<u32>)>,
    ) {
        let kind = match tree.kind(node) {
            // Campo de `tabela`, que só se usa pela tabela.
            VarDef if tree.parent(node).is_some_and(|p| tree.kind(p) == TableDef) => return,
            VarDef => DefKind::Variable(var_type(tree, node)),
            CursorDef => DefKind::Cursor,
            ListDef => DefKind::List,
            TableDef => DefKind::Table,
            WsDef => DefKind::WebService,
            FuncDecl | FuncImpl => {
                self.add_function_params(tree, text, node);
                DefKind::Function
            }
            Label => {
                if let Some(t) = first_token(tree, node) {
                    self.push(DefKind::Label, tree.tokens().range(t), node, None);
                }
                return;
            }
            NameRef => {
                let is_ident = tree.tokens().kind(tree.token_range(node).start) == Ident;
                if let Some(namespace) = namespace(tree, node).filter(|_| is_ident) {
                    uses.push((node, namespace, function));
                }
                return;
            }
            _ => return,
        };
        if let Some(name) = tree.children(node).find(|&c| tree.kind(c) == Name) {
            self.push(kind, tree.text_range(name), node, None);
        }
    }

    fn add_function_params(&mut self, tree: &Tree, text: &str, function: NodeId) {
        let Some(name) = tree.children(function).find(|&c| tree.kind(c) == Name) else {
            return;
        };
        let key = upper(&text[tree.text_range(name)]);
        let index = match self.function_keys.iter().position(|k| *k == key) {
            Some(i) => i,
            None => {
                self.function_keys.push(key);
                self.function_keys.len() - 1
            }
        };
        let Some(list) = tree.children(function).find(|&c| tree.kind(c) == ParamList) else {
            return;
        };
        for param in tree.children(list) {
            if let Some(name) = tree.children(param).find(|&c| tree.kind(c) == Name) {
                let kind = DefKind::Parameter(var_type(tree, param));
                self.push(kind, tree.text_range(name), param, Some(index as u32));
            }
        }
    }

    fn push(&mut self, kind: DefKind, name: TextRange, node: NodeId, function: Option<u32>) {
        self.definitions.push(Definition {
            kind,
            name,
            node,
            function,
        });
    }

    fn resolve(&mut self, tree: &Tree, text: &str, uses: Vec<(NodeId, Namespace, Option<u32>)>) {
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, def) in self.definitions.iter().enumerate() {
            by_name.entry(upper(&text[def.name])).or_default().push(i);
        }
        for (node, namespace, function) in uses {
            let range = tree.text_range(node);
            let candidates = by_name
                .get(&upper(&text[range]))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let (targets, foreign) = self.pick(candidates, namespace, function);
            if !targets.is_empty() {
                self.references.push(Reference {
                    range,
                    targets,
                    foreign,
                });
            }
        }
    }

    fn pick(
        &self,
        candidates: &[usize],
        namespace: Namespace,
        current: Option<u32>,
    ) -> (Vec<usize>, bool) {
        let of_kind = |pred: &dyn Fn(&Definition) -> bool| -> Vec<usize> {
            candidates
                .iter()
                .copied()
                .filter(|&i| pred(&self.definitions[i]))
                .collect()
        };
        match namespace {
            Namespace::Function => (of_kind(&|d| d.kind == DefKind::Function), false),
            Namespace::Label => (of_kind(&|d| d.kind == DefKind::Label), false),
            Namespace::Value => {
                let own =
                    of_kind(&|d| matches!(d.kind, DefKind::Parameter(_)) && d.function == current);
                if current.is_some() && !own.is_empty() {
                    return (own, false);
                }
                let global = of_kind(&|d| {
                    !matches!(
                        d.kind,
                        DefKind::Parameter(_) | DefKind::Function | DefKind::Label
                    )
                });
                if !global.is_empty() {
                    return (global, false);
                }
                let params = of_kind(&|d| matches!(d.kind, DefKind::Parameter(_)));
                let foreign = !params.is_empty();
                (params, foreign)
            }
        }
    }

    fn function_index(&self, tree: &Tree, text: &str, function: NodeId) -> Option<u32> {
        let name = tree.children(function).find(|&c| tree.kind(c) == Name)?;
        let key = upper(&text[tree.text_range(name)]);
        self.function_keys
            .iter()
            .position(|k| *k == key)
            .map(|i| i as u32)
    }

    /// Definição cujo nome contém `offset`, ou referência que contém `offset`, com as definições
    /// que ela alcança. Uma definição alcança a si mesma.
    pub fn at(&self, offset: TextSize) -> Option<Vec<usize>> {
        if let Some(i) = self
            .definitions
            .iter()
            .position(|d| d.name.contains_inclusive(offset))
        {
            return Some(vec![i]);
        }
        self.references
            .iter()
            .find(|r| r.range.contains_inclusive(offset))
            .map(|r| r.targets.clone())
    }
}

/// Onde procurar o nome, ou `None` para o campo depois do ponto, que vem do SQL do cursor, da
/// tabela ou do web service e não tem `definir`.
fn namespace(tree: &Tree, node: NodeId) -> Option<Namespace> {
    let Some(parent) = tree.parent(node) else {
        return Some(Namespace::Value);
    };
    let first_child = tree.children(parent).next() == Some(node);
    Some(match tree.kind(parent) {
        Goto => Namespace::Label,
        FieldExpr if !first_child => return None,
        CallExpr if first_child => Namespace::Function,
        _ => Namespace::Value,
    })
}

fn var_type(tree: &Tree, node: NodeId) -> Option<VarType> {
    tree.elements(node).find_map(|e| match e {
        Element::Token(t) => match tree.tokens().kind(t) {
            AlfaKw => Some(VarType::Alfa),
            NumeroKw => Some(VarType::Numero),
            DataKw => Some(VarType::Data),
            _ => None,
        },
        Element::Node(_) => None,
    })
}

fn first_token(tree: &Tree, node: NodeId) -> Option<usize> {
    tree.token_range(node)
        .find(|&t| !tree.tokens().kind(t).is_trivia())
}

fn upper(name: &str) -> String {
    name.to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Para cada uso de nome, o texto do uso e as linhas (a partir de 1) das definições.
    fn resolve(text: &str) -> Vec<(&str, Vec<usize>, bool)> {
        let tree = crate::parse(text).unwrap();
        let symbols = Symbols::build(&tree, text);
        let line = |offset: TextSize| text[..usize::from(offset)].matches('\n').count() + 1;
        symbols
            .references
            .iter()
            .map(|r| {
                let lines = r
                    .targets
                    .iter()
                    .map(|&i| line(symbols.definitions[i].name.start()))
                    .collect();
                (&text[r.range], lines, r.foreign)
            })
            .collect()
    }

    #[test]
    fn variavel_global_sem_caixa_e_antes_do_definir() {
        let text = "Abc = 1;\ndefinir numero abc;\nx = ABC;";
        assert_eq!(
            resolve(text),
            [("Abc", vec![2], false), ("ABC", vec![2], false)]
        );
    }

    #[test]
    fn definir_dentro_de_funcao_e_global() {
        let text = "definir funcao f();\nr = loc;\nfuncao f(); {\n definir numero loc;\n}";
        assert_eq!(resolve(text), [("loc", vec![4], false)]);
    }

    #[test]
    fn parametro_esconde_a_global_dentro_da_funcao() {
        let text = "definir numero x;\n\
                    definir funcao h(numero x);\n\
                    x = 1;\n\
                    funcao h(numero x); { x = x + 1; }";
        let r = resolve(text);
        // O `x` do programa é a global. Dentro de `h`, o parâmetro da `funcao` e o do `definir
        // funcao`, que é o mesmo.
        assert_eq!(r[0], ("x", vec![1], false));
        assert_eq!(r[1], ("x", vec![2, 4], false));
        assert_eq!(r[2], ("x", vec![2, 4], false));
    }

    #[test]
    fn parametro_de_outra_funcao_fica_marcado() {
        let text = "definir funcao f(numero p);\ndefinir funcao g();\n\
                    funcao f(numero p); { }\nfuncao g(); { r = p; }";
        assert_eq!(resolve(text), [("p", vec![1, 3], true)]);
    }

    #[test]
    fn funcao_rotulo_e_campo() {
        let text = "definir funcao f();\n\
                    definir cursor c;\n\
                    f();\n\
                    c.AbrirCursor();\n\
                    vapara fim2;\n\
                    fim2:\n\
                    funcao f(); { }";
        assert_eq!(
            resolve(text),
            [
                ("f", vec![1, 7], false),
                ("c", vec![2], false),
                ("fim2", vec![6], false),
            ]
        );
    }

    #[test]
    fn nome_sem_definir_fica_de_fora() {
        assert!(resolve("IntParaAlfa(x, aNum);").is_empty());
    }

    #[test]
    fn tipo_da_variavel() {
        let text = "definir alfa a; definir numero n; definir data d; definir cursor c;";
        let tree = crate::parse(text).unwrap();
        let kinds: Vec<_> = Symbols::build(&tree, text)
            .definitions
            .iter()
            .map(|d| d.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                DefKind::Variable(Some(VarType::Alfa)),
                DefKind::Variable(Some(VarType::Numero)),
                DefKind::Variable(Some(VarType::Data)),
                DefKind::Cursor,
            ]
        );
    }
}
