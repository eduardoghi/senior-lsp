//! Busca de campo do dicionário pela descrição, para o comando da extensão que insere
//! `TABELA.Campo` no cursor.

use std::sync::{Arc, Weak};

use serde::{Deserialize, Serialize};

use crate::tbs::Dictionary;

/// Pedido da extensão com o texto digitado na caixa de busca.
pub(crate) const FIELD_SEARCH_REQUEST: &str = "senior-lsp/buscarCampo";

/// Mais que isso não cabe na lista sem rolar muito, e o texto mais longo afunila.
const LIMIT: usize = 100;

#[derive(Deserialize)]
pub(crate) struct FieldSearch {
    pub(crate) texto: String,
}

#[derive(Serialize, Debug, PartialEq)]
pub(crate) struct FoundField {
    pub(crate) tabela: String,
    pub(crate) campo: String,
    pub(crate) tipo: String,
    pub(crate) descricao: String,
    #[serde(rename = "descricaoTabela")]
    pub(crate) table_description: String,
}

/// Texto normalizado de cada tabela e, de cada campo dela, o nome com a descrição e os títulos,
/// na ordem do dicionário.
type Folded = Vec<(String, Vec<(String, String)>)>;

/// O texto normalizado do último dicionário buscado. Refazer a cada tecla custaria percorrer e
/// converter todas as descrições de novo. A referência fraca deixa o dicionário antigo ser
/// liberado depois do recarregar, e enquanto ela existe o endereço não é reaproveitado.
static CACHE: std::sync::Mutex<Option<(Weak<Dictionary>, Arc<Folded>)>> =
    std::sync::Mutex::new(None);

fn folded(dictionary: &Arc<Dictionary>) -> Arc<Folded> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((cached, folded)) = cache.as_ref()
        && std::ptr::eq(cached.as_ptr(), Arc::as_ptr(dictionary))
    {
        return folded.clone();
    }
    let folded: Arc<Folded> = Arc::new(
        dictionary
            .tables
            .iter()
            .map(|table| {
                let fields = table
                    .fields
                    .iter()
                    .map(|f| {
                        (
                            fold(&format!("{}.{} {}", table.name, f.name, f.description)),
                            fold(&format!("{} {}", f.short_title, f.long_title)),
                        )
                    })
                    .collect();
                (
                    fold(&format!("{} {}", table.name, table.description)),
                    fields,
                )
            })
            .collect(),
    );
    *cache = Some((Arc::downgrade(dictionary), folded.clone()));
    folded
}

/// Campos em que todas as palavras do texto aparecem no nome da tabela, no nome do campo, na
/// descrição ou nos títulos, sem diferença de caixa nem de acento. Vêm primeiro os que têm mais
/// palavras no nome e na descrição do próprio campo, e em cada grupo a ordem é por tabela e campo.
pub(crate) fn search(dictionary: &Arc<Dictionary>, text: &str) -> Vec<FoundField> {
    let words: Vec<String> = fold(text).split_whitespace().map(str::to_owned).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let folded = folded(dictionary);
    let mut found = Vec::new();
    let mut in_table = vec![false; words.len()];
    for (table, (table_text, fields)) in dictionary.tables.iter().zip(folded.iter()) {
        // Uma vez por tabela, e não uma por campo dela.
        for (has, w) in in_table.iter_mut().zip(&words) {
            *has = table_text.contains(w.as_str());
        }
        for (field, (own, titles)) in table.fields.iter().zip(fields) {
            let mut outside = 0;
            let all = words.iter().zip(&in_table).all(|(w, &has)| {
                if own.contains(w.as_str()) {
                    true
                } else if has || titles.contains(w.as_str()) {
                    outside += 1;
                    true
                } else {
                    false
                }
            });
            if all {
                found.push((outside, table, field));
            }
        }
    }
    type Found<'a> = (usize, &'a crate::tbs::Table, &'a crate::tbs::Field);
    let order =
        |a: &Found, b: &Found| (a.0, &a.1.name, &a.2.name).cmp(&(b.0, &b.1.name, &b.2.name));
    // Uma palavra curta acha dezenas de milhares de campos, e só os primeiros aparecem. Separa
    // os primeiros sem ordenar o resto.
    if found.len() > LIMIT {
        found.select_nth_unstable_by(LIMIT, order);
        found.truncate(LIMIT);
    }
    found.sort_by(order);
    found
        .into_iter()
        .map(|(_, table, field)| FoundField {
            tabela: table.name.clone(),
            campo: field.name.clone(),
            tipo: field.ty.to_string(),
            descricao: field.description.clone(),
            table_description: table.description.clone(),
        })
        .collect()
}

