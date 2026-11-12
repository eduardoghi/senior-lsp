//! Dicionários de dados configurados, o de produção e o de teste.
//!
//! Cada `.tbs` é lido na primeira consulta e fica em memória. Depois, uma thread confere a data
//! do arquivo a cada [`RECHECK`] e lê de novo o que mudou. O arquivo costuma estar num
//! compartilhamento de rede, e a conferência fora da thread principal não trava a digitação quando
//! a rede está lenta ou o compartilhamento caiu. O `.tbs` só muda numa atualização do ERP ou numa
//! customização no CBDS, e para não esperar a conferência há o [`reload`]. Um arquivo que não pôde
//! ser lido fica registrado com a data, para não tentar de novo a cada consulta.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, SystemTime};

use crate::tbs::{Dictionary, Field, Table};

const RECHECK: Duration = Duration::from_secs(5 * 60);

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

    /// Tabela pelo dicionário de produção, ou pelo de teste quando produção não tem.
    pub fn table(&self, name: &str) -> Option<&Table> {
        [&self.production, &self.test]
            .into_iter()
            .flatten()
            .find_map(|d| d.table(name))
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
    /// O arquivo já foi lido uma vez, com sucesso ou não.
    read: bool,
    loaded: Option<Arc<Dictionary>>,
}

static SLOTS: Mutex<[Option<Slot>; 2]> = Mutex::new([None, None]);
static WATCHER: Once = Once::new();

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

/// Os dicionários configurados, lendo o arquivo na primeira vez. Depois disso a consulta não toca
/// no disco, e quem confere se o arquivo mudou é a thread do [`watch`].
pub fn current() -> Dictionaries {
    WATCHER.call_once(|| {
        // Sem a thread, o dicionário fica como foi lido até o servidor reiniciar ou o `reload`.
        let _ = std::thread::Builder::new()
            .name("dicionario".to_owned())
            .spawn(watch);
    });
    let mut slots = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    let [production, test] = &mut *slots;
    Dictionaries {
        production: production.as_mut().and_then(first_read),
        test: test.as_mut().and_then(first_read),
    }
}

fn first_read(slot: &mut Slot) -> Option<Arc<Dictionary>> {
    let path = slot.path.clone()?;
    if !slot.read {
        slot.read = true;
        slot.modified = modified(&path);
        slot.loaded = load(&path).map(Arc::new);
    }
    slot.loaded.clone()
}

/// Confere a cada [`RECHECK`] se algum `.tbs` mudou.
fn watch() {
    loop {
        std::thread::sleep(RECHECK);
        check(false);
    }
}

/// Lê de novo os dois `.tbs` agora, mesmo sem mudança na data. Trava quem chama enquanto lê, e o
/// resultado vale para os documentos analisados depois.
pub fn reload() {
    check(true);
}

/// Lê de novo o `.tbs` que mudou de data, ou todos com `force`. O disco é lido fora do lock, para a
/// consulta da thread principal não esperar a rede.
fn check(force: bool) {
    for i in 0..2 {
        let path = {
            let slots = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
            match &slots[i] {
                Some(Slot {
                    path: Some(path),
                    read: true,
                    ..
                }) => path.clone(),
                _ => continue,
            }
        };
        let modified = modified(&path);
        let same = {
            let slots = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
            slots[i]
                .as_ref()
                .is_some_and(|s| modified.is_some() && modified == s.modified)
        };
        if same && !force {
            continue;
        }
        let loaded = load(&path).map(Arc::new);
        let mut slots = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
        // O caminho pode ter mudado enquanto o arquivo era lido.
        if let Some(slot) = slots[i].as_mut()
            && slot.path.as_ref() == Some(&path)
        {
            slot.modified = modified;
            slot.loaded = loaded;
        }
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
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
