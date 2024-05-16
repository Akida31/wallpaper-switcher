use std::{
    io::{BufRead, BufReader, Write},
    net::Shutdown,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::mpsc::{channel, Receiver, Sender},
    thread,
};

use anyhow::{Context, Result};
use tracing::{debug, error, warn};

#[must_use]
fn get_socket_path() -> PathBuf {
    let mut socket_path = if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(dir)
    } else {
        PathBuf::from("/tmp/wallpaper")
    };
    socket_path.push("wallpaper.socket");

    socket_path
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum IpcEvent {
    /// Reload the state
    Reload,
    /// Set a new image now
    Switch {
        /// Only switch the wallpaper for this monitor
        monitor: Option<String>,
    },
    /// Select an image (or folder of images) which will be shown
    Select {
        path: String,
        /// whether to keep the old images
        keep_old: bool,
    },
}

#[derive(Debug)]
pub struct Listener {
    inner: Receiver<IpcEvent>,
    socket_path: PathBuf,
}

impl Listener {
    pub fn bind() -> Result<Self> {
        let socket_path = get_socket_path();
        debug!("connecting listener to {}", socket_path.display());
        let listener = UnixListener::bind(&socket_path).context("connecting listener to socket")?;

        let (sender, recv) = channel();

        thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let sender = sender.clone();
                        thread::spawn(move || handle_client(stream, sender));
                    }
                    Err(e) => error!("can't connect to client: {}", e),
                }
            }
        });

        Ok(Self {
            inner: recv,
            socket_path,
        })
    }
}

impl std::ops::Deref for Listener {
    type Target = Receiver<IpcEvent>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // There's no way to return a useful error here
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn handle_client(stream: UnixStream, sender: Sender<IpcEvent>) {
    let mut buf = String::new();
    let mut stream = BufReader::new(stream);
    loop {
        match stream.read_line(&mut buf) {
            Ok(read) => {
                if read == 0 {
                    // EOF
                    continue;
                }
            }
            Err(e) => {
                error!("stream returned error: {}", e);
                break;
            }
        };
        if buf.is_empty() {
            // TODO enable this
            // debug!("empty message");
            continue;
        }
        match serde_json::from_str(&buf) {
            Ok(msg) => {
                if let Err(e) = sender.send(msg) {
                    error!("can't send message to daemon receiver: {}", e);
                    return;
                }
            }
            Err(e) => {
                error!("invalid ipc message: {}", e);
                warn!("message was: {}", buf);
                // TODO remove this
                return;
            }
        };
        buf.clear();
    }
}

pub struct Client {
    inner: Sender<IpcEvent>,
    // wait that the sended message gets actually send
    fin_recv: Receiver<()>,
}

impl Client {
    pub fn connect() -> Result<Self> {
        let socket_path = get_socket_path();
        debug!("connecting sender to {}", socket_path.display());
        let mut stream = UnixStream::connect(socket_path).context("connecting sender to socket")?;

        let (sender, recv) = channel();
        let (fin_sender, fin_recv) = channel();

        let handle = thread::spawn(move || {
            for event in recv.iter() {
                debug!("received event");
                let mut buf = match serde_json::to_vec(&event) {
                    Ok(b) => b,
                    Err(e) => {
                        error!(
                            "can't serialize event {:?} before sending it to socket: {}",
                            event, e
                        );
                        continue;
                    }
                };
                buf.push(b'\n');
                debug!("sending message to daemon");
                if let Err(e) = stream.write_all(&buf) {
                    error!("can't send event {:?} to socket: {}", event, e);
                }
                let _ = fin_sender.send(());
            }
            warn!("ipc sender disconnected");
            let _ = stream.shutdown(Shutdown::Write);
        });

        assert!(!handle.is_finished());

        debug!("connected sender");

        Ok(Self {
            inner: sender,
            fin_recv,
        })
    }

    pub fn send(&self, event: IpcEvent) -> Result<()> {
        self.inner.send(event)?;
        self.fin_recv.recv()?;

        Ok(())
    }
}
