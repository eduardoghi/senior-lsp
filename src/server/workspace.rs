//! Diagnósticos das regras da pasta inteira, sem abrir cada arquivo.
//!
//! Ligado pela opção `analisar_pasta`. Ao iniciar, o servidor procura nas pastas do workspace os
//! `Rule.txt` e os JSON de `Modelos`, analisa cada um como está no disco e publica os
//! diagnósticos. Fica desligado por padrão porque no Claude Code os diagnósticos entram na
//! conversa, e milhares de arquivos encheriam o contexto.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use lsp_types::{Location, Position, Range, Uri};
use text_size::TextRange;

use super::documents::{Documents, is_report, is_report_rule, is_rule};
use super::{Result, Server};

/// Indicador de progresso da análise da pasta no editor, pelo `window/workDoneProgress/create` e
/// pelo `$/progress`. A porcentagem só é mandada quando muda, então são no máximo cem avisos.
struct Progress<'a> {
    out: super::output::Output<'a>,
    token: lsp_types::NumberOrString,
    total: usize,
    done: usize,
    shown: usize,
}

impl<'a> Progress<'a> {
    /// Começa o indicador, quando o editor aceita (`enabled`) e há arquivo para ler.
    fn begin(
        out: super::output::Output<'a>,
        enabled: bool,
        total: usize,
    ) -> Result<Option<Progress<'a>>> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static RUNS: AtomicU32 = AtomicU32::new(0);
        if !enabled || total == 0 {
            return Ok(None);
        }
        let run = RUNS.fetch_add(1, Ordering::Relaxed);
        let name = format!("senior-lsp/analisar-pasta/{run}");
        let token = lsp_types::NumberOrString::String(name.clone());
        let id = lsp_server::RequestId::from(name);
        out.request(
            id,
            "window/workDoneProgress/create",
            &lsp_types::WorkDoneProgressCreateParams {
                token: token.clone(),
            },
        )?;
        let progress = Progress {
            out,
            token,
            total,
            done: 0,
            shown: 0,
        };
        progress.send(lsp_types::WorkDoneProgress::Begin(
            lsp_types::WorkDoneProgressBegin {
                title: "Analisando as regras da pasta".to_owned(),
                cancellable: Some(false),
                message: Some(format!("0 de {total}")),
                percentage: Some(0),
            },
        ))?;
        Ok(Some(progress))
    }

    fn step(&mut self, files: usize) -> Result<()> {
        self.done = (self.done + files).min(self.total);
        let percentage = self.done * 100 / self.total;
        if percentage == self.shown {
            return Ok(());
        }
        self.shown = percentage;
        self.send(lsp_types::WorkDoneProgress::Report(
            lsp_types::WorkDoneProgressReport {
                cancellable: Some(false),
                message: Some(format!("{} de {}", self.done, self.total)),
                percentage: Some(percentage as u32),
            },
        ))
    }

    fn end(self) -> Result<()> {
        self.send(lsp_types::WorkDoneProgress::End(
            lsp_types::WorkDoneProgressEnd {
                message: Some(format!("{} arquivos analisados", self.total)),
            },
        ))
    }

    fn send(&self, value: lsp_types::WorkDoneProgress) -> Result<()> {
        let params = lsp_types::ProgressParams {
            token: self.token.clone(),
            value: lsp_types::ProgressParamsValue::WorkDone(value),
        };
        self.out.notify("$/progress", &params)
    }
}

/// Pastas que nunca têm regra e só custam tempo.
const SKIPPED: [&str; 4] = [".git", "node_modules", "target", ".vscode"];

/// O que uma thread analisa de uma vez. As regras de um mesmo formulário SGI vão juntas, porque
/// cada thread monta o formulário no próprio cache, e as de um modelo também, porque o modelo é
/// montado uma vez para todos os arquivos dele.
enum Work {
    Rules(Vec<RuleFile>),
    Model(Vec<ModelFile>),
}

impl Work {
    /// Quantos arquivos da lista contam no progresso.
    fn files(&self) -> usize {
        match self {
            Work::Rules(rules) => rules.len(),
            Work::Model(files) => files.len(),
        }
    }
}

/// Regra do disco com o que vem do estado do servidor, que as threads não acessam.
struct RuleFile {
    path: PathBuf,
    /// `None` quando o caminho não vira URI. O arquivo só conta no progresso.
    uri: Option<Uri>,
    context: crate::builtins::Context,
    levels: super::config::Levels,
    /// Aberto no editor, quando vale o texto de lá.
    open: bool,
}

