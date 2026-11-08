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
- hover com a linha do `definir`
- as funções que o ERP já traz, como `IntParaAlfa`, com a declaração no hover e a conferência dos
  parâmetros na chamada
- a lista de variáveis e funções do arquivo

A exportação guarda o código de cada regra num arquivo `Rule.txt` ou `rule.txt`, e o servidor
analisa só esses arquivos. A pasta diz onde a regra roda, e isso muda as funções do ERP que ela pode
chamar:

| Pasta | Arquivo | Regra |
|---|---|---|
| `Regras` | `Rule.txt` | regra de identificador, com as funções comuns do ERP |
| `Formulários SGI` | `rule.txt` | regra de tela SGI, que ganha as funções de tela e as definidas nas regras acima dela no formulário, como a da raiz |
| `Web Services` | `Rule.txt` | regra de web service, em que nome de função desconhecido não é acusado |

O servidor reconhece as pastas `Formulários SGI` e `Web Services`, e qualquer outro arquivo de regra
conta como regra de identificador, então renomear a pasta `Regras` não muda nada. Os outros tipos
de artefato não exportam arquivo de regra. Os modelos de relatório (`Modelos`) guardam as regras
dentro de arquivos JSON e não são analisados.

Os nomes seguem as regras do compilador. Eles não diferenciam caixa, e um `definir` vale no
arquivo inteiro, inclusive antes dele e dentro de funções. Variáveis que o ERP fornece não têm
`definir` e ficam sem verificação de tipo.

## Compilar

Precisa de Rust 1.86 ou mais novo.

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
guarda na pasta de dados do plugin. Há binário pronto para Linux x86_64. Em outro sistema,
instale a partir de um clone.

### A partir de um clone

Com o binário compilado, o plugin usa o `target/release` do clone em vez de baixar:

```sh
cargo build --release
claude plugin marketplace add "$PWD"
claude plugin install senior-lsp@senior-lsp
```

O Claude Code lê o plugin direto desta pasta, então depois de um `cargo build --release` basta
recarregar.

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
| SEN053 | erro | constante num parâmetro `end` de função do ERP |
| SEN060 | aviso | `e` ou `ou` seguido de `-`, que não grava o valor da atribuição |
| SEN061 | aviso | comando depois do bloco de uma função, que ainda faz parte dela |
| SEN062 | aviso | `funcao` com um bloco de outra função ainda aberto |
| SEN063 | aviso | parâmetro usado fora da sua função, que lê lixo |
| SEN064 | aviso | apóstrofo com letra minúscula, que o compilador lê em maiúscula |

## Licença

[MIT](LICENSE)
