mod config;

use std::path::PathBuf;

use anyhow::Context;
use rand::seq::SliceRandom;
use tracing::{debug, error, info, trace};

pub use crate::config::State;

pub fn init_sww() -> anyhow::Result<()> {
    debug!("initializing swww");
    std::process::Command::new("swww")
        .arg("init")
        .output()
        .context("while initializing swww")?;
    debug!("initialized swww");

    Ok(())
}

pub fn update_image(state: &mut State) -> anyhow::Result<()> {
    let mut get_image = || {
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
            .filter(|path| Some(path) != state.cache.last_image.as_ref())
            .collect();
        images.choose(&mut state.rng).cloned().unwrap_or_else(|| {
            PathBuf::from("/usr/share/backgrounds/sway/Sway_Wallpaper_Blue_1920x1080.png")
        })
    };
    let image = loop {
        let image = get_image();
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

    // swww img -t any --transition-step=2 --transition-fps=60 image_path.jpg
    if Some(&image) != state.cache.last_image.as_ref() {
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
            .arg(&image)
            .output()
            .context("while executing swww")?;

        if !cmd.status.success() {
            error!(
                "ewww returned error. Exit Code: {}.\nStdout: {}\n\nStderr:{}",
                cmd.status,
                String::from_utf8_lossy(&cmd.stdout),
                String::from_utf8_lossy(&cmd.stderr)
            );
        }
    } else {
        info!("not changing wallpaper because it is the same");
    }

    state.cache.update(image, transition);
    state.save().context("while saving cache")?;

    Ok(())
}
