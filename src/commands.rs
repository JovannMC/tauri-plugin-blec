use tauri::ipc::Channel;
use tauri::{async_runtime, command, AppHandle, Runtime};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::Result;
use crate::{get_handler, Error};
use crate::models::{BleDevice, ScanFilter, WriteType};

#[command]
pub(crate) async fn scan<R: Runtime>(
    _app: AppHandle<R>,
    filter: Option<ScanFilter>,
    timeout: u64,
    on_devices: Channel<Vec<BleDevice>>,
) -> Result<()> {
    tracing::info!("Scanning for BLE devices");
    let handler = get_handler()?;

    // Start scan and discover devices
    let devices = handler.start_scan(filter, timeout).await?;

    // Send devices to frontend
    on_devices
        .send(devices)
        .expect("failed to send devices to the front-end");

    Ok(())
}

#[command]
pub(crate) async fn stop_scan<R: Runtime>(_app: AppHandle<R>) -> Result<()> {
    tracing::info!("Stopping BLE scan");
    let handler = get_handler()?;
    handler.stop_scan().await?;
    Ok(())
}

#[command]
pub(crate) async fn connect<R: Runtime>(
    _app: AppHandle<R>,
    address: String,
    on_disconnect: Channel<()>,
) -> Result<()> {
    tracing::info!("Connecting to BLE device: {:?}", address);
    let handler = get_handler()?;
    let disconnect_handler = move || {
        on_disconnect
            .send(())
            .expect("failed to send disconnect event to the front-end");
    };
    handler.connect(&address, disconnect_handler.into()).await?;
    Ok(())
}

#[command]
pub(crate) async fn disconnect<R: Runtime>(
    _app: AppHandle<R>,
    address: Option<String>,
) -> Result<()> {
    let handler = get_handler()?;

    if let Some(address) = address {
        tracing::info!("Disconnecting from BLE device: {}", address);
        handler.disconnect(&address).await?;
    } else {
        tracing::info!("Disconnecting from all BLE devices");
        handler.disconnect_all().await?;
    }

    Ok(())
}

#[command]
pub(crate) async fn connection_state<R: Runtime>(
    _app: AppHandle<R>,
    update: Channel<(String, bool)>,
) -> Result<()> {
    let handler = get_handler()?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    // Set up the connection update channel
    handler.set_connection_update_channel(tx).await;

    // Send initial state
    let connected_devices = handler.connected_devices().await;
    for address in connected_devices {
        update
            .send((address.clone(), true))
            .expect("failed to send connection state");
    }

    // Listen for connection updates
    async_runtime::spawn(async move {
        while let Some((address, connected)) = rx.recv().await {
            debug!(
                "Sending connection update to frontend: {} {}",
                address, connected
            );
            update
                .send((address, connected))
                .expect("failed to send connection state to the front-end");
        }
        warn!("Connection state channel closed");
    });

    Ok(())
}

#[command]
pub(crate) async fn scanning_state<R: Runtime>(
    _app: AppHandle<R>,
    update: Channel<bool>,
) -> Result<()> {
    let handler = get_handler()?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    // Set up the scanning update channel
    handler.set_scanning_update_channel(tx).await;

    // Send initial state
    update
        .send(handler.is_scanning().await)
        .expect("failed to send scanning state");

    // Listen for scanning updates
    async_runtime::spawn(async move {
        while let Some(scanning) = rx.recv().await {
            debug!("Sending scanning update to frontend: {}", scanning);
            update
                .send(scanning)
                .expect("failed to send scanning state to the front-end");
        }
        warn!("Scanning state channel closed");
    });

    Ok(())
}

// Device-specific commands

#[command]
pub(crate) async fn send<R: Runtime>(
    _app: AppHandle<R>,
    address: String,
    characteristic: String,
    data: Vec<u8>,
    write_type: WriteType,
) -> Result<()> {
    let handler = get_handler()?;
    
    // Convert string to UUID
    let uuid = Uuid::parse_str(&characteristic)
        .map_err(|e| Error::UuidParse(e.to_string()))?;

    info!("Sending data to device {}: {:?}", address, data);
    handler
        .send_data(&address, uuid, &data, write_type)
        .await?;

    Ok(())
}

