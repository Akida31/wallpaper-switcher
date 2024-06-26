use std::{collections::BTreeMap, path::Path, sync::mpsc::RecvTimeoutError};

use anyhow::Context;
use clap::{Parser, Subcommand};
use tracing::{debug, error, info, warn, Level};
use tracing_subscriber::{
    filter::LevelFilter, fmt::writer::MakeWriterExt, layer::SubscriberExt, util::SubscriberInitExt,
    Layer,
};

use wallpaper::{
    get_monitors, init_sww,
    ipc::{self, IpcEvent},
    update_wallpapers, Monitors, State, ValidTime,
};

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
    debug!("hello world, logging initialized :)");

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
    /// Select an image (or folder of images) which will be shown
    Select {
        path: String,
        /// whether to keep the old images
        #[arg(default_value_t = false)]
        keep_old: bool,
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
                .map(ToString::to_string)
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
    info!("switching one time");

    let monitor = match monitor {
        Some(monitor) => Monitors::Some(vec![monitor]),
        None => Monitors::All,
    };

    update_wallpapers(state, monitor).context("while updating state")?;

    info!("switched one time");
    Ok(())
}

fn select(state: &mut State, path: &str, keep_old: bool) -> anyhow::Result<()> {
    fn get_images_rec(path: &Path) -> anyhow::Result<BTreeMap<String, Vec<ValidTime>>> {
        let mut res = BTreeMap::new();
        if path.is_file() {
            let path_s = path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("path {} is not valid utf-8", path.display()))?
                .to_string();
            res.insert(path_s, vec![ValidTime::ALL]);
        } else {
            for entry in std::fs::read_dir(path).context("reading image directory")? {
                let entry = entry.context("getting image directory entry")?;
                res.extend(get_images_rec(&entry.path())?);
            }
        }
        Ok(res)
    }

    let new_images = get_images_rec(path.as_ref())?;

    info!(
        "selected image path {} with {} images",
        path,
        new_images.len()
    );

    if keep_old {
        state.config.images.extend(new_images);
    } else {
        state.config.images = new_images;
    }

    update_wallpapers(state, Monitors::All).context("while updating state")?;

    Ok(())
}

fn daemon(state: &mut State) -> anyhow::Result<()> {
    init_sww()?;

    let listener = ipc::Listener::bind().context("while starting ipc server")?;

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
        let sleep_duration =
            std::time::Duration::from_nanos(to_sleep.try_into().context("can't sleep that long")?);

        let mut handle_msg = |msg| match msg {
            IpcEvent::Reload => {
                debug!("reloading state (ipc)");
                if let Err(e) = state.force_reload() {
                    error!("can't reload state: {}", e);
                }
                debug!("reloaded state (ipc)");
            }
            IpcEvent::Switch { monitor } => {
                if let Err(e) = switch(state, monitor) {
                    error!("can't switch wallpaper: {}", e);
                }
            }
            IpcEvent::Select { path, keep_old } => {
                if let Err(e) = select(state, &path, keep_old) {
                    error!("can't select wallpaper: {}", e);
                }
            }
        };

        match listener.recv_timeout(sleep_duration) {
            Ok(msg) => {
                handle_msg(msg);
                // process pending messages
                while let Ok(msg) = listener.try_recv() {
                    handle_msg(msg);
                }
            }
            Err(e) => match e {
                RecvTimeoutError::Timeout => {}
                RecvTimeoutError::Disconnected => todo!(),
            },
        }

        debug!("reloading state");
        state.reload().context("while reloading state")?;
        debug!("reloaded state");
    }
}

fn run_ipc(msg: IpcEvent) -> anyhow::Result<()> {
    let sender = ipc::Client::connect()?;
    sender.send(msg)?;

    Ok(())
}

fn main() -> anyhow::Result<()> {
    init_logging()?;

    let args = Args::parse();

    let mut state = State::load().context("while loading state")?;

    match args.command {
        Command::Daemon => daemon(&mut state),
        Command::Switch { monitor } => run_ipc(IpcEvent::Switch { monitor }),
        Command::Select { path, keep_old } => run_ipc(IpcEvent::Select { path, keep_old }),
        Command::Check => check(&state),
        Command::Print => print_state(&state),
    }
}
