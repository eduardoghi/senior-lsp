# Regras da exportação

A exportação guarda o código de cada regra num arquivo `Rule.txt` ou `rule.txt`, menos nos modelos
de relatório, em que as regras ficam dentro dos JSON de cada modelo. A pasta diz onde a regra roda,
e isso muda as funções do ERP que ela pode chamar:

| Pasta | Arquivo | Regra |
|---|---|---|
| `Regras` | `Rule.txt` | regra comum, como a de identificador, com as funções comuns do ERP |
| `Formulários SGI` | `rule.txt` | regra de tela SGI, que ganha as funções de tela, os campos da tela e as funções e variáveis definidas nas regras que vêm antes dela |
| `Web Services` | `Rule.txt` | regra de web service, com as mesmas funções da regra comum |
| `Modelos` | os `.json` de cada modelo | regra de relatório, que ganha as funções de relatório, as entradas e os controles do modelo e as funções e variáveis definidas nas outras regras dele |

O código de uma regra de relatório também pode ficar fora do JSON, num arquivo `Modelos/<modelo>/regras/<json>.<evento>.txt`, com o `lines` da regra no JSON apontando para ele (`"lines": "regras/..."`). Esse formato serve para versionar os relatórios com o código das regras em texto. O servidor trata esse arquivo como regra de relatório, com as entradas e os controles do modelo e as funções e variáveis das outras regras dele, estejam elas em arquivo separado ou dentro de um JSON.

O servidor reconhece as pastas `Formulários SGI` e `Modelos`. Qualquer outro arquivo de regra conta
como regra comum, então renomear a pasta `Regras` não muda nada. Os outros tipos de
artefato não exportam regra. Num modelo de relatório, as referências e o renomear atravessam todas
as regras e todos os JSON do modelo. Antes das regras, o ERP define as entradas, como `ECodFil`, com o
tipo do cadastro, e as descrições como alfa. A alfa de um caractere vira número com o código do
caractere. Os totais valem como números, e as fórmulas também, porque o compilador cria como número
o nome sem `definir`. Memorando e código de barras imprimem a variável de mesmo nome que a regra
define. O nome das entradas e desses controles não se renomeia pela regra, porque vem do cadastro
do modelo.

## Arquivos fora da exportação

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