/// Arquivo de um modelo de relatório, JSON ou regra separada.
struct ModelFile {
    path: PathBuf,
    uri: Option<Uri>,
    levels: super::config::Levels,
    open: bool,
}

/// O resultado de um arquivo analisado, para a thread principal guardar e publicar.
struct Analyzed {
    uri: Uri,
    /// Definições para o índice de símbolos, com o caminho do arquivo.
    symbols: Option<(PathBuf, super::workspace_symbols::File)>,
    diagnostics: Option<Vec<lsp_types::Diagnostic>>,
}

/// Analisa uma regra do disco. `None` quando ela está aberta ou não pôde ser lida.
fn analyze_rule_file(
    rule: &RuleFile,
    encoding: crate::line_index::ColumnEncoding,
) -> Result<Option<Analyzed>> {
    let Some(uri) = rule.uri.as_ref().filter(|_| !rule.open) else {
        return Ok(None);
    };
    let Some(text) = read(&rule.path, true) else {
        return Ok(None);
    };
    let mut documents = Documents::default();
    documents.set(uri.clone(), text, 0, rule.context)?;
    Ok(Some(analyzed(
        &documents,
        uri,
        &rule.path,
        &rule.levels,
        encoding,
    )))
}

/// Os diagnósticos do documento e, quando é uma regra em texto, as definições dela.
fn analyzed(
    documents: &Documents,
    uri: &Uri,
    path: &Path,
    levels: &super::config::Levels,
    encoding: crate::line_index::ColumnEncoding,
) -> Analyzed {
    let symbols = documents.get(uri).map(|doc| {
        let file = super::workspace_symbols::File::new(
            path,
            &doc.text,
            &doc.symbols,
            &doc.index,
            encoding,
        );
        (path.to_path_buf(), file)
    });
    Analyzed {
        uri: uri.clone(),
        symbols,
        diagnostics: super::document_diagnostics(documents, uri, levels, encoding)
            .map(|(diagnostics, _)| diagnostics),
    }
}

/// Os JSON e as regras separadas de um modelo, com o modelo montado uma vez.
fn analyze_model(
    files: &[ModelFile],
    encoding: crate::line_index::ColumnEncoding,
    results: &mut Vec<Analyzed>,
) -> Result<()> {
    let Some(first) = files.first() else {
        return Ok(());
    };
    let Some(first_json) = read(&first.path, false) else {
        return Ok(());
    };
    let model = crate::report::model_uncached(&first.path, &first_json);
    let inherited = super::documents::model_inherited(&model);
    for file in files {
        let Some(uri) = file.uri.as_ref().filter(|_| !file.open) else {
            continue;
        };
        let path = &file.path;
        let mut documents = Documents::default();
        if is_report_rule(uri) {
            let Some(text) = read(path, true) else {
                continue;
            };
            let context = crate::builtins::Context::Report;
            documents.set_with(uri.clone(), text, 0, context, inherited.clone())?;
        } else {
            // O modelo já leu e decodificou todos os JSON da pasta.
            let known = model.texts.iter().find(|(p, _)| p == path);
            let Some(json) = known
                .map(|(_, text)| text.clone())
                .or_else(|| read(path, false).map(Into::into))
            else {
                continue;
            };
            documents.set_report_in(uri.clone(), json, 0, &model, inherited.clone())?;
        }
        results.push(analyzed(&documents, uri, path, &file.levels, encoding));
    }
    Ok(())
}

/// Analisa os arquivos do trabalho e junta os resultados em `results`. Num erro, ficam os dos
/// arquivos anteriores, que a versão serial já tinha publicado.
fn analyze_work(
    work: &Work,
    encoding: crate::line_index::ColumnEncoding,
    results: &mut Vec<Analyzed>,
) -> Result<()> {
    match work {
        Work::Rules(rules) => {
            for rule in rules {
                results.extend(analyze_rule_file(rule, encoding)?);
            }
            Ok(())
        }
        Work::Model(files) => analyze_model(files, encoding, results),
    }
}

/// Como um trabalho terminou antes do fim.
enum Failure {
    Error(Box<dyn std::error::Error + Send + Sync>),
    /// O pânico, para ser relançado na thread principal.
    Panic(Box<dyn std::any::Any + Send>),
}

/// O que uma thread devolve de um trabalho, pelo índice dele.
type Finished = (usize, Vec<Analyzed>, Option<Failure>);

