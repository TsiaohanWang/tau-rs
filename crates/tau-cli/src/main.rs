mod config;

use std::io::{self, BufRead, Write};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use futures::StreamExt;
use tau_ai::anthropic::{AnthropicConfig, AnthropicProvider};
use tau_ai::openai::{OpenAIConfig, OpenAIProvider};
use tau_ai::stream::ProviderEvent;
use tau_types::{AgentMessage, UserMessage};

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
                match kind {
                    config::ProviderKind::Anthropic => {
                        let cfg = AnthropicConfig {
                            api_key,
                            base_url,
                            model,
                            max_tokens: cli.max_tokens,
                            max_retries,
                            timeout_seconds: timeout,
                            ..Default::default()
                        };
                        let provider = AnthropicProvider::new(cfg);
                        print_once_anthropic(&provider, &system, &prompt).await?;
                    }
                    config::ProviderKind::OpenaiCompatible => {
                        let cfg = OpenAIConfig {
                            api_key,
                            base_url,
                            model,
                            max_retries,
                            timeout_seconds: timeout,
                            ..Default::default()
                        };
                        let provider = OpenAIProvider::new(cfg);
                        print_once_openai(&provider, &system, &prompt).await?;
                    }
                }
            } else {
                if prompt_text.is_some() {
                    bail!("interactive mode does not accept a prompt; use --print with a prompt");
                }
                match kind {
                    config::ProviderKind::Anthropic => {
                        let cfg = AnthropicConfig {
                            api_key,
                            base_url,
                            model,
                            max_tokens: cli.max_tokens,
                            max_retries,
                            timeout_seconds: timeout,
                            ..Default::default()
                        };
                        let provider = AnthropicProvider::new(cfg);
                        eprintln!("tau-rs (anthropic) | type your message (Ctrl-D to exit)");
                        run_repl_anthropic(&provider, &system).await?;
                    }
                    config::ProviderKind::OpenaiCompatible => {
                        let cfg = OpenAIConfig {
                            api_key,
                            base_url,
                            model,
                            max_retries,
                            timeout_seconds: timeout,
                            ..Default::default()
                        };
                        let provider = OpenAIProvider::new(cfg);
                        eprintln!("tau-rs (openai) | type your message (Ctrl-D to exit)");
                        run_repl_openai(&provider, &system).await?;
                    }
                }
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
// REPL — consumes raw ProviderEvent streams directly
// ---------------------------------------------------------------------------

async fn run_repl_anthropic(provider: &AnthropicProvider, system: &str) -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut reader = stdin.lock().lines();
    let stdout = io::stdout();
    let mut out = stdout.lock();

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

        let messages: Vec<AgentMessage> = vec![AgentMessage::User(UserMessage::new(line.as_str()))];

        write!(out, "Assistant: ")?;
        out.flush()?;

        let stream = provider.stream_response(system, &messages, &[]);
        futures::pin_mut!(stream);
        while let Some(event) = stream.next().await {
            match event {
                ProviderEvent::TextDelta(delta) => {
                    write!(out, "{delta}")?;
                    out.flush()?;
                }
                ProviderEvent::ResponseEnd { .. } => {
                    writeln!(out)?;
                }
                ProviderEvent::Error { message, .. } => {
                    writeln!(out)?;
                    eprintln!("  [error: {message}]");
                }
                _ => {}
            }
        }
    }

    Ok(())
}
async fn run_repl_openai(provider: &OpenAIProvider, system: &str) -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut reader = stdin.lock().lines();
    let stdout = io::stdout();
    let mut out = stdout.lock();

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

        let messages: Vec<AgentMessage> = vec![AgentMessage::User(UserMessage::new(line.as_str()))];

        write!(out, "Assistant: ")?;
        out.flush()?;

        let stream = provider.stream_response(system, &messages, &[]);
        futures::pin_mut!(stream);
        while let Some(event) = stream.next().await {
            match event {
                ProviderEvent::TextDelta(delta) => {
                    write!(out, "{delta}")?;
                    out.flush()?;
                }
                ProviderEvent::ResponseEnd { .. } => {
                    writeln!(out)?;
                }
                ProviderEvent::Error { message, .. } => {
                    writeln!(out)?;
                    eprintln!("  [error: {message}]");
                }
                _ => {}
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// --print mode — single-shot query, prints to stdout and exits
// ---------------------------------------------------------------------------

async fn print_once_anthropic(
    provider: &AnthropicProvider,
    system: &str,
    prompt: &str,
) -> anyhow::Result<()> {
    let messages: Vec<AgentMessage> = vec![AgentMessage::User(UserMessage::new(prompt))];
    let stream = provider.stream_response(system, &messages, &[]);
    futures::pin_mut!(stream);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut got_content = false;
    while let Some(event) = stream.next().await {
        match event {
            ProviderEvent::TextDelta(delta) => {
                got_content = true;
                write!(out, "{delta}")?;
                out.flush()?;
            }
            ProviderEvent::ResponseEnd { message, .. } => {
                if let Some(err) = &message.error_message {
                    writeln!(out)?;
                    bail!("provider error: {err}");
                }
            }
            ProviderEvent::Error { message, .. } => {
                writeln!(out)?;
                bail!("provider error: {message}");
            }
            _ => {}
        }
    }
    writeln!(out)?;
    if !got_content {
        bail!("no content received from provider (possible rate limit or empty response)");
    }
    Ok(())
}

async fn print_once_openai(
    provider: &OpenAIProvider,
    system: &str,
    prompt: &str,
) -> anyhow::Result<()> {
    let messages: Vec<AgentMessage> = vec![AgentMessage::User(UserMessage::new(prompt))];
    let stream = provider.stream_response(system, &messages, &[]);
    futures::pin_mut!(stream);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut got_content = false;
    while let Some(event) = stream.next().await {
        match event {
            ProviderEvent::TextDelta(delta) => {
                got_content = true;
                write!(out, "{delta}")?;
                out.flush()?;
            }
            ProviderEvent::ResponseEnd { message, .. } => {
                if let Some(err) = &message.error_message {
                    writeln!(out)?;
                    bail!("provider error: {err}");
                }
            }
            ProviderEvent::Error { message, .. } => {
                writeln!(out)?;
                bail!("provider error: {message}");
            }
            _ => {}
        }
    }
    writeln!(out)?;
    if !got_content {
        bail!("no content received from provider (possible rate limit or empty response)");
    }
    Ok(())
}
