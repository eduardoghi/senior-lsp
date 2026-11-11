"""Mede o senior-lsp por stdio (abertura, digitação, pedidos e análise da pasta).

Uso: python3 -I bench.py BINARIO TBS CORPUS REGRA [REGRA ...]
"""

import json
import os
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

BIN, TBS, CORPUS = sys.argv[1], sys.argv[2], Path(sys.argv[3])
REGRAS = [Path(p) for p in sys.argv[4:]]


class Cliente:
    def __init__(self, analisar_pasta):
        self.p = subprocess.Popen([BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.id = 0
        self.resp = {}
        self.diag = {}
        self.cv = threading.Condition()
        threading.Thread(target=self._ler, daemon=True).start()
        uri = CORPUS.resolve().as_uri()
        t = time.perf_counter()
        self.pedir("initialize", {
            "processId": os.getpid(), "rootUri": uri,
            "workspaceFolders": [{"uri": uri, "name": "corpus"}],
            "capabilities": {"textDocument": {"publishDiagnostics": {}}},
            "initializationOptions": {"tbs_producao": TBS, "analisar_pasta": analisar_pasta},
        })
        self.t_init = time.perf_counter() - t
        self.notificar("initialized", {})

    def _ler(self):
        try:
            self._ler2()
        except Exception as e:
            import traceback; traceback.print_exc()

    def _ler2(self):
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
                    self.resp[corpo["id"]] = (time.perf_counter(), corpo)
                elif corpo.get("method") == "textDocument/publishDiagnostics":
                    pr = corpo["params"]
                    self.diag.setdefault(pr["uri"], []).append((time.perf_counter(), pr.get("version"), len(pr["diagnostics"])))
                elif "id" in corpo:  # pedido do servidor
                    self._enviar({"jsonrpc": "2.0", "id": corpo["id"], "result": None})
                self.cv.notify_all()

    def _enviar(self, msg):
        b = json.dumps(msg).encode()
        self.p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
        self.p.stdin.flush()

    def notificar(self, metodo, params):
        self._enviar({"jsonrpc": "2.0", "method": metodo, "params": params})

    def pedir(self, metodo, params, timeout=600):
        self.id += 1
        i = self.id
        t = time.perf_counter()
        self._enviar({"jsonrpc": "2.0", "id": i, "method": metodo, "params": params})
        with self.cv:
            if not self.cv.wait_for(lambda: i in self.resp, timeout): raise TimeoutError(metodo)
            fim, corpo = self.resp.pop(i)
        return fim - t, corpo.get("result")

    def esperar_diag(self, uri, versao, desde, timeout=600):
        with self.cv:
            ok = self.cv.wait_for(lambda: any(v == versao and t >= desde for t, v, _ in self.diag.get(uri, [])), timeout)
            if not ok:
                return None
            return min(t for t, v, _ in self.diag[uri] if v == versao and t >= desde) - desde

    def rss_mb(self):
        for linha in open(f"/proc/{self.p.pid}/status"):
            if linha.startswith("VmRSS"):
                return int(linha.split()[1]) / 1024
        return 0

    def fechar(self):
        try:
            self.pedir("shutdown", None, timeout=30)
            self.notificar("exit", None)
        except Exception:
            pass
        self.p.kill()


def ms(x):
    return f"{x * 1000:8.1f} ms"


def resumo(nome, vals):
    vals = sorted(vals)
    p95 = vals[int(len(vals) * 0.95) - 1] if len(vals) >= 20 else vals[-1]
    print(f"  {nome:28} med {ms(statistics.median(vals))}  p95 {ms(p95)}  max {ms(vals[-1])}  n={len(vals)}")


def posicoes(texto, n=40):
    """Posições espalhadas no arquivo, em cima de identificadores."""
    linhas = texto.split("\n")
    pos = []
    passo = max(1, len(linhas) // n)
    for l in range(0, len(linhas), passo):
        s = linhas[l]
        for c, ch in enumerate(s):
            if ch.isalpha() and (c == 0 or not s[c - 1].isalnum()):
                pos.append({"line": l, "character": c + 1})
                break
    return pos[:n]


def medir_regra(regra):
    texto = regra.read_bytes().decode("cp1252", errors="replace")
    uri = regra.resolve().as_uri()
    print(f"\n== {regra.relative_to(CORPUS)} ({len(texto) // 1024} KB, {texto.count(chr(10)) + 1} linhas)")
    c = Cliente(False)
    print(f"  initialize (carga do .tbs)   {ms(c.t_init)}   RSS {c.rss_mb():.0f} MB")
    t = time.perf_counter()
    c.notificar("textDocument/didOpen", {"textDocument": {"uri": uri, "languageId": "lsp-senior", "version": 1, "text": texto}})
    d = c.esperar_diag(uri, 1, t)
    print(f"  didOpen até diagnósticos     {ms(d)}   RSS {c.rss_mb():.0f} MB")

    # O servidor usa sincronização completa, então cada tecla manda o texto inteiro. Insere e apaga
    # um espaço no começo da linha do meio.
    linhas = texto.split("\n")
    meio = len(linhas) // 2
    com_espaco = "\n".join(linhas[:meio] + [" " + linhas[meio]] + linhas[meio + 1:])
    tempos = []
    versao = 1
    for k in range(30):
        versao += 1
        mud = {"text": com_espaco if k % 2 == 0 else texto}
        t = time.perf_counter()
        c.notificar("textDocument/didChange", {"textDocument": {"uri": uri, "version": versao}, "contentChanges": [mud]})
        tempos.append(c.esperar_diag(uri, versao, t))
    resumo("tecla até diagnósticos", tempos)

    doc = {"textDocument": {"uri": uri}}
    pos = posicoes(texto)
    for metodo, extra in [
        ("textDocument/hover", None), ("textDocument/definition", None), ("textDocument/references", {"context": {"includeDeclaration": True}}),
        ("textDocument/completion", None), ("textDocument/documentHighlight", None), ("textDocument/signatureHelp", None),
    ]:
        vals = []
        for p in pos:
            prm = dict(doc, position=p)
            if extra:
                prm.update(extra)
            vals.append(c.pedir(metodo, prm)[0])
        resumo(metodo.split("/")[1], vals)
    fim = {"line": len(linhas), "character": 0}
    for metodo, prm in [
        ("textDocument/semanticTokens/full", doc), ("textDocument/documentSymbol", doc), ("textDocument/foldingRange", doc),
        ("textDocument/codeLens", doc), ("textDocument/inlayHint", dict(doc, range={"start": {"line": 0, "character": 0}, "end": fim})),
        ("textDocument/codeAction", dict(doc, range={"start": {"line": meio, "character": 0}, "end": {"line": meio, "character": 0}}, context={"diagnostics": []})),
    ]:
        resumo(metodo.split("/")[-1] if "semantic" not in metodo else "semanticTokens/full", [c.pedir(metodo, prm)[0] for _ in range(5)])
    print(f"  RSS no fim                   {c.rss_mb():.0f} MB")
    c.fechar()


def medir_pasta():
    print("\n== workspace")
    t = time.perf_counter()
    c = Cliente(True)
    # A análise roda depois do initialized. O primeiro pedido só é atendido depois dela.
    d, _ = c.pedir("workspace/symbol", {"query": "calcula"})
    print(f"  analisar_pasta até atender   {ms(time.perf_counter() - t)}   RSS {c.rss_mb():.0f} MB   diagnósticos de {len(c.diag)} arquivos")
    vals = [c.pedir("workspace/symbol", {"query": q})[0] for q in ["calcula", "total", "x", "a", "gravar", "icms"] * 3]
    resumo("workspace/symbol", vals)
    c.fechar()
    c = Cliente(False)
    t = time.perf_counter()
    d, _ = c.pedir("workspace/symbol", {"query": "calcula"})
    print(f"  workspace/symbol frio        {ms(d)}   RSS {c.rss_mb():.0f} MB")
    c.fechar()


for r in REGRAS:
    medir_regra(r)
medir_pasta()
