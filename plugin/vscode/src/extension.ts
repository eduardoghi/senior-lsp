import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { chmod, mkdir, rename, rm, writeFile } from "node:fs/promises";
import * as path from "node:path";
import * as vscode from "vscode";
import { LanguageClient, LanguageClientOptions, ServerOptions } from "vscode-languageclient/node";

const REPO = "eduardoghi/senior-lsp";

/** Comando do servidor que troca o tipo da regra de um arquivo solto, e pedido que devolve o tipo. */
const SERVER_COMMAND = "senior-lsp.tipoRegra";
const SERVER_REQUEST = "senior-lsp/tipoRegra";

/** Tipos na ordem da lista, com o nome que o servidor usa. */
const KINDS = [
    { tipo: "comum", label: "Regra comum" },
    { tipo: "tela", label: "Tela SGI" },
    { tipo: "relatorio", label: "Modelo de relatório" },
];

/** Chave no estado do workspace com o tipo escolhido para cada arquivo, pelo URI. */
const CHOSEN_KEY = "seniorLsp.tipos";

interface LspPosition {
    line: number;
    character: number;
}

interface LspLocation {
    uri: string;
    range: { start: LspPosition; end: LspPosition };
}

function toPosition(p: LspPosition): vscode.Position {
    return new vscode.Position(p.line, p.character);
}

let client: LanguageClient | undefined;
let statusBar: vscode.StatusBarItem;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
    const output = vscode.window.createOutputChannel("Senior LSP", { log: true });
    context.subscriptions.push(output);

    statusBar = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
    context.subscriptions.push(
        statusBar,
        vscode.commands.registerCommand("seniorLsp.restart", () => restart(context, output)),
        vscode.commands.registerCommand("seniorLsp.escolherTipo", () => chooseKind()),
        // Clique na contagem de referências em cima de cada função. O servidor manda o JSON do
        // protocolo, e o painel do VS Code precisa dos tipos dele.
        vscode.commands.registerCommand(
            "senior-lsp.mostrarReferencias",
            (uri: string, position: LspPosition, locations: LspLocation[]) =>
                vscode.commands.executeCommand(
                    "editor.action.showReferences",
                    vscode.Uri.parse(uri),
                    toPosition(position),
                    locations.map(
                        (l) =>
                            new vscode.Location(
                                vscode.Uri.parse(l.uri),
                                new vscode.Range(toPosition(l.range.start), toPosition(l.range.end)),
                            ),
                    ),
                ),
        ),
        vscode.window.onDidChangeActiveTextEditor(() => void updateStatusBar()),
        vscode.workspace.onDidChangeConfiguration((e) => {
            if (e.affectsConfiguration("seniorLsp")) {
                void restart(context, output);
            }
        }),
    );

    await start(context, output);
}

export async function deactivate(): Promise<void> {
    await client?.stop();
    client = undefined;
}

async function restart(context: vscode.ExtensionContext, output: vscode.LogOutputChannel): Promise<void> {
    await deactivate();
    await start(context, output);
}

async function start(context: vscode.ExtensionContext, output: vscode.LogOutputChannel): Promise<void> {
    const config = vscode.workspace.getConfiguration("seniorLsp");
    let command: string;
    try {
        command = await serverCommand(context, config, output);
    } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        output.error(message);
        void vscode.window.showErrorMessage(`O Senior LSP não iniciou porque ${message}`);
        return;
    }

    const serverOptions: ServerOptions = { command, args: [] };
    const clientOptions: LanguageClientOptions = {
        // As regras dos modelos de relatório ficam dentro dos JSON do modelo, que continuam com a
        // linguagem JSON. Os outros JSON do workspace não vão para o servidor.
        documentSelector: [
            { scheme: "file", language: "lsp-senior" },
            { scheme: "untitled", language: "lsp-senior" },
            { scheme: "file", language: "json", pattern: "**/Modelos/**/*.json" },
        ],
        initializationOptions: {
            tbs_producao: config.get<string>("tbsProducao", ""),
            tbs_teste: config.get<string>("tbsTeste", ""),
            tipo_padrao: config.get<string>("tipoPadrao", "comum"),
            analisar_pasta: config.get<boolean>("analisarPasta", false),
        },
        // Com a pasta inteira analisada, o servidor precisa saber das regras que mudam no disco
        // sem estar abertas, como depois de um git pull.
        synchronize: {
            fileEvents: vscode.workspace.createFileSystemWatcher("**/*.{txt,TXT,json}"),
        },
        outputChannel: output,
        middleware: {
            // O servidor esquece a escolha quando reinicia, então ela é mandada de novo ao abrir.
            didOpen: async (document, next) => {
                await next(document);
                const uri = document.uri.toString();
                const saved = chosen(context);
                const tipo = saved[uri];
                // Um tipo que o servidor não conhece mais, de uma versão anterior, é descartado.
                if (tipo && !KINDS.some((k) => k.tipo === tipo)) {
                    delete saved[uri];
                    await context.workspaceState.update(CHOSEN_KEY, saved);
                } else if (tipo && client) {
                    try {
                        await client.sendRequest("workspace/executeCommand", {
                            command: SERVER_COMMAND,
                            arguments: [uri, tipo],
                        });
                    } catch (err) {
                        output.warn(`tipo ${tipo} não aplicado em ${uri} (${String(err)})`);
                    }
                }
                if (document === vscode.window.activeTextEditor?.document) {
                    void updateStatusBar();
                }
            },
            // Pela barra de status ou pela ação de código, a escolha passa por aqui e fica guardada.
            executeCommand: async (command, args, next) => {
                const result = await next(command, args);
                if (command === SERVER_COMMAND && args.length === 2) {
                    const saved = { ...chosen(context), [String(args[0])]: String(args[1]) };
                    await context.workspaceState.update(CHOSEN_KEY, saved);
                    void updateStatusBar();
                }
                return result;
            },
        },
    };

    client = new LanguageClient("seniorLsp", "Senior LSP", serverOptions, clientOptions);
    await client.start();
    void updateStatusBar();
}