/// Devolve ao sistema a memória livre dos heaps que o malloc do glibc criou para as threads da
/// análise. Eles continuam com as páginas dos documentos já soltos depois que as threads terminam,
/// uns 30 MB no corpus de 1.400 regras. No Windows o heap é um só e não há o que fazer.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn release_free_memory() {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> std::ffi::c_int;
    }
    // SAFETY: a função do glibc só devolve páginas livres, e pode ser chamada de qualquer thread.
    unsafe {
        malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn release_free_memory() {}

/// Regras fora de formulário SGI que vão juntas para uma thread, para o custo de passar o
/// resultado entre as threads não pesar nas regras pequenas.
const RULES_PER_WORK: usize = 8;

/// Threads da análise da pasta. A máquina de quem usa o editor está fazendo outras coisas.
const MAX_THREADS: usize = 4;

impl Server<'_> {
    /// Analisa todas as regras das pastas do workspace. A leitura, a análise e a validação de cada
    /// arquivo rodam em várias threads. O índice de símbolos e o envio ao editor ficam nesta, na
    /// ordem em que os arquivos foram achados.
    pub(super) fn analyze_workspace(&mut self) -> Result<()> {
        if !self.analyze_folders {
            return Ok(());
        }
        let mut files = Vec::new();
        for folder in &self.folders {
            collect(folder, &mut files);
        }
        let mut progress = Progress::begin(self.out, self.progress, files.len())?;
        let work = self.plan_work(files);
        let encoding = self.encoding;
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(MAX_THREADS));
        let next = std::sync::atomic::AtomicUsize::new(0);
        // Um erro ou um pânico para a análise como antes, quando era serial. Os trabalhos são
        // pegos em ordem, então os anteriores ao que falhou já estão com alguma thread.
        let stop = std::sync::atomic::AtomicBool::new(false);
        let outcome = std::thread::scope(|scope| -> Result<()> {
            let (sender, receiver) = std::sync::mpsc::channel();
            let (work, next, stop) = (&work, &next, &stop);
            let worker = move |sender: std::sync::mpsc::Sender<_>| {
                use std::sync::atomic::Ordering::Relaxed;
                while !stop.load(Relaxed) {
                    let i = next.fetch_add(1, Relaxed);
                    let Some(item) = work.get(i) else {
                        return;
                    };
                    let mut results = Vec::new();
                    let analyze = || analyze_work(item, encoding, &mut results);
                    let failure = match std::panic::catch_unwind(AssertUnwindSafe(analyze)) {
                        Ok(Ok(())) => None,
                        Ok(Err(error)) => Some(Failure::Error(error)),
                        Err(panic) => Some(Failure::Panic(panic)),
                    };
                    let failed = failure.is_some();
                    if failed {
                        stop.store(true, Relaxed);
                    }
                    let finished: Finished = (i, results, failure);
                    if sender.send(finished).is_err() || failed {
                        return;
                    }
                }
            };
            let handles: Vec<_> = (0..threads)
                .filter_map(|_| {
                    let sender = sender.clone();
                    // A pilha da thread principal no Linux, porque a análise desce uma chamada
                    // por nível da árvore e do SQL.
                    std::thread::Builder::new()
                        .stack_size(8 << 20)
                        .spawn_scoped(scope, move || worker(sender))
                        .ok()
                })
                .collect();
            // Sem thread, a análise roda inteira nesta antes de publicar.
            if handles.is_empty() {
                worker(sender.clone());
            }
            drop(sender);
            let published = self.publish_in_order(work, receiver, &mut progress);
            // O fim do `scope` não espera os destrutores das variáveis de cada thread, como o cache
            // de SQL, e o `join` espera. Sem ele, a memória deles ainda não está livre no trim.
            for handle in handles {
                let _ = handle.join();
            }
            published
        });
        release_free_memory();
        outcome?;
        if let Some(p) = progress {
            p.end()?;
        }
        // As regras foram lidas e analisadas, e as definições delas já estão no índice.
        self.index.set_loaded();
        Ok(())
    }

    /// Guarda e publica os resultados das threads na ordem dos trabalhos. Eles chegam fora de
    /// ordem e esperam aqui até chegar a vez deles.
    fn publish_in_order(
        &mut self,
        work: &[Work],
        receiver: std::sync::mpsc::Receiver<Finished>,
        progress: &mut Option<Progress>,
    ) -> Result<()> {
        let mut waiting = std::collections::BTreeMap::new();
        let mut turn = 0;
        for (i, results, failure) in receiver {
            waiting.insert(i, (results, failure));
            while let Some((results, failure)) = waiting.remove(&turn) {
                for analyzed in results {
                    self.apply(analyzed, false)?;
                }
                match failure {
                    None => {}
                    Some(Failure::Error(error)) => return Err(error),
                    Some(Failure::Panic(panic)) => std::panic::resume_unwind(panic),
                }
                if let Some(p) = progress {
                    p.step(work[turn].files())?;
                }
                turn += 1;
            }
        }
        Ok(())
    }

    /// Divide os arquivos em trabalhos para as threads, com o contexto, os níveis e se está
    /// aberto, que dependem do estado do servidor. Os modelos vão para o fim.
    fn plan_work(&self, files: Vec<PathBuf>) -> Vec<Work> {
        let mut work = Vec::new();
        let mut rules: Vec<RuleFile> = Vec::new();
        let mut group: Option<PathBuf> = None;
        // Os JSON de um modelo dependem uns dos outros. Montar o modelo uma vez por pasta evita
        // reler a pasta inteira para cada JSON, que num repositório com milhares de modelos
        // levava dezenas de segundos.
        let mut models: std::collections::BTreeMap<PathBuf, Vec<ModelFile>> = Default::default();
        for path in files {
            let uri = crate::form::uri_from_path(&path).parse::<Uri>().ok();
            // A regra separada fica em `regras`, um nível abaixo da pasta do modelo.
            let model_dir = match &uri {
                Some(uri) if is_report(uri) => path.parent(),
                Some(uri) if is_report_rule(uri) => path.parent().and_then(|p| p.parent()),
                _ => None,
            };
            // O arquivo de modelo só conta quando o modelo dele é analisado.
            if let Some(dir) = model_dir {
                let uri = uri.expect("só um URI dá a pasta do modelo");
                let file = ModelFile {
                    open: self.documents.report(&uri).is_some()
                        || self.documents.get(&uri).is_some(),
                    levels: self.levels_of(&uri),
                    path: path.clone(),
                    uri: Some(uri),
                };
                models.entry(dir.to_path_buf()).or_default().push(file);
                continue;
            }
            let form = crate::form::form_dir(&path);
            let full = form.is_none() && rules.len() >= RULES_PER_WORK;
            if !rules.is_empty() && (form != group || full) {
                work.push(Work::Rules(std::mem::take(&mut rules)));
            }
            group = form;
            // O arquivo que não é regra, ou não vira URI, só conta no progresso.
            let rule = uri.filter(is_rule);
            rules.push(RuleFile {
                open: rule
                    .as_ref()
                    .is_some_and(|uri| self.documents.get(uri).is_some()),
                context: rule
                    .as_ref()
                    .map_or(crate::builtins::Context::Rule, |uri| self.rule_context(uri)),
                levels: rule
                    .as_ref()
                    .map_or_else(Default::default, |uri| self.levels_of(uri)),
                path,
                uri: rule,
            });
        }
        if !rules.is_empty() {
            work.push(Work::Rules(rules));
        }
        work.extend(models.into_values().map(Work::Model));
        work
    }

    /// Guarda as definições do arquivo analisado no índice e publica os diagnósticos.
    /// `publish_empty` publica também a lista vazia, para limpar o que o editor mostrava antes.
    fn apply(&mut self, analyzed: Analyzed, publish_empty: bool) -> Result<()> {
        if let Some((path, file)) = analyzed.symbols {
            self.index.insert_file(path, file);
        }
        if let Some(diagnostics) = analyzed.diagnostics
            && (publish_empty
                || !diagnostics.is_empty()
                || self.published.borrow().contains(&analyzed.uri))
        {
            self.send_diagnostics(analyzed.uri, diagnostics, None)?;
        }
        Ok(())
    }

    /// Regra lida do disco. `publish_empty` publica também a lista vazia, para limpar o que o
    /// editor mostrava antes.
    fn analyze_rule(&mut self, path: &Path, uri: &Uri, publish_empty: bool) -> Result<()> {
        let rule = RuleFile {
            path: path.to_path_buf(),
            uri: Some(uri.clone()),
            context: self.rule_context(uri),
            levels: self.levels_of(uri),
            open: self.documents.get(uri).is_some(),
        };
        match analyze_rule_file(&rule, self.encoding)? {
            Some(analyzed) => self.apply(analyzed, publish_empty),
            None => Ok(()),
        }
    }

    /// Analisa o arquivo como está no disco e publica, a não ser que ele esteja aberto, quando
    /// vale o texto do editor.
    pub(super) fn analyze_disk(&mut self, path: &Path) -> Result<()> {
        let Ok(uri) = crate::form::uri_from_path(path).parse::<Uri>() else {
            return Ok(());
        };
        if self.documents.get(&uri).is_some() || self.documents.report(&uri).is_some() {
            return Ok(());
        }
        if is_report(&uri) {
            let Some(json) = read(path, false) else {
                return Ok(());
            };
            let mut documents = Documents::default();
            documents.set_report(uri.clone(), json, 0)?;
            let levels = self.levels_of(&uri);
            self.apply(
                analyzed(&documents, &uri, path, &levels, self.encoding),
                true,
            )
        } else if is_rule(&uri) || is_report_rule(&uri) {
            self.analyze_rule(path, &uri, true)
        } else {
            Ok(())
        }
    }

    /// Arquivo do disco que entra na análise da pasta.
    pub(super) fn analyzes(&self, path: &Path) -> bool {
        self.analyze_folders
            && crate::form::uri_from_path(path)
                .parse::<Uri>()
                .is_ok_and(|u| is_rule(&u) || is_report(&u) || is_report_rule(&u))
    }
}

