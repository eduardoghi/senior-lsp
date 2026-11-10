//! Leitura do dicionário de dados do ERP, o arquivo `.tbs`.
//!
//! O arquivo guarda as tabelas do banco com os campos, o tipo e a descrição de cada um, e é dele
//! que o ERP tira o tipo de um campo de tabela numa regra. O conteúdo segue o formato de
//! serialização de componentes do Delphi, em que cada valor vem com um byte de tipo (inteiro,
//! string, booleano, início e fim de lista), misturado com alguns blocos de bytes sem marcação.
//!
//! A estrutura, na ordem do arquivo:
//!
//! - cabeçalho com o modo (`COMPLETE`), um número e um booleano
//! - lista de textos, que os itens citam pela posição: nomes, descrições, máscaras
//! - lista de tipos, cada um um texto `tipo,tamanho,decimais,enumeração`
//! - nome da sessão e lista de bancos
//!
//! Cada banco começa com 20 bytes de versão. O primeiro inteiro é a versão da estrutura e o quarto
//! a das informações extras, e partes do conteúdo só existem a partir de certa versão. Depois vêm
//! as enumerações, as tabelas, os relacionamentos, os procedimentos e as visões. No fim do banco
//! ficam as ligações de busca dos campos, que citam as tabelas e as visões pela posição no arquivo.
//! Depois do banco o arquivo só tem a lista de módulos da sessão, que não é lida.

use std::fmt;

use crate::symbols::VarType;

/// Tabelas do dicionário, em ordem pelo nome em maiúsculas, para a busca binária.
#[derive(Debug, Clone, Default)]
pub struct Dictionary {
    pub tables: Vec<Table>,
    pub relations: Vec<Relation>,
    /// Enumerações pelo nome em maiúsculas.
    pub enumerations: std::collections::HashMap<String, Enumeration>,
    /// Procedimentos, em ordem pelo nome em maiúsculas.
    pub procedures: Vec<Procedure>,
    /// Visões, em ordem pelo nome em maiúsculas.
    pub views: Vec<View>,
}

/// Procedimento do banco. O código vem em uma versão por banco, cada uma com uma chave como
/// `ORACLE;0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Procedure {
    pub name: String,
    pub code: Vec<(String, String)>,
}

/// Visão do banco, com os campos que ela devolve e o SQL que a cria.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct View {
    pub name: String,
    pub description: String,
    pub fields: Vec<Field>,
    pub sql: String,
}

/// Ligação de busca do campo, a tabela ou visão em que o ERP procura o valor digitado.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Link {
    /// Campos da própria tabela que vão para a busca, o próprio campo e os que completam a chave.
    pub from_fields: Vec<String>,
    /// Tabela ou visão da busca.
    pub target: String,
    /// Campos da busca que recebem `from_fields`, na mesma ordem.
    pub target_fields: Vec<String>,
    /// Campo da busca mostrado ao lado do valor, como a descrição.
    pub result: Option<String>,
}

/// Enumeração, com o tipo dos valores e cada valor com a descrição, como `S` e `Sim`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enumeration {
    pub name: String,
    pub base: FieldType,
    pub values: Vec<(String, String)>,
}

/// Gatilho da tabela. O código pode vir em mais de uma versão, cada uma com uma chave como
/// `SENIOR;0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub name: String,
    pub code: Vec<(String, String)>,
}

/// Restrição de verificação, com a condição que o banco confere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub condition: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Table {
    pub name: String,
    pub description: String,
    pub fields: Vec<Field>,
    /// Posições dos campos da chave primária, vazia se a tabela não tem.
    pub primary_key: Vec<usize>,
    pub uniques: Vec<Key>,
    pub indexes: Vec<Key>,
    /// Posições em [`Dictionary::relations`] dos relacionamentos em que a tabela é origem ou destino.
    pub relations: Vec<usize>,
    pub triggers: Vec<Trigger>,
    pub checks: Vec<Check>,
}

/// Restrição única ou índice, com as posições dos campos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub name: String,
    pub fields: Vec<usize>,
    pub unique: bool,
}

/// Relacionamento entre tabelas, dos campos da origem para uma chave do destino (a primária ou uma
/// restrição única).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub name: String,
    pub from_table: String,
    pub from_fields: Vec<String>,
    pub to_table: String,
    pub to_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Field {
    pub name: String,
    pub ty: FieldType,
    pub description: String,
    pub short_title: String,
    pub long_title: String,
    pub mask: String,
    /// Valor que o campo recebe quando não é informado, como `0` ou `31/12/1900`.
    pub initial: String,
    pub link: Option<Link>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FieldType {
    pub kind: FieldKind,
    pub size: u32,
    pub decimals: u32,
    /// Nome da enumeração, quando o campo é de uma.
    pub enumeration: Option<String>,
    /// Tipo dos valores da enumeração, que é o tipo do campo numa regra.
    pub base: Option<FieldKind>,
}

/// Tipo físico do campo, como o dicionário numera.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldKind {
    #[default]
    Unknown,
    Text,
    Number,
    Boolean,
    Date,
    Time,
    Blob,
    LongText,
    Image,
    Enumeration,
    Double,
}

