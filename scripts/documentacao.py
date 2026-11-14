#!/usr/bin/env python3
"""Refaz src/builtins/documentacao.txt, com o endereço da página de cada função do ERP na
documentação online da Senior.

As funções da linguagem vêm do sitemap do manual de Tecnologia, em que o nome da página costuma ser
o nome da função em minúsculas. Quando não é, como em `http-put.htm`, o nome sai do título da
página, que termina nele. As funções do ERP vêm do índice das funções de programador, que liga o
nome de cada uma à página dela. Só entram as funções que o servidor conhece pelos arquivos de
src/builtins.

    python3 -I scripts/documentacao.py
"""
import html
import re
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path

VERSAO = "5.10.4"
BASE = "https://documentacao.senior.com.br/"
SITEMAP = f"{BASE}tecnologia/{VERSAO}/sitemap.xml"
INDICE_ERP = f"{BASE}gestaoempresarialerp/{VERSAO}/regra_funcoes/indice_funcoes/"
RAIZ = Path(__file__).resolve().parent.parent
BUILTINS = RAIZ / "src" / "builtins"
SAIDA = BUILTINS / "documentacao.txt"
NOME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def endereco(url):
    """O endereço com os caracteres fora do ASCII codificados, como o de uma página com acento."""
    return urllib.parse.quote(url, safe=":/%#?=&")


def baixar(url):
    pedido = urllib.request.Request(endereco(url), headers={"User-Agent": "senior-lsp-documentacao"})
    with urllib.request.urlopen(pedido, timeout=60) as resposta:
        return resposta.read().decode("utf-8", errors="replace")


def conhecidas():
    """Nome de cada função dos arquivos de src/builtins, pela forma em minúsculas."""
    nomes = {}
    for arquivo in BUILTINS.glob("*.txt"):
        if arquivo == SAIDA:
            continue
        texto = arquivo.read_text(encoding="utf-8", errors="ignore")
        for m in re.finditer(r"(?im)^\s*funcao\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(", texto):
            nomes.setdefault(m.group(1).lower(), m.group(1))
    return nomes


def da_linguagem(nomes):
    """Função da linguagem e o caminho da página dela, pelo sitemap."""
    achadas = {}
    paginas = re.findall(r"<loc>([^<]*/lsp/funcoes/[^<]+/[^<]+\.htm)</loc>", baixar(SITEMAP))
    for url in paginas:
        nome = url.rsplit("/", 1)[1][:-4]
        if nome not in nomes:
            # O nome da página não é o da função. O título termina no nome.
            pagina = baixar(url)
            time.sleep(0.2)
            m = re.search(r"<title>(.*?)</title>", pagina, re.S)
            nome = html.unescape(m.group(1)).rsplit(">", 1)[-1].strip().lower() if m else ""
        if nome in nomes:
            achadas.setdefault(nome, endereco(url).removeprefix(BASE))
    return achadas


def do_erp(nomes):
    """Função do ERP e o caminho da página dela, pelo índice das funções de programador."""
    achadas = {}
    texto = baixar(INDICE_ERP)
    padrao = r'<a[^>]*href="(' + re.escape(BASE) + r'gestaoempresarialerp/[^"#]+)"[^>]*>(.*?)</a>'
    for url, rotulo in re.findall(padrao, texto, re.S):
        nome = re.sub("<[^>]+>", "", html.unescape(rotulo)).strip()
        if NOME.fullmatch(nome) and nome.lower() in nomes:
            achadas.setdefault(nome.lower(), endereco(url).removeprefix(BASE))
    return achadas


def main():
    nomes = conhecidas()
    paginas = do_erp(nomes)
    # A página do manual da linguagem vale primeiro quando a função está nos dois.
    paginas.update(da_linguagem(nomes))
    linhas = [
        "# Página de cada função na documentação da Senior, relativa a " + BASE,
        "# Gerado por scripts/documentacao.py. Não edite à mão.",
    ]
    linhas += [f"{nomes[n]} {paginas[n]}" for n in sorted(paginas)]
    SAIDA.write_text("\n".join(linhas) + "\n", encoding="utf-8")
    print(f"{len(paginas)} de {len(nomes)} funções com página", file=sys.stderr)


if __name__ == "__main__":
    main()