impl Server<'_> {
    /// Onde o campo do dicionário aparece nas regras das pastas do workspace e nos documentos
    /// abertos, no código e nos textos de SQL (veja o [`super::sql::field_occurrences`]). O
    /// documento aberto vale como está no editor. Nos JSON de modelo, a busca é nas regras de
    /// cada JSON, e a posição volta para o JSON. Os arquivos que não citam o nome do campo nem
    /// são analisados.
    pub(super) fn field_references(&self, table: &str, field: &str) -> Vec<Location> {
        let mut files = Vec::new();
        for folder in &self.folders {
            collect(folder, &mut files);
        }
        for uri in self.documents.uris() {
            files.extend(crate::form::path_from_uri(uri.as_str()));
        }
        files.sort();
        files.dedup();
        let mut out = Vec::new();
        for path in files {
            let Ok(uri) = crate::form::uri_from_path(&path).parse::<Uri>() else {
                continue;
            };
            let json = is_report(&uri);
            let text = match (self.documents.report(&uri), self.documents.get(&uri)) {
                (Some(report), _) => Some(report.json.to_string()),
                (_, Some(doc)) => Some(doc.text.clone()),
                _ if json => read(&path, false),
                _ if is_rule(&uri) || is_report_rule(&uri) => read(&path, true),
                _ => None,
            };
            let Some(text) = text.filter(|t| contains_ignore_case(t, field)) else {
                continue;
            };
            let ranges: Vec<TextRange> = if json {
                crate::report::extract(&text)
                    .iter()
                    .flat_map(|rule| {
                        super::sql::field_occurrences(&rule.text, table, field)
                            .into_iter()
                            .map(|r| rule.range_to_json(r))
                    })
                    .collect()
            } else {
                super::sql::field_occurrences(&text, table, field)
            };
            if ranges.is_empty() {
                continue;
            }
            let index = crate::line_index::LineIndex::new(&text);
            let position = |offset| {
                let lc = index.line_col(&text, offset, self.encoding);
                Position::new(lc.line, lc.col)
            };
            for range in ranges {
                let range = Range::new(position(range.start()), position(range.end()));
                out.push(Location::new(uri.clone(), range));
            }
        }
        out
    }
}