impl FieldKind {
    fn from_code(code: u32) -> FieldKind {
        match code {
            1 => FieldKind::Text,
            2 => FieldKind::Number,
            3 => FieldKind::Boolean,
            4 => FieldKind::Date,
            5 => FieldKind::Time,
            6 => FieldKind::Blob,
            7 => FieldKind::LongText,
            8 => FieldKind::Image,
            9 => FieldKind::Enumeration,
            10 => FieldKind::Double,
            _ => FieldKind::Unknown,
        }
    }

    /// Tipo que o campo tem numa regra. Hora vale como número, como no resto da linguagem.
    fn var_type(self) -> Option<VarType> {
        match self {
            FieldKind::Text | FieldKind::LongText => Some(VarType::Alfa),
            FieldKind::Number | FieldKind::Double | FieldKind::Time | FieldKind::Boolean => {
                Some(VarType::Numero)
            }
            FieldKind::Date => Some(VarType::Data),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            FieldKind::Unknown => "desconhecido",
            FieldKind::Text => "alfa",
            FieldKind::Number => "numero",
            FieldKind::Boolean => "booleano",
            FieldKind::Date => "data",
            FieldKind::Time => "hora",
            FieldKind::Blob => "blob",
            FieldKind::LongText => "texto",
            FieldKind::Image => "imagem",
            FieldKind::Enumeration => "enumeracao",
            FieldKind::Double => "double",
        }
    }
}

impl FieldType {
    /// Tipo do campo numa regra, com a enumeração pelo tipo dos valores dela. O alfa de um
    /// caractere fica sem tipo, porque o compilador aceita ele tanto como texto quanto como o
    /// código do caractere, em comparação e em atribuição.
    pub fn var_type(&self) -> Option<VarType> {
        let kind = match self.kind {
            FieldKind::Enumeration => self.base?,
            kind => kind,
        };
        if kind == FieldKind::Text && self.size == 1 {
            return None;
        }
        kind.var_type()
    }

    /// Lê um texto da lista de tipos, como `2,15,2,` ou `9,0,0,LSitEmp`.
    fn parse(text: &str) -> FieldType {
        let mut parts = text.split(',');
        let mut number = || {
            parts
                .next()
                .and_then(|p| p.trim().parse().ok())
                .unwrap_or(0)
        };
        let kind = FieldKind::from_code(number());
        let size = number();
        let decimals = number();
        let enumeration = parts
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        FieldType {
            kind,
            size,
            decimals,
            enumeration,
            base: None,
        }
    }
}

impl fmt::Display for FieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            FieldKind::Enumeration => self.base.unwrap_or(FieldKind::Unknown),
            kind => kind,
        };
        f.write_str(kind.label())?;
        match kind {
            FieldKind::Text | FieldKind::Number => write!(f, " {}", self.size)?,
            _ => {}
        }
        if self.decimals > 0 {
            write!(f, ",{}", self.decimals)?;
        }
        if let Some(enumeration) = &self.enumeration {
            write!(f, " {enumeration}")?;
        }
        Ok(())
    }
}

/// Arquivo que não pôde ser lido, com a posição em que a leitura parou.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (byte {})", self.message, self.offset)
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

impl Dictionary {
    /// Lê o dicionário. Só o primeiro banco do arquivo é usado, que é o único que o ERP grava.
    pub fn read(bytes: &[u8]) -> Result<Dictionary> {
        let mut r = Reader { bytes, pos: 0 };
        let mode = r.string()?;
        if mode != "COMPLETE" {
            return Err(r.error(format!("modo {mode:?} não suportado")));
        }
        r.integer()?;
        r.boolean()?;
        let texts = r.list(Reader::string)?;
        let types = r.list(|r| {
            r.integer()?;
            Ok(FieldType::parse(&r.string()?))
        })?;
        r.string()?;
        r.list_begin()?;
        if r.end_of_list()? {
            return Ok(Dictionary::default());
        }
        let mut ctx = Context {
            texts,
            types,
            version: 0,
            extra: 0,
        };
        ctx.database(&mut r)
    }

    /// Tabela pelo nome, sem diferença de caixa.
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables
            .binary_search_by(|t| cmp_upper(&t.name, name))
            .ok()
            .map(|i| &self.tables[i])
    }

    /// Relacionamentos em que a tabela é origem ou destino.
    pub fn relations_of<'a>(&'a self, table: &'a Table) -> impl Iterator<Item = &'a Relation> {
        table
            .relations
            .iter()
            .filter_map(|&i| self.relations.get(i))
    }

    /// Campo de uma tabela, sem diferença de caixa.
    pub fn field(&self, table: &str, field: &str) -> Option<&Field> {
        self.table(table)?
            .fields
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(field))
    }
}

