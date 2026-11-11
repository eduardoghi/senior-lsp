# Senior LSP

Extensão do VS Code para o [senior-lsp](https://github.com/eduardoghi/senior-lsp), o servidor de
linguagem das regras LSP do ERP Senior. Os arquivos `Rule.txt` e `rule.txt` abrem na linguagem
LSP Senior, com destaque de sintaxe, e os JSON dos modelos de relatório (pasta `Modelos`) também
passam pelo servidor.

Na primeira vez, a extensão baixa o binário do servidor do release do GitHub da mesma versão,
confere o SHA-256 e guarda no armazenamento da extensão. Há binário para Linux x86_64 e Windows
x86_64. Em outro sistema, compile o servidor e informe o caminho em `seniorLsp.server.path`.

Configurações:

- `seniorLsp.server.path`: executável do servidor. Vazio usa o do release.
- `seniorLsp.tbsProducao` e `seniorLsp.tbsTeste`: dicionários de dados do ERP (`sapiens.tbs`),
  opcionais. Veja a seção Dicionário de dados no README do repositório.

O comando "Senior LSP: Reiniciar o servidor" reinicia o servidor. As mensagens dele ficam no canal
de saída Senior LSP.
