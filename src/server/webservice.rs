//! Ir para a regra da porta de um web service interno a partir do `definir`.
//!
//! Em `Definir interno.com.empresa.vendas.CalcularComissao ws;` o primeiro trecho é o
//! provedor, o último é a porta e o meio é o serviço. A exportação do gerenciador de artefatos
//! guarda a regra da porta em `Web Services/<provedor>/<serviço>/<porta>/Rule.txt`, e a pasta
//! `Web Services` é procurada subindo a partir do arquivo aberto.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::{Location, Position, Range, Uri};
use text_size::TextSize;

use super::navigation::Context;
use crate::SyntaxKind;
use crate::form::Watch;

impl Context<'_> {
    /// Regra da porta do web service cujo nome qualificado está em `offset`.
    pub(crate) fn webservice_definition(&self, offset: TextSize) -> Option<Location> {
        let name = self.qualified_at(offset)?;
        let rule = child(&self.port_dir(name)?, "Rule.txt")?;
        let uri: Uri = crate::form::uri_from_path(&rule).parse().ok()?;
        Some(Location::new(
            uri,
            Range::new(Position::new(0, 0), Position::new(0, 0)),
        ))
    }

    /// Descrição e parâmetros da porta, no hover do nome qualificado.
    pub(crate) fn webservice_hover(&self, offset: TextSize) -> Option<lsp_types::Hover> {
        let name = self.qualified_at(offset)?;
        let port = self.port(name)?;
        let mut value = format!("```lsp\n{name}\n```\n");
        if !port.description.is_empty() {
            value.push_str(&port.description);
            value.push_str("\n\n");
        }
        for kind in ["entrada", "entrada e saída", "saída"] {
            let list: Vec<String> = port
                .params
                .iter()
                .filter(|p| p.kind == kind)
                .map(|p| {
                    let help = if p.help.is_empty() {
                        String::new()
                    } else {
                        format!(", {}", p.help)
                    };
                    format!("- `{}` ({}{help})", p.name, p.ty)
                })
                .collect();
            if !list.is_empty() {
                let title = match kind {
                    "entrada" => "Entrada",
                    "saída" => "Saída",
                    _ => "Entrada e saída",
                };
                value.push_str(&format!("{title}\n\n{}\n\n", list.join("\n")));
            }
        }
        Some(lsp_types::Hover {
            contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
                kind: lsp_types::MarkupKind::Markdown,
                value: value.trim_end().to_owned(),
            }),
            range: None,
        })
    }

    /// Nome qualificado de um `definir` de web service em `offset`.
    fn qualified_at(&self, offset: TextSize) -> Option<&str> {
        let tree = &self.doc.tree;
        let mut node = tree.root();
        let qualified = loop {
            let child = tree
                .children(node)
                .find(|&c| tree.text_range(c).contains_inclusive(offset))?;
            if tree.kind(child) == SyntaxKind::QualifiedName {
                break child;
            }
            node = child;
        };
        if tree.parent(qualified).map(|p| tree.kind(p)) != Some(SyntaxKind::WsDef) {
            return None;
        }
        Some(&self.doc.text[tree.text_range(qualified)])
    }
}

