# Diagnósticos

Cada diagnóstico tem um código fixo. O SEN061 e o SEN063 trazem também o outro lugar envolvido (a `funcao` a que o comando pertence e o parâmetro na função dele), que o editor mostra junto da mensagem.

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
| SEN069 | aviso | membro de web service que não é parâmetro da porta no `Port.json` da exportação, nem um dos implícitos (`Usuario`, `Senha`, `Token`, `ModoExecucao`, `ErroExecucao`) |
| SEN070 | dica | função definida e nunca chamada, só na regra comum |
| SEN071 | aviso | instruções depois de `Pare`, `Continue`, `Cancel`, `Cancelar` ou `VaPara` no mesmo bloco, que nunca executam |
| SEN072 | aviso | diretiva `senior-lsp-*` mal escrita, como sem código, com código que não existe ou de erro de sintaxe, e `senior-lsp-enable` sem `senior-lsp-disable` antes |
| SEN073 | aviso | diretiva que não escondeu nenhum diagnóstico do código citado |
| SEN074 | erro | coluna do SELECT de um cursor com uma tabela que não está no dicionário de dados, como um apelido de tabela ou uma função |
| SEN075 | erro | coluna do SELECT de um cursor com um campo que a tabela não tem no dicionário de dados |
| SEN076 | dica | coluna do SELECT de um cursor que a regra não lê, ou `*` de que ela só lê parte dos campos, só na regra comum |
| SEN077 | erro | coluna do SELECT de um cursor com apelido ou `DISTINCT` e sem a tabela na frente |
| SEN078 | erro | o mesmo nome de campo vindo de duas tabelas no SELECT de um cursor |
| SEN079 | erro | `:variavel` no SQL de um cursor ou de um `ExecSql` que ainda não foi definida nem usada na regra, só na regra comum |
| SEN080 | erro | JOIN no SQL de um cursor, na regra que usa o Senior SQL 2 |
| SEN081 | erro | tabela do SQL de um cursor, de um `ExecSql` ou de um `ExecSqlEx` que não está no dicionário de dados, na regra que usa o Senior SQL 2 |
| SEN082 | erro | nome do WHERE, do valor do SET, dos valores do INSERT ou das colunas do SELECT do `ExecSql` que não é campo das tabelas do comando, ou que mais de uma delas tem, e campo do SET, do INSERT ou do GROUP BY que a tabela não tem, na regra que usa o Senior SQL 2 |
| SEN083 | erro | campo do ORDER BY de um cursor ou de um SELECT no `ExecSql` que não está no SELECT, na regra que usa o Senior SQL 2 |
| SEN084 | erro | INSERT com a quantidade de valores diferente da de campos, na regra que usa o Senior SQL 2 |
| SEN085 | aviso | `enquanto ()` com a condição vazia, que o compilador trata como verdadeira, sem `Pare`, `VaPara`, `Cancel` nem outra saída no bloco |
| SEN086 | aviso | tabela do SQL nativo que não está no dicionário de dados, fora as do catálogo do Oracle, a `DUAL`, os nomes do `WITH` e as do `extraTables` do [`senior-lsp.json`](configuracao.md#arquivo-de-configuração) |
| SEN087 | aviso | campo com a tabela ou o apelido na frente, no SQL nativo, que a tabela não tem no dicionário de dados |
| SEN088 | aviso | o que o Senior SQL 2 recusa (SEN080 a SEN083) no SELECT de um `SQL_DefinirComando`, que compila e para na execução. O objeto usa o Senior SQL 2 até receber `SQL_UsarSQLSenior2(x, 0)`, também na regra que não usa a opção. Só vale para o objeto criado com `SQL_Criar` no mesmo arquivo, porque o de outra regra da tela pode ter recebido o SQL nativo lá |
| SEN089 | erro | `Pare` ou `Continue` fora de um `para` ou `enquanto` |
| SEN090 | aviso | `Continue` num `para`, que volta sem rodar o incremento, e o laço não termina se nada mais muda o contador |
| SEN091 | dica | parâmetro da `funcao` que o corpo não usa, com a ação "Tirar o parâmetro" |
| SEN092 | erro | `Definir Tabela` sem o `[tamanho]` depois do nome ou sem nenhuma coluna entre as chaves |
| SEN093 | erro | coluna de tabela sem o índice da linha (`t.Codigo`), coluna numérica com tamanho sem o segundo índice (`t[1].v` com `Numero v[2]`) ou coluna que a tabela não tem |
| SEN094 | erro | índice literal fora da faixa de uma tabela, de uma variável numérica ou de uma coluna |
| SEN095 | erro | uso inválido de comando do cursor, como `AbrirCursor(1)`, `Achou()`, `UsaAbrangencia()`, `UsaTraducao` no SQL nativo ou atribuição a um campo do cursor |
| SEN096 | erro | uso inválido de lista, como membro que não existe, função sem `()`, propriedade com `()` e `AdicionarCampo` ou `Chave` mal escritos |
| SEN097 | erro | `VaPara` para um nome que não é rótulo do arquivo |
| SEN098 | erro | `funcao` sem um `Definir Funcao` do mesmo nome antes dela |
| SEN099 | erro | chamada a uma função com 32 caracteres ou mais no nome |
| SEN100 | erro | concatenação de alfa num argumento de função |
| SEN101 | erro | `Definir` do mesmo nome com outro tipo |
| SEN102 | aviso | chamada a uma função declarada que nenhuma `funcao` implementa, que para na execução |
| SEN103 | aviso | chamada a uma função com exatamente 31 caracteres no nome, que o ERP não resolve |
| SEN104 | aviso | texto literal num parâmetro alfa de função da regra, que para na execução |
| SEN105 | aviso | índice alfa numa tabela, que vale 0 e para na execução |
| SEN106 | aviso | segundo índice literal que passa do tamanho da coluna, que grava na linha seguinte da tabela |
| SEN107 | aviso | `Definir Funcao` repetido com outra assinatura, que o ERP ignora |
| SEN108 | dica | parâmetro repetido na mesma função, de que o corpo só enxerga o último |
| SEN109 | dica | coluna repetida na `Definir Tabela`, de que o nome acessa só a primeira |
| SEN110 | dica | rótulo repetido, de que o `VaPara` vai para o último |
| SEN111 | dica | dois rótulos com os mesmos 30 primeiros caracteres, que o ERP trata como um só |
| SEN112 | dica | tamanho de tabela acima de 65535, que o ERP guarda em 16 bits |

O SEN068 e o SEN070 saem como dica, que o editor mostra esmaecendo o nome. Um nome citado dentro de um texto
conta como usado, porque o SQL lê a variável como `:nome` e algumas regras passam o nome da
variável num texto para uma função preencher. Na tela e no relatório o aviso não sai, porque as
regras seguintes podem usar o nome.

O SEN076 esmaece no texto do SQL a coluna que a regra não lê pelo cursor (`cur.CAMPO`). O campo que
aparece no ORDER BY conta como lido, porque com o Senior SQL 2 o ORDER BY só aceita o que está no
SELECT. Quando a regra não lê nenhuma coluna, como no cursor que só serve ao `Achou`, a primeira
fica sem aviso, porque o SELECT precisa de pelo menos uma. O `*` é avisado quando a regra lê menos
campos do que a tabela tem, o que precisa do dicionário de dados. Na tela e no relatório o aviso
não sai, pelo mesmo motivo do SEN068.

O SQL nativo vai para o banco como está, e a tabela ou o campo que não existe só para na execução. Ele é o do `SQL_DefinirComando` com `SQL_UsarSQLSenior2(x, 0)` e, na regra que não usa o Senior SQL 2, o do cursor e do `ExecSql`. Na regra sem a opção na exportação o modo não é conhecido, e o SEN086 e o SEN087 não saem no cursor nem no `ExecSql`. No cursor fica de fora o que o SEN074 e o SEN075 já acusaram.

As conferências de tabela, cursor e lista usam a definição que a regra tem. Com o nome vindo de outra
regra da tela, só valem as que não dependem das colunas, do tamanho ou dos campos. A lista de outra regra
não diz os campos, então o SEN096 não acusa o membro dela. O membro desconhecido também só é conferido na
regra comum, porque num modelo de relatório ou numa tela outra regra pode criar o campo com
`AdicionarCampo`. A tabela de outra regra só entra no
`t.coluna` sem índice (SEN093) e no índice alfa (SEN105). O SEN093 e o SEN094 conferem só o índice
escrito como número, porque o ERP não confere variável nem expressão na compilação. A coluna `Alfa
Nome[20]` não pede segundo índice, porque o `[n]` do alfa é o tamanho do texto.

O segundo índice de `t[i].v[k]` é conferido contra o número de linhas da tabela, e não contra o
tamanho da coluna. Com `t[3]` e `Numero v[2]`, o `v[3]` compila e grava na linha seguinte (SEN106),
e o `v[4]` não compila (SEN094). O tamanho da tabela é guardado em 16 bits, então `[65536]` vira 0 e
`[70000]` vira 4464 (SEN112), e o SEN094 usa o tamanho já cortado.

O SEN095 só acusa `UsaTraducao` quando a exportação diz que a regra usa o SQL nativo. Sem a
informação do modo, a regra fica sem aviso.

A coluna da tabela sem nome (`Numero ;`) e a função de lista sem parênteses como comando
(`lst.Primeiro;`) são erros de sintaxe que o parser já acusa, e por isso não têm código próprio aqui.

O SEN097 compara os rótulos pelos 30 primeiros caracteres e sem diferença de caixa, e o nome de uma
função não serve de rótulo. O ERP corta o nome da função em 31 caracteres. Com 32 ou mais a chamada não
é reconhecida e não compila (SEN099), e com exatamente 31 compila e para na execução (SEN103). O
SEN098 compara os 31 primeiros caracteres, e deixa passar a função que a regra da raiz da tela
declara. Numa tela, o SEN102 segue a ordem das regras. A função implementada nesta regra ou numa
anterior não dá aviso, e a que só uma regra seguinte implementa também para na execução, com "Não é
possível determinar a regra atual para o interpretador.", porque a regra seguinte não está carregada.
O SEN100 vale para qualquer
função, do ERP ou da regra, e a conta numérica em argumento numérico compila. O SEN104 não vale nas
funções do ERP, onde o texto literal é normal, e o parâmetro `end` com constante já é o SEN053.

O SEN107 sai na segunda declaração. As chamadas são conferidas contra a primeira, que é a que vale
(SEN048 e SEN049). O `Definir Funcao` repetido igual compila e não sai.

O SEN071 cobre da primeira instrução depois do comando até a última do bloco, e o editor esmaece
o trecho. Um rótulo (`nome:`) no meio do bloco volta a tornar o código alcançável, porque um
`VaPara` pode chegar nele. Um `Pare`, `Continue`, `Cancel`, `Cancelar` ou `VaPara` dentro de um `se` só afeta o resto
do bloco do `se`.

## Esconder diagnósticos

Um comentário com uma diretiva esconde os diagnósticos dos códigos citados. Vale nos formatos `@ ... @` e `/* ... */`, em qualquer tipo de regra, inclusive nas que ficam dentro dos JSON dos modelos de relatório e nos arquivos `regras/*.txt`. A diretiva é a primeira palavra do comentário.

```text
@ senior-lsp-disable-next-line SEN071 @
Mensagem(Retorna, aMsg);

@ senior-lsp-disable SEN048, SEN061 @
...
@ senior-lsp-enable SEN048, SEN061 @
```

- `senior-lsp-disable-next-line` esconde os diagnósticos que começam na próxima linha com código, sem contar as linhas vazias e as que só têm comentário. Um diagnóstico que ocupa várias linhas é escondido quando começa nela.
- `senior-lsp-disable` esconde os diagnósticos que começam depois do comentário, até o `senior-lsp-enable` do mesmo código. Sem `enable`, vale até o fim do arquivo. Numa regra dentro do JSON de um modelo, o fim é o da regra.
- Os códigos são obrigatórios, separados por vírgula ou espaço, e podem estar em maiúscula ou minúscula. Cada código tem o seu `enable`, então `enable SEN048` não religa o `SEN061` de um `disable SEN048, SEN061`.
- O comentário `@ ... @` tem que fechar na mesma linha. O `/* ... */` pode ocupar várias linhas, e os códigos podem quebrar de linha dentro dele.
- Os erros de sintaxe, do SEN001 ao SEN028, não se escondem.

Uma diretiva que não serve para nada vira aviso no próprio comentário.

- SEN072 quando a diretiva não tem código, tem um código que não existe ou que não se pode esconder (erro de sintaxe, SEN072 e SEN073), tem um nome desconhecido (`senior-lsp-` seguido de outra palavra) ou quando um `enable` não tem o `disable` do código antes.
- SEN073 quando a diretiva não escondeu nenhum diagnóstico do código citado, como o `@ts-expect-error` do TypeScript e o `#[expect]` do Rust. Numa lista com vários códigos, só os que não esconderam nada são marcados. Um `disable` de um código que já estava desligado por outro `disable` também sai como SEN073. Quando o código está em `off` no `senior-lsp.json`, a diretiva continua valendo como usada, porque o diagnóstico existia antes da configuração.

O SEN072 e o SEN073 não se escondem por diretiva, mas mudam de nível no [`senior-lsp.json`](configuracao.md#arquivo-de-configuração).

## Ignorar pela correção rápida

Em cada diagnóstico que se pode esconder, a ação de código "Ignorar SENxxx nesta linha" põe um `@ senior-lsp-disable-next-line SENxxx @` na linha de cima, com a indentação da linha do diagnóstico. Se a linha de cima já é um `senior-lsp-disable-next-line` dessa linha, o código entra no fim da lista dele. A ação não aparece dentro das regras de um JSON de modelo, onde o texto da regra é uma string escapada do JSON.
