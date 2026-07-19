use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn tau_cmd() -> Command {
    let mut cmd = Command::cargo_bin("tau").unwrap();
    // Disable tracing to avoid interference
    cmd.env("RUST_LOG", "off");
    cmd
}

fn setup_home(tmp: &TempDir) {
    let providers = r#"{
        "default_provider": "anthropic",
        "provider_preferences": {
            "anthropic": { "default_model": "claude-sonnet-4-20250514", "max_retries": 3 },
            "openai": { "default_model": "gpt-4o" }
        }
    }"#;
    fs::write(tmp.path().join("providers.json"), providers).unwrap();

    let catalog = r#"schema_version = 1

[[providers]]
name = "anthropic"
display_name = "Anthropic"
kind = "anthropic"
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
credential_name = "anthropic"
models = ["claude-sonnet-4-20250514", "claude-opus-4-20250514"]
default_model = "claude-sonnet-4-20250514"

[[providers]]
name = "deepseek"
display_name = "DeepSeek"
kind = "openai-compatible"
base_url = "https://api.deepseek.com"
api_key_env = "DEEPSEEK_API_KEY"
credential_name = "deepseek"
models = ["deepseek-v4-flash"]
default_model = "deepseek-v4-flash"

[[providers]]
name = "openai"
display_name = "OpenAI"
kind = "openai-compatible"
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
models = ["gpt-4o"]
default_model = "gpt-4o"
"#;
    fs::write(tmp.path().join("catalog.toml"), catalog).unwrap();

    let creds = r#"{"anthropic": "test-key-123", "deepseek": "ds-key-456"}"#;
    fs::write(tmp.path().join("credentials.json"), creds).unwrap();
}

// ---------------------------------------------------------------------------
// providers subcommand
// ---------------------------------------------------------------------------

#[test]
fn providers_lists_all_entries() {
    let tmp = TempDir::new().unwrap();
    setup_home(&tmp);

    tau_cmd()
        .arg("providers")
        .env("TAU_HOME", tmp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("anthropic"))
        .stdout(predicate::str::contains("deepseek"))
        .stdout(predicate::str::contains("openai"))
        .stdout(predicate::str::contains("claude-sonnet"));
}

#[test]
fn providers_empty_catalog() {
    let tmp = TempDir::new().unwrap();
    // Empty user catalog — builtin providers are still visible via merge
    fs::write(tmp.path().join("catalog.toml"), "schema_version = 1\n").unwrap();

    tau_cmd()
        .arg("providers")
        .env("TAU_HOME", tmp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("openai"))
        .stdout(predicate::str::contains("anthropic"));
}

// ---------------------------------------------------------------------------
// config subcommand
// ---------------------------------------------------------------------------

#[test]
fn config_shows_resolved_fields() {
    let tmp = TempDir::new().unwrap();
    setup_home(&tmp);

    tau_cmd()
        .args(["config", "anthropic"])
        .env("TAU_HOME", tmp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Provider: anthropic"))
        .stdout(predicate::str::contains("default_model: claude-sonnet"))
        .stdout(predicate::str::contains("max_retries:   3"))
        .stdout(predicate::str::contains("kind:          anthropic"))
        .stdout(predicate::str::contains(
            "base_url:      https://api.anthropic.com",
        ))
        .stdout(predicate::str::contains("api_key:       resolved"));
}

#[test]
fn config_missing_provider_shows_no_prefs() {
    let tmp = TempDir::new().unwrap();
    setup_home(&tmp);

    tau_cmd()
        .args(["config", "nonexistent"])
        .env("TAU_HOME", tmp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("no preferences in providers.json"))
        .stdout(predicate::str::contains("no entry in catalog.toml"))
        .stdout(predicate::str::contains("api_key:       NOT FOUND"));
}

#[test]
fn config_shows_api_key_resolved_from_file() {
    let tmp = TempDir::new().unwrap();
    setup_home(&tmp);

    tau_cmd()
        .args(["config", "deepseek"])
        .env("TAU_HOME", tmp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("api_key:       resolved"));
}

// ---------------------------------------------------------------------------
// --print help
// ---------------------------------------------------------------------------

#[test]
fn help_shows_usage() {
    let tmp = TempDir::new().unwrap();
    setup_home(&tmp);

    tau_cmd()
        .arg("--help")
        .env("TAU_HOME", tmp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage:"))
        .stdout(predicate::str::contains("--print"))
        .stdout(predicate::str::contains("--provider"))
        .stdout(predicate::str::contains("providers"));
}

// ---------------------------------------------------------------------------
// --print without prompt errors
// ---------------------------------------------------------------------------

#[test]
fn print_without_prompt_errors() {
    let tmp = TempDir::new().unwrap();
    setup_home(&tmp);

    tau_cmd()
        .arg("--print")
        .env("TAU_HOME", tmp.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--print requires a prompt"));
}

// ---------------------------------------------------------------------------
// interactive mode rejects prompt args
// ---------------------------------------------------------------------------

#[test]
fn interactive_mode_rejects_prompt_args() {
    let tmp = TempDir::new().unwrap();
    setup_home(&tmp);

    tau_cmd()
        .arg("hello world")
        .env("TAU_HOME", tmp.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "interactive mode does not accept a prompt",
        ));
}

// ---------------------------------------------------------------------------
// missing catalog/providers files
// ---------------------------------------------------------------------------

#[test]
fn handles_missing_providers_json() {
    let tmp = TempDir::new().unwrap();
    // No providers.json — should still work with defaults

    tau_cmd()
        .arg("providers")
        .env("TAU_HOME", tmp.path())
        .assert()
        .success();
}

#[test]
fn handles_missing_catalog_toml() {
    let tmp = TempDir::new().unwrap();
    // No catalog.toml — builtin catalog is loaded via load_user_or_default

    tau_cmd()
        .arg("providers")
        .env("TAU_HOME", tmp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("openai"));
}
