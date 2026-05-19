use bytes::Bytes;
use tokio::sync::mpsc;

pub struct TeeArms {
    pub client: mpsc::Receiver<Bytes>,
    pub telemetry: mpsc::Receiver<Bytes>,
}

pub fn spawn_tee(_client_buf: usize, _telemetry_buf: usize) -> TeeArms {
    let (_c_tx, c_rx) = mpsc::channel(16);
    let (_t_tx, t_rx) = mpsc::channel(64);
    TeeArms {
        client: c_rx,
        telemetry: t_rx,
    }
}