fn cmp_upper(a: &str, b: &str) -> std::cmp::Ordering {
    a.bytes()
        .map(|c| c.to_ascii_uppercase())
        .cmp(b.bytes().map(|c| c.to_ascii_uppercase()))
}

/// O que o banco tem depois dos relacionamentos.
#[derive(Default)]
struct Rest {
    procedures: Vec<Procedure>,
    views: Vec<View>,
}

/// Ligação como o arquivo grava, com posições no lugar dos nomes.
struct RawLink {
    from: Vec<i32>,
    target: i32,
    to: Vec<i32>,
    result: i32,
}

impl RawLink {
    /// Troca as posições pelos nomes. `own` são os campos da tabela ou visão do campo e `rowsets` as
    /// tabelas e depois as visões, na ordem do arquivo.
    fn resolve(self, own: &[String], rowsets: &[(String, Vec<String>)]) -> Option<Link> {
        let (target, fields) = usize::try_from(self.target)
            .ok()
            .and_then(|i| rowsets.get(i))?;
        let names = |list: &[i32], names: &[String]| -> Vec<String> {
            list.iter()
                .filter_map(|&i| usize::try_from(i).ok().and_then(|i| names.get(i)).cloned())
                .collect()
        };
        Some(Link {
            from_fields: names(&self.from, own),
            target: target.clone(),
            target_fields: names(&self.to, fields),
            result: usize::try_from(self.result)
                .ok()
                .and_then(|i| fields.get(i))
                .cloned(),
        })
    }
}

struct Context {
    texts: Vec<String>,
    types: Vec<FieldType>,
    /// Versão da estrutura do banco.
    version: i32,
    /// Versão das informações extras do banco, como as descrições.
    extra: i32,
}

