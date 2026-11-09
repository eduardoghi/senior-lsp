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
//! as enumerações e as tabelas. O resto do banco (relacionamentos, visões, procedimentos) não é
//! lido, porque os tipos e as descrições já estão nas tabelas.

use std::fmt;

use crate::symbols::VarType;

/// Tabelas do dicionário, em ordem pelo nome em maiúsculas, para a busca binária.
#[derive(Debug, Clone, Default)]
pub struct Dictionary {
    pub tables: Vec<Table>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub name: String,
    pub description: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ty: FieldType,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
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

    /// Campo de uma tabela, sem diferença de caixa.
    pub fn field(&self, table: &str, field: &str) -> Option<&Field> {
        self.table(table)?
            .fields
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(field))
    }

    /// Diferenças de `self` para `other`, uma por linha. A linha começa com `-` para o que só existe
    /// em `self`, `+` para o que só existe em `other` e `~` para campo com tipo diferente.
    pub fn diff(&self, other: &Dictionary) -> String {
        let mut out = String::new();
        let (mut a, mut b) = (
            self.tables.iter().peekable(),
            other.tables.iter().peekable(),
        );
        loop {
            let order = match (a.peek(), b.peek()) {
                (None, None) => break,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(x), Some(y)) => cmp_upper(&x.name, &y.name),
            };
            match order {
                std::cmp::Ordering::Less => {
                    let t = a.next().unwrap();
                    out.push_str(&format!("- {}\t{}\n", t.name, clean(&t.description)));
                }
                std::cmp::Ordering::Greater => {
                    let t = b.next().unwrap();
                    out.push_str(&format!("+ {}\t{}\n", t.name, clean(&t.description)));
                }
                std::cmp::Ordering::Equal => {
                    let (x, y) = (a.next().unwrap(), b.next().unwrap());
                    for f in &x.fields {
                        match y
                            .fields
                            .iter()
                            .find(|g| g.name.eq_ignore_ascii_case(&f.name))
                        {
                            None => out.push_str(&format!("- {}.{}\t{}\n", x.name, f.name, f.ty)),
                            Some(g) if g.ty != f.ty => out.push_str(&format!(
                                "~ {}.{}\t{} -> {}\n",
                                x.name, f.name, f.ty, g.ty
                            )),
                            Some(_) => {}
                        }
                    }
                    for g in &y.fields {
                        if !x
                            .fields
                            .iter()
                            .any(|f| f.name.eq_ignore_ascii_case(&g.name))
                        {
                            out.push_str(&format!("+ {}.{}\t{}\n", y.name, g.name, g.ty));
                        }
                    }
                }
            }
        }
        out
    }

    /// Texto com uma linha por tabela e uma por campo, separadas por tabulação. A linha da tabela
    /// tem o nome e a descrição, e a do campo `TABELA.Campo`, o tipo e a descrição.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for table in &self.tables {
            out.push_str(&format!("{}\t{}\n", table.name, clean(&table.description)));
            for field in &table.fields {
                out.push_str(&format!(
                    "{}.{}\t{}\t{}\n",
                    table.name,
                    field.name,
                    field.ty,
                    clean(&field.description)
                ));
            }
        }
        out
    }
}

fn cmp_upper(a: &str, b: &str) -> std::cmp::Ordering {
    a.bytes()
        .map(|c| c.to_ascii_uppercase())
        .cmp(b.bytes().map(|c| c.to_ascii_uppercase()))
}