/// Diz se `text` tem `needle` sem diferença de caixa. O nome é ASCII, e a busca começa pela
/// primeira letra, nas duas caixas.
fn contains_ignore_case(text: &str, needle: &str) -> bool {
    let (bytes, needle) = (text.as_bytes(), needle.as_bytes());
    let Some(&first) = needle.first() else {
        return false;
    };
    memchr::memchr2_iter(
        first.to_ascii_lowercase(),
        first.to_ascii_uppercase(),
        bytes,
    )
    .any(|at| {
        bytes
            .get(at..at + needle.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(needle))
    })
}

/// Regras e JSON de modelo abaixo de `dir`.
pub(super) fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        match entry.file_type() {
            Ok(t) if t.is_dir() => {
                if !SKIPPED.iter().any(|s| name.eq_ignore_ascii_case(s)) {
                    collect(&path, files);
                }
            }
            Ok(t) if t.is_file() => {
                let candidate = name.eq_ignore_ascii_case("rule.txt")
                    || name.to_ascii_lowercase().ends_with(".json")
                    || crate::report::is_rule_file(&path);
                if candidate {
                    files.push(path);
                }
            }
            _ => {}
        }
    }
}

/// Texto do arquivo. As regras vêm em cp1252, como o gerenciador de artefatos exporta. O JSON
/// é lido em UTF-8 e, se não for, em cp1252.
pub(super) fn read(path: &Path, rule: bool) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if rule {
        return crate::cp1252::decode(&bytes);
    }
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(e) => crate::cp1252::decode(e.as_bytes()),
    }
}
