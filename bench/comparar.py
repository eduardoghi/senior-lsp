"""Compara as respostas de dois binários do senior-lsp sobre as mesmas regras.

Uso: python3 -I comparar.py BINARIO TBS CORPUS SAIDA [amostra]

Grava em SAIDA (um JSON por linha) os diagnósticos da análise da pasta inteira, os diagnósticos
e as respostas dos pedidos de um conjunto de regras abertas e a busca de símbolos. Rode uma vez com
o binário de antes e outra com o de depois, e compare as duas saídas com `diff`. Para a
sincronização incremental, as edições do cenário vão como o servidor anunciar (texto inteiro ou
ranges), então os dois binários chegam ao mesmo texto.

`amostra` é o passo da escolha das regras do CORPUS (padrão 6, uma a cada 6).
"""

import json
import os
import re
import subprocess
import sys
import threading
import time
import zlib
from pathlib import Path

BIN, TBS, CORPUS, SAIDA = sys.argv[1], sys.argv[2], Path(sys.argv[3]), sys.argv[4]
PASSO = int(sys.argv[5]) if len(sys.argv) > 5 else 6

METODOS_POSICAO = [
    "textDocument/hover",
    "textDocument/definition",
    "textDocument/implementation",
    "textDocument/documentHighlight",
    "textDocument/signatureHelp",
    "textDocument/completion",
    "textDocument/prepareRename",
    "textDocument/prepareCallHierarchy",
]