function chosen(context: vscode.ExtensionContext): Record<string, string> {
    return context.workspaceState.get<Record<string, string>>(CHOSEN_KEY, {});
}

/** Mostra o tipo da regra do editor ativo. Num arquivo da pasta de tela, o tipo não muda. */
async function updateStatusBar(): Promise<void> {
    const document = vscode.window.activeTextEditor?.document;
    if (!client || !document || document.languageId !== "lsp-senior") {
        statusBar.hide();
        return;
    }
    const answer = await client.sendRequest<{ tipo: string; fixo: boolean } | null>(SERVER_REQUEST, {
        uri: document.uri.toString(),
    });
    if (!answer || vscode.window.activeTextEditor?.document !== document) {
        statusBar.hide();
        return;
    }
    const label = KINDS.find((k) => k.tipo === answer.tipo)?.label ?? answer.tipo;
    statusBar.text = `LSP Senior (${label})`;
    if (answer.fixo) {
        statusBar.tooltip = "Tipo da regra pela pasta da exportação";
        statusBar.command = undefined;
    } else {
        statusBar.tooltip = "Tipo da regra. Clique para trocar.";
        statusBar.command = "seniorLsp.escolherTipo";
    }
    statusBar.show();
}

async function chooseKind(): Promise<void> {
    const document = vscode.window.activeTextEditor?.document;
    if (!client || !document || document.languageId !== "lsp-senior") {
        return;
    }
    const pick = await vscode.window.showQuickPick(
        KINDS.map((k) => ({ label: k.label, tipo: k.tipo })),
        { placeHolder: "Tipo da regra deste arquivo" },
    );
    if (pick) {
        await vscode.commands.executeCommand(SERVER_COMMAND, document.uri.toString(), pick.tipo);
    }
}

/** Caminho do executável, que é o configurado, o compilado no clone ou o baixado do release. */
async function serverCommand(
    context: vscode.ExtensionContext,
    config: vscode.WorkspaceConfiguration,
    output: vscode.LogOutputChannel,
): Promise<string> {
    const configured = config.get<string>("server.path", "").trim();
    if (configured) {
        return configured;
    }

    const exe = process.platform === "win32" ? ".exe" : "";
    // A extensão rodando de dentro do clone (plugin/vscode) usa o binário do cargo build --release.
    const localBuild = path.join(context.extensionPath, "..", "..", "target", "release", `senior-lsp${exe}`);
    if (existsSync(localBuild)) {
        return localBuild;
    }

    return download(context, output, exe);
}

/** Baixa do release a versão igual à da extensão, confere o SHA-256 e guarda no armazenamento da extensão. */
async function download(context: vscode.ExtensionContext, output: vscode.LogOutputChannel, exe: string): Promise<string> {
    const target = releaseTarget();
    const version = context.extension.packageJSON.version as string;
    const dir = context.globalStorageUri.fsPath;
    const binary = path.join(dir, `senior-lsp-${version}${exe}`);
    if (existsSync(binary)) {
        return binary;
    }

    const url = `https://github.com/${REPO}/releases/download/v${version}/senior-lsp-${target}${exe}`;
    output.info(`baixando ${url}`);
    const [data, checksum] = await Promise.all([fetchBytes(url), fetchBytes(`${url}.sha256`)]);
    const expected = new TextDecoder().decode(checksum).split(/\s+/)[0];
    const actual = createHash("sha256").update(data).digest("hex");
    if (expected !== actual) {
        throw new Error("o SHA-256 do binário baixado não confere com o do release.");
    }

    await mkdir(dir, { recursive: true });
    // Grava com outro nome e renomeia, para um download interrompido não ficar no lugar do binário.
    const tmp = `${binary}.download`;
    await writeFile(tmp, data);
    await chmod(tmp, 0o755);
    await rm(binary, { force: true });
    await rename(tmp, binary);
    return binary;
}

function releaseTarget(): string {
    const key = `${process.platform}-${process.arch}`;
    switch (key) {
        case "linux-x64":
            return "x86_64-unknown-linux-musl";
        case "win32-x64":
            return "x86_64-pc-windows-msvc";
        default:
            throw new Error(
                `não há binário pronto para ${key}. Compile com cargo build --release e informe o caminho em seniorLsp.server.path.`,
            );
    }
}

async function fetchBytes(url: string): Promise<Uint8Array> {
    const response = await fetch(url);
    if (!response.ok) {
        throw new Error(`não foi possível baixar ${url} (HTTP ${response.status}).`);
    }
    return new Uint8Array(await response.arrayBuffer());
}
