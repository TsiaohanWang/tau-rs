mod config;

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use futures::StreamExt;
use tau_agent::harness::{AgentHarness, AgentHarnessConfig, QueueMode};
use tau_agent::provider::ModelProvider;
use tau_ai::anthropic::{AnthropicConfig, AnthropicModelProvider, AnthropicProvider};
use tau_ai::openai::{OpenAIConfig, OpenAIModelProvider, OpenAIProvider};
use tau_types::AgentEvent;

#[derive(Parser)]
#[command(
    name = "tau",
    about = "Rust rewrite of the HuggingFace Tau coding agent"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Print the response and exit (non-interactive mode)
    #[arg(short = 'p', long)]
    print: bool,

    /// The prompt to send (requires --print)
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,

    /// Provider name (e.g. anthropic, openai, deepseek)
    #[arg(short = 'P', long)]
    provider: Option<String>,

    /// Model override
    #[arg(short, long)]
    model: Option<String>,

    /// System prompt
    #[arg(short = 'S', long)]
    system: Option<String>,

    /// Maximum tokens for response
    #[arg(short = 'M', long)]
    max_tokens: Option<u32>,

    /// Verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// List available providers from the catalog
    Providers,
    /// Show resolved configuration for a provider
    Config {
        /// Provider name
        name: String,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env from current directory or parent directories
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();

    let filter = if cli.verbose {
        "tau_cli=debug,tau_ai=debug"
    } else {
        "tau_cli=warn,tau_ai=warn"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| filter.into()),
        )
        .init();

    let home = config::TauHome::discover();
    let providers =
        config::ProvidersConfig::load(&home.providers_path()).context("loading providers.json")?;
    let credentials = config::CredentialsConfig::load(&home.credentials_path())
        .context("loading credentials.json")?;
    let catalog =
        config::CatalogConfig::load(&home.catalog_path()).context("loading catalog.toml")?;

    match cli.command {
        Some(Commands::Providers) => {
            cmd_providers(&catalog);
        }
        Some(Commands::Config { name }) => {
            cmd_config(&name, &providers, &catalog, &credentials)?;
        }
        None => {
            let provider_name = cli
                .provider
                .as_deref()
                .unwrap_or(&providers.default_provider);
            let api_key =
                config::resolve_api_key(&catalog, &credentials, provider_name).unwrap_or_default();

            let model = cli.model.clone().unwrap_or_else(|| {
                providers
                    .provider_preferences
                    .get(provider_name)
                    .and_then(|p| p.default_model.clone())
                    .or_else(|| {
                        catalog
                            .find_provider(provider_name)
                            .and_then(|p| p.default_model.clone())
                    })
                    .unwrap_or_else(|| "gpt-4o".to_string())
            });

            let system = cli
                .system
                .unwrap_or_else(|| "You are a helpful assistant.".to_string());

            let prefs = providers.provider_preferences.get(provider_name);
            let max_retries = prefs.and_then(|p| p.max_retries).unwrap_or(2);
            let timeout = prefs
                .and_then(|p| p.timeout_seconds)
                .map(|s| s as u64)
                .unwrap_or(60);

            let kind = config::ProviderKind::from_catalog(&catalog, provider_name);
            let base_url = catalog
                .find_provider(provider_name)
                .and_then(|p| p.base_url.clone())
                .unwrap_or_else(|| match kind {
                    config::ProviderKind::Anthropic => "https://api.anthropic.com".to_string(),
                    config::ProviderKind::OpenaiCompatible => "https://api.openai.com".to_string(),
                });

            let prompt_text = if cli.prompt.is_empty() {
                None
            } else {
                Some(cli.prompt.join(" "))
            };

            if cli.print {
                let prompt = prompt_text.context("--print requires a prompt argument")?;
                let cwd = std::env::current_dir().context("failed to get current directory")?;
                let model_provider: Arc<dyn ModelProvider + Send + Sync> = match kind {
                    config::ProviderKind::Anthropic => {
                        let cfg = AnthropicConfig {
                            api_key,
                            base_url,
                            model: model.clone(),
                            max_tokens: cli.max_tokens,
                            max_retries,
                            timeout_seconds: timeout,
                            ..Default::default()
                        };
                        let provider = AnthropicProvider::new(cfg);
                        Arc::new(AnthropicModelProvider::new(provider))
                    }
                    config::ProviderKind::OpenaiCompatible => {
                        let cfg = OpenAIConfig {
                            api_key,
                            base_url,
                            model: model.clone(),
                            max_retries,
                            timeout_seconds: timeout,
                            ..Default::default()
                        };
                        let provider = OpenAIProvider::new(cfg);
                        Arc::new(OpenAIModelProvider::new(provider))
                    }
                };
                print_once(model_provider, &system, &prompt, &model, &cwd, &home).await?;
            } else {
                if prompt_text.is_some() {
                    bail!("interactive mode does not accept a prompt; use --print with a prompt");
                }
                let cwd = std::env::current_dir().context("failed to get current directory")?;
                let model_provider: Arc<dyn ModelProvider + Send + Sync> = match kind {
                    config::ProviderKind::Anthropic => {
                        let cfg = AnthropicConfig {
                            api_key,
                            base_url,
                            model: model.clone(),
                            max_tokens: cli.max_tokens,
                            max_retries,
                            timeout_seconds: timeout,
                            ..Default::default()
                        };
                        let provider = AnthropicProvider::new(cfg);
                        Arc::new(AnthropicModelProvider::new(provider))
                    }
                    config::ProviderKind::OpenaiCompatible => {
                        let cfg = OpenAIConfig {
                            api_key,
                            base_url,
                            model: model.clone(),
                            max_retries,
                            timeout_seconds: timeout,
                            ..Default::default()
                        };
                        let provider = OpenAIProvider::new(cfg);
                        Arc::new(OpenAIModelProvider::new(provider))
                    }
                };
                eprintln!("tau-rs ({provider_name}) | type your message (Ctrl-D to exit)");
                run_repl(model_provider, &system, &model, &cwd, &home).await?;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_providers(catalog: &config::CatalogConfig) {
    println!("Available providers (catalog.toml):");
    for p in &catalog.providers {
        let model = p.default_model.as_deref().unwrap_or("n/a");
        println!(
            "  {:<20} {:<30} default: {}",
            p.name,
            p.display_name.as_deref().unwrap_or(""),
            model
        );
    }
    if catalog.providers.is_empty() {
        println!("  (no providers configured in catalog.toml)");
    }
}

fn cmd_config(
    name: &str,
    providers: &config::ProvidersConfig,
    catalog: &config::CatalogConfig,
    credentials: &config::CredentialsConfig,
) -> anyhow::Result<()> {
    let prefs = providers.provider_preferences.get(name);
    let cat = catalog.find_provider(name);

    println!("Provider: {name}");
    if let Some(p) = prefs {
        println!(
            "  default_model: {}",
            p.default_model.as_deref().unwrap_or("(none)")
        );
        println!("  max_retries:   {}", p.max_retries.unwrap_or(2));
        println!("  timeout:       {}s", p.timeout_seconds.unwrap_or(60.0));
    } else {
        println!("  (no preferences in providers.json)");
    }
    if let Some(cp) = cat {
        println!(
            "  kind:          {}",
            cp.kind.as_deref().unwrap_or("unknown")
        );
        println!(
            "  base_url:      {}",
            cp.base_url.as_deref().unwrap_or("(none)")
        );
        println!(
            "  api_key_env:   {}",
            cp.api_key_env.as_deref().unwrap_or("(none)")
        );
        println!("  models:        {}", cp.models.join(", "));
    } else {
        println!("  (no entry in catalog.toml)");
    }

    let has_key = config::resolve_api_key(catalog, credentials, name).is_some();
    println!(
        "  api_key:       {}",
        if has_key { "resolved ✓" } else { "NOT FOUND" }
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Session persistence helpers
// ---------------------------------------------------------------------------

/// Open (or create) a per-project session journal, append a `session_info`
/// entry with the current cwd, and return the storage handle along with the
/// session id.
fn open_or_create_session(
    home: &config::TauHome,
    project_dir: &Path,
) -> anyhow::Result<(String, tau_coding::session::JsonlSessionStorage)> {
    let sessions_dir = home.root.join("sessions");
    let mgr = tau_coding::session::SessionManager::new(sessions_dir);
    let (path, storage) = mgr.create(project_dir)?;
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Seed the journal with a session_info row so the file is non-empty and
    // downstream tooling always sees the header first.
    let info = tau_types::SessionEntry::SessionInfo(tau_types::SessionInfoEntry {
        id: tau_types::message::new_entry_id(),
        parent_id: None,
        timestamp: tau_types::current_timestamp_secs(),
        r#type: tau_types::EntryType::SessionInfo,
        created_at: tau_types::current_timestamp_secs(),
        cwd: project_dir.to_str().map(|s| s.to_string()),
        title: None,
    });
    let rt = tokio::runtime::Handle::current();
    rt.block_on(storage.append(&info))?;
    Ok((session_id, storage))
}

/// Persist a single agent message as a `message` entry, then append a `leaf`
/// entry pointing at it (matching the Python journal shape: each completed
/// assistant/user turn is followed by a leaf row so the tree can be sketched).
async fn persist_message(
    storage: &tau_coding::session::JsonlSessionStorage,
    message: tau_types::AgentMessage,
) -> anyhow::Result<String> {
    let id = tau_types::message::new_entry_id();
    let entry = tau_types::SessionEntry::Message(Box::new(tau_types::MessageEntry {
        id: id.clone(),
        parent_id: None,
        timestamp: tau_types::current_timestamp_secs(),
        r#type: tau_types::EntryType::Message,
        message,
    }));
    storage.append(&entry).await?;
    storage
        .append(&tau_types::SessionEntry::Leaf(tau_types::LeafEntry {
            id: tau_types::message::new_entry_id(),
            parent_id: None,
            timestamp: tau_types::current_timestamp_secs(),
            r#type: tau_types::EntryType::Leaf,
            entry_id: Some(id.clone()),
        }))
        .await?;
    Ok(id)
}

/// Build the shared `AgentHarness` used by both print and REPL modes.
fn build_harness(
    provider: Arc<dyn ModelProvider + Send + Sync>,
    system: &str,
    model: &str,
    cwd: &Path,
) -> AgentHarness {
    let tools = tau_coding::tools::create_coding_tools(cwd);
    AgentHarness::new(AgentHarnessConfig {
        provider,
        model: model.to_string(),
        system: system.to_string(),
        tools,
        max_turns: Some(20),
        queue_mode: QueueMode::OneAtATime,
        before_tool_call: None,
        after_tool_call: None,
    })
}

// ---------------------------------------------------------------------------
// REPL — interactive loop over AgentHarness + session persistence
// ---------------------------------------------------------------------------

async fn run_repl(
    provider: Arc<dyn ModelProvider + Send + Sync>,
    system: &str,
    model: &str,
    cwd: &Path,
    home: &config::TauHome,
) -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut reader = stdin.lock().lines();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    let harness = build_harness(provider, system, model, cwd);

    let session = open_or_create_session(home, cwd)?;
    let (_session_id, storage) = session;
    if cli_verbose() {
        writeln!(out, "session: {}", storage.path().display())?;
    }

    loop {
        write!(out, "You: ")?;
        out.flush()?;

        let line = match reader.next() {
            Some(Ok(l)) => l,
            Some(Err(e)) => bail!("read error: {e}"),
            None => {
                writeln!(out)?;
                break;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        write!(out, "Assistant: ")?;
        out.flush()?;

        // Persist the user's message before driving the loop so the journal
        // ordering matches Python's (user, assistant, leaf).
        let _ = persist_message(
            &storage,
            tau_types::AgentMessage::User(tau_types::UserMessage::new(line.as_str())),
        )
        .await;

        let mut final_assistant: Option<tau_types::AgentMessage> = None;
        let stream = harness.prompt(&line)?;
        futures::pin_mut!(stream);
        while let Some(event) = stream.next().await {
            match event {
                AgentEvent::MessageUpdate(update) => {
                    if let tau_types::AssistantMessageEvent::TextDelta(delta) =
                        &update.assistant_message_event
                    {
                        write!(out, "{}", delta.delta)?;
                        out.flush()?;
                    }
                }
                AgentEvent::MessageEnd(end) => {
                    final_assistant = Some(end.message);
                }
                AgentEvent::AgentEnd(_) => {
                    writeln!(out)?;
                }
                _ => {}
            }
        }
        if let Some(msg) = final_assistant {
            let _ = persist_message(&storage, msg).await;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// --print mode — single-shot query, prints to stdout and persists session
// ---------------------------------------------------------------------------

async fn print_once(
    provider: Arc<dyn ModelProvider + Send + Sync>,
    system: &str,
    prompt: &str,
    model: &str,
    cwd: &Path,
    home: &config::TauHome,
) -> anyhow::Result<()> {
    let harness = build_harness(provider, system, model, cwd);

    // Persistence failures should never break print mode — log and continue.
    let session = open_or_create_session(home, cwd).ok();
    if let Some((_, ref storage)) = session {
        let _ = persist_message(
            storage,
            tau_types::AgentMessage::User(tau_types::UserMessage::new(prompt)),
        )
        .await;
    }

    let stream = harness.prompt(prompt)?;
    futures::pin_mut!(stream);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut got_content = false;
    let mut final_assistant: Option<tau_types::AgentMessage> = None;

    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::MessageUpdate(update) => {
                if let tau_types::AssistantMessageEvent::TextDelta(delta) =
                    &update.assistant_message_event
                {
                    got_content = true;
                    write!(out, "{}", delta.delta)?;
                    out.flush()?;
                }
            }
            AgentEvent::MessageEnd(end) => {
                final_assistant = Some(end.message);
            }
            AgentEvent::AgentEnd(_) => {
                writeln!(out)?;
            }
            _ => {}
        }
    }

    if let Some((_, storage)) = session {
        if let Some(msg) = final_assistant {
            let _ = persist_message(&storage, msg).await;
        }
    }

    if !got_content {
        bail!("no content received from provider (possible rate limit or empty response)");
    }
    Ok(())
}

/// Globally inspect verbosity. Kept as a tiny helper so the REPL/print paths
/// can surface session-path info without threading `cli` through call chains.
fn cli_verbose() -> bool {
    std::env::var("TAU_VERBOSE").is_ok()
}
