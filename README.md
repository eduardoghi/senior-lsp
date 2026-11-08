# senior-lsp

Servidor de linguagem ([Language Server Protocol](https://microsoft.github.io/language-server-protocol/))
para as regras LSP do ERP Senior, os arquivos `Rule.txt`. Ele aponta no editor os erros que o
compilador do ERP daria, sem precisar compilar a regra no ERP, e ajuda a navegar em regras grandes.

O servidor oferece:

- erros de sintaxe e de tipo, com o texto do compilador entre parênteses para quem já conhece a
  mensagem do ERP
- avisos de código que compila mas faz outra coisa do que parece
- ir para a definição e achar as referências de variáveis, funções, parâmetros e rótulos
- hover com a linha do `definir`
- a lista de variáveis e funções do arquivo

Só arquivos chamados `Rule.txt`, sem diferenciar maiúsculas, são analisados. Os nomes seguem as
regras do compilador. Eles não diferenciam caixa, e um `definir` vale no arquivo inteiro,
inclusive antes dele e dentro de funções. Funções e variáveis que o próprio ERP fornece, como `IntParaAlfa`,
não têm `definir` e ficam sem definição e sem verificação de tipo.

## Compilar

Precisa de Rust 1.86 ou mais novo.

```sh
cargo build --release
```

O executável fica em `target/release/senior-lsp` e conversa pelo stdin e stdout, como qualquer
servidor LSP.

## Instalar no Claude Code

O repositório tem um catálogo de plugin do Claude Code. O plugin procura o executável em
`plugin/claude-code/server/senior-lsp`, então compile e crie o link antes de instalar:

```sh
cargo build --release
mkdir -p plugin/claude-code/server
ln -s ../../../target/release/senior-lsp plugin/claude-code/server/senior-lsp
claude plugin marketplace add "$PWD"
claude plugin install senior-lsp@senior-lsp
```

Numa sessão aberta, `/reload-plugins --force` carrega o plugin. O Claude Code lê o plugin direto
desta pasta, então depois de um `cargo build --release` basta recarregar.

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
| SEN060 | aviso | `e` ou `ou` seguido de `-`, que não grava o valor da atribuição |
| SEN061 | aviso | comando depois do bloco de uma função, que ainda faz parte dela |
| SEN062 | aviso | `funcao` com um bloco de outra função ainda aberto |
| SEN063 | aviso | parâmetro usado fora da sua função, que lê lixo |
| SEN064 | aviso | apóstrofo com letra minúscula, que o compilador lê em maiúscula |

## Licença

[MIT](LICENSE)
