// Conferências que rodam dentro do VS Code com a extensão carregada. As duas regras da pasta de
// trabalho são iguais e só diferem no Senior SQL 2, ligado em SQL2.LSP e desligado em
// NATIVO.LSP. O dicionário é o fictício, com E070EMP (CodEmp, NomEmp, SigEmp), E070FIL,
// E075PRO e E085CLI.
const fs = require("node:fs");
const path = require("node:path");
const vscode = require("vscode");

const WORKSPACE = path.resolve(__dirname, "..", "workspace");
const results = [];

function check(name, ok, detail) {
    results.push({ name, ok: Boolean(ok), detail: detail === undefined ? "" : String(detail) });
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitFor(condition, ms = 20000) {
    const start = Date.now();
    while (Date.now() - start < ms) {
        const value = await condition();
        if (value) {
            return value;
        }
        await sleep(100);
    }
    return null;
}

const code = (d) => String(d.code && d.code.value ? d.code.value : d.code);
const diagnostics = (uri, wanted) => vscode.languages.getDiagnostics(uri).filter((d) => code(d) === wanted);
const codes = (uri) => vscode.languages.getDiagnostics(uri).map(code);

async function open(rule) {
    const document = await vscode.workspace.openTextDocument(path.join(WORKSPACE, "regras", rule, "Rule.txt"));
    return vscode.window.showTextDocument(document, { preview: false });
}

async function setText(editor, text) {
    const document = editor.document;
    const all = new vscode.Range(document.positionAt(0), document.positionAt(document.getText().length));
    await editor.edit((builder) => builder.replace(all, text));
}

/** Aplica a ação de código de título `title` no ponto logo depois do começo de `needle`. */
async function applyFix(editor, original, needle, title) {
    await setText(editor, original);
    await waitFor(() => diagnostics(editor.document.uri, "SEN075").length > 0, 8000);
    const document = editor.document;
    const at = document.positionAt(document.getText().indexOf(needle) + 1);
    const actions =
        (await vscode.commands.executeCommand("vscode.executeCodeActionProvider", document.uri, new vscode.Range(at, at))) || [];
    const action = actions.find((a) => a.title.startsWith(title));
    if (!action) {
        return { text: null, titles: actions.map((a) => a.title) };
    }
    await vscode.workspace.applyEdit(action.edit);
    return { text: document.getText(), titles: actions.map((a) => a.title) };
}

/** Nomes que o completar oferece logo depois de `needle`. */
async function complete(document, needle, trigger) {
    const at = document.positionAt(document.getText().indexOf(needle) + needle.length);
    const list = await vscode.commands.executeCommand("vscode.executeCompletionItemProvider", document.uri, at, trigger);
    return ((list && list.items) || []).map((item) => (typeof item.label === "string" ? item.label : item.label.label));
}

async function main() {
    // 1. Diagnósticos com o dicionário e o Senior SQL 2.
    const editor = await open("SQL2.LSP");
    const document = editor.document;
    const uri = document.uri;
    const original = document.getText();
    check("a regra abre na linguagem LSP Senior", document.languageId === "lsp-senior", document.languageId);
    const field = await waitFor(() => diagnostics(uri, "SEN075")[0]);
    check(
        "o campo que a tabela não tem é acusado e tem sugestão (SEN075)",
        field && document.getText(field.range) === "E070EMP.NomEmpp" && field.message.endsWith("Você quis dizer NomEmp?"),
        field ? `${document.getText(field.range)} :: ${field.message}` : codes(uri).join(" "),
    );
    const func = diagnostics(uri, "SEN052")[0];
    check("a função que não existe tem sugestão (SEN052)", func && func.message.endsWith("Você quis dizer IntParaAlfa?"), func && func.message);
    const unread = diagnostics(uri, "SEN076");
    check(
        "a coluna que a regra não lê sai como dica esmaecida (SEN076)",
        unread.some((d) => document.getText(d.range) === "NomEmp") &&
            unread.every((d) => d.severity === vscode.DiagnosticSeverity.Hint && (d.tags || []).includes(vscode.DiagnosticTag.Unnecessary)),
        unread.map((d) => document.getText(d.range)).join(" | "),
    );
    const sql2 = diagnostics(uri, "SEN082").map((d) => document.getText(d.range));
    check("com o Senior SQL 2, o WHERE e o SET são conferidos (SEN082)", sql2.includes("NaoExiste") && sql2.includes("Nada"), sql2.join(" | "));

    const native = await open("NATIVO.LSP");
    const nativeUri = native.document.uri;
    await waitFor(() => diagnostics(nativeUri, "SEN075").length > 0);
    check("no SQL nativo, o WHERE e o SET não são conferidos", !codes(nativeUri).includes("SEN082") && codes(nativeUri).includes("SEN075"), codes(nativeUri).join(" "));

    // 2. Correções rápidas.
    const sqlEditor = await vscode.window.showTextDocument(document, { preview: false });
    let result = await applyFix(sqlEditor, original, "NomEmpp", "Trocar por NomEmp");
    check("a correção troca o campo pelo nome parecido", result.text && result.text.includes("E070EMP.NomEmp, NomEmp FROM"), result.text ? "" : result.titles.join(" | "));
    result = await applyFix(sqlEditor, original, " NomEmp FROM", "Tirar NomEmp do SELECT");
    check("a correção tira a coluna que a regra não lê", result.text && result.text.includes("E070EMP.NomEmpp FROM E070EMP"), result.text ? "" : result.titles.join(" | "));
    result = await applyFix(sqlEditor, original, "IntParaAlfaa", "Trocar por IntParaAlfa");
    check("a correção troca a função pela parecida", result.text && result.text.includes("\nIntParaAlfa(n, a);"), result.text ? "" : result.titles.join(" | "));
    await setText(sqlEditor, original);

    // 3. Completar dentro do SQL.
    let items = await complete(document, "SELECT E070FIL.", ".");
    check("depois de TABELA. saem os campos da tabela", ["CodEmp", "CodFil", "NomFil"].every((n) => items.includes(n)), items.join(" "));
    items = await complete(document, "NomEmp FROM ");
    check("depois do FROM saem as tabelas", ["E070EMP", "E070FIL", "E075PRO", "E085CLI"].every((n) => items.includes(n)), items.join(" "));
    items = await complete(document, "WHERE CodEmp = :");
    check("depois de : saem as variáveis da regra", items.includes("n") && items.includes("a") && !items.includes("CodEmp"), items.join(" "));

    // 4. Ctrl+/ comenta com /* */, porque @ não serve de comentário de linha.
    const line = document.lineCount - 2;
    sqlEditor.selection = new vscode.Selection(line, 0, line, 0);
    await vscode.commands.executeCommand("editor.action.commentLine");
    check("Ctrl+/ comenta a linha com /* */", document.lineAt(line).text === "/* n = n + 10; */", document.lineAt(line).text);
    await setText(sqlEditor, original);

    // 5. A correção do SEN086 cria o senior-lsp.json com a tabela, e o aviso some.
    const nativeEditor = await vscode.window.showTextDocument(native.document, { preview: false });
    const nativeText = native.document.getText();
    const configFile = path.join(WORKSPACE, "senior-lsp.json");
    try {
        await setText(nativeEditor, `${nativeText}ExecSql "DELETE FROM TB_FORA";\n`);
        const table = await waitFor(() => diagnostics(nativeUri, "SEN086")[0], 8000);
        check("a tabela fora do dicionário no SQL nativo é avisada (SEN086)", table, codes(nativeUri).join(" "));
        if (table) {
            const actions =
                (await vscode.commands.executeCommand("vscode.executeCodeActionProvider", nativeUri, table.range)) || [];
            const action = actions.find((a) => a.title === "Acrescentar TB_FORA ao extraTables do senior-lsp.json");
            check("a correção oferece acrescentar a tabela ao extraTables", action, actions.map((a) => a.title).join(" | "));
            if (action) {
                await vscode.commands.executeCommand(action.command.command, ...(action.command.arguments || []));
                const gone = await waitFor(() => diagnostics(nativeUri, "SEN086").length === 0, 10000);
                const written = fs.existsSync(configFile) ? fs.readFileSync(configFile, "utf8") : "";
                check(
                    "a correção grava o senior-lsp.json e o aviso some",
                    gone && written.includes('"TB_FORA"'),
                    `${codes(nativeUri).join(" ")} :: ${JSON.stringify(written)}`,
                );
            }
        }
    } finally {
        await setText(nativeEditor, nativeText);
        await vscode.commands.executeCommand("workbench.action.files.saveAll");
        fs.rmSync(configFile, { force: true });
    }

    // 6. Tirar o dicionário pela configuração tira o aviso, sem reiniciar o servidor.
    const config = vscode.workspace.getConfiguration("seniorLsp");
    const dictionary = config.get("tbsProducao");
    await config.update("tbsProducao", "", vscode.ConfigurationTarget.Global);
    const gone = await waitFor(() => diagnostics(uri, "SEN075").length === 0, 10000);
    check("tirar o dicionário pela configuração tira o SEN075", gone, codes(uri).join(" "));
    await config.update("tbsProducao", dictionary, vscode.ConfigurationTarget.Global);
    const back = await waitFor(() => diagnostics(uri, "SEN075").length > 0, 10000);
    check("pôr o dicionário de volta traz o SEN075", back, codes(uri).join(" "));
}

exports.run = async function () {
    try {
        await main();
    } catch (error) {
        check("as conferências rodaram até o fim", false, error && error.stack ? error.stack : String(error));
    }
    for (const r of results) {
        console.log(`${r.ok ? "ok   " : "FALHA"} ${r.name}${r.ok || !r.detail ? "" : `\n      ${r.detail}`}`);
    }
    const failed = results.filter((r) => !r.ok);
    console.log(`${results.length - failed.length} de ${results.length} conferências passaram`);
    if (failed.length) {
        throw new Error(`${failed.length} conferência(s) falharam`);
    }
};