impl Context {
    fn text(&self, index: i32) -> String {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.texts.get(i))
            .cloned()
            .unwrap_or_default()
    }

    fn field_type(&self, index: i32) -> FieldType {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.types.get(i))
            .cloned()
            .unwrap_or(FieldType {
                kind: FieldKind::Unknown,
                size: 0,
                decimals: 0,
                enumeration: None,
                base: None,
            })
    }

    fn database(&mut self, r: &mut Reader) -> Result<Dictionary> {
        let version = r.raw(20)?;
        let word = |i: usize| i32::from_le_bytes(version[i * 4..i * 4 + 4].try_into().unwrap());
        self.version = word(0);
        self.extra = word(3);
        r.integer()?;
        if self.version >= 0x1998_1109 {
            r.raw(8)?;
        }
        if self.version >= 0x1998_1026 {
            r.boolean()?;
            r.boolean()?;
        }
        let mut enumerations = std::collections::HashMap::new();
        r.list_begin()?;
        while !r.end_of_list()? {
            if let Some(e) = self.enumeration(r)? {
                enumerations.insert(e.name.to_uppercase(), e);
            }
        }
        r.list_end()?;
        let mut tables = Vec::new();
        let mut replication = Vec::new();
        r.list_begin()?;
        while !r.end_of_list()? {
            let (mut table, mode) = self.table(r)?;
            replication.push(mode);
            for field in &mut table.fields {
                let base = field
                    .ty
                    .enumeration
                    .as_ref()
                    .and_then(|name| enumerations.get(&name.to_uppercase()))
                    .map(|e: &Enumeration| &e.base);
                if let Some(base) = base {
                    field.ty.base = Some(base.kind);
                    field.ty.size = base.size;
                    field.ty.decimals = base.decimals;
                }
            }
            tables.push(table);
        }
        r.list_end()?;
        // Os relacionamentos e as ligações citam as tabelas pela posição no arquivo, então vêm antes
        // de ordenar. O que não pôde ser lido daqui em diante não tira as tabelas, que já estão
        // completas.
        let relations = self.relations(r, &tables);
        let rest = match relations {
            Ok(_) => self.rest(r, &mut tables, &replication).unwrap_or_default(),
            Err(_) => Rest::default(),
        };
        let relations = relations.unwrap_or_default();
        for (i, relation) in relations.iter().enumerate() {
            for name in [&relation.from_table, &relation.to_table] {
                if let Some(t) = tables.iter_mut().find(|t| &t.name == name)
                    && t.relations.last() != Some(&i)
                {
                    t.relations.push(i);
                }
            }
        }
        tables.sort_by_cached_key(|t| t.name.to_ascii_uppercase());
        let Rest {
            mut procedures,
            mut views,
        } = rest;
        procedures.sort_by_cached_key(|p| p.name.to_ascii_uppercase());
        views.sort_by_cached_key(|v| v.name.to_ascii_uppercase());
        Ok(Dictionary {
            tables,
            relations,
            enumerations,
            procedures,
            views,
        })
    }

    /// Lê o banco depois dos relacionamentos, com os procedimentos, as visões e as ligações de
    /// busca, que ficam gravadas nos campos de `tables`.
    ///
    /// Só lê dicionários a partir de 28/02/2011. Antes disso as visões e os procedimentos mudam de
    /// formato em várias versões, e um dicionário assim fica sem essa parte.
    fn rest(&self, r: &mut Reader, tables: &mut [Table], replication: &[i32]) -> Result<Rest> {
        if self.version < 0x2011_0228 || self.extra < 0x2011_0228 {
            return Ok(Rest::default());
        }
        let procedures = r.list(|r| {
            let name = self.text(r.integer()?);
            r.raw(8)?;
            let code = r.list(|r| {
                r.boolean()?;
                let key = r.string()?;
                r.string()?;
                Ok((key, r.string()?))
            })?;
            r.boolean()?;
            Ok(Procedure { name, code })
        })?;
        r.boolean()?;
        let mut views = r.list(|r| {
            let name = self.text(r.integer()?);
            let fields = r.list(|r| self.field(r))?;
            let sql = r.string()?;
            r.string()?;
            r.raw(1)?;
            let description = self.text(r.integer()?);
            Ok(View {
                name,
                description,
                fields,
                sql,
            })
        })?;
        // Campos das tabelas usados por cada visão.
        for _ in &views {
            r.list(|r| Ok((r.integer()?, r.integer()?)))?;
        }
        r.integer()?;
        for _ in 0..r.integer()? {
            r.string()?;
        }
        r.boolean()?;
        // Tipos especiais de campo, com nome, descrição, máscara, tamanho e tipo.
        r.list(|r| {
            for _ in 0..5 {
                r.integer()?;
            }
            Ok(())
        })?;
        for _ in 0..3 {
            r.integer()?;
        }
        r.string()?;
        for _ in 0..5 {
            r.integer()?;
        }
        if self.extra >= 0x2016_1003 {
            r.date()?;
        }
        // Índices com tabela mestre e as tabelas replicadas.
        for table in tables.iter() {
            for _ in &table.indexes {
                if r.boolean()? {
                    r.integer()?;
                    r.integer()?;
                }
            }
        }
        for &mode in replication {
            if mode == 3 || mode == 4 {
                r.integer()?;
            }
        }
        // Tabela mestre de cada tabela e, por fim, a ligação de cada campo das tabelas e depois das
        // visões. A busca cita a tabela ou a visão pela posição, com as tabelas primeiro.
        for _ in tables.iter() {
            if r.boolean()? {
                r.integer()?;
            }
        }
        let mut raw = Vec::new();
        for table in tables.iter() {
            for _ in &table.fields {
                raw.push(Self::raw_link(r)?);
            }
        }
        for view in &views {
            for _ in &view.fields {
                raw.push(Self::raw_link(r)?);
            }
        }
        let rowsets: Vec<(String, Vec<String>)> = tables
            .iter()
            .map(|t| {
                (
                    t.name.clone(),
                    t.fields.iter().map(|f| f.name.clone()).collect(),
                )
            })
            .chain(views.iter().map(|v| {
                (
                    v.name.clone(),
                    v.fields.iter().map(|f| f.name.clone()).collect(),
                )
            }))
            .collect();
        let fields = tables
            .iter_mut()
            .flat_map(|t| t.fields.iter_mut())
            .chain(views.iter_mut().flat_map(|v| v.fields.iter_mut()));
        for ((field, raw), own) in fields.zip(raw).zip(
            rowsets
                .iter()
                .flat_map(|(_, names)| std::iter::repeat_n(names, names.len())),
        ) {
            field.link = raw.and_then(|raw| raw.resolve(own, &rowsets));
        }
        Ok(Rest { procedures, views })
    }

    fn raw_link(r: &mut Reader) -> Result<Option<RawLink>> {
        let result = r.integer()?;
        if !r.boolean()? {
            return Ok(None);
        }
        let from = r.list(Reader::integer)?;
        let target = r.integer()?;
        let to = r.list(Reader::integer)?;
        Ok(Some(RawLink {
            from,
            target,
            to,
            result,
        }))
    }

    /// Enumeração, com o nome que os campos citam e o tipo dos valores.
    fn enumeration(&self, r: &mut Reader) -> Result<Option<Enumeration>> {
        let name = self.field_type(r.integer()?).enumeration;
        r.integer()?;
        let base = self.field_type(r.integer()?);
        let values = r.list(|r| {
            let value = self.text(r.integer()?);
            let description = if self.version >= 0x1998_1124 {
                self.text(r.integer()?)
            } else {
                r.string()?
            };
            Ok((value, description))
        })?;
        if self.version >= 0x2003_0919 {
            r.integer()?;
        }
        Ok(name.map(|name| Enumeration { name, base, values }))
    }

    /// Relacionamentos, logo depois das tabelas. Cada um tem o nome, a tabela de origem e a de
    /// destino (pela posição no arquivo), a chave do destino e os campos da origem. A chave conta a
    /// primária como a primeira, quando a tabela tem, e depois as restrições únicas.
    fn relations(&self, r: &mut Reader, tables: &[Table]) -> Result<Vec<Relation>> {
        if self.version <= 0x1998_0330 {
            return Err(r.error("versão de dicionário antiga demais".to_owned()));
        }
        let table = |i: i32| usize::try_from(i).ok().and_then(|i| tables.get(i));
        let names = |t: &Table, fields: &[usize]| -> Vec<String> {
            fields
                .iter()
                .filter_map(|&f| t.fields.get(f).map(|f| f.name.clone()))
                .collect()
        };
        let mut out = Vec::new();
        r.list_begin()?;
        while !r.end_of_list()? {
            let name = self.text(r.integer()?);
            let (from, to) = (r.integer()?, r.integer()?);
            let key = r.integer()?;
            let fields = r.list(Reader::integer)?;
            let (Some(from), Some(to)) = (table(from), table(to)) else {
                continue;
            };
            let key = usize::try_from(key).ok().and_then(|k| {
                if to.primary_key.is_empty() {
                    to.uniques.get(k).map(|u| u.fields.as_slice())
                } else if k == 0 {
                    Some(to.primary_key.as_slice())
                } else {
                    to.uniques.get(k - 1).map(|u| u.fields.as_slice())
                }
            });
            let fields: Vec<usize> = fields
                .into_iter()
                .filter_map(|f| usize::try_from(f).ok())
                .collect();
            out.push(Relation {
                name,
                from_table: from.name.clone(),
                from_fields: names(from, &fields),
                to_table: to.name.clone(),
                to_fields: key.map(|k| names(to, k)).unwrap_or_default(),
            });
        }
        r.list_end()?;
        Ok(out)
    }

    /// Tabela e o modo de replicação dela, que decide se o fim do banco tem um número a mais para
    /// ela.
    fn table(&self, r: &mut Reader) -> Result<(Table, i32)> {
        let name = self.text(r.integer()?);
        let mut fields = Vec::new();
        r.list_begin()?;
        while !r.end_of_list()? {
            fields.push(self.field(r)?);
        }
        r.list_end()?;
        let positions = |list: Vec<i32>| -> Vec<usize> {
            list.into_iter()
                .filter_map(|f| usize::try_from(f).ok())
                .collect()
        };
        // Restrições únicas e índices, cada um com o nome e a lista dos campos.
        let uniques = r.list(|r| {
            let name = self.text(r.integer()?);
            let fields = positions(r.list(Reader::integer)?);
            Ok(Key {
                name,
                fields,
                unique: true,
            })
        })?;
        let indexes = r.list(|r| {
            let name = self.text(r.integer()?);
            let fields = r.list(|r| {
                let field = r.integer()?;
                if self.version >= 0x2025_0220 {
                    r.integer()?;
                } else {
                    r.boolean()?;
                }
                Ok(field)
            })?;
            Ok(Key {
                name,
                fields: positions(fields),
                unique: r.boolean()?,
            })
        })?;
        // A chave primária vem como as posições dos campos separadas por `;`.
        let primary_key = r
            .string()?
            .split(';')
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        let triggers = if self.version >= 0x1998_1109 {
            r.list(|r| self.trigger(r))?
        } else {
            Vec::new()
        };
        if self.version >= 0x2009_0730 {
            r.boolean()?;
        }
        let checks = if self.version >= 0x2011_1128 {
            r.list(|r| {
                Ok(Check {
                    name: self.text(r.integer()?),
                    condition: r.string()?,
                })
            })?
        } else {
            Vec::new()
        };
        if self.version >= 0x2018_0912 {
            r.boolean()?;
        }
        let description = self.text(r.integer()?);
        let replication = r.integer()?;
        if self.extra >= 0x2009_0731 {
            r.boolean()?;
        }
        for _ in &fields {
            r.string()?;
        }
        let table = Table {
            name,
            description,
            fields,
            primary_key,
            uniques,
            indexes,
            relations: Vec::new(),
            triggers,
            checks,
        };
        Ok((table, replication))
    }

    fn trigger(&self, r: &mut Reader) -> Result<Trigger> {
        let name = self.text(r.integer()?);
        r.raw(1)?;
        r.boolean()?;
        let code = if self.version >= 0x2011_0228 {
            let code = r.list(|r| {
                r.boolean()?;
                Ok((r.string()?, r.string()?))
            })?;
            r.boolean()?;
            code
        } else {
            vec![(String::new(), r.string()?)]
        };
        Ok(Trigger { name, code })
    }

    fn field(&self, r: &mut Reader) -> Result<Field> {
        let name = self.text(r.integer()?);
        let ty = self.field_type(r.integer()?);
        r.boolean()?;
        if self.version >= 0x1998_1109 {
            r.raw(8)?;
        }
        // Informações extras, a descrição, os títulos curto e longo, a máscara e o contexto de
        // ajuda.
        let description = self.text(r.integer()?);
        let short_title = self.text(r.integer()?);
        let long_title = self.text(r.integer()?);
        let mask = self.text(r.integer()?);
        r.integer()?;
        if self.extra < 0x1998_1113 {
            return Err(r.error("versão de dicionário antiga demais".to_owned()));
        }
        r.raw(1)?;
        if self.extra >= 0x2001_0421 {
            r.raw(1)?;
        }
        let initial = if self.extra >= 0x1998_1123 {
            self.text(r.integer()?)
        } else {
            r.string()?
        };
        if self.extra >= 0x1998_0831 {
            r.integer()?;
        }
        if self.extra >= 0x2008_0807 {
            r.boolean()?;
            r.string()?;
            r.string()?;
        }
        if self.extra >= 0x2010_0918 {
            r.integer()?;
            r.string()?;
        }
        Ok(Field {
            name,
            ty,
            description,
            short_title,
            long_title,
            mask,
            initial,
            link: None,
        })
    }
}