impl Context<'_> {
    /// Pasta da porta na exportação, pelo nome qualificado `provedor.serviço.porta`.
    pub(crate) fn port_dir(&self, qualified: &str) -> Option<PathBuf> {
        self.find_port_dir(qualified, &mut Watch::default())
    }

    /// A pasta de [`Context::port_dir`], registrando em `watch` cada pasta listada na procura.
    fn find_port_dir(&self, qualified: &str, watch: &mut Watch) -> Option<PathBuf> {
        let parts: Vec<&str> = qualified.split('.').map(str::trim).collect();
        let [provider, service @ .., port] = parts.as_slice() else {
            return None;
        };
        if service.is_empty() {
            return None;
        }
        let service = service.join(".");
        let file = crate::form::path_from_uri(self.uri.as_str())?;
        file.ancestors().skip(1).find_map(|dir| {
            watch.add(dir);
            let root = child(dir, "Web Services")?;
            watch.add(&root);
            let provider = child(&root, provider)?;
            watch.add(&provider);
            let service = child(&provider, &service)?;
            watch.add(&service);
            child(&service, port)
        })
    }

    /// Nome qualificado do web service definido como `variable` na regra.
    pub(crate) fn webservice_name(&self, variable: &str) -> Option<&str> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        let def = self.doc.symbols.definitions.iter().find(|d| {
            d.kind == crate::symbols::DefKind::WebService
                && text[d.name].eq_ignore_ascii_case(variable)
        })?;
        let qualified = tree
            .children(def.node)
            .find(|&c| tree.kind(c) == SyntaxKind::QualifiedName)?;
        Some(&text[tree.text_range(qualified)])
    }

    /// Parâmetros da porta do web service definido como `variable`.
    pub(crate) fn webservice_params(&self, variable: &str) -> Option<Arc<Port>> {
        self.port(self.webservice_name(variable)?)
    }

    /// Porta pelo nome qualificado, lida do `Port.json` uma vez e guardada. Achar a pasta da porta
    /// custa um `read_dir` por pasta acima do arquivo, e o diagnóstico de web service roda a cada
    /// tecla. A porta guardada vale enquanto o `Port.json` tiver a mesma data e o mesmo tamanho.
    /// Uma pasta `Web Services` criada mais perto do arquivo depois da primeira leitura só é vista
    /// quando o `Port.json` guardado mudar ou o servidor reiniciar.
    ///
    /// A porta que não está na exportação, como a de um web service nativo do ERP, também fica
    /// guardada, com a data de cada pasta listada na procura. Sem isso a procura inteira, até a
    /// raiz do disco, se repetia a cada tecla e a cada pedido de ação de código.
    fn port(&self, qualified: &str) -> Option<Arc<Port>> {
        let file = crate::form::path_from_uri(self.uri.as_str())?;
        let key = (file.parent()?.to_path_buf(), qualified.to_ascii_uppercase());
        let cached = PORTS.with_borrow(|ports| match ports.get(&key) {
            Some(Cached::Found { json, stamp, port }) if crate::form::stamp(json) == *stamp => {
                Some(Some(port.clone()))
            }
            Some(Cached::Missing(watch)) if watch.unchanged() => Some(None),
            _ => None,
        });
        if let Some(port) = cached {
            return port;
        }
        let mut watch = Watch::default();
        let found = self.read_port(qualified, &mut watch);
        let port = found.as_ref().map(|(_, _, port)| port.clone());
        let entry = match found {
            Some((json, stamp, port)) => Cached::Found { json, stamp, port },
            None => Cached::Missing(watch),
        };
        PORTS.with_borrow_mut(|ports| ports.insert(key, entry));
        port
    }

    /// Lê a porta da exportação, com o caminho e a data do `Port.json`. Registra em `watch` o que
    /// foi listado e lido, para a porta que falta ser procurada de novo só quando algo mudar.
    fn read_port(
        &self,
        qualified: &str,
        watch: &mut Watch,
    ) -> Option<(PathBuf, crate::form::Stamp, Arc<Port>)> {
        let dir = self.find_port_dir(qualified, watch)?;
        watch.add(&dir);
        let json = child(&dir, "Port.json")?;
        watch.add(&json);
        let stamp @ Some(_) = crate::form::stamp(&json) else {
            return None;
        };
        let port = Arc::new(parse_port(&json)?);
        Some((json, stamp, port))
    }
}

/// Resultado guardado da procura de uma porta.
enum Cached {
    Found {
        json: PathBuf,
        stamp: crate::form::Stamp,
        port: Arc<Port>,
    },
    /// A porta não está na exportação ou o `Port.json` dela não pôde ser lido.
    Missing(Watch),
}

thread_local! {
    /// Portas já procuradas, pela pasta do arquivo da regra e pelo nome qualificado em maiúsculas.
    static PORTS: std::cell::RefCell<rustc_hash::FxHashMap<(PathBuf, String), Cached>> =
        Default::default();
}

/// Parâmetros que o ERP acrescenta a toda porta, com o tipo e a descrição que ele registra
/// para cada uma. Não aparecem no `Port.json`.
pub(crate) const IMPLICIT: [(&str, crate::symbols::VarType, &str); 5] = [
    (
        "Usuario",
        crate::symbols::VarType::Alfa,
        "usuário solicitante do serviço",
    ),
    (
        "Senha",
        crate::symbols::VarType::Alfa,
        "senha do usuário solicitante do serviço",
    ),
    (
        "Token",
        crate::symbols::VarType::Alfa,
        "token do usuário solicitante do serviço",
    ),
    (
        "ModoExecucao",
        crate::symbols::VarType::Numero,
        "forma de execução do serviço",
    ),
    (
        "ErroExecucao",
        crate::symbols::VarType::Alfa,
        "erro ocorrido no servidor ao executar o serviço, vazio quando deu certo",
    ),
];

/// Porta lida do `Port.json` da exportação.
pub(crate) struct Port {
    pub(crate) description: String,
    pub(crate) params: Vec<PortParam>,
}

/// Parâmetro da porta. Um conjunto (`ptComplexSet`) tem colunas, que também são parâmetros.
pub(crate) struct PortParam {
    pub(crate) name: String,
    /// `entrada`, `saída` ou `entrada e saída`.
    pub(crate) kind: &'static str,
    /// O tipo como a regra vê, ou `conjunto`.
    pub(crate) ty: &'static str,
    pub(crate) help: String,
    pub(crate) columns: Vec<PortParam>,
}

impl PortParam {
    pub(crate) fn is_set(&self) -> bool {
        self.ty == "conjunto"
    }
}

