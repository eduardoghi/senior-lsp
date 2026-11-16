# Senior LSP

Extensão do VS Code para o [senior-lsp](https://github.com/eduardoghi/senior-lsp), o servidor de
linguagem das regras LSP do ERP Senior. Os arquivos `Rule.txt` e `rule.txt` abrem na linguagem
LSP Senior, com destaque de sintaxe, e os JSON dos modelos de relatório (pasta `Modelos`) também
passam pelo servidor.

> [!IMPORTANT]
> **Aviso legal**
>
> O senior-lsp é um projeto pessoal e independente, sem vínculo, patrocínio ou endosso da Senior
> Sistemas. Não é uma ferramenta oficial e não substitui o compilador, a documentação nem o suporte
> do ERP. Quando o servidor e o ERP discordarem, na compilação ou na execução, vale o ERP.
>
> O repositório não contém nem distribui código-fonte, binários ou regras da Senior Sistemas. Os
> nomes e marcas da Senior Sistemas pertencem a ela e aparecem aqui só para identificar o sistema
> com que o servidor trabalha.
>
> O software é fornecido no estado em que se encontra, sem garantias, nos termos da
> [licença MIT](LICENSE).

O servidor aponta os erros que o compilador do ERP daria, sem compilar a regra no ERP, e confere o
SQL dos cursores e comandos contra o dicionário de dados. Também oferece ir para a definição,
referências, renomear, refactors de parâmetros de função, completar com modelos de comando e
formatação do recuo e do espaço dentro das linhas, inclusive ao digitar. A lista completa está no
[README do repositório](https://github.com/eduardoghi/senior-lsp#readme).

Na primeira vez, a extensão baixa o binário do servidor do release do GitHub da mesma versão,
confere o SHA-256 e guarda no armazenamento da extensão, apagando o binário das versões
anteriores. Há binário para Linux x86_64 e Windows x86_64. Em outro sistema, compile o servidor e
informe o caminho em `seniorLsp.server.path`.

Configurações:

- `seniorLsp.server.path` é o executável do servidor. Vazio usa o do release.
- `seniorLsp.analisarPasta` analisa todas as regras da pasta aberta, e não só as abertas no editor, e mostra os problemas no painel Problemas.
- `seniorLsp.tipoPadrao` é o tipo da regra num arquivo fora da exportação, como um arquivo novo.
- `seniorLsp.tbsProducao` e `seniorLsp.tbsTeste` são os dicionários de dados do ERP
  (`sapiens.tbs`), opcionais. Veja a seção Dicionário de dados no README do repositório.

O arquivo `senior-lsp.json` na raiz do repositório muda ou desliga diagnósticos por código, e a
extensão completa os códigos e os níveis dele. As diretivas `senior-lsp-disable-next-line`,
`senior-lsp-disable` e `senior-lsp-enable` nos comentários escondem diagnósticos no arquivo. Veja
a seção Esconder diagnósticos no README do repositório.

Num arquivo fora da exportação, a barra de status mostra o tipo da regra (regra comum, tela SGI ou
modelo de relatório) e troca ao clicar. A escolha fica guardada no workspace.

O comando "Senior LSP: Reiniciar o servidor" reinicia o servidor. As mensagens dele ficam no canal
de saída Senior LSP.
