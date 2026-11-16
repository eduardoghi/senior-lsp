# Recursos

O servidor oferece:

- erros de sintaxe e de tipo, com o texto do compilador entre parênteses para quem já conhece a
  mensagem do ERP
- avisos de código que compila mas faz outra coisa do que parece
- [diretivas em comentário](diagnosticos.md#esconder-diagnósticos) e um arquivo `senior-lsp.json` que escondem diagnósticos por código ou mudam o nível deles, com a correção rápida "Ignorar SENxxx nesta linha"
- ir para a definição e achar as referências de variáveis, funções, parâmetros e rótulos
- achar onde um campo do dicionário de dados é usado em todas as regras da pasta aberta, a partir
  do `TABELA.Campo` no código ou do campo dentro de um SQL. Vale o campo com a tabela na frente e,
  nos textos de SQL, com o apelido dela ou sozinho num comando que cita a tabela, inclusive nas
  regras dos JSON de modelo
- ir para a implementação, do `definir funcao` ou da chamada para a `funcao`, e da `funcao` de volta
- hierarquia de chamadas, com quem chama a função (outras funções, o corpo da regra e, numa tela
  SGI, as outras regras) e as funções da regra que ela chama
- ir da definição de um web service interno (`Definir interno.com.x.Servico.Porta ws;`) para o
  `Rule.txt` da porta, na pasta `Web Services` da mesma exportação
- no JSON de modelo com as regras separadas, o `"lines": "regras/..."` vira link para o arquivo da regra
- os parâmetros da porta do web service, lidos do `Port.json` da exportação, no hover do nome
  qualificado e no completar depois de `ws.`, junto com `Executar` e os parâmetros que o ERP
  acrescenta a toda porta (`Usuario`, `Senha`, `Token`, `ModoExecucao` e `ErroExecucao`). Depois de
  `ws.conjunto.` ou `ws.conjunto[i].` vêm as colunas do conjunto e `QtdLinhas`, `LinhaAtual`,
  `CriarLinha` e `ExcluirLinha`
- destacar as outras ocorrências do nome que está no cursor e as palavras que andam juntas. No
  `enquanto`, no `para`, num `Pare` ou num `Continue` acendem o laço e os `Pare` e `Continue` dele,
  sem os de um laço de dentro. No `se` ou no `senao` acendem os dois, e na abertura ou no
  fechamento de um bloco, o par
- contagem de referências em cima de cada `funcao`, que abre a lista das chamadas no VS Code. Nos
  outros editores, o clique chama o comando `senior-lsp.mostrarReferencias`, que o editor precisa
  tratar
- correção rápida que cria o `definir` de um nome usado sem definição, com o tipo deduzido do uso
  (texto atribuído, parâmetro da função do ERP que recebe o nome ou a letra inicial, como `aTexto`)
- diagnósticos da pasta inteira, opcionais, sem abrir cada regra
- correções rápidas que apagam a definição sem uso (SEN068), põem a comparação entre parênteses
  (SEN041), trocam o apóstrofo por maiúscula (SEN064), acrescentam o `SQL_UsarAbrangencia(obj, 0)`
  que falta (SEN066), tiram do SELECT do cursor a coluna que a regra não lê ou trocam o `*` pelos
  campos lidos (SEN076), declaram a função chamada que não existe (SEN052), com o
  `Definir Funcao` junto dos outros `definir` e a `funcao` vazia no fim do arquivo, e acrescentam ao
  `extraTables` do `senior-lsp.json` a tabela criada direto no banco (SEN086)
- ação "Corrigir o arquivo" (`source.fixAll`), que aplica de uma vez só as correções que não mudam o
  que a regra faz. O apóstrofo minúsculo vira maiúsculo, sai o `definir` sem uso e sai a coluna do
  SELECT que a regra não lê. No VS Code ela também roda ao salvar, com
  `"editor.codeActionsOnSave": { "source.fixAll": "explicit" }`
- "Você quis dizer?" no campo, na tabela e na função que não existem (SEN052, SEN055, SEN074,
  SEN075, SEN081 e SEN082). A mensagem cita o nome mais parecido, e a correção rápida "Trocar por"
  põe ele no lugar. Só sai quando há um único nome a uma letra de distância, ou a duas no nome
  de oito letras ou mais
- ação de código "Extrair função" (`refactor.extract`) para comandos inteiros e contíguos do mesmo
  bloco, selecionados no editor. A chamada `NovaFuncao();` ocupa o lugar do trecho, o `Definir
  Funcao` entra junto dos outros `definir` e a `funcao` vai para o fim do arquivo, com o corpo
  reindentado. Como as variáveis de fora de função são globais, a função não tem parâmetros. O nome
  começa em `NovaFuncao` e ganha um número se algo com esse nome já existe no arquivo, na tela ou no
  ERP, e o F2 renomeia depois. A ação não aparece quando a seleção corta um comando, quando o trecho
  tem `definir` ou `funcao`, quando um `pare` ou `continue` perde o laço, quando um `vapara` e o
  rótulo dele ficam em lados diferentes do trecho, quando o trecho dentro de uma `funcao` usa
  parâmetro ou variável definida nela, e nas regras dentro do JSON de modelo de relatório
- ação de código "Embutir função" (`refactor.inline`), o inverso da anterior, com o cursor no nome
  de uma chamada. Os comandos da função entram no lugar da chamada, e a função continua onde está.
  Vale para a função sem parâmetros, com o corpo num bloco só, sem `definir`, rótulo nem `vapara`
  e sem `pare` ou `continue` que passariam a valer para um laço de fora
- ação de código "Extrair variável" (`refactor.extract`) para a conta ou o texto selecionado. O
  `Definir` da variável nova entra junto dos outros, com o tipo da expressão, e a atribuição na
  linha de cima do comando. O nome começa em `nNovaVariavel` ou `aNovaVariavel`. A ação não
  aparece quando o tipo de alguma parte da expressão não é conhecido, para comparação e `e` ou
  `ou`, que não podem ir sozinhos numa atribuição, na condição de um `enquanto` ou `para`, que é
  refeita a cada volta, e no comando sem bloco de um `se` ou de um laço
- ação de código "Inverter o se" (`refactor.rewrite`), com o cursor na palavra `se` ou na condição
  de um `se` com `senao`. A linguagem não tem negação, então cada comparação vira a contrária, o
  `e` vira `ou` e o contrário, com os parênteses que isso pede, `cur.Achou` vira `cur.NaoAchou`, e
  os dois lados trocam de lugar. Não aparece quando a condição tem uma parte sem contrária, como
  uma variável sozinha, nem quando o lado do `senao` é outro `se` ou um laço
- ações de código "Envolver com Se" e "Envolver com Enquanto" (`refactor.rewrite`) para comandos
  inteiros e contíguos do mesmo bloco. Eles vão para dentro de um bloco, com a condição vazia para
  preencher. A do `se` vazia é erro de compilação, e a do `enquanto` o compilador aceita
- ações de código "Trocar chaves por Inicio e Fim" e "Trocar Inicio e Fim por chaves"
  (`refactor.rewrite`), com o cursor na abertura ou no fechamento do bloco
- [formatação](#formatação) do recuo e do espaço dentro das linhas, no documento, no trecho
  selecionado e ao digitar
- ações de código "Tirar o parâmetro" e "Mover para a esquerda" ou "para a direita"
  (`refactor.rewrite`), com o cursor no nome de um parâmetro no `Definir Funcao` ou na `funcao`.
  Mudam as duas listas de parâmetros e os argumentos de cada chamada, nesta regra e, num formulário
  SGI, nas regras seguintes que chamam a função. Não aparecem quando alguma chamada não tem um
  argumento por parâmetro, nas regras de modelo de relatório e, para tirar, quando o corpo usa o
  parâmetro, mesmo num texto como o `:nome` de um SQL, ou quando o argumento tem uma chamada
  dentro, que deixaria de rodar. Para mover, também não aparecem quando um dos dois argumentos tem
  uma chamada dentro, que mudaria de ordem
- ação de código "Acrescentar parâmetro" (`refactor.rewrite`), depois do parâmetro no cursor ou, com
  o cursor no nome da função, no fim. A extensão do VS Code pergunta o tipo, o nome e o valor que as
  chamadas que já existem passam a receber, e as mesmas listas e chamadas mudam. O tipo segue a
  caixa dos outros parâmetros. Como precisa perguntar, só aparece no editor que liga a opção
  `acrescentar_parametro`, e a extensão liga sozinha
- hover com a linha do `definir` e, com o dicionário de dados do ERP configurado, o tipo, a
  descrição, as chaves, os relacionamentos e a busca dos campos de tabela
- hover das funções do ERP com a declaração que o compilador usa e o link da página da função na
  documentação online da Senior, quando ela tem uma. O endereço de cada página fica em
  `src/builtins/documentacao.txt`, que o `scripts/documentacao.py` refaz a partir do sitemap do
  manual da linguagem e do índice das funções do ERP
- dentro do SQL de um texto (`cur.SQL "..."`, `ExecSql` e `SQL_DefinirComando`), o `:variavel` vai para o
  `definir` e mostra a linha dele no hover, e `TABELA.Campo` e o nome de uma tabela mostram a
  descrição do dicionário de dados. O hover de tabela só vale em textos que começam com `SELECT`,
  `UPDATE`, `INSERT`, `DELETE` ou `WITH`. O apelido declarado no `FROM`, no `JOIN` ou no `UPDATE`
  do mesmo texto (`FROM e070emp e`, `JOIN e075pro AS p`) mostra o campo da tabela dele e, sobre o
  próprio apelido, a descrição da tabela. Ir para a definição do apelido leva à declaração. Uma
  subconsulta enxerga os apelidos de fora e pode declarar os seus
- ir de `cur.COLUNA` para a coluna na lista do `SELECT` do `cur.SQL`, pelo nome do campo ou pelo
  apelido (`AS`). Com `SELECT *` ou uma expressão sem apelido, não há para onde ir
- as funções que o ERP já traz, como `IntParaAlfa`, com a declaração no hover e a conferência dos
  parâmetros na chamada
- ajuda de assinatura, com a declaração da função e o parâmetro atual enquanto se digita a chamada
- o nome do parâmetro antes de cada argumento da chamada, sem mudar o texto, como em
  `IntParaAlfa(Origem: n, end Retorno: aTxt)`. Fica de fora nas funções de um parâmetro e no
  argumento com o mesmo nome do parâmetro
- seleção expandida, que aumenta a seleção do nome para a expressão, o comando, o bloco e a função
- recolher cada `funcao`, os blocos, os comentários `/* */` e a sequência de `definir` do começo
- destaque semântico, que colore cada nome pelo que ele é (função do ERP ou da regra, variável,
  parâmetro, campo depois do ponto, tabela e web service)
- a lista de variáveis e funções do arquivo
- busca de símbolos nas regras da pasta aberta (Ctrl+T no VS Code), com as funções (`funcao` e
  `definir funcao`), os `definir` e os rótulos de todos os `Rule.txt` e das regras de relatório
  separadas do JSON, cada um com o nome da regra (`<NOME>.LSP/Rule.txt` aparece como `NOME`). A
  consulta casa sem diferenciar maiúsculas, pelo começo, por um trecho ou pelas letras em ordem, e
  vêm no máximo 500 símbolos, os que começam com a consulta primeiro. Funciona com a opção
  `analisar_pasta` desligada, lendo as regras na primeira busca, e depois só acompanha os eventos de
  arquivo do editor. Um arquivo aberto vale pelo texto do editor. As regras que ficam dentro dos JSON
  de modelo não entram
- completar nomes num editor, com as variáveis e funções do arquivo, as do ERP e as palavras da
  linguagem
- modelos de comando no completar, nos editores que aceitam snippets, com o laço de cursor do SQL
  ao `FecharCursor`, `se` com `senao`, `enquanto`, `para`, `definir funcao`, o bloco da `funcao` e o
  `ExecSqlEx` com a conferência do erro
- modelos pós-fixados, também nos editores que aceitam snippets. Depois de uma expressão e um ponto,
  como `n.se` ou `cur.Achou.enquanto`, o completar oferece `se`, `enquanto`, `para` (de 1 até a
  expressão) e `mensagem`, que envolvem a expressão no comando. Depois do nome de um cursor, lista,
  tabela ou web service o ponto continua sendo o acesso ao membro
- completar depois do ponto, com as operações e as colunas do SELECT do cursor, as operações e os
  campos criados com `AdicionarCampo` na lista, os campos de uma `definir tabela` e, com o
  dicionário de dados, os campos da tabela do ERP
- completar dentro do texto de um SQL (`cur.SQL "..."`, `ExecSql "..."` e qualquer texto que comece
  com `SELECT`, `UPDATE`, `INSERT` ou `DELETE`), com o dicionário de dados. Depois de `TABELA.` ou
  do apelido saem os campos, depois de `FROM`, `JOIN`, `UPDATE` e `INSERT INTO` as tabelas e as
  visões, no resto do comando os campos das tabelas que ele cita, e depois de `:` as variáveis
  que já apareceram na regra. No VS Code a lista abre
  sozinha depois do ponto. Nos outros lugares do texto ela abre com Ctrl+Espaço, a não ser que
  `editor.quickSuggestions` esteja ligado para `strings`
- renomear num editor, trocando só o símbolo certo, nos outros arquivos do formulário SGI inclusive

## Formatação

O "Formatar documento" e o "Formatar seleção" acertam o recuo do começo das linhas e o espaço
entre os tokens de cada linha. A formatação ao digitar acerta só o recuo. O tamanho do nível e a
escolha entre espaço e tabulação vêm do editor, no pedido de formatação (no VS Code, o "Tab Size" e
o "Insert Spaces" do arquivo). O formatador não junta nem quebra linhas e não muda a chave de
lugar.

- O conteúdo de um bloco, com chaves ou com `inicio` e `fim`, entra um nível. A chave ou o `inicio`
  sozinho na linha fica na coluna do comando, como o fechamento.
- O `senao` fica na coluna do seu `se`, o `senao se` continua na mesma coluna, e o comando sem bloco
  na linha de baixo de um `se`, `senao`, `enquanto` ou `para` entra um nível.
- A linha que continua um comando começado numa linha de cima, como uma condição longa, anda junto
  com a primeira e mantém o alinhamento que a pessoa fez, sem ficar à esquerda dela.
- O comentário sozinho na linha segue o nível do bloco em que está.

Dentro da linha, só muda o espaço entre dois tokens com nada além de espaço entre eles:

- um espaço em volta de `=`, `<>`, `<`, `>`, `<=`, `>=`, `+`, `-`, `*`, `/`, `e` e `ou` quando eles
  têm dois lados, depois da vírgula e do `;` seguidos de outro token e entre `se`, `enquanto` ou
  `para` e o `(`
- nenhum antes da vírgula e do `;`, depois do `(`, antes do `)` e entre o nome de uma função e o `(`
  dela, na chamada e na declaração

O menos de um número negativo, o `++`, o ponto de `cur.Campo`, os dois-pontos de um rótulo e o
espaço com um comentário no meio ficam como estão.

A primeira formatação de uma regra que nunca passou pelo formatador costuma mexer no espaço de
muitas linhas, o que dá um diff grande numa exportação versionada. Com `"format": { "spaces": false }`
no [`senior-lsp.json`](configuracao.md#arquivo-de-configuração), a formatação da pasta só acerta o recuo.

Ao digitar, o `}` e o `fim` levam a linha para o nível dela, e a linha nova do Enter já começa no
nível do bloco. Enquanto o bloco ainda não tem fechamento e a regra tem erro, o Enter usa o recuo
da linha de cima, com um nível a mais quando ela termina em `{` ou `inicio`. A extensão do VS Code
liga o `editor.formatOnType` para a linguagem, e as regras de indentação dela fazem o "Reindentar
linhas" seguir o `inicio` e o `fim`.

Ficam como estão a linha de dentro de um texto que continua na linha de baixo, em que os espaços
fazem parte do texto, o miolo de um comentário `/* */` de várias linhas, a linha que passaria do
limite do compilador (`linha para o compilador é muito grande`) com o recuo maior e o trecho entre
`@ senior-lsp-format-off @` e `@ senior-lsp-format-on @`. A regra com erro de sintaxe não é
formatada, e o resultado só é devolvido quando os tokens, fora os espaços, continuam os mesmos. Nas
regras dentro do JSON de modelo de relatório a formatação ainda não vale.