/// Lê o `Port.json`, que o gerenciador grava em cp1252.
fn parse_port(json: &Path) -> Option<Port> {
    let bytes = std::fs::read(json).ok()?;
    let text = match std::str::from_utf8(&bytes) {
        Ok(t) => t.to_owned(),
        Err(_) => crate::cp1252::decode(&bytes)?,
    };
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let description = json["description"].as_str().unwrap_or_default().to_owned();
    Some(Port {
        description,
        params: params(&json["params"]),
    })
}

fn params(list: &serde_json::Value) -> Vec<PortParam> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some(PortParam {
                name: p["name"].as_str()?.to_owned(),
                kind: match p["kind"].as_str() {
                    Some("pkIn") => "entrada",
                    Some("pkOut") => "saída",
                    _ => "entrada e saída",
                },
                ty: match p["type"].as_str() {
                    Some("ptString" | "ptBlob") => "alfa",
                    Some("ptDateTime") => "data",
                    Some("ptComplexSet") => "conjunto",
                    _ => "numero",
                },
                help: p["help"].as_str().unwrap_or_default().to_owned(),
                columns: params(&p["columns"]),
            })
        })
        .collect()
}

/// Entrada da pasta com o nome, sem diferenciar maiúsculas, como o ERP e o Windows.
fn child(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}

impl Context<'_> {
    /// Membros de web service que não estão no `Port.json` da porta, como `ws.Campo` ou
    /// `ws.conjunto.Coluna`. Sem a porta na exportação, nada é conferido.
    pub(crate) fn webservice_diagnostics(&self) -> Vec<super::diagnostics::Found> {
        let tree = &self.doc.tree;
        let text = &self.doc.text;
        // Sem `definir` de web service, nenhum nome do arquivo é um, e a árvore nem precisa ser lida.
        if !self
            .doc
            .symbols
            .definitions
            .iter()
            .any(|d| d.kind == crate::symbols::DefKind::WebService)
        {
            return Vec::new();
        }
        let mut ports: std::collections::HashMap<String, Option<Arc<Port>>> =
            std::collections::HashMap::new();
        let mut diagnostics = Vec::new();
        let mut stack = vec![tree.root()];
        while let Some(node) = stack.pop() {
            stack.extend(tree.children(node));
            if tree.kind(node) != SyntaxKind::FieldExpr {
                continue;
            }
            let mut parts = tree.children(node);
            let (Some(object), Some(member)) = (parts.next(), parts.next()) else {
                continue;
            };
            if tree.kind(member) != SyntaxKind::NameRef {
                continue;
            }
            let member_name = &text[tree.text_range(member)];
            // `ws.Membro` ou `ws.conjunto.Coluna`, com ou sem índice no conjunto.
            let set_access = match tree.kind(object) {
                SyntaxKind::IndexExpr => tree.children(object).next(),
                SyntaxKind::FieldExpr => Some(object),
                _ => None,
            }
            .filter(|&o| tree.kind(o) == SyntaxKind::FieldExpr);
            let (ws, set) = match set_access {
                Some(inner) => {
                    let mut inner_parts = tree.children(inner);
                    let (Some(ws), Some(set)) = (inner_parts.next(), inner_parts.next()) else {
                        continue;
                    };
                    (ws, Some(&text[tree.text_range(set)]))
                }
                None => (object, None),
            };
            if tree.kind(ws) != SyntaxKind::NameRef {
                continue;
            }
            let ws_name = &text[tree.text_range(ws)];
            if self.webservice_name(ws_name).is_none() {
                continue;
            }
            let port = ports
                .entry(ws_name.to_ascii_uppercase())
                .or_insert_with(|| self.webservice_params(ws_name));
            let Some(port) = port else {
                continue;
            };
            let implicit: Vec<&str> = std::iter::once("Executar")
                .chain(IMPLICIT.iter().map(|(name, _, _)| *name))
                .collect();
            let (known, fixed, place): (&[PortParam], &[&str], String) = match set {
                None => (&port.params, &implicit, "da porta".to_owned()),
                Some(set) => {
                    let Some(param) = port
                        .params
                        .iter()
                        .find(|p| p.is_set() && p.name.eq_ignore_ascii_case(set))
                    else {
                        continue;
                    };
                    (
                        &param.columns,
                        &["QtdLinhas", "LinhaAtual", "CriarLinha", "ExcluirLinha"],
                        format!("do conjunto {}", param.name),
                    )
                }
            };
            let exists = known
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case(member_name))
                || fixed.iter().any(|f| f.eq_ignore_ascii_case(member_name));
            if !exists {
                diagnostics.push(super::diagnostics::Found {
                    range: tree.text_range(member),
                    code: "SEN069",
                    severity: lsp_types::DiagnosticSeverity::WARNING,
                    message: format!(
                        "{member_name} não é parâmetro {place} no Port.json da exportação."
                    ),
                });
            }
        }
        diagnostics.sort_by_key(|d| d.range.start());
        diagnostics
    }
}
