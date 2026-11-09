//! Dicionários de dados configurados, o de produção e o de teste.
//!
//! Cada `.tbs` é lido na primeira consulta e fica em memória. A data do arquivo é conferida no
//! máximo a cada [`RECHECK`], porque o arquivo costuma estar num compartilhamento de rede, e um
//! arquivo com data nova é lido de novo. Um arquivo que não pôde ser lido fica registrado com a
//! data, para não tentar de novo a cada consulta.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::tbs::{Dictionary, Field, Table};

const RECHECK: Duration = Duration::from_secs(5);

/// Os dois dicionários, quando configurados e legíveis.
#[derive(Debug, Clone, Default)]
pub struct Dictionaries {
    pub production: Option<Arc<Dictionary>>,
    pub test: Option<Arc<Dictionary>>,
}

/// Campo achado num dicionário.
pub struct Found<'a> {
    pub dictionary: &'a Dictionary,
    pub table: &'a Table,
    pub field: &'a Field,
    /// O campo existe no dicionário de teste e não no de produção, como uma customização ainda
    /// não promovida.
    pub only_in_test: bool,
}

impl Dictionaries {
    pub fn is_empty(&self) -> bool {
        self.production.is_none() && self.test.is_none()
    }

    /// Campo pelo dicionário de produção, ou pelo de teste quando produção não tem.
    pub fn field<'a>(&'a self, table: &str, field: &str) -> Option<Found<'a>> {
        let find = |d: &'a Arc<Dictionary>| {
            let t = d.table(table)?;
            let f = t
                .fields
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(field))?;
            Some((d.as_ref(), t, f))
        };
        if let Some((dictionary, table, field)) = self.production.as_ref().and_then(find) {
            return Some(Found {
                dictionary,
                table,
                field,
                only_in_test: false,
            });
        }
        let (dictionary, table, field) = self.test.as_ref().and_then(find)?;
        Some(Found {
            dictionary,
            table,
            field,
            only_in_test: self.production.is_some(),
        })
    }

    /// Diz se a tabela existe em algum dos dicionários.
    pub fn has_table(&self, table: &str) -> bool {
        [&self.production, &self.test]
            .into_iter()
            .flatten()
            .any(|d| d.table(table).is_some())
    }
}

#[derive(Default)]
struct Slot {
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    checked: Option<Instant>,
    loaded: Option<Arc<Dictionary>>,
}

static SLOTS: Mutex<[Option<Slot>; 2]> = Mutex::new([None, None]);

/// Define os caminhos do `.tbs` de produção e do de teste. Caminho vazio desliga o dicionário.
pub fn configure(production: Option<PathBuf>, test: Option<PathBuf>) {
    let mut slots = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    for (slot, path) in slots.iter_mut().zip([production, test]) {
        *slot = path.filter(|p| !p.as_os_str().is_empty()).map(|path| Slot {
            path: Some(path),
            ..Slot::default()
        });
    }
}

/// Os dicionários configurados, lendo o arquivo quando for a primeira vez ou quando ele mudou.
pub fn current() -> Dictionaries {
    let mut slots = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    let [production, test] = &mut *slots;
    Dictionaries {
        production: production.as_mut().and_then(refresh),
        test: test.as_mut().and_then(refresh),
    }
}

fn refresh(slot: &mut Slot) -> Option<Arc<Dictionary>> {
    let path = slot.path.clone()?;
    if slot.checked.is_some_and(|t| t.elapsed() < RECHECK) {
        return slot.loaded.clone();
    }
    slot.checked = Some(Instant::now());
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    if modified.is_some() && modified == slot.modified {
        return slot.loaded.clone();
    }
    slot.modified = modified;
    slot.loaded = load(&path).map(Arc::new);
    slot.loaded.clone()
}

fn load(path: &Path) -> Option<Dictionary> {
    let read = std::fs::read(path)
        .map_err(|e| e.to_string())
        .and_then(|b| Dictionary::read(&b).map_err(|e| e.to_string()));
    match read {
        Ok(d) => {
            eprintln!(
                "senior-lsp: dicionário {} com {} tabelas",
                path.display(),
                d.tables.len()
            );
            Some(d)
        }
        Err(e) => {
            eprintln!("senior-lsp: dicionário {}: {e}", path.display());
            None
        }
    }
}
