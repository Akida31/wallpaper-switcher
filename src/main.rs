use anyhow::Context;
use clap::{Parser, Subcommand};
use tracing::{debug, info, Level, error};
use tracing_subscriber::{
    filter::LevelFilter, fmt::writer::MakeWriterExt, layer::SubscriberExt, util::SubscriberInitExt,
    Layer,
};

use wallpaper::{update_image, State, init_sww};

fn init_logging() -> anyhow::Result<()> {
    let project_dirs = State::project_dirs()?;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer().with_writer(
                tracing_appender::rolling::daily(
                    project_dirs.cache_dir().join("logs"),
                    "wallpaper.log",
                )
                .with_max_level(Level::DEBUG),
            ),
        )
        .with(
            tracing_subscriber::fmt::layer().with_filter(
                tracing_subscriber::EnvFilter::builder()
                    .with_default_directive(tracing_subscriber::filter::Directive::from(
                        LevelFilter::INFO,
                    ))
                    .from_env_lossy(),
            ),
        )
        .init();

    Ok(())
}

#[derive(Parser, Debug)]
struct Args {
    /// Subcommand to run
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the daemon which changes the wallpaper at specific times
    Daemon,
    /// Set a new image now
    Switch,
    /// Check the config for errors
    Check,
}

fn check(state: &mut State) -> anyhow::Result<()> {
    info!("checking the config for errors");

    for file_path in state.config.images.keys() {
        let image = state.config.image_dir.join(file_path);
        if !image.is_file() {
            error!("image {} does not exist!", image.to_string_lossy());
        }
    }

    info!("checked the config for errors");

    Ok(())
}

fn switch(state: &mut State) -> anyhow::Result<()> {
    init_sww()?;
    info!("switching one time");

    update_image(state).context("while updating state")?;

    info!("switched one time");
    Ok(())
}

fn daemon(state: &mut State) -> anyhow::Result<()> {
    init_sww()?;
    info!("starting mainloop");

    loop {
        let check_interval = state.config.check_interval.as_nanos();
        let update_interval = state.config.update_interval.as_nanos();

        let current_time = std::time::UNIX_EPOCH
            .elapsed()
            .context("after unix epoch")?
            .as_nanos();

        let last_time = state
            .cache
            .last_update
            .duration_since(std::time::UNIX_EPOCH)
            .context("after unix epoch")?
            .as_nanos();

        if last_time / update_interval < current_time / update_interval {
            info!("updating wallpaper");
            update_image(state).context("while updating state")?;
        }

        let to_sleep = check_interval - (current_time % check_interval);

        debug!("waiting for next time :)");
        std::thread::sleep(std::time::Duration::from_nanos(
            to_sleep.try_into().context("can't sleep that long")?,
        ));

        debug!("reloading state");
        state.reload().context("while reloading state")?;
        debug!("reloaded state");
    }
}

fn main() -> anyhow::Result<()> {
    init_logging()?;

    let args = Args::parse();

    let mut state = State::load().context("while loading state")?;

    match args.command {
        Command::Daemon => daemon(&mut state),
        Command::Switch => switch(&mut state),
        Command::Check => check(&mut state),
    }
}
