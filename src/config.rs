use std::{collections::HashMap, path::PathBuf};

use anyhow::{anyhow, Context};
use chrono::{NaiveTime, Timelike};
use directories::ProjectDirs;
use humantime::{Duration, Timestamp};
use serde::{de::Error, Deserialize, Serialize};
use tracing::{debug, info};

#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(serialize_with = "ser_duration")]
    #[serde(deserialize_with = "deser_duration")]
    pub check_interval: Duration,
    #[serde(serialize_with = "ser_duration")]
    #[serde(deserialize_with = "deser_duration")]
    pub update_interval: Duration,
    pub transitions: Vec<String>,
    #[serde(deserialize_with = "deser_images")]
    pub images: HashMap<String, Vec<ValidTime>>,
    pub image_dir: PathBuf,
    pub fps: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            check_interval: std::time::Duration::from_secs(60 * 5).into(),
            update_interval: std::time::Duration::from_secs(60 * 60).into(),
            transitions: Vec::new(),
            images: HashMap::new(),
            image_dir: PathBuf::default(),
            fps: 30,
        }
    }
}

#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct Cache {
    #[serde(serialize_with = "ser_timestamp")]
    #[serde(deserialize_with = "deser_timestamp")]
    pub last_update: Timestamp,
    pub last_transition: Option<String>,
    pub last_image: Option<PathBuf>,
}

impl Cache {
    pub fn update(&mut self, image: PathBuf, transition: String) {
        self.last_update = std::time::SystemTime::now().into();
        self.last_image = Some(image);
        self.last_transition = Some(transition);
    }
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            last_update: std::time::UNIX_EPOCH.into(),
            last_image: None,
            last_transition: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct State {
    pub cache: Cache,
    pub config: Config,
    project_dirs: ProjectDirs,
    pub rng: rand::rngs::ThreadRng,
}

impl State {
    pub fn project_dirs() -> anyhow::Result<ProjectDirs> {
        ProjectDirs::from("", "akida", "wallpaper")
            .ok_or_else(|| anyhow!("can't find project directories"))
    }

    pub fn load() -> anyhow::Result<Self> {
        let config = Config::default();
        let cache = Cache::default();

        let mut s = Self {
            config,
            cache,
            project_dirs: Self::project_dirs()?,
            rng: rand::thread_rng(),
        };
        s.reload()?;
        Ok(s)
    }

    pub fn reload(&mut self) -> anyhow::Result<()> {
        let cache_dir = self.project_dirs.cache_dir();
        if !cache_dir.is_dir() {
            info!("cache dir does not exist. Creating it now");
            std::fs::create_dir(cache_dir).context("while creating cache dir")?;
        }
        let cache_file = cache_dir.join("cache.json");
        if cache_file.is_file() {
            debug!("reading cache file");
            let file = std::fs::File::open(&cache_file).context("while opening cache file")?;
            let cache: Cache = serde_json::from_reader(file).context("while parsing cache file")?;
            if cache.last_image.is_some() {
                self.cache.last_image = cache.last_image;
            }
            if cache.last_transition.is_some() {
                self.cache.last_transition = cache.last_transition;
            }
            self.cache.last_update = cache.last_update;
        } else {
            info!(
                "no cache file found. Writing default to {}",
                cache_file.to_string_lossy()
            );
            self.save().context("while writing default cache file")?;
        }

        let config_dir = self.project_dirs.config_dir();
        if !config_dir.is_dir() {
            info!("config dir does not exist. Creating it now");
            std::fs::create_dir(config_dir).context("while creating config dir")?;
        }
        let config_file = config_dir.join("config.json");
        if config_file.is_file() {
            debug!("reading config file");
            let file = std::fs::File::open(&config_file).context("while opening config file")?;
            let config = serde_json::from_reader(file).context("while parsing config file")?;
            self.config = config;
        } else {
            info!(
                "no config file found. Writing default to {}",
                config_file.to_string_lossy()
            );
            let file = std::fs::File::create(config_file)
                .context("while opening config file for write")?;
            serde_json::to_writer(file, &self.config).context("while writing config file")?;
            debug!("created config file");
        }

        Ok(())
    }

