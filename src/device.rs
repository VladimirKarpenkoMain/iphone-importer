//! iPhone через usbmuxd (Apple Devices) → AFC. Блокирующий API поверх своего tokio-runtime.
use crate::import::{FetchError, RemoteFile, kind_of};
use idevice::afc::AfcClient;
use idevice::afc::opcode::AfcFopenMode;
use idevice::usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection};
use idevice::{IdeviceError, IdeviceService};
use std::io::Write;
use tokio::runtime::Runtime;

const CHUNK: u64 = 1 << 20;

#[derive(Debug)]
pub enum ConnectError {
    NoUsbmuxd,
    NoDevice,
    NotTrusted,
    Other(String),
}

pub struct Device {
    pub udid: String,
    rt: Runtime,
    afc: AfcClient,
}

/// Подключается к первому iPhone, подключённому по USB (Wi-Fi игнорируем).
pub fn connect() -> Result<Device, ConnectError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| ConnectError::Other(e.to_string()))?;
    let (udid, afc) = rt.block_on(async {
        let mut mux = UsbmuxdConnection::default().await.map_err(|_| ConnectError::NoUsbmuxd)?;
        let devices = mux.get_devices().await.map_err(|e| ConnectError::Other(e.to_string()))?;
        let dev = devices
            .into_iter()
            .find(|d| matches!(d.connection_type, Connection::Usb))
            .ok_or(ConnectError::NoDevice)?;
        let provider = dev.to_provider(UsbmuxdAddr::default(), "iphone-importer");
        let afc = AfcClient::connect(&provider).await.map_err(|e| match e {
            IdeviceError::InvalidHostID | IdeviceError::DeviceLocked | IdeviceError::NotFound => {
                ConnectError::NotTrusted
            }
            e => ConnectError::Other(e.to_string()),
        })?;
        Ok((dev.udid, afc))
    })?;
    Ok(Device { udid, rt, afc })
}

impl Device {
    /// Все фото и видео из `/DCIM/<папка>/`.
    pub fn list(&mut self) -> Result<Vec<RemoteFile>, IdeviceError> {
        let Device { rt, afc, .. } = self;
        rt.block_on(async {
            let mut out = vec![];
            for d in afc.list_dir("/DCIM").await? {
                let dir = format!("/DCIM/{d}");
                if d.starts_with('.') || afc.get_file_info(&dir).await?.st_ifmt != "S_IFDIR" {
                    continue;
                }
                for name in afc.list_dir(&dir).await? {
                    if name.starts_with('.') || kind_of(&name).is_none() {
                        continue;
                    }
                    let path = format!("{dir}/{name}");
                    let info = afc.get_file_info(&path).await?;
                    if info.st_ifmt == "S_IFREG" {
                        out.push(RemoteFile { path, size: info.size as u64 });
                    }
                }
            }
            Ok(out)
        })
    }

    /// Читает файл блоками по 1 МБ в `out`.
    pub fn fetch(&mut self, f: &RemoteFile, out: &mut dyn Write) -> Result<(), FetchError> {
        let Device { rt, afc, .. } = self;
        rt.block_on(async {
            let mut fd = afc.open(f.path.as_str(), AfcFopenMode::RdOnly).await.map_err(classify)?;
            let copied = async {
                let mut left = f.size;
                while left > 0 {
                    let chunk = fd.read_n(left.min(CHUNK) as usize).await.map_err(classify)?;
                    if chunk.is_empty() {
                        break;
                    }
                    left = left.saturating_sub(chunk.len() as u64);
                    out.write_all(&chunk).map_err(|e| FetchError::File(e.to_string()))?;
                }
                Ok(())
            }
            .await;
            let closed = fd.close().await.map_err(classify);
            copied.and(closed)
        })
    }

    /// Телефон всё ещё виден usbmuxd по USB.
    pub fn still_connected(&self) -> bool {
        self.rt.block_on(async {
            let Ok(mut mux) = UsbmuxdConnection::default().await else { return false };
            mux.get_devices()
                .await
                .map(|ds| ds.iter().any(|d| d.udid == self.udid && matches!(d.connection_type, Connection::Usb)))
                .unwrap_or(false)
        })
    }
}

/// Ошибка AFC про конкретный файл — пропускаем файл; всё остальное считаем обрывом связи.
fn classify(e: IdeviceError) -> FetchError {
    match e {
        IdeviceError::Afc(_) | IdeviceError::NotFound => FetchError::File(e.to_string()),
        e => FetchError::Connection(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Нужен подключённый и доверенный iPhone: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn lists_and_reads_from_real_iphone() {
        let mut dev = connect().expect("connect");
        let files = dev.list().expect("list");
        assert!(!files.is_empty(), "в /DCIM нет фото/видео");
        let small = files.iter().min_by_key(|f| f.size).unwrap();
        let mut buf = Vec::new();
        dev.fetch(small, &mut buf).expect("fetch");
        assert_eq!(buf.len() as u64, small.size);
        assert!(dev.still_connected());
        println!("{} файлов, прочитан {} ({} байт)", files.len(), small.path, small.size);
    }
}
