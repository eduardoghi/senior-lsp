//! O script do plugin baixa o binário pela versão escrita nele. Se ela não andar junto com a do
//! Cargo.toml e a do plugin.json, o plugin baixa um release que não é o desta versão.

#[test]
fn versoes_do_plugin_iguais_a_do_cargo() {
    let version = env!("CARGO_PKG_VERSION");
    let script = include_str!("../plugin/claude-code/server/senior-lsp");
    assert!(
        script.lines().any(|l| l == format!("VERSION={version}")),
        "o script do plugin não está na versão {version}"
    );
    let manifest = include_str!("../plugin/claude-code/.claude-plugin/plugin.json");
    assert!(
        manifest.contains(&format!("\"version\": \"{version}\"")),
        "o plugin.json não está na versão {version}"
    );
}