    pub fn save(&self) -> anyhow::Result<()> {
        debug!("saving cache file");
        let cache_file = self.project_dirs.cache_dir().join("cache.json");
        let file =
            std::fs::File::create(cache_file).context("while opening cache file for write")?;
        serde_json::to_writer(file, &self.cache).context("while writing cache file")?;
        debug!("saved cache file");

        Ok(())
    }
}

fn ser_duration<S>(val: &Duration, ser: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let v = humantime::format_duration(**val).to_string();
    ser.serialize_str(&v)
}

fn deser_duration<'de, D>(deser: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deser)?;
    let duration = s.parse();

    duration.map_err(|e| D::Error::custom(format!("can't parse duration: {}", e)))
}

fn ser_timestamp<S>(val: &Timestamp, ser: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let v = humantime::format_rfc3339(**val).to_string();
    ser.serialize_str(&v)
}

fn deser_timestamp<'de, D>(deser: D) -> Result<Timestamp, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deser)?;
    let timestamp = s.parse();

    timestamp.map_err(|e| D::Error::custom(format!("can't parse timestamp: {}", e)))
}

fn deser_images<'de, D>(deser: D) -> Result<HashMap<String, Vec<ValidTime>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize, Debug)]
    #[serde(untagged)]
    enum OneOrMany {
        One(ValidTime),
        Vec(Vec<ValidTime>),
    }
    let s: HashMap<String, OneOrMany> = HashMap::deserialize(deser)?;
    Ok(s.into_iter()
        .map(|(k, v)| {
            (
                k,
                match v {
                    OneOrMany::Vec(v) => v,
                    OneOrMany::One(v) => vec![v],
                },
            )
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidTime {
    start: NaiveTime,
    end: NaiveTime,
}

impl ValidTime {
    pub fn matches(&self, time: &NaiveTime) -> bool {
        (self.start..=self.end).contains(time)
    }
}

impl serde::Serialize for ValidTime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        fn to_s(date: &NaiveTime) -> impl std::fmt::Display {
            if date.second() != 0 {
                date.format("%H:%M:%S")
            } else if date.minute() != 0 {
                date.format("%H:%M")
            } else {
                date.format("%H")
            }
        }
        let v = format!("{}-{}", to_s(&self.start), to_s(&self.end));
        serializer.serialize_str(&v)
    }
}

impl<'de> serde::Deserialize<'de> for ValidTime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?.trim().to_owned();
        let max = NaiveTime::from_hms_nano_opt(23, 59, 59, 1_999_999_999).unwrap();

        fn from_s<'de, D: serde::Deserializer<'de>>(
            s: &str,
            what: &str,
        ) -> Result<NaiveTime, D::Error> {
            if let Ok(t) = NaiveTime::parse_from_str(s, "%H:%M:%S") {
                Ok(t)
            } else if let Ok(t) = NaiveTime::parse_from_str(s, "%H:%M") {
                Ok(t)
            } else if let Ok(v) = s.parse() {
                if v == 24 {
                    let max = NaiveTime::from_hms_nano_opt(23, 59, 59, 1_999_999_999).unwrap();
                    Ok(max)
                } else {
                    NaiveTime::from_hms_opt(v, 0, 0).ok_or_else(|| {
                        D::Error::custom(format!("invalid hour for {} in {}", what, s))
                    })
                }
            } else {
                Err(D::Error::custom(format!(
                    "invalid time for {} in {}",
                    what, s
                )))
            }
        }

        let (start, end) = if s == "*" {
            (NaiveTime::MIN, max)
        } else if s.contains('-') {
            let (start_s, end_s) = s.split_once('-').unwrap();

            let start = from_s::<D>(start_s, "start")?;
            let end = from_s::<D>(end_s, "end")?;

            (start, end)
        } else {
            let v = from_s::<D>(&s, "single time")?;
            (v, v + chrono::Duration::hours(1))
        };

        Ok(Self { start, end })
    }
}