/// Leitor dos valores serializados, com o byte de tipo na frente de cada um.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

const LIST: u8 = 1;
const INT8: u8 = 2;
const INT16: u8 = 3;
const INT32: u8 = 4;
const STRING: u8 = 6;
const FALSE: u8 = 8;
const TRUE: u8 = 9;
const LONG_STRING: u8 = 12;
const DATE: u8 = 17;
const WIDE_STRING: u8 = 18;
const UTF8_STRING: u8 = 20;

impl Reader<'_> {
    fn error(&self, message: String) -> Error {
        Error {
            offset: self.pos,
            message,
        }
    }

    fn raw(&mut self, n: usize) -> Result<&[u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.bytes.len());
        let Some(end) = end else {
            return Err(self.error("fim do arquivo".to_owned()));
        };
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn tag(&mut self) -> Result<u8> {
        Ok(self.raw(1)?[0])
    }

    fn unexpected<T>(&mut self, what: &str, tag: u8) -> Result<T> {
        self.pos -= 1;
        Err(self.error(format!("esperava {what}, veio o tipo {tag}")))
    }

    fn integer(&mut self) -> Result<i32> {
        match self.tag()? {
            INT8 => Ok(i32::from(self.raw(1)?[0] as i8)),
            INT16 => Ok(i32::from(i16::from_le_bytes(
                self.raw(2)?.try_into().unwrap(),
            ))),
            INT32 => Ok(i32::from_le_bytes(self.raw(4)?.try_into().unwrap())),
            tag => self.unexpected("inteiro", tag),
        }
    }

    fn length(&mut self) -> Result<usize> {
        let n = u32::from_le_bytes(self.raw(4)?.try_into().unwrap());
        usize::try_from(n).map_err(|_| self.error("tamanho inválido".to_owned()))
    }

    fn string(&mut self) -> Result<String> {
        match self.tag()? {
            STRING => {
                let n = usize::from(self.raw(1)?[0]);
                Ok(cp1252(self.raw(n)?))
            }
            LONG_STRING => {
                let n = self.length()?;
                Ok(cp1252(self.raw(n)?))
            }
            UTF8_STRING => {
                let n = self.length()?;
                Ok(String::from_utf8_lossy(self.raw(n)?).into_owned())
            }
            WIDE_STRING => {
                let n = self.length()?;
                let bytes = self.raw(n.saturating_mul(2))?;
                let units: Vec<u16> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                Ok(String::from_utf16_lossy(&units))
            }
            tag => self.unexpected("texto", tag),
        }
    }

    fn boolean(&mut self) -> Result<bool> {
        match self.tag()? {
            FALSE => Ok(false),
            TRUE => Ok(true),
            tag => self.unexpected("booleano", tag),
        }
    }

    fn date(&mut self) -> Result<()> {
        match self.tag()? {
            DATE => self.raw(8).map(drop),
            tag => self.unexpected("data", tag),
        }
    }

    fn list_begin(&mut self) -> Result<()> {
        match self.tag()? {
            LIST => Ok(()),
            tag => self.unexpected("início de lista", tag),
        }
    }

    fn end_of_list(&mut self) -> Result<bool> {
        match self.bytes.get(self.pos) {
            Some(&b) => Ok(b == 0),
            None => Err(self.error("fim do arquivo".to_owned())),
        }
    }

    fn list_end(&mut self) -> Result<()> {
        match self.tag()? {
            0 => Ok(()),
            tag => self.unexpected("fim de lista", tag),
        }
    }

    fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        self.list_begin()?;
        let mut out = Vec::new();
        while !self.end_of_list()? {
            out.push(item(self)?);
        }
        self.list_end()?;
        Ok(out)
    }
}

