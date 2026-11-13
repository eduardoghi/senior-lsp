"""Procura entradas que derrubam o senior-lsp ou dão pânico nele.

Uso: python3 -I robustez.py BINARIO TBS CORPUS [iteracoes] [semente]

São três etapas. A primeira manda parâmetros fora do formato para cada pedido e notificação. A
segunda abre regras do CORPUS com trechos apagados, repetidos, trocados de lugar ou com caracteres
soltos no meio, como regra comum, de tela e de relatório, e faz pedidos em posições sorteadas. A
terceira faz o mesmo com os JSON de `Modelos`, se a pasta existir.

O servidor responde o pânico de um pedido com erro interno e registra no stderr o de uma
notificação. Qualquer um dos dois, ou o processo encerrado, é uma falha. A entrada que causou
fica gravada numa pasta temporária, e o script termina com código 1.

`iteracoes` é o número de regras mutiladas (padrão 5000, e metade disso de JSON). A mesma
`semente` (padrão 1) repete as mesmas entradas.
"""

import itertools
import json
import os
import random
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

BIN, TBS, CORPUS = sys.argv[1], sys.argv[2], Path(sys.argv[3])
ITERACOES = int(sys.argv[4]) if len(sys.argv) > 4 else 5000
SEMENTE = int(sys.argv[5]) if len(sys.argv) > 5 else 1
ERRO_INTERNO = -32603

POSICAO = [
    "textDocument/hover",
    "textDocument/definition",
    "textDocument/implementation",
    "textDocument/references",
    "textDocument/completion",
    "textDocument/signatureHelp",
    "textDocument/documentHighlight",
    "textDocument/prepareRename",
]
DOCUMENTO = [
    "textDocument/documentSymbol",
    "textDocument/semanticTokens/full",
    "textDocument/foldingRange",
    "textDocument/codeLens",
    "textDocument/documentLink",
]
PEDIDOS = POSICAO + DOCUMENTO + [
    "textDocument/rename",
    "textDocument/prepareCallHierarchy",
    "callHierarchy/incomingCalls",
    "callHierarchy/outgoingCalls",
    "textDocument/selectionRange",
    "textDocument/inlayHint",
    "textDocument/codeAction",
    "workspace/symbol",
    "workspace/executeCommand",
    "senior-lsp/tipoRegra",
]
NOTIFICACOES = [
    "textDocument/didOpen",
    "textDocument/didChange",
    "textDocument/didClose",
    "workspace/didChangeWatchedFiles",
    "workspace/didChangeConfiguration",
    "initialized",
    "$/cancelRequest",
    "outro/qualquer",
]
# O que entra no meio do texto. As sequências longas procuram recursão sem limite.
SOLTOS = [
    '"', "'", "@", "/*", "*/", "{", "}", "(", ")", ";", ".", ",", "\\", "\n", "\r", "\t", ":", "=", "-",
    "é", " ", "😀", "1E", "funcao ", "definir ", "se (", "senao ", "end ", 'cur.SQL "', "SELECT ",
    '"SELECT * FROM (',
    "(" * 3000, "{" * 3000, "-" * 3000, "f(" * 3000, "a." * 3000, "1 + " * 5000, "se (a) " * 3000,
    "se (a) { " * 2000, "x = (" * 2000, '"SELECT (' * 2000,
]

falhas = []


class Morreu(Exception):
    pass


