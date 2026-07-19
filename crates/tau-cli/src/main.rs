#![deny(unsafe_code)]

mod config;
mod render;
mod repl;
#[cfg(feature = "tui")]
mod tui;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use futures::StreamExt;
use tau_agent::harness::{AgentHarness, AgentHarnessConfig, QueueMode};
use tau_agent::provider::ModelProvider;
use tau_ai::anthropic::{AnthropicConfig, AnthropicModelProvider, AnthropicProvider};
use tau_ai::openai::{OpenAIConfig, OpenAIModelProvider, OpenAIProvider};
use tau_coding::config::{CatalogConfig, ProviderKind, load_user_or_default};
use tau_types::{AgentEvent, AssistantMessageEvent};

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

    /// Resume a previous session. Pass "latest" or a session ID.
    #[arg(short = 'r', long)]
    resume: Option<String>,

    /// Output format: plain | json | transcript
    #[arg(long, default_value = "plain")]
    format: String,

    /// Use the interactive ratatui TUI (requires the `tui` feature at build time)
    #[arg(long)]
    tui: bool,
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

    if !render::FORMATS.contains(&cli.format.as_str()) {
        bail!(
            "unknown --format '{}' (expected one of: {})",
            cli.format,
            render::FORMATS.join(", ")
        );
    }

    let providers =
        config::ProvidersConfig::load(&home.providers_path()).context("loading providers.json")?;
    let credentials = config::CredentialsConfig::load(&home.credentials_path())
        .context("loading credentials.json")?;
    let catalog = load_user_or_default(&home.catalog_path()).context("loading catalog.toml")?;

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
                            .providers
                            .iter()
                            .find(|p| p.name == provider_name)
                            .and_then(|p| p.default_model.clone())
                    })
                    .unwrap_or_else(|| "gpt-4o".to_string())
            });

            // System prompt: pass-through as `Option<String>`. The
            // `CodingSession` builder routes `None` to the default in
            // `build_system_prompt`, and assembled tool snippets are layered on
            // top regardless of user system. This wires architecture-issues
            // #10 — the hard-coded "You are a helpful assistant." fallback is
            // now dead, replaced by `prompt.rs::DEFAULT_SYSTEM_PROMPT`.
            let system: Option<String> = cli.system.clone();

            let prefs = providers.provider_preferences.get(provider_name);
            let max_retries = prefs.and_then(|p| p.max_retries).unwrap_or(5);
            let timeout = prefs
                .and_then(|p| p.timeout_seconds)
                .map(|s| s as u64)
                .unwrap_or(60);

            let kind = catalog
                .providers
                .iter()
                .find(|p| p.name == provider_name)
                .map(ProviderKind::from_provider)
                .unwrap_or(ProviderKind::OpenaiCompatible);

            let base_url = catalog
                .providers
                .iter()
                .find(|p| p.name == provider_name)
                .and_then(|p| p.base_url.clone())
                .unwrap_or_else(|| match kind {
                    ProviderKind::Anthropic => "https://api.anthropic.com".to_string(),
                    ProviderKind::OpenaiCompatible | ProviderKind::OpenaiResponses => {
                        "https://api.openai.com".to_string()
                    }
                });

            let prompt_text = if cli.prompt.is_empty() {
                None
            } else {
                Some(cli.prompt.join(" "))
            };

            let cwd = std::env::current_dir().context("failed to get current directory")?;

            // Resolve resume target before provider construction so we know
            // early whether there is something to resume.
            let resume_id = match cli.resume.as_deref() {
                Some("latest") | Some("") => {
                    let sessions_dir = home.root.join("sessions");
                    let mgr = tau_coding::session::SessionManager::new(sessions_dir);
                    let entry = mgr
                        .latest_for_project(&cwd)
                        .await
                        .context("looking up latest session")?
                        .ok_or_else(|| anyhow::anyhow!("no sessions found for this project"))?;
                    Some(entry.session_id)
                }
                Some(id) => Some(id.to_string()),
                None => None,
            };

            let model_provider = build_provider(
                kind,
                &api_key,
                &base_url,
                &model,
                cli.max_tokens,
                max_retries,
                timeout,
            );

            if cli.print {
                let prompt = prompt_text.context("--print requires a prompt argument")?;
                if let Some(ref session_id) = resume_id {
                    resume_print_once(
                        model_provider,
                        system,
                        &prompt,
                        model,
                        &cwd,
                        &home,
                        session_id,
                        &cli.format,
                    )
                    .await?;
                } else {
                    print_once(
                        model_provider,
                        system,
                        &prompt,
                        model,
                        &cwd,
                        &home,
                        &cli.format,
                    )
                    .await?;
                }
            } else {
                if prompt_text.is_some() {
                    bail!("interactive mode does not accept a prompt; use --print with a prompt");
                }
                let history_path = home.root.join("history");
                if cli.tui {
                    #[cfg(feature = "tui")]
                    {
                        let session = match resume_id {
                            Some(ref id) => resume_session(
                                model_provider,
                                system,
                                model,
                                cwd.to_path_buf(),
                                &home,
                                id,
                            )
                            .await
                            .context("resuming session for TUI")?,
                            None => open_or_create_session(
                                model_provider,
                                system,
                                model,
                                cwd.to_path_buf(),
                                &home,
                            )
                            .await
                            .context("opening session for TUI")?,
                        };
                        crate::tui::app::run(
                            session,
                            &cwd,
                            &history_path,
                            cli.verbose,
                            &cli.format,
                        )
                        .await?;
                        return Ok(());
                    }
                    #[cfg(not(feature = "tui"))]
                    {
                        bail!(
                            "this build was compiled without the `tui` feature; rebuild with --features tui"
                        );
                    }
                }
                let session = match resume_id {
                    Some(ref id) => {
                        resume_session(model_provider, system, model, cwd.to_path_buf(), &home, id)
                            .await
                            .context("resuming session for REPL")?
                    }
                    None => open_or_create_session(
                        model_provider,
                        system,
                        model,
                        cwd.to_path_buf(),
                        &home,
                    )
                    .await
                    .context("opening session")?,
                };
                repl::run(session, &cwd, &history_path, cli.verbose, &cli.format).await?;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Provider construction
// ---------------------------------------------------------------------------

/// Build a `ModelProvider` trait object from the catalog kind and resolved
/// configuration. Handles Anthropic, OpenAI-compatible, and OpenAI-responses
/// dispatch in one place (previously duplicated across print/REPL branches).
fn build_provider(
    kind: ProviderKind,
    api_key: &str,
    base_url: &str,
    model: &str,
    max_tokens: Option<u32>,
    max_retries: u32,
    timeout: u64,
) -> Arc<dyn ModelProvider + Send + Sync> {
    match kind {
        ProviderKind::Anthropic => {
            let cfg = AnthropicConfig {
                api_key: api_key.to_string(),
                base_url: base_url.to_string(),
                model: model.to_string(),
                max_tokens,
                max_retries,
                timeout_seconds: timeout,
                ..Default::default()
            };
            Arc::new(AnthropicModelProvider::new(AnthropicProvider::new(cfg)))
        }
        ProviderKind::OpenaiCompatible | ProviderKind::OpenaiResponses => {
            let cfg = OpenAIConfig {
                api_key: api_key.to_string(),
                base_url: base_url.to_string(),
                model: model.to_string(),
                max_tokens,
                max_retries,
                timeout_seconds: timeout,
                ..Default::default()
            };
            Arc::new(OpenAIModelProvider::new(OpenAIProvider::new(cfg)))
        }
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_providers(catalog: &CatalogConfig) {
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
    catalog: &CatalogConfig,
    credentials: &config::CredentialsConfig,
) -> anyhow::Result<()> {
    let prefs = providers.provider_preferences.get(name);
    let cat = catalog.providers.iter().find(|p| p.name == name);

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
// Session + CodingSession construction
// ---------------------------------------------------------------------------

/// Open (or create on first run) a session file under `~/.tau/sessions` and
/// wrap it in a fresh `CodingSession`. The `SessionInfo` entry is written on
/// this first call; subsequent `--resume` (5.2) will reuse the existing file.
async fn open_or_create_session(
    provider: Arc<dyn ModelProvider + Send + Sync>,
    system: Option<String>,
    model: String,
    cwd: PathBuf,
    home: &config::TauHome,
) -> anyhow::Result<tau_coding::session::CodingSession> {
    let sessions_dir = home.root.join("sessions");
    let mgr = tau_coding::session::SessionManager::new(sessions_dir);
    let (_path, storage) = mgr.create(&cwd).await?;

    let cfg = tau_coding::session::CodingSessionConfig {
        provider,
        model,
        system,
        cwd,
        context_window: None,
        compaction_reserve: tau_coding::session::DEFAULT_RESERVE,
        max_turns: Some(20),
        provider_name: None,
        thinking_level: None,
    };
    let mut session = tau_coding::session::CodingSession::new(storage, cfg);
    session.write_session_info().await?;
    Ok(session)
}

/// Open an existing session by ID from `~/.tau/sessions` and wrap it in a
/// loaded `CodingSession`. Returns an error if the session does not exist.
async fn resume_session(
    provider: Arc<dyn ModelProvider + Send + Sync>,
    system: Option<String>,
    model: String,
    cwd: PathBuf,
    home: &config::TauHome,
    session_id: &str,
) -> anyhow::Result<tau_coding::session::CodingSession> {
    let sessions_dir = home.root.join("sessions");
    let mgr = tau_coding::session::SessionManager::new(sessions_dir);
    let storage = mgr.load(&cwd, session_id).await?;

    let cfg = tau_coding::session::CodingSessionConfig {
        provider,
        model,
        system,
        cwd,
        context_window: None,
        compaction_reserve: tau_coding::session::DEFAULT_RESERVE,
        max_turns: Some(20),
        provider_name: None,
        thinking_level: None,
    };
    tau_coding::session::CodingSession::load(storage, cfg)
        .await
        .context("loading resumed session")
}

// ---------------------------------------------------------------------------
// --print mode
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn print_once(
    provider: Arc<dyn ModelProvider + Send + Sync>,
    system: Option<String>,
    prompt: &str,
    model: String,
    cwd: &Path,
    home: &config::TauHome,
    format: &str,
) -> anyhow::Result<()> {
    let mut session = match open_or_create_session(
        provider.clone(),
        system,
        model,
        cwd.to_path_buf(),
        home,
    )
    .await
    {
        Ok(s) => Some(s),
        Err(_) => {
            // Could not open a session file — fall back to a single-turn
            // ephemeral harness call so `--print` still produces output for
            // pipelines.
            return ephemeral_print(provider, prompt, cwd, format).await;
        }
    };

    let mut got_content = false;
    let mut renderer = render::build_renderer(format);
    let tools = session.as_ref().unwrap().tools().to_vec();

    // The stream borrows `&mut session`; advance it inside this scope and drop
    // it before `session` itself does. RAII keeps the borrow chain intact.
    {
        let stream = session.as_mut().unwrap().prompt(prompt)?;
        futures::pin_mut!(stream);
        while let Some(event) = stream.next().await {
            if let AgentEvent::MessageUpdate(update) = &event {
                if let AssistantMessageEvent::TextDelta(_) = &update.assistant_message_event {
                    got_content = true;
                }
            }
            renderer.on_event(&event, &tools);
        }
        renderer.flush();
    }

    if !got_content {
        bail!("no content received from provider (possible rate limit or empty response)");
    }
    Ok(())
}

/// Fallback used when session storage cannot be opened — runs a single-turn
/// harness call and writes assistant text deltas to stdout. Never persists.
async fn ephemeral_print(
    provider: Arc<dyn ModelProvider + Send + Sync>,
    prompt: &str,
    cwd: &Path,
    format: &str,
) -> anyhow::Result<()> {
    let tools = tau_coding::tools::create_coding_tools(cwd);
    let harness = AgentHarness::new(AgentHarnessConfig {
        provider,
        model: String::new(),
        system: tau_coding::prompt::build_system_prompt(&tools, ""),
        tools: tools.clone(),
        max_turns: Some(20),
        queue_mode: QueueMode::OneAtATime,
        thinking_level: None,
        before_tool_call: None,
        after_tool_call: None,
    });
    let stream = harness.prompt(prompt)?;
    futures::pin_mut!(stream);
    let mut got_content = false;
    let mut renderer = render::build_renderer(format);

    while let Some(event) = stream.next().await {
        if let AgentEvent::MessageUpdate(update) = &event {
            if let AssistantMessageEvent::TextDelta(_) = &update.assistant_message_event {
                got_content = true;
            }
        }
        renderer.on_event(&event, &tools);
    }
    renderer.flush();
    if !got_content {
        bail!("no content received from provider (possible rate limit or empty response)");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// --print mode (resumed session)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn resume_print_once(
    provider: Arc<dyn ModelProvider + Send + Sync>,
    system: Option<String>,
    prompt: &str,
    model: String,
    cwd: &Path,
    home: &config::TauHome,
    session_id: &str,
    format: &str,
) -> anyhow::Result<()> {
    let mut session = resume_session(provider, system, model, cwd.to_path_buf(), home, session_id)
        .await
        .context("resuming session for --print")?;

    let mut got_content = false;
    let mut renderer = render::build_renderer(format);
    let tools = session.tools().to_vec();

    {
        let stream = session.prompt(prompt)?;
        futures::pin_mut!(stream);
        while let Some(event) = stream.next().await {
            if let AgentEvent::MessageUpdate(update) = &event {
                if let AssistantMessageEvent::TextDelta(_) = &update.assistant_message_event {
                    got_content = true;
                }
            }
            renderer.on_event(&event, &tools);
        }
        renderer.flush();
    }

    if !got_content {
        bail!("no content received from provider (possible rate limit or empty response)");
    }
    Ok(())
}