/// Minúsculas e sem acento, para "emissao" achar "Emissão".
fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(text: &str) -> Vec<String> {
        let bytes = std::fs::read("tests/fixtures/dicionario-ficticio.tbs").unwrap();
        let dictionary = Arc::new(Dictionary::read(&bytes).unwrap());
        search(&dictionary, text)
            .into_iter()
            .map(|f| format!("{}.{}", f.tabela, f.campo))
            .collect()
    }

    fn table(name: &str, description: &str, fields: &[(&str, &str)]) -> crate::tbs::Table {
        crate::tbs::Table {
            name: name.to_owned(),
            description: description.to_owned(),
            fields: fields
                .iter()
                .map(|(n, d)| crate::tbs::Field {
                    name: (*n).to_owned(),
                    description: (*d).to_owned(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn ordem_dos_grupos_e_limite() {
        // O campo que tem a palavra na própria descrição vem antes do que só a tem na descrição
        // da tabela, mesmo com a tabela antes na ordem alfabética.
        let dictionary = Arc::new(Dictionary {
            tables: vec![
                table("A001", "Notas", &[("Cod", "Código")]),
                table("B001", "Outros", &[("Num", "Número da nota")]),
            ],
            ..Default::default()
        });
        let order: Vec<_> = search(&dictionary, "nota")
            .into_iter()
            .map(|f| format!("{}.{}", f.tabela, f.campo))
            .collect();
        assert_eq!(order, ["B001.Num", "A001.Cod"]);

        // Com mais campos que o limite, vêm os 100 primeiros em ordem.
        let fields: Vec<(std::string::String, &str)> =
            (0..150).map(|i| (format!("C{i:03}"), "valor")).collect();
        let refs: Vec<(&str, &str)> = fields.iter().map(|(n, d)| (n.as_str(), *d)).collect();
        let dictionary = Arc::new(Dictionary {
            tables: vec![table("T", "Tabela", &refs)],
            ..Default::default()
        });
        let found = search(&dictionary, "valor");
        assert_eq!(found.len(), LIMIT);
        assert_eq!(found[0].campo, "C000");
        assert_eq!(found[LIMIT - 1].campo, "C099");
    }

    #[test]
    fn sem_acento_em_todas_as_vogais() {
        assert_eq!(
            fold("ÁÀÂÃÄ ÉÈÊË ÍÌÎÏ ÓÒÔÕÖ ÚÙÛÜ Ç"),
            "aaaaa eeee iiii ooooo uuuu c"
        );
    }

    #[test]
    fn cache_nao_prende_o_dicionario() {
        let bytes = std::fs::read("tests/fixtures/dicionario-ficticio.tbs").unwrap();
        let dictionary = Arc::new(Dictionary::read(&bytes).unwrap());
        search(&dictionary, "empresa");
        let weak = Arc::downgrade(&dictionary);
        drop(dictionary);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn busca_pelas_palavras_sem_caixa_nem_acento() {
        assert_eq!(found("descricao PRODUTO"), ["E075PRO.DesPro"]);
        // A palavra que só a descrição da tabela tem vem depois das que o campo tem.
        assert_eq!(found("codigo cliente"), ["E085CLI.CodCli"]);
        assert_eq!(found("empresa filiais"), ["E070FIL.CodEmp"]);
        assert_eq!(found("nomfil"), ["E070FIL.NomFil"]);
        assert!(found("   ").is_empty() && found("inexistente").is_empty());
    }
}