class Cliente:
    def __init__(self):
        self.erros = tempfile.TemporaryFile()
        self.p = subprocess.Popen([BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.erros)
        self.id = 0
        self.resp = {}
        self.fim = False
        self.cv = threading.Condition()
        threading.Thread(target=self._ler, daemon=True).start()
        self.pedir("initialize", {"capabilities": {}, "initializationOptions": {"tbs_producao": TBS}})
        self.notificar("initialized", {})

    def _ler(self):
        f = self.p.stdout
        while True:
            tamanho = None
            while True:
                linha = f.readline()
                if not linha:
                    with self.cv:
                        self.fim = True
                        self.cv.notify_all()
                    return
                linha = linha.strip()
                if not linha:
                    break
                k, v = linha.decode().split(":", 1)
                if k.lower() == "content-length":
                    tamanho = int(v)
            corpo = json.loads(f.read(tamanho))
            with self.cv:
                if "id" in corpo and "method" not in corpo:
                    self.resp[corpo["id"]] = corpo
                    self.cv.notify_all()

    def _enviar(self, msg):
        b = json.dumps(msg).encode()
        try:
            self.p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
            self.p.stdin.flush()
        except OSError:
            raise Morreu(f"o servidor fechou a entrada, código {self.p.poll()}") from None

    def notificar(self, metodo, params):
        self._enviar({"jsonrpc": "2.0", "method": metodo, "params": params})

    def pedir(self, metodo, params):
        """Resultado do pedido. O erro interno fica anotado e vale como resultado vazio."""
        self.id += 1
        i = self.id
        self._enviar({"jsonrpc": "2.0", "id": i, "method": metodo, "params": params})
        with self.cv:
            if not self.cv.wait_for(lambda: i in self.resp or self.fim, 60):
                raise Morreu(f"{metodo} sem resposta em 60 segundos")
            if i not in self.resp:
                raise Morreu(f"o servidor encerrou em {metodo}, código {self.p.wait(5)}")
            corpo = self.resp.pop(i)
        erro = corpo.get("error") or {}
        if erro.get("code") == ERRO_INTERNO:
            self.internos.append(f"{metodo}: {erro.get('message')}")
        return corpo.get("result")

    internos = []

    def log(self):
        """Linhas do stderr do servidor com pânico ou erro interno."""
        self.erros.seek(0)
        texto = self.erros.read().decode(errors="replace")
        return sorted({l.strip() for l in texto.splitlines() if "panicked at" in l or "erro interno" in l})


def falha(descricao, entrada=None):
    """Anota a falha e grava a entrada que a causou."""
    if entrada is not None:
        pasta = Path(tempfile.gettempdir()) / f"senior-lsp-robustez-{os.getpid()}"
        pasta.mkdir(exist_ok=True)
        arquivo = pasta / f"entrada-{len(falhas)}.txt"
        arquivo.write_text(entrada, encoding="utf-8")
        descricao += f"\n  entrada em {arquivo}"
    falhas.append(descricao)
    print(f"FALHA {descricao}", flush=True)


def malformados():
    c = Cliente()
    uri = "file:///robustez/Rule.txt"
    texto = "definir numero x;\nfuncao f(numero a);\n{ x = a; }\nf(1);\n"
    abrir = {"textDocument": {"uri": uri, "languageId": "lsp-senior", "version": 1, "text": texto}}
    c.notificar("textDocument/didOpen", abrir)
    doc = {"uri": uri}
    pos = {"line": 0, "character": 9}
    fora = {"line": 2**40, "character": 2**40}
    item = {"name": "f", "kind": 12, "uri": uri, "range": {"start": pos, "end": pos},
            "selectionRange": {"start": {"line": 99, "character": 99}, "end": pos}}
    params = [
        None, {}, [], "x", 5, True,
        {"textDocument": None}, {"textDocument": {}}, {"textDocument": {"uri": 5}}, {"textDocument": doc},
        {"textDocument": {"uri": "não é uri"}, "position": pos},
        {"textDocument": doc, "position": {"line": -1, "character": 0}},
        {"textDocument": doc, "position": {"line": 0.5, "character": "a"}},
        {"textDocument": doc, "position": fora},
        {"textDocument": doc, "position": pos, "newName": 5},
        {"textDocument": doc, "position": pos, "newName": ""},
        {"textDocument": doc, "position": pos, "newName": "é😀 x"},
        {"textDocument": doc, "positions": [None, pos, {}]},
        {"textDocument": doc, "positions": [fora]},
        {"textDocument": doc, "range": {"start": {"line": 9, "character": 9}, "end": {"line": 0, "character": 0}}},
        {"textDocument": doc, "range": {"start": pos}, "context": {}},
        {"textDocument": doc, "range": {"start": pos, "end": fora}, "context": {"diagnostics": []}},
        {"textDocument": doc, "range": {"start": pos, "end": pos}, "context": {"diagnostics": [{}], "only": [5]}},
        {"item": None}, {"item": {}}, {"item": item},
        {"query": None}, {"query": 5},
        {"command": None}, {"command": "senior-lsp.tipoRegra"},
        {"command": "senior-lsp.tipoRegra", "arguments": [5, 5]},
        {"command": "senior-lsp.tipoRegra", "arguments": [uri, "x"]},
        {"command": "senior-lsp.recarregarDicionario", "arguments": None},
        {"uri": None}, {"uri": {}}, {"uri": "x"},
        {"settings": 5}, {"settings": [1]}, {"settings": {"tbs_teste": {}, "analisar_pasta": [], "tipo_padrao": 5}},
        {"changes": None}, {"changes": [{}]}, {"changes": [{"uri": "x", "type": 99}]},
        {"changes": [{"uri": uri, "type": "a"}]},
        {"textDocument": {"uri": uri, "version": "a"}, "contentChanges": [{}]},
        {"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"range": None, "text": None}]},
        {"textDocument": {"uri": uri, "version": 2},
         "contentChanges": [{"range": {"start": pos, "end": {"line": -5, "character": 0}}, "text": "x"}]},
        {"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"range": {"start": fora, "end": fora}, "text": "x"}]},
        {"textDocument": {"uri": uri, "languageId": 5, "version": 1, "text": "x"}},
        {"textDocument": {"uri": uri, "languageId": "lsp-senior", "version": 1, "text": None}},
    ]
    total = 0
    try:
        for metodo, p in itertools.product(PEDIDOS, params):
            c.pedir(metodo, p)
            total += 1
            for interno in c.internos:
                falha(f"erro interno com parâmetros {json.dumps(p, ensure_ascii=False)[:200]}\n  {interno}")
            c.internos.clear()
        for metodo, p in itertools.product(NOTIFICACOES, params):
            c.notificar(metodo, p)
            total += 1
        # Depois de tudo, um pedido certo ainda tem resposta.
        c.notificar("textDocument/didOpen", abrir)
        if not c.pedir("textDocument/hover", {"textDocument": doc, "position": {"line": 0, "character": 16}}):
            falha("o hover de um documento aberto ficou sem resposta depois dos pedidos fora do formato")
    except Morreu as e:
        falha(str(e))
    encerrar(c)
    print(f"{total} pedidos e notificações fora do formato", flush=True)


def mutilar(texto):
    if not texto:
        return random.choice(SOLTOS)
    i = random.randrange(len(texto))
    j = min(len(texto), i + random.randint(1, 40))
    sorte = random.random()
    if sorte < 0.2:
        return texto[:i]
    if sorte < 0.4:
        return texto[:i] + texto[j:]
    if sorte < 0.6:
        return texto[:i] + random.choice(SOLTOS) + texto[i:]
    if sorte < 0.75:
        return texto[:i] + texto[i:j] * random.randint(2, 5) + texto[j:]
    if sorte < 0.9:
        k = random.randrange(len(texto))
        return texto[:i] + texto[k:k + random.randint(1, 80)] + texto[j:]
    linhas = texto.split("\n")
    random.shuffle(linhas)
    return "\n".join(linhas)


def visitar(c, uri, texto):
    """Pedidos do documento inteiro e em posições sorteadas, e uma edição por cima."""
    doc = {"textDocument": {"uri": uri}}
    linhas = texto.split("\n")
    for metodo in DOCUMENTO:
        c.pedir(metodo, doc)
    fim = {"line": len(linhas), "character": 0}
    c.pedir("textDocument/inlayHint", dict(doc, range={"start": {"line": 0, "character": 0}, "end": fim}))
    for _ in range(6):
        l = random.randrange(len(linhas))
        pos = {"line": l, "character": random.randint(0, len(linhas[l]) + 1)}
        l2 = min(len(linhas) - 1, l + random.randint(0, 6))
        ate = {"line": l2, "character": random.randint(0, len(linhas[l2]))}
        metodo = random.choice(POSICAO)
        extra = {"context": {"includeDeclaration": True}} if metodo.endswith("references") else {}
        c.pedir(metodo, dict(doc, position=pos, **extra))
        c.pedir("textDocument/codeAction", dict(doc, range={"start": pos, "end": ate}, context={"diagnostics": []}))
        c.pedir("textDocument/selectionRange", dict(doc, positions=[pos]))
        c.pedir("textDocument/rename", dict(doc, position=pos, newName="nOvo"))
        for item in (c.pedir("textDocument/prepareCallHierarchy", dict(doc, position=pos)) or [])[:2]:
            c.pedir("callHierarchy/incomingCalls", {"item": item})
            c.pedir("callHierarchy/outgoingCalls", {"item": item})
    l = random.randrange(len(linhas))
    l2 = min(len(linhas) - 1, l + random.randint(0, 3))
    mudanca = {
        "range": {"start": {"line": l, "character": random.randint(0, len(linhas[l]))},
                  "end": {"line": l2, "character": random.randint(0, len(linhas[l2]))}},
        "text": random.choice(["", "x", "\n", '"', "é😀", "}\n{", "/*"]),
    }
    c.notificar("textDocument/didChange", {"textDocument": {"uri": uri, "version": 2}, "contentChanges": [mudanca]})
    c.pedir("textDocument/documentSymbol", doc)
    c.pedir("workspace/symbol", {"query": "a"})
    c.notificar("textDocument/didClose", doc)


def mutilados(nome, textos, iteracoes, json_de_modelo):
    if not textos:
        print(f"{nome}: nada no corpus", flush=True)
        return
    c = Cliente()
    tipos = ["comum", "tela", "relatorio"]
    feitas = 0
    for n in range(iteracoes):
        texto = random.choice(textos)
        for _ in range(random.randint(1, 4)):
            texto = mutilar(texto)
        tipo = tipos[n % 3]
        if json_de_modelo:
            uri, linguagem = f"file:///robustez/Modelos/M{n % 7}/x{n % 3}.json", "json"
        else:
            uri, linguagem = f"file:///robustez/{tipo}/solto{n % 7}.txt", "lsp-senior"
        abrir = {"textDocument": {"uri": uri, "languageId": linguagem, "version": 1, "text": texto}}
        try:
            c.notificar("textDocument/didOpen", abrir)
            if not json_de_modelo:
                c.pedir("workspace/executeCommand", {"command": "senior-lsp.tipoRegra", "arguments": [uri, tipo]})
            visitar(c, uri, texto)
        except Morreu as e:
            falha(f"{nome}, iteração {n}: {e}", texto)
            c.p.kill()
            c = Cliente()
            continue
        for interno in c.internos:
            falha(f"{nome}, iteração {n}: erro interno em {interno}", texto)
        c.internos.clear()
        feitas += 1
    encerrar(c)
    print(f"{nome}: {feitas} de {iteracoes} sem derrubar o servidor", flush=True)


def encerrar(c):
    """Fecha o servidor e anota o que ficou no stderr, como o pânico de uma notificação."""
    try:
        c.pedir("shutdown", None)
        c.notificar("exit", None)
        c.p.wait(10)
    except (Morreu, subprocess.TimeoutExpired):
        c.p.kill()
    for linha in c.log():
        if not any(linha in f for f in falhas):
            falha(f"no stderr do servidor: {linha}")


def ler(caminhos, codificacao):
    textos = []
    for caminho in caminhos:
        dados = caminho.read_bytes()
        if len(dados) < 60_000:
            textos.append(dados.decode(codificacao, errors="replace"))
    return textos


random.seed(SEMENTE)
malformados()
regras = sorted(p for p in CORPUS.rglob("*") if p.name.lower() == "rule.txt")
mutilados("regras", ler(regras, "cp1252"), ITERACOES, False)
modelos = sorted((CORPUS / "Modelos").glob("*/*.json"))
random.shuffle(modelos)
com_regra = [t for t in ler(modelos[:3000], "utf-8") if '"lines"' in t]
mutilados("JSON de modelo", com_regra, ITERACOES // 2, True)
if falhas:
    print(f"{len(falhas)} falhas", flush=True)
    sys.exit(1)
print("nenhuma falha", flush=True)