/// Tira tabulação e quebra de linha, que separam colunas e linhas no texto.
fn clean(text: &str) -> String {
    text.split(['\t', '\r', '\n'])
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
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
        // Tipo dos valores de cada enumeração, pelo nome dela em maiúsculas.
        let mut bases = std::collections::HashMap::new();
        r.list_begin()?;
        while !r.end_of_list()? {
            let (name, base) = self.enumeration(r)?;
            if let Some(name) = name {
                bases.insert(name.to_uppercase(), base);
            }
        }
        r.list_end()?;
        let mut tables = Vec::new();
        r.list_begin()?;
        while !r.end_of_list()? {
            let mut table = self.table(r)?;
            for field in &mut table.fields {
                let base = field
                    .ty
                    .enumeration
                    .as_ref()
                    .and_then(|name| bases.get(&name.to_uppercase()));
                if let Some(base) = base {
                    field.ty.base = Some(base.kind);
                    field.ty.size = base.size;
                    field.ty.decimals = base.decimals;
                }
            }
            tables.push(table);
        }
        r.list_end()?;
        tables.sort_by_cached_key(|t| t.name.to_ascii_uppercase());
        Ok(Dictionary { tables })
    }

    /// Enumeração, com o nome que os campos citam e o tipo dos valores.
    fn enumeration(&self, r: &mut Reader) -> Result<(Option<String>, FieldType)> {
        let name = self.field_type(r.integer()?).enumeration;
        r.integer()?;
        let base = self.field_type(r.integer()?);
        r.list_begin()?;
        while !r.end_of_list()? {
            r.integer()?;
            if self.version >= 0x1998_1124 {
                r.integer()?;
            } else {
                r.string()?;
            }
        }
        r.list_end()?;
        if self.version >= 0x2003_0919 {
            r.integer()?;
        }
        Ok((name, base))
    }

    fn table(&self, r: &mut Reader) -> Result<Table> {
        let name = self.text(r.integer()?);
        let mut fields = Vec::new();
        r.list_begin()?;
        while !r.end_of_list()? {
            fields.push(self.field(r)?);
        }
        r.list_end()?;
        // Restrições únicas e índices, cada um com o nome e a lista dos campos.
        r.list(|r| {
            r.integer()?;
            r.list(Reader::integer)
        })?;
        r.list(|r| {
            r.integer()?;
            r.list(|r| {
                r.integer()?;
                if self.version >= 0x2025_0220 {
                    r.integer()
                } else {
                    r.boolean().map(i32::from)
                }
            })?;
            r.boolean()
        })?;
        r.string()?;
        if self.version >= 0x1998_1109 {
            r.list(|r| self.trigger(r))?;
        }
        if self.version >= 0x2009_0730 {
            r.boolean()?;
        }
        if self.version >= 0x2011_1128 {
            r.list(|r| {
                r.integer()?;
                r.string()
            })?;
        }
        if self.version >= 0x2018_0912 {
            r.boolean()?;
        }
        let description = self.text(r.integer()?);
        r.integer()?;
        if self.extra >= 0x2009_0731 {
            r.boolean()?;
        }
        for _ in &fields {
            r.string()?;
        }
        Ok(Table {
            name,
            description,
            fields,
        })
    }

    fn trigger(&self, r: &mut Reader) -> Result<()> {
        r.integer()?;
        r.raw(1)?;
        r.boolean()?;
        if self.version >= 0x2011_0228 {
            r.list(|r| {
                r.boolean()?;
                r.string()?;
                r.string()
            })?;
            r.boolean()?;
        } else {
            r.string()?;
        }
        Ok(())
    }

    fn field(&self, r: &mut Reader) -> Result<Field> {
        let name = self.text(r.integer()?);
        let ty = self.field_type(r.integer()?);
        r.boolean()?;
        if self.version >= 0x1998_1109 {
            r.raw(8)?;
        }
        // Informações extras. A primeira é a descrição.
        let description = self.text(r.integer()?);
        for _ in 0..4 {
            r.integer()?;
        }
        if self.extra < 0x1998_1113 {
            return Err(r.error("versão de dicionário antiga demais".to_owned()));
        }
        r.raw(1)?;
        if self.extra >= 0x2001_0421 {
            r.raw(1)?;
        }
        if self.extra >= 0x1998_1123 {
            r.integer()?;
        } else {
            r.string()?;
        }
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
                    .chunks_exact(2)
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
        w.end();
        w.begin().end().begin().end().str("0");
        w.begin().end().bool(false).begin().end().bool(false);
        w.small(2).small(0).bool(false).str("").str("");
        w.end().end();
        w.0
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
        assert_eq!(
            dict.to_text(),
            "E070EMP\tEmpresas\n\
             E070EMP.CodEmp\tnumero 4\tCódigo da empresa\n\
             E070EMP.SitEmp\talfa 1 LSitEmp\tSituação\n"
        );
    }

    #[test]
    fn busca_e_compara() {
        let dict = Dictionary::read(&sample()).unwrap();
        assert_eq!(dict.field("e070emp", "CODEMP").unwrap().name, "CodEmp");
        assert!(dict.field("E070EMP", "Outro").is_none());
        assert!(dict.table("E999XXX").is_none());

        let mut other = dict.clone();
        other.tables[0].fields.remove(1);
        other.tables[0].fields[0].ty.size = 5;
        other.tables.push(Table {
            name: "USU_TNOVA".to_owned(),
            description: "Nova".to_owned(),
            fields: Vec::new(),
        });
        assert_eq!(
            dict.diff(&other),
            "~ E070EMP.CodEmp\tnumero 4 -> numero 5\n\
             - E070EMP.SitEmp\talfa 1 LSitEmp\n\
             + USU_TNOVA\tNova\n"
        );
    }

    #[test]
    fn arquivo_cortado_da_erro_sem_panico() {
        let bytes = sample();
        // A leitura para depois das tabelas, então só o último byte, que fecha a lista de bancos,
        // pode faltar.
        for n in 0..bytes.len() - 1 {
            assert!(Dictionary::read(&bytes[..n]).is_err(), "cortado em {n}");
        }
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
