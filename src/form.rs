//! Regras do mesmo formulário SGI.
//!
//! Numa tela SGI, a regra da raiz do formulário define funções que as regras das ações e das
//! tabelas chamam. A exportação guarda o formulário como pastas, cada regra num `rule.txt`, então
//! as regras acima de um arquivo são as das pastas acima dele, até a raiz do formulário, que fica
//! dois níveis abaixo da pasta `Formulários SGI` (`Formulários SGI/interface/<formulário>`).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::symbols::{DefKind, Symbols};

/// Nomes de função, em maiúsculas, definidos nas regras das pastas acima de `rule` dentro do
/// mesmo formulário. Vazio fora de um formulário SGI ou quando um arquivo não pode ser lido.
pub fn inherited_functions(rule: &Path) -> HashSet<String> {
    let mut names = HashSet::new();
    let Some(root) = form_root(rule) else {
        return names;
    };
    let mut dir = rule.parent();
    while let Some(d) = dir.and_then(Path::parent) {
        if !d.starts_with(&root) {
            break;
        }
        collect(&d.join(rule_file(d)), &mut names);
        dir = Some(d);
    }
    names
}

/// Caminho local de uma URI `file://`. `None` para outro esquema.
pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    Some(PathBuf::from(crate::builtins::percent_decode(rest)))
}

/// Pasta do formulário, dois níveis abaixo de `Formulários SGI`.
fn form_root(rule: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = rule.components().collect();
    let i = parts.iter().position(|c| {
        c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("Formulários SGI")
    })?;
    // Precisa ter pelo menos o tipo, o formulário e uma pasta dentro dele antes do arquivo.
    if parts.len() < i + 5 {
        return None;
    }
    Some(parts[..i + 3].iter().collect())
}

/// Nome do arquivo de regra na pasta, sem diferença de caixa. `rule.txt` se não houver.
fn rule_file(dir: &Path) -> String {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.eq_ignore_ascii_case("rule.txt"))
        .unwrap_or_else(|| "rule.txt".to_owned())
}

fn collect(path: &Path, names: &mut HashSet<String>) {
    let Ok(bytes) = fs::read(path) else {
        return;
    };
    let Some(text) = crate::cp1252::decode(&bytes) else {
        return;
    };
    let Ok(tree) = crate::parse(&text) else {
        return;
    };
    for def in Symbols::build(&tree, &text).definitions {
        if def.kind == DefKind::Function {
            names.insert(text[def.name].to_uppercase());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn junta_as_funcoes_das_pastas_acima() {
        let base = std::env::temp_dir().join(format!("senior-lsp-form-{}", std::process::id()));
        let form = base.join("Formulários SGI/interface/F1");
        let action = form.join("actions/A1");
        fs::create_dir_all(&action).unwrap();
        fs::write(
            form.join("rule.txt"),
            "definir funcao Global();\nfuncao Global(); { }",
        )
        .unwrap();
        fs::write(form.join("actions/rule.txt"), "definir funcao DoMeio();").unwrap();
        fs::write(action.join("rule.txt"), "definir funcao Propria();").unwrap();
        let names = inherited_functions(&action.join("rule.txt"));
        assert!(names.contains("GLOBAL") && names.contains("DOMEIO"));
        // A própria regra não entra, porque o editor manda o texto dela.
        assert!(!names.contains("PROPRIA"));
        // Fora de formulário SGI não há regras herdadas.
        assert!(inherited_functions(&base.join("regras/R1/Rule.txt")).is_empty());
        fs::remove_dir_all(&base).unwrap();
    }
}
