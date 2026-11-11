import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { chmod, mkdir, rename, rm, writeFile } from "node:fs/promises";
import * as path from "node:path";
import * as vscode from "vscode";
import { LanguageClient, LanguageClientOptions, ServerOptions } from "vscode-languageclient/node";

const REPO = "eduardoghi/senior-lsp";

let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
    const output = vscode.window.createOutputChannel("Senior LSP", { log: true });
    context.subscriptions.push(output);

    context.subscriptions.push(
        vscode.commands.registerCommand("seniorLsp.restart", () => restart(context, output)),
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
        void vscode.window.showErrorMessage(`Senior LSP: ${message}`);
        return;
    }

    const serverOptions: ServerOptions = { command, args: [] };
    const clientOptions: LanguageClientOptions = {
        // As regras dos modelos de relatório ficam dentro dos JSON do modelo, que continuam com a
        // linguagem JSON. Os outros JSON do workspace não vão para o servidor.
        documentSelector: [
            { scheme: "file", language: "lsp-senior" },
            { scheme: "file", language: "json", pattern: "**/Modelos/**/*.json" },
        ],
        initializationOptions: {
            tbs_producao: config.get<string>("tbsProducao", ""),
            tbs_teste: config.get<string>("tbsTeste", ""),
        },
        outputChannel: output,
    };

    client = new LanguageClient("seniorLsp", "Senior LSP", serverOptions, clientOptions);
    await client.start();
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
        throw new Error(`falha ao baixar ${url} (HTTP ${response.status}).`);
    }
    return new Uint8Array(await response.arrayBuffer());
}
