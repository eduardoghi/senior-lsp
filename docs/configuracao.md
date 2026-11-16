# Configuração

## Arquivo de configuração

O `senior-lsp.json` muda o nível de cada código para uma pasta inteira, lista as tabelas que existem no banco fora do dicionário de dados e escolhe o que a formatação acerta. O servidor procura o arquivo subindo a partir da pasta do arquivo analisado, como o `.editorconfig`, e usa o primeiro que achar, sem juntar com os de cima.

```json
{
    "diagnostics": {
        "SEN048": "off",
        "SEN071": "hint"
    },
    "overrides": [
        { "files": "Modelos/**", "diagnostics": { "SEN048": "off" } }
    ],
    "extraTables": ["TB_CRIADA_NO_BANCO"],
    "format": { "spaces": false }
}
```

- Os níveis são `off` (o diagnóstico some), `hint`, `info`, `warning` e `error`. Um código fora do arquivo fica como o servidor o emite.
- Os `overrides` valem em ordem, e o último que casa com o arquivo vence, código a código. O `files` é um glob relativo à pasta do `senior-lsp.json`, em texto ou em lista. Ele não diferencia maiúsculas, o `*` não atravessa pastas, o `**` atravessa e um nome de pasta com barra no fim (`Modelos/`) vale para tudo o que há dentro dela.
- Os erros de sintaxe não mudam de nível.
- O `format.spaces` com `false` deixa a formatação só com o recuo, sem mexer no espaço dentro das linhas. Sem a chave, os dois mudam.
- O `extraTables` lista tabelas e visões criadas direto no banco, que o SEN086 não cobra no SQL nativo. A correção rápida do SEN086 acrescenta a tabela ao arquivo que vale para a regra e grava no disco. Sem arquivo, cria um na pasta do workspace que tem a regra. Os nomes não diferenciam maiúsculas. A lista não vale para o Senior SQL 2 nem para o SELECT do cursor, que procuram a tabela no dicionário. Uma diretiva que só escondia o SEN086 de uma tabela da lista passa a sair como SEN073.
- Um JSON inválido faz o servidor ignorar o arquivo e mostrar um aviso (`window/showMessage`), e a análise segue sem configuração. Chave desconhecida, código que não existe e nível inválido também viram aviso, e o resto do arquivo vale.
- A configuração vale nos arquivos abertos e na análise da pasta inteira. Quando o editor avisa que um `senior-lsp.json` mudou (o VS Code avisa), o servidor lê de novo e publica os diagnósticos outra vez. Nos editores que não avisam, como o Kate, a leitura é refeita ao abrir qualquer arquivo.

No VS Code, a extensão traz o esquema do arquivo, então o editor completa os códigos e os níveis.

## Pasta inteira

Com a opção `analisar_pasta` ligada, o servidor procura ao iniciar todas as regras das pastas do
workspace, os `Rule.txt` e os JSON de `Modelos`, e publica os diagnósticos de todas, sem precisar
abrir cada arquivo. Só vão os arquivos com algum diagnóstico. No editor que mostra progresso
(`window.workDoneProgress`), como o VS Code, a análise aparece com a contagem de arquivos. Quando o editor avisa que um arquivo
mudou no disco (`workspace/didChangeWatchedFiles`), ele é analisado de novo, e um arquivo fechado
volta a mostrar os diagnósticos do disco. A análise usa até quatro threads. Numa exportação com
cerca de 1.400 regras e 24 mil JSON de modelo, a primeira leva menos de um segundo.

A opção fica desligada por padrão porque o Claude Code coloca os diagnósticos na conversa. No VS
Code ela é a configuração `seniorLsp.analisarPasta`.

## Dicionário de dados

O ERP guarda as tabelas do banco, com o tipo e a descrição de cada campo, no arquivo `.tbs` (como
o `sapiens.tbs` da pasta do ERP). Com ele configurado, o servidor confere o tipo de `TABELA.Campo`
nas regras e dá tipo ao campo de busca da tela SGI, pela tabela de destino da ligação. O campo alfa
de um caractere fica sem tipo, porque o compilador aceita ele como texto e como número.

O SELECT de um cursor (`cur.SQL "..."`) também é conferido com o dicionário, do jeito que o
compilador confere ao compilar a regra. Ele lê o que vem antes do primeiro ponto de cada coluna
como tabela ou visão, e por isso recusa apelido de tabela e função (SEN074). A coluna sem tabela é
procurada na primeira tabela do FROM (SEN075).

O compilador confere mais quando a regra usa o Senior SQL 2. Ele não aceita JOIN (SEN080), cobra
que as tabelas do FROM existam (SEN081), procura nelas cada nome do WHERE (SEN082) e só aceita no
ORDER BY o que está no SELECT (SEN083). No `ExecSql "..."` e no `ExecSqlEx("...", ...)` com o
comando escrito na regra, ele cobra a tabela, os campos do SET e do INSERT, os nomes do WHERE e
dos valores e a quantidade de valores do INSERT (SEN084), e lê o SELECT como o do cursor. No SQL
nativo nada disso é conferido na compilação. O
servidor sabe o modo da regra pela opção `useSQLSenior2` do JSON que a exportação grava ao lado do
texto dela (`General_Infos.json` na regra, `Rule.json` no web service e `definition.json` na tela
SGI), e a do modelo de relatório pela opção `sql_senior2` do `Base_Structure.json` dele. Sem
esse arquivo, como numa regra solta, essas conferências não saem.

São dois caminhos, ambos opcionais, o `.tbs` de produção e o da base de teste. O tipo vem do de
produção. Com os dois, o servidor avisa (SEN065) quando a regra usa um campo que só existe no de
teste, que compila na base de teste e não em produção. No Claude Code, os caminhos são opções do
plugin, perguntadas na instalação e alteradas depois pelo `/config` ou pelo
`/plugin configure senior-lsp@<marketplace>`. Em outros editores, vão em `tbs_producao` e
`tbs_teste` nas opções de inicialização do servidor, ou nas variáveis `SENIOR_LSP_TBS_PRODUCAO` e
`SENIOR_LSP_TBS_TESTE`. Cada arquivo é lido na primeira consulta, e a data dele é conferida a cada 5
minutos fora da digitação, para ler de novo o que mudou. Depois de uma atualização do ERP ou de uma
customização no CBDS, o comando `senior-lsp.recarregarDicionario` lê os dois na hora. No VS Code ele
é o "Senior LSP: Recarregar o dicionário de dados" da paleta de comandos.

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
