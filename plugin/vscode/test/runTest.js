// Baixa o VS Code e roda test/suite com a extensão desta pasta carregada, na pasta
// test/workspace. O servidor é o do cargo build do clone, de release quando existe e senão o de
// depuração, ou o de SENIOR_LSP_SERVER. O dicionário é o fictício de tests/fixtures.
//
// No Linux sem tela, rode com xvfb-run -a npm test.
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { runTests } = require("@vscode/test-electron");

const EXTENSION = path.resolve(__dirname, "..");
const REPO = path.resolve(EXTENSION, "..", "..");
const EXE = process.platform === "win32" ? ".exe" : "";

function server() {
    if (process.env.SENIOR_LSP_SERVER) {
        return process.env.SENIOR_LSP_SERVER;
    }
    const release = path.join(REPO, "target", "release", `senior-lsp${EXE}`);
    const debug = path.join(REPO, "target", "debug", `senior-lsp${EXE}`);
    if (fs.existsSync(release)) {
        return release;
    }
    if (fs.existsSync(debug)) {
        return debug;
    }
    throw new Error("compile o servidor antes, com cargo build na raiz do repositório.");
}

async function main() {
    // Um perfil novo a cada rodada, com o servidor e o dicionário configurados.
    const userData = fs.mkdtempSync(path.join(os.tmpdir(), "senior-lsp-vscode-"));
    fs.mkdirSync(path.join(userData, "User"), { recursive: true });
    const settings = {
        "seniorLsp.server.path": server(),
        "seniorLsp.tbsProducao": path.join(REPO, "tests", "fixtures", "dicionario-ficticio.tbs"),
        "telemetry.telemetryLevel": "off",
        "update.mode": "none",
        "workbench.startupEditor": "none",
    };
    fs.writeFileSync(path.join(userData, "User", "settings.json"), JSON.stringify(settings, null, 4));
    try {
        await runTests({
            extensionDevelopmentPath: EXTENSION,
            extensionTestsPath: path.join(__dirname, "suite"),
            launchArgs: [
                path.join(__dirname, "workspace"),
                `--user-data-dir=${userData}`,
                "--disable-extensions",
                "--disable-workspace-trust",
            ],
        });
    } finally {
        fs.rmSync(userData, { recursive: true, force: true });
    }
}

main().catch((error) => {
    console.error(error.message || error);
    process.exit(1);
});
