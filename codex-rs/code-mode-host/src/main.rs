use clap::Parser;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Debug, Parser)]
struct Cli {
    #[arg(long, hide = true, conflicts_with = "listen")]
    codex_ui_plugin_process: bool,

    /// Transport endpoint: `stdio`, `stdio://`, `grpc+stdio://`, or `grpc://IP:PORT`.
    #[arg(
        long,
        value_name = "URL",
        default_value = codex_code_mode_host::DEFAULT_LISTEN_URL
    )]
    listen: String,
}

#[tokio::main(flavor = "current_thread")]
async fn run(cli: Cli) -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .with_filter(tracing_subscriber::filter::LevelFilter::INFO),
        )
        .init();
    tracing::info_span!(
        "code_mode_host.startup",
        otel.name = "code_mode_host.startup"
    )
    .in_scope(|| {});

    codex_code_mode_host::run_main(&cli.listen).await
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if cli.codex_ui_plugin_process {
        return codex_code_mode_runtime::run_ui_sandbox().map_err(Into::into);
    }
    run(cli)
}
