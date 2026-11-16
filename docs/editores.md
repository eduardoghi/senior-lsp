# Editores

A instalação no VS Code e no Claude Code está no [README](../README.md#instalar).

## VS Code

A extensão fica em `plugin/vscode`. Ela abre os `Rule.txt` e `rule.txt` na linguagem LSP Senior,
com destaque de sintaxe, e manda também os JSON da pasta `Modelos` para o servidor, sem mudar a
linguagem deles. Os textos que começam com `SELECT`, `UPDATE`, `INSERT`, `DELETE` ou `WITH` são
coloridos como SQL, pela gramática `source.sql` que o VS Code já traz. Na primeira vez, baixa o binário do release da mesma versão da extensão e confere
o SHA-256, como o plugin do Claude Code.

O `.vsix` de cada versão também vai no release do GitHub. Para gerar e instalar o pacote a partir
de um clone:

```sh
cd plugin/vscode
npm ci
npm run compile
npm run package
code --install-extension senior-lsp-*.vsix
```

As configurações são `seniorLsp.server.path`, para usar outro executável, `seniorLsp.tipoPadrao`,
com o tipo dos [arquivos fora da exportação](regras.md#arquivos-fora-da-exportação), e `seniorLsp.tbsProducao`
e `seniorLsp.tbsTeste`, com os dicionários descritos em [Dicionário de dados](configuracao.md#dicionário-de-dados).
Mudar o caminho do executável reinicia o servidor. As outras valem na hora, sem reiniciar. A
extensão manda as opções no `settings` do `workspace/didChangeConfiguration`, com as mesmas chaves
das opções de inicialização (`tbs_producao`, `tbs_teste`, `tipo_padrao` e `analisar_pasta`), e
qualquer editor pode mandar o mesmo aviso. Só as chaves que vêm no aviso mudam. A extensão também
manda `acrescentar_parametro` ligada, porque sabe perguntar o que a ação "Acrescentar parâmetro"
precisa. Um editor que liga a opção tem que atender o comando `senior-lsp.acrescentarParametro` da
ação e mandar as respostas no pedido `senior-lsp/acrescentarParametro`, que devolve as edições.

Num arquivo fora da exportação, a barra de status mostra o tipo da regra e troca ao clicar, como o
comando "Senior LSP: Escolher o tipo da regra". A extensão guarda a escolha de cada arquivo no
workspace. Um arquivo novo analisado precisa estar na linguagem LSP Senior, que se escolhe no
canto da barra de status.

Para desenvolver, abra `plugin/vscode` no VS Code e rode com F5. Rodando de dentro do clone, a
extensão usa o `target/release` em vez de baixar.

Os testes da extensão baixam o VS Code e rodam nele conferências de diagnósticos, correções
rápidas, completar e configuração, com o dicionário fictício de `tests/fixtures` e o servidor do
`cargo build` do clone. Em `plugin/vscode`, rode `npm test`, ou `xvfb-run -a npm test` no Linux
sem tela. O CI roda os dois testes, o do servidor e o da extensão.

## Claude Code

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

## Kate

O cliente LSP do Kate escolhe o servidor pelo modo de destaque do documento, e não pela extensão.
A definição de sintaxe em `plugin/kate/lsp-senior.xml` cria o modo "LSP Senior" para os arquivos
`Rule.txt` e `rule.txt` e para as regras de relatório separadas (`*.SOnPrint.txt` e os outros eventos) e colore as palavras reservadas, os comentários e os textos. Os textos que começam com `SELECT`,
`UPDATE`, `INSERT`, `DELETE` ou `WITH` seguem as regras da sintaxe SQL que o Kate já traz. Copie para a
pasta de sintaxes do usuário:

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
`initializationOptions` são os dicionários descritos em [Dicionário de dados](configuracao.md#dicionário-de-dados)
e podem ficar de fora, assim como `tipo_padrao`. Os JSON dos modelos de relatório não passam pelo
servidor no Kate, porque ligar o servidor no modo JSON faria ele abrir todos os arquivos JSON.

Qualquer arquivo no modo LSP Senior vai para o servidor. Num arquivo novo, escolha Ferramentas →
Modo → Fontes → LSP Senior. O tipo da regra troca pelas ações de código do cliente LSP. As
mensagens do servidor aparecem na aba Saída do cliente LSP.

## Compilar

Precisa de Rust 1.99 ou mais novo.

```sh
cargo build --release
```

O executável fica em `target/release/senior-lsp` e conversa pelo stdin e stdout, como qualquer
servidor LSP.