fn cp1252(bytes: &[u8]) -> String {
    crate::cp1252::decode(bytes).unwrap_or_else(|| bytes.iter().map(|&b| char::from(b)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Escreve valores no mesmo formato, para montar um dicionário pequeno nos testes.
    #[derive(Default)]
    struct Writer(Vec<u8>);

    impl Writer {
        fn int(&mut self, v: i32) -> &mut Self {
            self.0.push(INT32);
            self.0.extend(v.to_le_bytes());
            self
        }
        fn small(&mut self, v: i8) -> &mut Self {
            self.0.push(INT8);
            self.0.push(v as u8);
            self
        }
        /// Grava em cp1252, como o ERP. Os textos dos testes só usam acentos do latin-1.
        fn str(&mut self, s: &str) -> &mut Self {
            let bytes: Vec<u8> = s.chars().map(|c| c as u8).collect();
            self.0.push(STRING);
            self.0.push(bytes.len() as u8);
            self.0.extend(bytes);
            self
        }
        fn bool(&mut self, v: bool) -> &mut Self {
            self.0.push(if v { TRUE } else { FALSE });
            self
        }
        fn begin(&mut self) -> &mut Self {
            self.0.push(LIST);
            self
        }
        fn end(&mut self) -> &mut Self {
            self.0.push(0);
            self
        }
        fn raw(&mut self, b: &[u8]) -> &mut Self {
            self.0.extend(b);
            self
        }
    }

    fn sample() -> Vec<u8> {
        sample_with_end().0
    }

    /// Amostra e a posição em que terminam as tabelas.
    fn sample_with_end() -> (Vec<u8>, usize) {
        fn field(w: &mut Writer, name: i8, ty: i8, desc: i8) {
            w.small(name).small(ty).bool(false).raw(&[0; 8]);
            w.small(desc).small(-1).small(-1).small(-1).small(0);
            w.raw(&[0, 0])
                .small(-1)
                .small(-1)
                .bool(false)
                .str("")
                .str("");
            w.small(-1).str("");
        }
        let mut w = Writer::default();
        w.str("COMPLETE").int(20_010_302).bool(true);
        // Textos 0 a 6.
        let texts = [
            "BASE",
            "E070EMP",
            "Empresas",
            "CodEmp",
            "Código da empresa",
            "SitEmp",
            "Situação",
            "E070EMPIndice2",
            "R_SITUACAO",
            "GETNEXT",
            "VW_EMP",
            "Visão das empresas",
        ];
        w.begin();
        for t in texts {
            w.str(t);
        }
        w.end();
        // Tipos 0 a 2.
        w.begin();
        for t in ["2,4,0,", "1,1,0,", "9,0,0,LSitEmp"] {
            w.int(1).str(t);
        }
        w.end();
        w.str("Sessao").begin();
        let mut version = Vec::new();
        for v in [0x2018_0912_i32, 0x2018_0912, 0x2018_0912, 0x2016_1003, 0] {
            version.extend(v.to_le_bytes());
        }
        w.raw(&version)
            .small(0)
            .raw(&[0xff; 8])
            .bool(true)
            .bool(true);
        // Uma enumeração alfa de um caractere.
        w.begin().small(2).small(-1).small(1);
        w.begin().small(5).small(6).end();
        w.small(0).end();
        // Uma tabela com dois campos.
        w.begin().small(1).begin();
        for (name, ty, desc) in [(3, 0, 4), (5, 2, 6)] {
            field(&mut w, name, ty, desc);
        }
        w.end();
        // Sem restrição única, um índice único em SitEmp e a chave primária em CodEmp.
        w.begin().end();
        w.begin()
            .small(7)
            .begin()
            .small(1)
            .bool(true)
            .end()
            .bool(true)
            .end();
        w.str("0");
        w.begin().end().bool(false).begin().end().bool(false);
        w.small(2).small(0).bool(false).str("").str("");
        w.end();
        let end = w.0.len();
        // Um relacionamento de SitEmp para a chave primária da mesma tabela.
        w.begin().small(8).small(0).small(0).small(0);
        w.begin().small(1).end();
        w.end();
        // Um procedimento com o código do Oracle.
        w.begin().small(9).raw(&[0; 8]);
        w.begin()
            .bool(false)
            .str("ORACLE;0")
            .str("")
            .str("BEGIN NULL; END;")
            .end();
        w.bool(false).end();
        w.bool(false);
        // Uma visão com o campo CodEmp, que usa o CodEmp da tabela.
        w.begin().small(10).begin();
        field(&mut w, 3, 0, 4);
        w.end()
            .str("SELECT CodEmp FROM E070EMP")
            .str("")
            .raw(&[0])
            .small(11);
        w.end();
        w.begin().small(0).small(0).end();
        w.small(0).small(1).str("0=Padrão").bool(false);
        // Sem tipos especiais, os números do banco completo e a data.
        w.begin().end();
        w.small(5).small(10).small(4).str("SAPIENS");
        for v in [1, 2, 3, 4, 5] {
            w.small(v);
        }
        w.raw(&[17]).raw(&[0; 8]);
        // O índice sem mestre e a tabela sem replicação nem tabela mestre.
        w.bool(false).bool(false);
        // CodEmp sem ligação. SitEmp busca na própria tabela e mostra CodEmp. O campo da visão busca
        // na visão, a posição 1 depois da tabela.
        w.small(-1).bool(false);
        w.small(0).bool(true).begin().small(1).end().small(0);
        w.begin().small(1).end();
        w.small(-1).bool(true).begin().small(0).end().small(1);
        w.begin().small(0).end();
        w.end();
        (w.0, end)
    }

    #[test]
    fn le_tabelas_campos_e_enumeracoes() {
        let dict = Dictionary::read(&sample()).unwrap();
        assert_eq!(dict.tables.len(), 1);
        let table = &dict.tables[0];
        assert_eq!(table.name, "E070EMP");
        assert_eq!(table.description, "Empresas");
        let cod = &table.fields[0];
        assert_eq!(cod.name, "CodEmp");
        assert_eq!(cod.description, "Código da empresa");
        assert_eq!(cod.ty.var_type(), Some(VarType::Numero));
        let sit = &table.fields[1];
        assert_eq!(sit.ty.kind, FieldKind::Enumeration);
        // Enumeração alfa de um caractere fica sem tipo.
        assert_eq!(sit.ty.var_type(), None);
    }

    #[test]
    fn chaves_indices_e_relacionamentos() {
        let dict = Dictionary::read(&sample()).unwrap();
        let table = dict.table("E070EMP").unwrap();
        assert_eq!(table.primary_key, [0]);
        assert_eq!(
            table.indexes,
            [Key {
                name: "E070EMPIndice2".to_owned(),
                fields: vec![1],
                unique: true
            }]
        );
        assert_eq!(
            dict.enumerations["LSITEMP"].values,
            [("SitEmp".to_owned(), "Situação".to_owned())]
        );
        let relations: Vec<_> = dict.relations_of(table).collect();
        assert_eq!(
            relations,
            [&Relation {
                name: "R_SITUACAO".to_owned(),
                from_table: "E070EMP".to_owned(),
                from_fields: vec!["SitEmp".to_owned()],
                to_table: "E070EMP".to_owned(),
                to_fields: vec!["CodEmp".to_owned()],
            }]
        );
    }

    #[test]
    fn busca_sem_diferenca_de_caixa() {
        let dict = Dictionary::read(&sample()).unwrap();
        assert_eq!(dict.field("e070emp", "CODEMP").unwrap().name, "CodEmp");
        assert!(dict.field("E070EMP", "Outro").is_none());
        assert!(dict.table("E999XXX").is_none());
    }

    #[test]
    fn arquivo_cortado_da_erro_sem_panico() {
        let (bytes, fim_das_tabelas) = sample_with_end();
        // Até o fim das tabelas, o arquivo cortado dá erro. Depois, o que vem após as tabelas fica
        // de fora. O último byte fecha a lista de bancos e não é lido.
        for n in 0..bytes.len() {
            let lido = Dictionary::read(&bytes[..n]);
            if n < fim_das_tabelas {
                assert!(lido.is_err(), "cortado em {n}");
            } else {
                let lido = lido.unwrap();
                assert_eq!(lido.tables.len(), 1, "cortado em {n}");
                assert!(
                    lido.views.is_empty() || n == bytes.len() - 1,
                    "cortado em {n}"
                );
            }
        }
    }

    #[test]
    fn procedimentos_visoes_e_ligacoes() {
        let dict = Dictionary::read(&sample()).unwrap();
        assert_eq!(
            dict.procedures,
            [Procedure {
                name: "GETNEXT".to_owned(),
                code: vec![("ORACLE;0".to_owned(), "BEGIN NULL; END;".to_owned())],
            }]
        );
        assert_eq!(dict.views.len(), 1);
        let view = &dict.views[0];
        assert_eq!(
            (
                view.name.as_str(),
                view.description.as_str(),
                view.sql.as_str()
            ),
            ("VW_EMP", "Visão das empresas", "SELECT CodEmp FROM E070EMP")
        );
        assert_eq!(
            view.fields[0].link,
            Some(Link {
                from_fields: vec!["CodEmp".to_owned()],
                target: "VW_EMP".to_owned(),
                target_fields: vec!["CodEmp".to_owned()],
                result: None,
            })
        );
        let table = &dict.tables[0];
        assert_eq!(table.fields[0].link, None);
        assert_eq!(
            table.fields[1].link,
            Some(Link {
                from_fields: vec!["SitEmp".to_owned()],
                target: "E070EMP".to_owned(),
                target_fields: vec!["SitEmp".to_owned()],
                result: Some("CodEmp".to_owned()),
            })
        );
    }

    #[test]
    fn modo_desconhecido() {
        let mut w = Writer::default();
        w.str("SIMPLE");
        assert!(
            Dictionary::read(&w.0)
                .unwrap_err()
                .message
                .contains("SIMPLE")
        );
    }
}
