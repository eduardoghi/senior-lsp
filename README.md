# senior-lsp

Servidor de linguagem ([Language Server Protocol](https://microsoft.github.io/language-server-protocol/))
para as regras LSP do ERP Senior, como o gerenciador de artefatos exporta. Ele aponta no editor os
erros que o compilador do ERP daria, sem precisar compilar a regra no ERP, e ajuda a navegar em
regras grandes.

O servidor oferece:

- erros de sintaxe e de tipo, com o texto do compilador entre parênteses para quem já conhece a
  mensagem do ERP
- avisos de código que compila mas faz outra coisa do que parece
- ir para a definição e achar as referências de variáveis, funções, parâmetros e rótulos
- destacar as outras ocorrências do nome que está no cursor
- hover com a linha do `definir` e, com o dicionário de dados do ERP configurado, o tipo, a
  descrição, as chaves, os relacionamentos e a busca dos campos de tabela
- as funções que o ERP já traz, como `IntParaAlfa`, com a declaração no hover e a conferência dos
  parâmetros na chamada
- ajuda de assinatura, com a declaração da função e o parâmetro atual enquanto se digita a chamada
- destaque semântico, que colore cada nome pelo que ele é: função do ERP ou da regra, variável,
  parâmetro, campo depois do ponto, tabela e web service
- a lista de variáveis e funções do arquivo
- completar nomes num editor, com as variáveis e funções do arquivo, as do ERP e as palavras da
  linguagem
- renomear num editor, trocando só o símbolo certo, nos outros arquivos do formulário SGI inclusive

A exportação guarda o código de cada regra num arquivo `Rule.txt` ou `rule.txt`, menos nos modelos
de relatório, em que as regras ficam dentro dos JSON de cada modelo. A pasta diz onde a regra roda,
e isso muda as funções do ERP que ela pode chamar:

| Pasta | Arquivo | Regra |
|---|---|---|
| `Regras` | `Rule.txt` | regra comum, como a de identificador, com as funções comuns do ERP |
| `Formulários SGI` | `rule.txt` | regra de tela SGI, que ganha as funções de tela, os campos da tela e as funções e variáveis definidas nas regras que vêm antes dela |
| `Web Services` | `Rule.txt` | regra de web service, com as mesmas funções da regra comum |
| `Modelos` | os `.json` de cada modelo | regra de relatório, que ganha as funções de relatório, as entradas e os controles do modelo e as funções e variáveis definidas nas outras regras dele |

O servidor reconhece as pastas `Formulários SGI` e `Modelos`. Qualquer outro arquivo de regra conta
como regra comum, então renomear a pasta `Regras` não muda nada. Os outros tipos de
artefato não exportam regra. Num modelo de relatório, as referências e o renomear atravessam todas
as regras e todos os JSON do modelo. Antes das regras, o ERP define as entradas, como `ECodFil`, com o
tipo do cadastro, e as descrições como alfa. A alfa de um caractere vira número com o código do
caractere. Os totais valem como números, e as fórmulas também, porque o compilador cria como número
o nome sem `definir`. Memorando e código de barras imprimem a variável de mesmo nome que a regra
define. O nome das entradas e desses controles não se renomeia pela regra, porque vem do cadastro
do modelo.

### Arquivos fora da exportação

Um arquivo com outro nome, como um texto novo ainda sem salvar, é analisado quando o editor manda
a linguagem `lsp-senior`. O VS Code e o Kate mandam quando o arquivo está na linguagem LSP Senior,
escolhida à mão ou pelo nome. O plugin do Claude Code associa o servidor pela extensão `.txt`, então
lá só o nome `Rule.txt` vale.

Fora da pasta `Formulários SGI`, o tipo da regra pode ser trocado por arquivo. O `Rule.txt` começa
como regra comum e os outros arquivos começam com o tipo da opção `tipo_padrao`
(`comum`, `tela` ou `relatorio`), passada nas opções de inicialização. As ações de código do
arquivo trocam o tipo em qualquer editor, pelo comando `senior-lsp.tipoRegra`, que recebe o URI e o
tipo. O editor pergunta o tipo atual pelo pedido `senior-lsp/tipoRegra`, com o URI, e recebe o tipo
e se ele vem da pasta (`fixo`).

Num arquivo solto, a regra de tela ganha as funções de tela, mas não os campos da interface, que
vêm do `definition.json` da exportação. A de relatório ganha as funções de relatório, sem as
entradas e os controles do modelo. Os nomes que faltam valem como número, como qualquer nome sem
`definir`.

Numa tela SGI, o ERP compila todas as regras da interface juntas. Primeiro vem a regra da
interface, depois as ações da interface e, para cada tabela, a regra dela seguida das ações dela. O que uma regra define vale
nas seguintes. Tabelas e ações da interface vão em ordem alfabética, como a importação do
gerenciador grava, e as ações de cada tabela na ordem do `definition.json` dela. Os campos da tela
valem em todas as regras. Campo da interface fica com o nome do cadastro, campo de tabela com a
tabela na frente (`E160CTR_CodEmp`) e também com `_old` no fim, e `CodOpe`, `MsgErro`, `MsgAviso`
e `TelOpe` existem em toda tela.

Os nomes seguem as regras do compilador. Eles não diferenciam caixa, e um `definir` vale no
arquivo inteiro, inclusive antes dele e dentro de funções. O compilador cria como número o nome
sem `definir`, depois de procurar nas variáveis do sistema, como `DatSis` e `NomUsu`, que valem em
toda regra com o tipo delas. Na tela SGI e no relatório o servidor também trata o nome desconhecido
como número. Na regra de identificador ele fica sem verificação de tipo, porque cada identificador
traz variáveis próprias que o servidor não conhece.

## Compilar

Precisa de Rust 1.99 ou mais novo.

```sh
cargo build --release
```

O executável fica em `target/release/senior-lsp` e conversa pelo stdin e stdout, como qualquer
servidor LSP.

## Instalar no Claude Code

```sh
claude plugin marketplace add eduardoghi/senior-lsp
claude plugin install senior-lsp@senior-lsp
```

Numa sessão aberta, `/reload-plugins --force` carrega o plugin. Na primeira vez que o servidor
inicia, o plugin baixa o binário da versão instalada do release do GitHub, confere o SHA-256 e
guarda na pasta de dados do plugin. Há binário pronto para Linux x86_64 e para Windows x86_64,
que o plugin baixa quando o Claude Code roda o script pelo Git Bash. Em outro sistema, instale a
partir de um clone.

### A partir de um clone

Com o binário compilado, o plugin usa o `target/release` do clone em vez de baixar:

```sh
cargo build --release
claude plugin marketplace add "$PWD"
claude plugin install senior-lsp@senior-lsp
```

O Claude Code lê o plugin direto desta pasta, então depois de um `cargo build --release` basta
recarregar.

## Usar no VS Code

A extensão fica em `plugin/vscode`. Ela abre os `Rule.txt` e `rule.txt` na linguagem LSP Senior,
com destaque de sintaxe, e manda também os JSON da pasta `Modelos` para o servidor, sem mudar a
linguagem deles. Na primeira vez, baixa o binário do release da mesma versão da extensão e confere
o SHA-256, como o plugin do Claude Code.

Para gerar e instalar o pacote:

```sh
cd plugin/vscode
npm ci
npm run compile
npm run package
code --install-extension senior-lsp-*.vsix
```

As configurações são `seniorLsp.server.path`, para usar outro executável, `seniorLsp.tipoPadrao`,
com o tipo dos [arquivos fora da exportação](#arquivos-fora-da-exportação), e `seniorLsp.tbsProducao`
e `seniorLsp.tbsTeste`, com os dicionários descritos em [Dicionário de dados](#dicionário-de-dados).
Mudar qualquer uma reinicia o servidor.

Num arquivo fora da exportação, a barra de status mostra o tipo da regra e troca ao clicar, como o
comando "Senior LSP: Escolher o tipo da regra". A extensão guarda a escolha de cada arquivo no
workspace. Um arquivo novo analisado precisa estar na linguagem LSP Senior, que se escolhe no
canto da barra de status.

Para desenvolver, abra `plugin/vscode` no VS Code e rode com F5. Rodando de dentro do clone, a
extensão usa o `target/release` em vez de baixar.

## Usar no Kate

O cliente LSP do Kate escolhe o servidor pelo modo de destaque do documento, e não pela extensão.
A definição de sintaxe em `plugin/kate/lsp-senior.xml` cria o modo "LSP Senior" para os arquivos
`Rule.txt` e `rule.txt` e colore as palavras reservadas, os comentários e os textos. Copie para a pasta de
sintaxes do usuário:

```sh
mkdir -p ~/.local/share/org.kde.syntax-highlighting/syntax
cp plugin/kate/lsp-senior.xml ~/.local/share/org.kde.syntax-highlighting/syntax/
```

No Windows, a pasta é `%USERPROFILE%\AppData\Local\org.kde.syntax-highlighting\syntax`.

Depois, em Configurações → Configurar o Kate → Cliente LSP → Configurações do servidor do
usuário, acrescente o servidor:

```json
{
    "servers": {
        "lsp-senior": {
            "command": ["/caminho/do/senior-lsp"],
            "url": "https://github.com/eduardoghi/senior-lsp",
            "highlightingModeRegex": "^LSP Senior$",
            "initializationOptions": {
                "tbs_producao": "/caminho/do/sapiens.tbs",
                "tbs_teste": "/caminho/do/sapiens-teste.tbs"
            }
        }
    }
}
```

O `command` aceita só o nome (`senior-lsp`) quando o executável está no `PATH`. As duas opções de
`initializationOptions` são os dicionários descritos em [Dicionário de dados](#dicionário-de-dados)
e podem ficar de fora, assim como `tipo_padrao`. Os JSON dos modelos de relatório não passam pelo
servidor no Kate, porque ligar o servidor no modo JSON faria ele abrir todos os arquivos JSON.

Qualquer arquivo no modo LSP Senior vai para o servidor. Num arquivo novo, escolha Ferramentas →
Modo → Fontes → LSP Senior. O tipo da regra troca pelas ações de código do cliente LSP. As
mensagens do servidor aparecem na aba Saída do cliente LSP.

## Diagnósticos

Cada diagnóstico tem um código fixo.

| Código | Tipo | Caso |
|---|---|---|
| SEN001 | erro | caractere inválido |
| SEN002 | erro | string sem as aspas de fechamento |
| SEN003 | erro | comentário `@` sem o `@` de fechamento na mesma linha |
| SEN004 | erro | comentário `/*` sem o `*/` |
| SEN005 | erro | nome com mais de 100 caracteres |
| SEN006 | erro | apóstrofo sem o de fechamento |
| SEN007 | erro | número que o compilador não aceita |
| SEN008 | erro | linha maior do que o compilador aceita |
| SEN020 | erro | falta um token, como `;` ou `)` |
| SEN021 | erro | falta um valor |
| SEN022 | erro | mais de uma comparação na mesma expressão |
| SEN023 | erro | algo que não começa um comando, como um `;` solto |
| SEN024 | erro | fim de bloco sem início |
| SEN025 | erro | aninhamento grande demais |
| SEN026 | erro | falta o tipo num `definir` ou parâmetro |
| SEN027 | erro | tipo do `Mensagem` que não é `Erro`, `Refaz` nem `Retorna` |
| SEN028 | erro | texto do `Mensagem` que não é string nem variável |
| SEN040 | erro | `-` com mais de um token depois de `*`, `/` ou `e` |
| SEN041 | erro | comparação sem parênteses no lado direito da atribuição |
| SEN042 | erro | variável numérica ou data no texto do `Mensagem` |
| SEN043 | erro | número ou parênteses em atribuição alfa |
| SEN044 | erro | operador que não é `+` em atribuição alfa |
| SEN045 | erro | alfa em conta numérica ou comparado com número |
| SEN046 | erro | duas strings comparadas |
| SEN047 | erro | alfa em parâmetro numérico |
| SEN048 | erro | parâmetros a mais na chamada |
| SEN049 | erro | parâmetros a menos na chamada |
| SEN050 | erro | número em parâmetro alfa |
| SEN051 | erro | `+` entre alfas dentro de uma comparação |
| SEN052 | erro | chamada a uma função que não existe no ERP nem no arquivo |
| SEN053 | erro | constante num parâmetro `end` |
| SEN054 | erro | constante entre as colunas do SELECT de um cursor |
| SEN055 | erro | campo do cursor que não está no SELECT dele |
| SEN060 | aviso | `e` ou `ou` seguido de `-`, que não grava o valor da atribuição |
| SEN061 | aviso | comando depois do bloco de uma função, que ainda faz parte dela |
| SEN062 | aviso | `funcao` com um bloco de outra função ainda aberto |
| SEN063 | aviso | parâmetro usado fora da sua função, que lê lixo |
| SEN064 | aviso | apóstrofo com letra minúscula, que o compilador lê em maiúscula |
| SEN065 | aviso | campo de tabela que só existe no dicionário de teste |
| SEN066 | aviso | SQL nativo sem `SQL_UsarAbrangencia(x, 0)`, que para na execução |
| SEN067 | aviso | coluna do `SQL_Retornar*` que não está no SELECT, que para na execução |
| SEN068 | dica | variável, cursor, lista, tabela ou web service definido e nunca usado, só na regra comum |

O SEN068 sai como dica, que o editor mostra esmaecendo o nome. Um nome citado dentro de um texto
conta como usado, porque o SQL lê a variável como `:nome` e algumas regras passam o nome da
variável num texto para uma função preencher. Na tela e no relatório o aviso não sai, porque as
regras seguintes podem usar o nome.

## Dicionário de dados

O ERP guarda as tabelas do banco, com o tipo e a descrição de cada campo, no arquivo `.tbs` (como
o `sapiens.tbs` da pasta do ERP). Com ele configurado, o servidor confere o tipo de `TABELA.Campo`
nas regras e dá tipo ao campo de busca da tela SGI, pela tabela de destino da ligação. O campo alfa
de um caractere fica sem tipo, porque o compilador aceita ele como texto e como número.

São dois caminhos, ambos opcionais, o `.tbs` de produção e o da base de teste. O tipo vem do de
produção. Com os dois, o servidor avisa (SEN065) quando a regra usa um campo que só existe no de
teste, que compila na base de teste e não em produção. No Claude Code, os caminhos são opções do
plugin, perguntadas na instalação e alteradas depois pelo `/config` ou pelo
`/plugin configure senior-lsp@<marketplace>`. Em outros editores, vão em `tbs_producao` e
`tbs_teste` nas opções de inicialização do servidor, ou nas variáveis `SENIOR_LSP_TBS_PRODUCAO` e
`SENIOR_LSP_TBS_TESTE`. Cada arquivo é lido na primeira consulta e de novo só quando muda.

O executável também imprime as tabelas e os campos em texto, uma linha por tabela e uma por
campo, separadas por tabulação:

```sh
senior-lsp tbs sapiens.tbs > tabelas.txt
```

```text
E070EMP	Cadastros - Empresas
E070EMP.CodEmp	numero 4	Código da empresa
E070EMP.NomEmp	alfa 100	Nome da empresa
E070EMP.EstLef	alfa 1 LSimNao	Indicativo se lança os movimentos de estoque no plano financeiro
```

Campo de enumeração sai com o tipo dos valores e o nome da enumeração. Com `--pasta`, o dicionário
inteiro vai para arquivos de texto numa pasta vazia ou que ainda não existe, para versionar:

```sh
senior-lsp tbs sapiens.tbs --pasta dicionario
```

| Arquivo | Uma linha por |
| --- | --- |
| `tabelas.txt` | tabela e campo, como no exemplo acima |
| `enumeracoes.txt` | enumeração (tipo dos valores) e valor (descrição) |
| `indices.txt` | chave primária, colunas únicas e índice, com o tipo e os campos |
| `relacionamentos.txt` | relacionamento, com a tabela e os campos de origem e de destino |
| `check-constraints.txt` | check constraint, com a condição |
| `ligacoes.txt` | campo com busca, com a tabela ou view, os campos e o campo mostrado |
| `views.txt` | view e campo da view |

O código fica em `triggers/TABELA.TRIGGER.banco.sql`, `stored-procedures/NOME.banco.sql` e
`views/NOME.sql`. Os nomes seguem os do CBDS. O banco vem da versão do código no dicionário, como `senior` para a genérica e
`oracle` para a do Oracle. A primeira coluna dos arquivos `.txt` identifica o item.

Com dois arquivos `.tbs`, o executável lista as diferenças do primeiro para o segundo, agrupadas por
arquivo. A linha começa com `-` para o que só existe no primeiro, `+` para o que só existe no segundo
e `~` para o item que mudou, com o valor de antes e o de depois. Do código sai só o caminho do arquivo
que mudou:

```sh
senior-lsp tbs producao.tbs teste.tbs
```

```text
tabelas.txt
~ E070EMP.CodEmp	numero 4	Código da empresa -> numero 5	Código da empresa
+ USU_TNOVA	Nova

código
+ stored-procedures/CLEARREPLICATION.oracle.sql
```

## Licença

[MIT](LICENSE)
