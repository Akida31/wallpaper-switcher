use anyhow::Context;
use tracing::{debug, info, Level};
use tracing_subscriber::{
    filter::LevelFilter, fmt::writer::MakeWriterExt, layer::SubscriberExt, util::SubscriberInitExt,
    Layer,
};

use wallpaper::{update_image, State};

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

fn main() -> anyhow::Result<()> {
    init_logging()?;
    let mut state = State::load().context("while loading state")?;

    info!("starting mainloop");

    loop {
        debug!("reloading state");
        state.reload().context("while reloading state")?;
        debug!("reloaded state");

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
            update_image(&mut state).context("while updating state")?;
        }

        let to_sleep = check_interval - (current_time % check_interval);

        debug!("waiting for next time :)");
        std::thread::sleep(std::time::Duration::from_nanos(
            to_sleep.try_into().context("can't sleep that long")?,
        ));
    }
}
