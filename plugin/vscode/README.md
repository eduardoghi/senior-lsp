# Senior LSP

Extensão do VS Code para o [senior-lsp](https://github.com/eduardoghi/senior-lsp), o servidor de
linguagem das regras LSP do ERP Senior. Os arquivos `Rule.txt` e `rule.txt` abrem na linguagem
LSP Senior, com destaque de sintaxe, e os JSON dos modelos de relatório (pasta `Modelos`) também
passam pelo servidor.

Na primeira vez, a extensão baixa o binário do servidor do release do GitHub da mesma versão,
confere o SHA-256 e guarda no armazenamento da extensão. Há binário para Linux x86_64 e Windows
x86_64. Em outro sistema, compile o servidor e informe o caminho em `seniorLsp.server.path`.

Configurações:

- `seniorLsp.server.path` é o executável do servidor. Vazio usa o do release.
- `seniorLsp.analisarPasta` analisa todas as regras da pasta aberta, e não só as abertas no editor, e mostra os problemas no painel Problemas.
- `seniorLsp.tipoPadrao` é o tipo da regra num arquivo fora da exportação, como um arquivo novo.
- `seniorLsp.tbsProducao` e `seniorLsp.tbsTeste` são os dicionários de dados do ERP
  (`sapiens.tbs`), opcionais. Veja a seção Dicionário de dados no README do repositório.

Num arquivo fora da exportação, a barra de status mostra o tipo da regra (regra comum, tela SGI ou
modelo de relatório) e troca ao clicar. A escolha fica guardada no workspace.

O comando "Senior LSP: Reiniciar o servidor" reinicia o servidor. As mensagens dele ficam no canal
de saída Senior LSP.
