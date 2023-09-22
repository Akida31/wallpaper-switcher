use anyhow::Context;
use clap::{Parser, Subcommand};
use tracing::{debug, error, info, warn, Level};
use tracing_subscriber::{
    filter::LevelFilter, fmt::writer::MakeWriterExt, layer::SubscriberExt, util::SubscriberInitExt,
    Layer,
};

use wallpaper::{get_monitors, init_sww, update_wallpapers, Monitors, State};

fn init_logging() -> anyhow::Result<()> {
    let project_dirs = State::project_dirs()?;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer().with_writer(
                tracing_appender::rolling::daily(
                    project_dirs.cache_dir().join("logs"),
                    "wallpaper.log",
                )
                .with_max_level(Level::TRACE),
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
    Switch {
        /// Only switch the wallpaper for this monitor
        monitor: Option<String>,
    },
    /// Check the config for errors
    Check,
    /// Print the current state and config
    Print,
}

fn print_state(state: &State) -> anyhow::Result<()> {
    println!("last update: {}", state.cache.last_update);
    for (monitor, transition) in &state.cache.last_transitions {
        println!("last transition for monitor {}: {}", monitor, transition);
    }
    for (monitor, image) in &state.cache.last_images {
        println!(
            "last image for monitor {}: {}",
            monitor,
            image.to_string_lossy()
        );
    }
    println!("check interval: {}", state.config.check_interval);
    println!("update interval: {}", state.config.update_interval);
    println!("transitions: {:#?}", state.config.transitions);
    let images: Vec<_> = state
        .config
        .images
        .iter()
        .map(|(name, times)| {
            let times = times
                .iter()
                .map(|time| time.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}: [{}]", name, times)
        })
        .collect();
    println!("images: {:#?}", images);
    println!(
        "image directory: {}",
        state.config.image_dir.to_string_lossy()
    );
    println!("fps: {}", state.config.fps);

    Ok(())
}

fn check(state: &State) -> anyhow::Result<()> {
    info!("checking the config for errors");

    for (file_path, times) in &state.config.images {
        let image = state.config.image_dir.join(file_path);
        if !image.is_file() {
            error!("image {} does not exist!", image.to_string_lossy());
        }
        for time in times {
            if let Err(e) = time.check() {
                error!(
                    "image {}: {}. Consider creating multiple time slots",
                    image.to_string_lossy(),
                    e
                );
            }
        }
    }

    let monitors = get_monitors()?;
    match &state.config.monitors {
        Monitors::Some(list) => {
            for monitor in list {
                if !monitors.contains(monitor) {
                    warn!("monitor {} not available", monitor);
                }
            }
        }
        Monitors::All => {}
    }

    info!("checked the config for errors");

    Ok(())
}

fn switch(state: &mut State, monitor: Option<String>) -> anyhow::Result<()> {
    init_sww()?;
    info!("switching one time");

    let monitor = match monitor {
        Some(monitor) => Monitors::Some(vec![monitor]),
        None => Monitors::All,
    };

    update_wallpapers(state, monitor).context("while updating state")?;

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
            // FIXME: allow setting only some monitors?
            update_wallpapers(state, Monitors::All).context("while updating state")?;
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
        Command::Switch { monitor } => switch(&mut state, monitor),
        Command::Check => check(&state),
        Command::Print => print_state(&state),
    }
}