class Cliente:
    def __init__(self, analisar_pasta):
        self.p = subprocess.Popen([BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.id = 0
        self.resp = {}
        self.diag = {}
        self.cv = threading.Condition()
        threading.Thread(target=self._ler, daemon=True).start()
        uri = CORPUS.resolve().as_uri()
        _, init = self.pedir("initialize", {
            "processId": os.getpid(), "rootUri": uri,
            "workspaceFolders": [{"uri": uri, "name": "corpus"}],
            "capabilities": {"textDocument": {"publishDiagnostics": {}}},
            "initializationOptions": {"tbs_producao": TBS, "analisar_pasta": analisar_pasta},
        })
        sync = init["capabilities"]["textDocumentSync"]
        self.incremental = (sync.get("change") if isinstance(sync, dict) else sync) == 2
        self.notificar("initialized", {})

    def _ler(self):
        f = self.p.stdout
        while True:
            cab = {}
            while True:
                linha = f.readline()
                if not linha:
                    return
                linha = linha.strip()
                if not linha:
                    break
                k, v = linha.decode().split(":", 1)
                cab[k.lower()] = v.strip()
            corpo = json.loads(f.read(int(cab["content-length"])))
            with self.cv:
                if "id" in corpo and "method" not in corpo:
                    self.resp[corpo["id"]] = corpo
                elif corpo.get("method") == "textDocument/publishDiagnostics":
                    pr = corpo["params"]
                    self.diag.setdefault(pr["uri"], []).append(pr)
                elif "id" in corpo:
                    self._enviar({"jsonrpc": "2.0", "id": corpo["id"], "result": None})
                self.cv.notify_all()

    def _enviar(self, msg):
        b = json.dumps(msg).encode()
        self.p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
        self.p.stdin.flush()

    def notificar(self, metodo, params):
        self._enviar({"jsonrpc": "2.0", "method": metodo, "params": params})

    def pedir(self, metodo, params, timeout=900):
        self.id += 1
        i = self.id
        self._enviar({"jsonrpc": "2.0", "id": i, "method": metodo, "params": params})
        with self.cv:
            if not self.cv.wait_for(lambda: i in self.resp, timeout):
                raise TimeoutError(metodo)
            corpo = self.resp.pop(i)
        return corpo.get("error"), corpo.get("result")

    def diag_da_versao(self, uri, versao, timeout=60):
        with self.cv:
            ok = self.cv.wait_for(lambda: any(p.get("version") == versao for p in self.diag.get(uri, [])), timeout)
            if not ok:
                return None
            return [p for p in self.diag[uri] if p.get("version") == versao][-1]["diagnostics"]

    def fechar(self):
        try:
            self.pedir("shutdown", None, timeout=30)
            self.notificar("exit", None)
        except Exception:
            pass
        self.p.kill()


def posicoes(texto, n=24):
    """Posições espalhadas pelo arquivo, no começo e no meio de identificadores e depois de pontos."""
    achados = [m for m in re.finditer(r"[A-Za-z_][A-Za-z0-9_]*|\.", texto)]
    if not achados:
        return []
    passo = max(1, len(achados) // n)
    linhas_ate = [0]
    for linha in texto.split("\n"):
        linhas_ate.append(linhas_ate[-1] + len(linha) + 1)
    import bisect

    pos = []
    for k, m in enumerate(achados[::passo]):
        ini = m.start() + (k % 3) * (max(0, (m.end() - m.start()) // 2) if m.group() != "." else 0)
        ini = min(ini, m.end())
        if m.group() == ".":
            ini = m.end()
        l = bisect.bisect_right(linhas_ate, ini) - 1
        pos.append({"line": l, "character": ini - linhas_ate[l]})
    return pos[:n]


def ler(caminho):
    return caminho.read_bytes().decode("cp1252", errors="replace")


def canon(x):
    return json.dumps(x, sort_keys=True, ensure_ascii=False)


def abrir_pasta(out):
    c = Cliente(True)
    # Pedido que só é atendido depois da análise da pasta no servidor de antes. No de depois ele
    # entra na fila e a análise segue em segundo plano, então espera os diagnósticos se acalmarem.
    c.pedir("workspace/symbol", {"query": "zzzz"})
    ultimo = -1
    while True:
        with c.cv:
            total = sum(len(v) for v in c.diag.values())
        if total == ultimo:
            break
        ultimo = total
        time.sleep(1.5)
    final = {}
    for uri, lista in c.diag.items():
        final[uri] = lista[-1]["diagnostics"]
    out.write(canon({"pasta": {u: final[u] for u in sorted(final)}}) + "\n")
    for q in ["calcula", "total", "x", "a", "gravar", "icms", "", "Verifica", "cursor"]:
        _, r = c.pedir("workspace/symbol", {"query": q})
        out.write(canon({"workspace/symbol": q, "r": r}) + "\n")
    c.fechar()


def edicoes(texto):
    """Edições do cenário de digitação, como (inicio, fim, novo) em linha e coluna UTF-16."""
    linhas = texto.split("\n")
    meio = len(linhas) // 2
    ult = len(linhas) - 1
    return [
        ((meio, 0), (meio, 0), " "),
        ((meio, 0), (meio, 1), ""),
        ((meio, 0), (meio, 0), "é😀x"),
        ((meio, 0), (meio, 4), ""),
        ((ult, len(linhas[ult])), (ult, len(linhas[ult])), "\n// fim\n"),
        ((0, 0), (0, 0), "@ ç @\r\n"),
        ((meio, 0), (meio + 2, 0), ""),
    ]


def aplicar(texto, ini, fim, novo):
    """Aplica uma edição em UTF-16 ao texto (colunas em unidades UTF-16)."""
    linhas = texto.split("\n")

    def desloc(l, col):
        base = sum(len(x) + 1 for x in linhas[:l])
        s = linhas[l]
        u = 0
        for i, ch in enumerate(s):
            if u >= col:
                return base + i
            u += 2 if ord(ch) > 0xFFFF else 1
        return base + len(s)

    a, b = desloc(*ini), desloc(*fim)
    return texto[:a] + novo + texto[b:]


def abrir_regras(out, regras):
    c = Cliente(False)
    versao = {}
    for regra in regras:
        texto = ler(regra)
        uri = regra.resolve().as_uri()
        nome = str(regra.relative_to(CORPUS))
        c.notificar("textDocument/didOpen", {"textDocument": {"uri": uri, "languageId": "lsp-senior", "version": 1, "text": texto}})
        diag = c.diag_da_versao(uri, 1)
        out.write(canon({"arq": nome, "diag": diag}) + "\n")
        doc = {"textDocument": {"uri": uri}}
        for pos in posicoes(texto):
            for metodo in METODOS_POSICAO:
                _, r = c.pedir(metodo, dict(doc, position=pos))
                out.write(canon({"arq": nome, "m": metodo, "pos": pos, "r": r}) + "\n")
            _, r = c.pedir("textDocument/references", dict(doc, position=pos, context={"includeDeclaration": True}))
            out.write(canon({"arq": nome, "m": "references", "pos": pos, "r": r}) + "\n")
        _, r = c.pedir("textDocument/selectionRange", dict(doc, positions=posicoes(texto, 8)))
        out.write(canon({"arq": nome, "m": "selectionRange", "r": r}) + "\n")
        n = texto.count("\n") + 1
        for metodo, prm in [
            ("textDocument/semanticTokens/full", doc), ("textDocument/documentSymbol", doc),
            ("textDocument/foldingRange", doc), ("textDocument/codeLens", doc),
            ("textDocument/documentLink", doc),
            ("textDocument/inlayHint", dict(doc, range={"start": {"line": 0, "character": 0}, "end": {"line": n, "character": 0}})),
            ("textDocument/codeAction", dict(doc, range={"start": {"line": n // 2, "character": 0}, "end": {"line": n // 2, "character": 3}}, context={"diagnostics": (diag or [])[:5]})),
        ]:
            _, r = c.pedir(metodo, prm)
            out.write(canon({"arq": nome, "m": metodo, "r": r}) + "\n")
        # Digitação. As edições vão como o servidor anunciar.
        if regra.stat().st_size < 40_000:
            atual = texto
            for k, (ini, fim, novo) in enumerate(edicoes(texto)):
                v = k + 2
                depois = aplicar(atual, ini, fim, novo)
                if c.incremental:
                    mud = {"range": {"start": {"line": ini[0], "character": ini[1]}, "end": {"line": fim[0], "character": fim[1]}}, "text": novo}
                else:
                    mud = {"text": depois}
                c.notificar("textDocument/didChange", {"textDocument": {"uri": uri, "version": v}, "contentChanges": [mud]})
                d = c.diag_da_versao(uri, v)
                out.write(canon({"arq": nome, "edicao": k, "diag": d}) + "\n")
                atual = depois
        c.notificar("textDocument/didClose", {"textDocument": {"uri": uri}})
    c.fechar()


def escolher():
    todas = sorted(p for p in CORPUS.rglob("*") if p.name.lower() == "rule.txt" and p.stat().st_size < 400_000)
    # Cada tipo de pasta (regra comum, tela SGI, web service, relatório separado) entra na amostra.
    escolhidas = todas[::PASSO]
    for p in todas:
        s = str(p)
        if ("Formulários SGI" in s or "Web Services" in s) and p not in escolhidas and zlib.crc32(s.encode()) % 3 == 0:
            escolhidas.append(p)
    return escolhidas


with open(SAIDA, "w") as out:
    regras = escolher()
    print(f"{len(regras)} regras abertas")
    abrir_regras(out, regras)
    abrir_pasta(out)