#[command]
pub(crate) async fn send_string<R: Runtime>(
    app: AppHandle<R>,
    address: String,
    characteristic: String,
    data: String,
    write_type: WriteType,
) -> Result<()> {
    let data = data.as_bytes().to_vec();
    send(app, address, characteristic, data, write_type).await
}

#[command]
pub(crate) async fn read<R: Runtime>(
    _app: AppHandle<R>,
    address: String,
    characteristic: String,
) -> Result<Vec<u8>> {
    let handler = get_handler()?;
    info!("Reading data from device: {}", address);
    info!("Characteristic: {}", characteristic);
    // Convert string to UUID
    let uuid = Uuid::parse_str(&characteristic)
        .map_err(|e| Error::UuidParse(e.to_string()))?;
    info!("UUID: {}", uuid);
    let data = handler.read_data(&address, uuid).await?;
    Ok(data)
}

#[command]
pub(crate) async fn read_string<R: Runtime>(
    app: AppHandle<R>,
    address: String,
    characteristic: String,
) -> Result<String> {
    let data = read(app, characteristic, address).await?;
    Ok(String::from_utf8(data).expect("failed to convert data to string"))
}

async fn subscribe_channel(
    address: String,
    characteristic: String,
) -> Result<mpsc::Receiver<Vec<u8>>> {
    let handler = get_handler()?;
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    
    // Convert string to UUID
    let uuid = Uuid::parse_str(&characteristic)
        .map_err(|e| Error::UuidParse(e.to_string()))?;

    let callback = move |data: Vec<u8>| {
        tx.try_send(data)
            .expect("failed to send data to the channel");
    };

    handler
        .subscribe(&address, uuid, callback)
        .await?;

    Ok(rx)
}

#[command]
pub(crate) async fn subscribe<R: Runtime>(
    _app: AppHandle<R>,
    address: String,
    characteristic: String,
    on_data: Channel<Vec<u8>>,
) -> Result<()> {
    let mut rx = subscribe_channel(characteristic, address).await?;
    async_runtime::spawn(async move {
        while let Some(data) = rx.recv().await {
            on_data
                .send(data)
                .expect("failed to send data to the front-end");
        }
    });
    Ok(())
}

#[command]
pub(crate) async fn subscribe_string<R: Runtime>(
    _app: AppHandle<R>,
    address: String,
    characteristic: String,
    on_data: Channel<String>,
) -> Result<()> {
    let mut rx = subscribe_channel(characteristic, address).await?;
    async_runtime::spawn(async move {
        while let Some(data) = rx.recv().await {
            let data = String::from_utf8(data).expect("failed to convert data to string");
            on_data
                .send(data)
                .expect("failed to send data to the front-end");
        }
    });
    Ok(())
}

#[command]
pub(crate) async fn unsubscribe<R: Runtime>(
    _app: AppHandle<R>,
    address: String,
    characteristic: String,
) -> Result<()> {
    let handler = get_handler()?;
    
    // Convert string to UUID
    let uuid = Uuid::parse_str(&characteristic)
        .map_err(|e| Error::UuidParse(e.to_string()))?;

    handler.unsubscribe(&address, uuid).await?;

    Ok(())
}

#[command]
pub(crate) fn check_permissions() -> Result<bool> {
    crate::check_permissions()
}

#[command]
pub(crate) async fn get_connected_devices<R: Runtime>(_app: AppHandle<R>) -> Result<Vec<String>> {
    let handler = get_handler()?;
    let devices = handler.connected_devices().await;
    Ok(devices)
}

pub fn commands<R: Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool {
    tauri::generate_handler![
        scan,
        stop_scan,
        connect,
        disconnect,
        connection_state,
        send,
        send_string,
        read,
        read_string,
        subscribe,
        subscribe_string,
        unsubscribe,
        scanning_state,
        check_permissions,
        get_connected_devices
    ]
}
