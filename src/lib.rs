mod config;

use std::{collections::HashSet, path::PathBuf};

use anyhow::Context;
use rand::seq::SliceRandom;
use tracing::{debug, error, info, trace};

pub use crate::config::{Monitors, State};

pub fn init_sww() -> anyhow::Result<()> {
    debug!("initializing swww");
    std::process::Command::new("swww")
        .arg("init")
        .output()
        .context("while initializing swww")?;
    debug!("initialized swww");

    Ok(())
}

pub fn get_monitors() -> anyhow::Result<HashSet<String>> {
    info!("trying to query monitors");
    let cmd = std::process::Command::new("swww")
        .arg("query")
        .output()
        .context("while trying to query monitors")?;
    if !cmd.status.success() {
        error!(
            "swww returned error. Exit Code: {}.\nStdout: {}\n\nStderr:{}",
            cmd.status,
            String::from_utf8_lossy(&cmd.stdout),
            String::from_utf8_lossy(&cmd.stderr)
        );
    }
    let stdout = String::from_utf8(cmd.stdout).context("invalid command output from swww query")?;
    stdout
        .lines()
        .map(|line| {
            let (name, _rest) = line
                .split_once(':')
                .ok_or_else(|| anyhow::anyhow!("invalid line in output: {}", line))?;
            Ok(name.to_owned())
        })
        .collect()
}

pub fn update_wallpapers(state: &mut State, monitors: Monitors) -> anyhow::Result<()> {
    let get_image = |last_images: &HashSet<PathBuf>, rng: &mut rand::rngs::ThreadRng| {
        let now = chrono::offset::Local::now().time();
        let images: Vec<_> = state
            .config
            .images
            .iter()
            .filter(|(path, time)| {
                let res = time.iter().any(|t| t.matches(&now));
                trace!("{} is valid? {}", path, res);
                res
            })
            .map(|(path, _time)| state.config.image_dir.join(path))
            .filter(|path| !last_images.contains(path))
            .collect();
        images.choose(rng).cloned().unwrap_or_else(|| {
            PathBuf::from("/usr/share/backgrounds/sway/Sway_Wallpaper_Blue_1920x1080.png")
        })
    };
    let connected_monitors = get_monitors()?;
    let monitors = match monitors {
        Monitors::All => connected_monitors.clone(),
        Monitors::Some(monitors) => monitors
            .into_iter()
            .filter(|monitor| {
                if connected_monitors.contains(monitor) {
                    true
                } else {
                    error!("ignoring not connected monitor {}", monitor);
                    false
                }
            })
            .collect(),
    };
    if monitors.is_empty() {
        if connected_monitors.is_empty() {
            return Err(anyhow::anyhow!("no monitors connected"));
        } else {
            info!("valid monitors: {}", connected_monitors.into_iter().collect::<Vec<_>>().join(", "));
            return Err(anyhow::anyhow!("no valid monitor available"));
        }
    }

    let last_images = state.cache.last_images.values().cloned().collect();
    for monitor in monitors {
        let last_image = state.cache.last_images.get(&monitor).cloned();

        let image = loop {
            let image = get_image(&last_images, &mut state.rng);
            if image.is_file() {
                break image;
            } else {
                error!("image {} does not exist!", image.to_string_lossy());
            }
        };
        let transition = state
            .config
            .transitions
            .choose(&mut state.rng)
            .cloned()
            .unwrap_or_else(|| String::from("simple"));

        // swww img --transition-step=2 --transition-fps=60 --transition-type any --output monitor image_path.jpg
        if Some(&image) != last_image.as_ref() {
            info!(
                "updating to {} with transition {}",
                image.to_string_lossy(),
                &transition
            );
            let cmd = std::process::Command::new("swww")
                .args(["img", "--transition-step=2", "--transition-fps"])
                .arg(state.config.fps.to_string())
                .arg("--transition-type")
                .arg(&transition)
                .arg("--outputs")
                .arg(&monitor)
                .arg(&image)
                .output()
                .context("while executing swww")?;

            if !cmd.status.success() {
                error!(
                    "swww returned error. Exit Code: {}.\nStdout: {}\n\nStderr:{}",
                    cmd.status,
                    String::from_utf8_lossy(&cmd.stdout),
                    String::from_utf8_lossy(&cmd.stderr)
                );
            }
        } else {
            info!("not changing wallpaper because it is the same");
        }

        state.cache.update(monitor, image, transition);
        state.save().context("while saving cache")?;
    }

    Ok(())
}
