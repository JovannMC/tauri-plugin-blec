use crate::error::Error;
use crate::models::{self, fmt_addr, BleDevice, ScanFilter, Service};
use btleplug::api::CentralEvent;
use btleplug::api::{Central, Characteristic, Manager as _, Peripheral as _};
use btleplug::platform::PeripheralId;
use futures::{Stream, StreamExt};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tauri::async_runtime;
use tokio::sync::{mpsc, oneshot, watch, Mutex, RwLock};
use tokio::time::sleep;
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

#[cfg(target_os = "android")]
use crate::android::{Adapter, Manager, Peripheral};
#[cfg(not(target_os = "android"))]
use btleplug::platform::{Adapter, Manager, Peripheral};

struct Listener {
    uuid: Uuid,
    callback: SubscriptionHandler,
}

struct DeviceState {
    peripheral: Peripheral,
    characs: Vec<Characteristic>,
    notify_listeners: Vec<Listener>,
    listen_handle: Option<tauri::async_runtime::JoinHandle<()>>,
    on_disconnect: OnDisconnectHandler,
}

struct HandlerState {
    connection_update_channels: Vec<mpsc::Sender<(String, bool)>>,
    scan_update_channels: Vec<mpsc::Sender<bool>>,
    device_scan_channels: Vec<mpsc::Sender<BleDevice>>,
    scan_task: Option<tokio::task::JoinHandle<()>>,
    scan_stream_task: Option<tokio::task::JoinHandle<()>>,
}

impl HandlerState {
    fn new() -> Self {
        Self {
            connection_update_channels: vec![],
            scan_update_channels: vec![],
            device_scan_channels: vec![],
            scan_task: None,
            scan_stream_task: None,
        }
    }
}

pub struct Handler {
    devices: Arc<Mutex<HashMap<String, Peripheral>>>,
    adapter: Arc<Adapter>,
    connected_devices: Arc<RwLock<HashMap<String, DeviceState>>>,
    scanning_rx: watch::Receiver<bool>,
    scanning_tx: watch::Sender<bool>,
    state: Mutex<HandlerState>,
}

async fn get_central() -> Result<Adapter, Error> {
    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    let central = adapters.into_iter().next().ok_or(Error::NoAdapters)?;
    Ok(central)
}

pub enum OnDisconnectHandler {
    None,
    Sync(Arc<Mutex<Option<Box<dyn FnOnce() + Send + 'static>>>>),
    Async(Arc<Mutex<Option<futures::future::BoxFuture<'static, ()>>>>),
}

impl OnDisconnectHandler {
    async fn run(self) {
        match self {
            OnDisconnectHandler::None => {}
            OnDisconnectHandler::Sync(f) => {
                if let Some(func) = f.lock().await.take() {
                    func();
                }
            }
            OnDisconnectHandler::Async(f) => {
                if let Some(future) = f.lock().await.take() {
                    future.await;
                }
            }
        }
    }

    #[must_use]
    pub fn take(&mut self) -> Self {
        std::mem::replace(self, OnDisconnectHandler::None)
    }
}

impl<F: FnOnce() + Send + 'static> From<F> for OnDisconnectHandler {
    fn from(func: F) -> Self {
        OnDisconnectHandler::Sync(Arc::new(Mutex::new(Some(Box::new(func)))))
    }
}

impl From<futures::future::BoxFuture<'static, ()>> for OnDisconnectHandler {
    fn from(future: futures::future::BoxFuture<'static, ()>) -> Self {
        OnDisconnectHandler::Async(Arc::new(Mutex::new(Some(future))))
    }
}

#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub enum SubscriptionHandler {
    Sync(Arc<dyn Fn(Vec<u8>) + Send + Sync + 'static>),
    ASync(
        Arc<dyn (Fn(Vec<u8>) -> futures::future::BoxFuture<'static, ()>) + Send + Sync + 'static>,
    ),
}

impl SubscriptionHandler {
    pub fn from_async(
        func: impl (Fn(Vec<u8>) -> futures::future::BoxFuture<'static, ()>) + Send + Sync + 'static,
    ) -> Self {
        SubscriptionHandler::ASync(Arc::new(func))
    }

    async fn run(self, data: Vec<u8>) {
        match self {
            SubscriptionHandler::Sync(f) => tokio::task::spawn_blocking(move || f(data))
                .await
                .expect("failed to run sync callback"),
            SubscriptionHandler::ASync(f) => f(data).await,
        }
    }
}

impl<F: Fn(Vec<u8>) + Send + Sync + 'static> From<F> for SubscriptionHandler {
    fn from(func: F) -> Self {
        SubscriptionHandler::Sync(Arc::new(func))
    }
}

impl Handler {
    pub(crate) async fn new() -> Result<Self, Error> {
        let central = get_central().await?;
        let (scanning_tx, scanning_rx) = watch::channel(false);
        Ok(Self {
            devices: Arc::new(Mutex::new(HashMap::new())),
            adapter: Arc::new(central),
            connected_devices: Arc::new(RwLock::new(HashMap::new())),
            scanning_rx,
            scanning_tx,
            state: Mutex::new(HandlerState::new()),
        })
    }

    pub async fn get_connected_devices(&self) -> Vec<BleDevice> {
        let connected = self.connected_devices.read().await;
        let mut devices = vec![];
        for (_address, device) in connected.iter() {
            match BleDevice::from_peripheral(&device.peripheral).await {
                Ok(ble_device) => devices.push(ble_device),
                Err(e) => error!("Failed to create BleDevice from peripheral: {}", e),
            }
        }
        devices
    }

    pub async fn is_connected(&self, address: &str) -> bool {
        let connected = self.connected_devices.read().await;
        connected.contains_key(address)
    }

    /// Returns true if the adapter is scanning
    pub async fn is_scanning(&self) -> bool {
        if let Some(handle) = &self.state.lock().await.scan_task {
            !handle.is_finished()
        } else {
            false
        }
    }

    /// Takes a sender that will be used to send changes in the scanning status
    /// # Example
    /// ```no_run
    /// use tauri::async_runtime;
    /// use tokio::sync::mpsc;
    /// async_runtime::block_on(async {
    ///     let handler = tauri_plugin_blec::get_handler().unwrap();
    ///     let (tx, mut rx) = mpsc::channel(1);
    ///     handler.set_scanning_update_channel(tx).await;
    ///     while let Some(scanning) = rx.recv().await {
    ///         println!("Scanning: {scanning}");
    ///     }
    /// });
    /// ```
    pub async fn set_scanning_update_channel(&self, tx: mpsc::Sender<bool>) {
        self.state.lock().await.scan_update_channels.push(tx);
    }

    /// Takes a sender that will be used to send changes in the connection status
    /// # Example
    /// ```no_run
    /// use tauri::async_runtime;
    /// use tokio::sync::mpsc;
    /// async_runtime::block_on(async {
    ///     let handler = tauri_plugin_blec::get_handler().unwrap();
    ///     let (tx, mut rx) = mpsc::channel(1);
    ///     handler.set_connection_update_channel(tx).await;
    ///     while let Some(connected) = rx.recv().await {
    ///         println!("Connected: {connected}");
    ///     }
    /// });
    /// ```
    pub async fn set_connection_update_channel(&self, tx: mpsc::Sender<(String, bool)>) {
        self.state.lock().await.connection_update_channels.push(tx);
    }

    /// Takes a sender that will be used to send discovered devices during streaming scan
    /// # Example
    /// ```no_run
    /// use tauri::async_runtime;
    /// use tokio::sync::mpsc;
    /// async_runtime::block_on(async {
    ///     let handler = tauri_plugin_blec::get_handler().unwrap();
    ///     let (tx, mut rx) = mpsc::channel(1);
    ///     handler.set_scan_channel(tx).await;
    ///     while let Some(device) = rx.recv().await {
    ///         println!("Discovered device: {:?}", device);
    ///     }
    /// });
    /// ```
    pub async fn set_scan_channel(&self, tx: mpsc::Sender<BleDevice>) {
        self.state.lock().await.device_scan_channels.push(tx);
    }

    /// Start continuous scanning for devices by streaming
    /// Devices will be sent to registered scan channels as they are found
    /// # Errors
    /// Returns an error if scanning is already in progress, or if scanning fails
    pub async fn start_scan_stream(&self, filter: Option<ScanFilter>) -> Result<(), Error> {
        debug!("Starting device scan stream");

        let state = self.state.lock().await;
        if let Some(task) = &state.scan_stream_task {
            if !task.is_finished() {
                debug!("Scan stream already in progress");
                return Err(Error::ScanAlreadyRunning);
            }
        }
        drop(state);

        self.devices.lock().await.clear();

        let btleplug_filter = match &filter {
            Some(ScanFilter::None) | None => btleplug::api::ScanFilter { services: vec![] },
            Some(ScanFilter::Service(uuid)) => btleplug::api::ScanFilter {
                services: vec![*uuid],
            },
            Some(ScanFilter::AnyService(uuids)) | Some(ScanFilter::AllServices(uuids)) => {
                btleplug::api::ScanFilter {
                    services: uuids.clone(),
                }
            }
            // ManufacturerData filters can't be mapped to btleplug::ScanFilter directly
            Some(ScanFilter::ManufacturerData(_, _))
            | Some(ScanFilter::ManufacturerDataMasked(_, _, _)) => {
                btleplug::api::ScanFilter { services: vec![] }
            }
        };

        debug!("Starting continuous scan");
        self.adapter.start_scan(btleplug_filter).await?;
        self.send_scan_update(true).await;
        let adapter_clone = self.adapter.clone();
        let devices_clone = self.devices.clone();
        let scan_filter_clone = filter.clone();

        let scan_channels = {
            let state = self.state.lock().await;
            state.device_scan_channels.clone()
        };

        let scan_stream_task = tokio::spawn(async move {
            debug!("Scan stream task started");

            let mut events = adapter_clone.events().await.unwrap();

            while let Some(event) = events.next().await {
                match event {
                    CentralEvent::DeviceDiscovered(id) => {
                        debug!("Device discovered: {:?}", id);

                        if let Ok(peripheral) = adapter_clone.peripheral(&id).await {
                            if let Ok(Some(properties)) = peripheral.properties().await {
                                let address = fmt_addr(properties.address);
                                {
                                    let mut devices = devices_clone.lock().await;
                                    devices.insert(address.clone(), peripheral.clone());
                                }
                                if let Ok(ble_device) =
                                    BleDevice::from_peripheral(&peripheral).await
                                {
                                    let mut should_send = true;
                                    if let Some(filter) = &scan_filter_clone {
                                        let mut devices_to_filter = vec![peripheral.clone()];
                                        filter_peripherals(&mut devices_to_filter, filter).await;
                                        should_send = !devices_to_filter.is_empty();
                                    }
                                    if should_send {
                                        for tx in &scan_channels {
                                            if let Err(e) = tx.send(ble_device.clone()).await {
                                                warn!("Failed to send discovered device: {e}");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            debug!("Scan stream task ended");
        });

        self.state.lock().await.scan_stream_task = Some(scan_stream_task);

        Ok(())
    }

    /// Stops device scan stream
    /// # Errors
    /// Returns an error if stopping the scan fails
    pub async fn stop_scan_stream(&self) -> Result<(), Error> {
        debug!("Stopping device scan stream");

        let scan_update_channels = {
            let state = self.state.lock().await;
            state.scan_update_channels.clone()
        };

        let mut state = self.state.lock().await;
        if let Some(task) = state.scan_stream_task.take() {
            if !task.is_finished() {
                debug!("Aborting scan stream task");
                task.abort();
            }
        }

        if let Err(e) = self.adapter.stop_scan().await {
            warn!("Failed to stop scan: {e}");
        }

        drop(state);
        for tx in &scan_update_channels {
            if let Err(e) = tx.send(false).await {
                warn!("Failed to send scan update: {e}");
            }
        }
        let _ = self.scanning_tx.send(false);

        debug!("Scan stream stopped");
        Ok(())
    }

    /// Connects to the given address
    /// If a callback is provided, it will be called when the device is disconnected.
    /// Because connecting sometimes fails especially on android, this method tries up to 3 times
    /// before returning an error
    /// # Errors
    /// Returns an error if no devices are found, if the device is already connected,
    /// if the connection fails, or if the service/characteristics discovery fails
    /// # Example
    /// ```no_run
    /// use tauri::async_runtime;
    /// async_runtime::block_on(async {
    ///    let handler = tauri_plugin_blec::get_handler().unwrap();
    ///    handler.connect("00:00:00:00:00:00", (|| println!("disconnected")).into()).await.unwrap();
    /// });
    /// ```
    pub async fn connect(
        &'static self,
        address: &str,
        on_disconnect: OnDisconnectHandler,
    ) -> Result<(), Error> {
        if self.devices.lock().await.len() == 0 {
            return Err(Error::NoDevicesDiscovered);
        }
        // cancel any running discovery
        let _ = self.stop_scan().await;
        // connect to the given address
        // try up to 3 times before returning an error
        let mut connected = Ok(());
        for i in 0..3 {
            if let Err(e) = self.connect_device(address).await {
                if i < 2 {
                    warn!("Failed to connect device, retrying in 1s: {e}");
                    sleep(Duration::from_secs(1)).await;
                    continue;
                }
                connected = Err(e);
            } else {
                connected = Ok(());
                break;
            }
        }
        if let Err(e) = connected {
            error!("Failed to connect device: {e}");
            return Err(e);
        }

        let peripheral = {
            let devices = self.devices.lock().await;
            devices
                .get(address)
                .ok_or(Error::UnknownPeripheral(address.to_string()))?
                .clone()
        };

        debug!("Discovering services for device {address}");
        peripheral.discover_services().await?;

        let services = peripheral.services();
        let mut characteristics = Vec::new();
        for s in &services {
            for c in &s.characteristics {
                characteristics.push(c.clone());
            }
        }

        // Start notification listener
        let notify_listeners = Vec::new();
        let listen_handle =
            async_runtime::spawn(listen_notify(peripheral.clone(), address.to_string(), self));

        let device_state = DeviceState {
            peripheral: peripheral.clone(),
            characs: characteristics,
            notify_listeners,
            listen_handle: Some(listen_handle),
            on_disconnect,
        };

        let mut devices = self.connected_devices.write().await;
        devices.insert(address.to_string(), device_state);

        self.send_connection_update(address, true).await;
        info!("connecting done");
        Ok(())
    }

    async fn connect_device(&self, address: &str) -> Result<(), Error> {
        debug!("connecting to {address}",);
        let devices = self.devices.lock().await;
        let device = devices
            .get(address)
            .ok_or(Error::UnknownPeripheral(address.to_string()))?;

        if device.is_connected().await? {
            debug!("Device already connected");
            return Ok(());
        }

        debug!("Connecting to device {address}");
        run_with_timeout(device.connect(), "Connect").await?;

        // Verify the connection was established
        if !device.is_connected().await? {
            warn!("Device {address} still not connected after connect call");
            return Err(Error::ConnectionFailed);
        }

        info!("Device {address} connected");
        Ok(())
    }

    /// Disconnects from the connected device
    /// This triggers a disconnect and then waits for the actual disconnect event from the adapter
    /// # Errors
    /// Returns an error if no device is connected or if the disconnect fails
    /// # Panics
    /// panics if there is an error with handling the internal disconnect event
    pub async fn disconnect(&self, address: &str) -> Result<(), Error> {
        debug!("Disconnect for device {address} triggered by user");
        let peripheral = {
            let devices = self.connected_devices.read().await;
            match devices.get(address) {
                Some(state) => state.peripheral.clone(),
                None => return Err(Error::NoDeviceConnected),
            }
        };

        if !peripheral.is_connected().await? {
            debug!("Device {address} is not connected");
            return Err(Error::NoDeviceConnected);
        }

        // Trigger disconnect
        peripheral.disconnect().await?;

        // Wait for the device to actually disconnect
        for _ in 0..10 {
            sleep(Duration::from_millis(100)).await;
            if !peripheral.is_connected().await? {
                break;
            }
        }

        if peripheral.is_connected().await? {
            return Err(Error::DisconnectFailed);
        }

        // Clean up device state
        self.handle_device_disconnect(address).await?;
        Ok(())
    }

    /// Disconnects from all connected devices
    /// # Errors
    /// Returns an error if any device fails to disconnect
    pub async fn disconnect_all(&self) -> Result<(), Error> {
        let addresses = self.connected_devices().await;
        let mut last_error = None;
        for address in addresses {
            if let Err(e) = self.disconnect(&address).await {
                warn!("Failed to disconnect device {address}: {e}");
                last_error = Some(e);
            }
        }

        if let Some(error) = last_error {
            return Err(error);
        }

        Ok(())
    }

    /// Handles disconnection cleanup for a device
    async fn handle_device_disconnect(&self, address: &str) -> Result<(), Error> {
        debug!("Handling disconnect for device {address}");

        let device_state = {
            let mut devices = self.connected_devices.write().await;
            devices.remove(address)
        };

        if let Some(mut state) = device_state {
            // Stop the notification listener
            if let Some(handle) = state.listen_handle.take() {
                handle.abort();
            }

            // Run the disconnect handler
            state.on_disconnect.take().run().await;

            // Send connection update
            self.send_connection_update(address, false).await;

            info!("Device {address} disconnected");
            Ok(())
        } else {
            warn!("Attempted to handle disconnect for non-connected device {address}");
            Err(Error::NoDeviceConnected)
        }
    }

    /// Handle discovery of a new device
    async fn handle_discover(&self, id: PeripheralId) -> Result<(), Error> {
        let peripheral = self.adapter.peripheral(&id).await?;
        let properties = peripheral.properties().await?;

        // Store device if it has properties
        if let Some(properties) = properties {
            debug!(
                "Device {} discovered: {}",
                fmt_addr(properties.address),
                properties.local_name.unwrap_or_default()
            );

            let mut devices = self.devices.lock().await;
            let address = fmt_addr(properties.address);
            devices.insert(address, peripheral);
        }

        Ok(())
    }

    /// Clears internal state, updates connected flag and calls disconnect callback
    async fn handle_disconnect(&self, peripheral_id: PeripheralId) -> Result<(), Error> {
        // Find which device this is
        let mut disconnected_address = None;
        {
            let devices = self.connected_devices.read().await;
            for (address, state) in devices.iter() {
                if state.peripheral.id() == peripheral_id {
                    disconnected_address = Some(address.clone());
                    break;
                }
            }
        }

        if let Some(address) = disconnected_address {
            debug!("Received disconnect event for device {address}");
            self.handle_device_disconnect(&address).await?;
        } else {
            warn!("Received disconnect event for unknown device {peripheral_id:?}");
        }

        Ok(())
    }

    /// Scans for `timeout` milliseconds and periodically sends discovered devices
    /// to the given channel.
    /// A task is spawned to handle the scan and send the devices, so the function
    /// returns immediately.
    ///
    /// A Variant of [`ScanFilter`] can be provided to filter the discovered devices
    ///
    /// # Errors
    /// Returns an error if starting the scan fails
    /// # Panics
    /// Panics if there is an error getting devices from the adapter
    /// # Example
    /// ```no_run
    /// use tauri::async_runtime;
    /// use tokio::sync::mpsc;
    /// use tauri_plugin_blec::models::ScanFilter;
    ///
    /// async_runtime::block_on(async {
    ///     let handler = tauri_plugin_blec::get_handler().unwrap();
    ///     let (tx, mut rx) = mpsc::channel(1);
    ///     handler.discover(Some(tx),1000, ScanFilter::None).await.unwrap();
    ///     while let Some(devices) = rx.recv().await {
    ///         println!("Discovered {devices:?}");
    ///     }
    /// });
    /// ```

    /// Discover provided services and characteristics
    /// If the device is not connected, a connection is made in order to discover the services and characteristics
    /// After the discovery is done, the device is disconnected
    /// If the devices was already connected, it will stay connected
    /// # Errors
    /// Returns an error if the device is not found, if the connection fails, or if the discovery fails
    /// # Panics
    /// Panics if there is an error with the internal disconnect event
    pub async fn discover_services(&self, address: &str) -> Result<Vec<Service>, Error> {
        debug!("Discovering services for device {address}");

        // Get the peripheral
        let devices = self.connected_devices.read().await;
        let state = devices.get(address).ok_or(Error::NoDeviceConnected)?;

        // If the device is not connected, connect it
        if !state.peripheral.is_connected().await? {
            self.connect_device(address).await?;
        }

        // Discover services
        state.peripheral.discover_services().await?;

        // Get the services and characteristics
        let services = state
            .peripheral
            .services()
            .iter()
            .map(Service::from)
            .collect();
        Ok(services)
    }

    /// Start scanning for devices for a certain duration, then stopping.
    /// Duration is in milliseconds
    /// # Errors
    /// Returns an error if scanning is already in progress, or if scanning fails
    pub async fn start_scan(
        &self,
        scan_filter: ScanFilter,
        duration: u64,
    ) -> Result<Vec<BleDevice>, Error> {
        debug!("Discover called for duration of {duration}ms");

        let state = self.state.lock().await;
        if let Some(task) = &state.scan_task {
            if !task.is_finished() {
                debug!("Scan already in progress");
                return Err(Error::ScanAlreadyRunning);
            }
        }
        drop(state);

        // Clear existing devices
        self.devices.lock().await.clear();

        // Create filter from scan_filter
        let filter = match &scan_filter {
            ScanFilter::None => btleplug::api::ScanFilter { services: vec![] },
            ScanFilter::Service(uuid) => btleplug::api::ScanFilter {
                services: vec![*uuid],
            },
            ScanFilter::AnyService(uuids) | ScanFilter::AllServices(uuids) => {
                btleplug::api::ScanFilter {
                    services: uuids.clone(),
                }
            }
            // ManufacturerData filters can't be mapped to btleplug::ScanFilter directly
            ScanFilter::ManufacturerData(_, _) | ScanFilter::ManufacturerDataMasked(_, _, _) => {
                btleplug::api::ScanFilter { services: vec![] }
            }
        };

        // Start scan
        debug!("Starting scan");
        self.adapter.start_scan(filter).await?;
        self.send_scan_update(true).await;

        // Clone what we need for the future
        let adapter_clone = self.adapter.clone();
        let devices_clone = self.devices.clone();
        let scanning_tx_clone = self.scanning_tx.clone();
        let scan_update_channels_clone = {
            let state = self.state.lock().await;
            state.scan_update_channels.clone()
        };

        // Set up timeout
        let fut = async move {
            sleep(Duration::from_millis(duration)).await;

            // Stop scan
            debug!("Scan timeout reached");
            if let Err(e) = adapter_clone.stop_scan().await {
                warn!("Failed to stop scan: {e}");
            }
            // Send scan update to all channels
            for tx in &scan_update_channels_clone {
                let _ = tx.send(false).await;
            }
            let _ = scanning_tx_clone.send(false);

            debug!("Scan completed");

            // Return discovered devices
            let mut discovered: Vec<_> = {
                let devices = devices_clone.lock().await;
                devices.values().cloned().collect()
            };

            // Apply scan_filter
            filter_peripherals(&mut discovered, &scan_filter).await;

            let futures = discovered
                .iter()
                .map(|p| BleDevice::from_peripheral(p))
                .collect::<Vec<_>>();
            let result: Vec<BleDevice> = futures::future::join_all(futures)
                .await
                .into_iter()
                .filter_map(|res| match res {
                    Ok(ble_device) => Some(ble_device),
                    Err(e) => {
                        error!("Failed to create BleDevice from peripheral: {}", e);
                        None
                    }
                })
                .collect();
            debug!("Found {} devices", result.len());
            result
        };

        // Create the task
        let (tx, rx) = oneshot::channel();

        // Spawn task that will complete when scan is done
        let handle = tokio::spawn(async move {
            let result = fut.await;
            let _ = tx.send(result);
        });

        // Store the task for cancellation
        self.state.lock().await.scan_task = Some(handle);

        // Wait for scan to complete
        match rx.await {
            Ok(devices) => Ok(devices),
            Err(_) => {
                error!("Scan task failed");
                Err(Error::ScanFailed)
            }
        }
    }

    /// Stop any ongoing scan
    /// # Errors
    /// Returns an error if scanning is not in progress, or if stopping the scan fails
    pub async fn stop_scan(&self) -> Result<(), Error> {
        debug!("Stopping ongoing scan");

        // Get the channels before acquiring lock to avoid deadlock
        let scan_update_channels = {
            let state = self.state.lock().await;
            state.scan_update_channels.clone()
        };

        let mut state = self.state.lock().await;
        if let Some(task) = state.scan_task.take() {
            if !task.is_finished() {
                debug!("Aborting scan task");
                task.abort();
            }
        }

        // Stop scan on adapter
        if let Err(e) = self.adapter.stop_scan().await {
            warn!("Failed to stop scan: {e}");
        }

        // Send updates without holding the lock
        for tx in &scan_update_channels {
            if let Err(e) = tx.send(false).await {
                warn!("Failed to send scan update: {e}");
            }
        }
        let _ = self.scanning_tx.send(false);

        debug!("Scan stopped");
        Ok(())
    }

    /// Get the characteristic for the given UUID from a specific device
    async fn get_characteristic(&self, address: &str, uuid: Uuid) -> Result<Characteristic, Error> {
        let devices = self.connected_devices.read().await;
        let state = devices.get(address).ok_or(Error::NoDeviceConnected)?;

        let characteristic = state
            .characs
            .iter()
            .find(|c| c.uuid == uuid)
            .cloned()
            .ok_or_else(|| Error::CharacNotAvailable(uuid.to_string()))?;

        Ok(characteristic)
    }

    /// Sends data to the given characteristic of the connected device
    /// # Errors
    /// Returns an error if no device is connected or the characteristic is not available
    /// or if the write operation fails
    /// # Example
    /// ```no_run
    /// use tauri::async_runtime;
    /// use uuid::{Uuid,uuid};
    /// use tauri_plugin_blec::models::WriteType;
    ///
    /// const address : &str = "00:00:00:00:00:00";
    /// const CHARACTERISTIC_UUID: Uuid = uuid!("51FF12BB-3ED8-46E5-B4F9-D64E2FEC021B");
    /// async_runtime::block_on(async {
    ///     let handler = tauri_plugin_blec::get_handler().unwrap();
    ///     let data = [1,2,3,4,5];
    ///     let response = handler.send_data(address, CHARACTERISTIC_UUID, &data, WriteType::WithResponse).await.unwrap();
    /// });
    /// ```
    pub async fn send_data(
        &self,
        address: &str,
        c: Uuid,
        data: &[u8],
        write_type: models::WriteType,
    ) -> Result<(), Error> {
        debug!("Sending data to {c} on device {address}");

        let devices = self.connected_devices.read().await;
        let state = devices.get(address).ok_or(Error::NoDeviceConnected)?;

        let characteristic = state
            .characs
            .iter()
            .find(|char| char.uuid == c)
            .cloned()
            .ok_or_else(|| Error::CharacNotAvailable(c.to_string()))?;

        state
            .peripheral
            .write(&characteristic, data, write_type.into())
            .await?;

        Ok(())
    }

    /// Receives data from the given characteristic of the connected device
    /// Returns the data as a vector of bytes
    /// # Errors
    /// Returns an error if no device is connected or the characteristic is not available
    /// or if the read operation fails
    /// # Example
    /// ```no_run
    /// use tauri::async_runtime;
    /// use uuid::{Uuid,uuid};
    /// const ADDRESS = "00:00:00:00:00:00";
    /// const CHARACTERISTIC_UUID: Uuid = uuid!("51FF12BB-3ED8-46E5-B4F9-D64E2FEC021B");
    /// async_runtime::block_on(async {
    ///     let handler = tauri_plugin_blec::get_handler().unwrap();
    ///     let response = handler.read(ADDRESS, CHARACTERISTIC_UUID).await.unwrap();
    /// });
    /// ```
    pub async fn read_data(&self, address: &str, c: Uuid) -> Result<Vec<u8>, Error> {
        debug!("Reading data from {c} on device {address}");

        let devices = self.connected_devices.read().await;
        let state = devices.get(address).ok_or(Error::NoDeviceConnected)?;

        let characteristic = state
            .characs
            .iter()
            .find(|char| char.uuid == c)
            .cloned()
            .ok_or_else(|| Error::CharacNotAvailable(c.to_string()))?;

        let data = state.peripheral.read(&characteristic).await?;

        Ok(data)
    }

    /// Subscribe to notifications from the given characteristic
    /// The callback will be called whenever a notification is received
    /// # Errors
    /// Returns an error if no device is connected or the characteristic is not available
    /// or if the subscribe operation fails
    /// # Example
    /// ```no_run
    /// use tauri::async_runtime;
    /// use uuid::{Uuid,uuid};
    /// const CHARACTERISTIC_UUID: Uuid = uuid!("51FF12BB-3ED8-46E5-B4F9-D64E2FEC021B");
    /// async_runtime::block_on(async {
    ///     let handler = tauri_plugin_blec::get_handler().unwrap();
    ///     let response = handler.subscribe(CHARACTERISTIC_UUID,|data| println!("received {data:?}")).await.unwrap();
    /// });
    /// ```
    pub async fn subscribe(
        &self,
        address: &str,
        c: Uuid,
        callback: impl Into<SubscriptionHandler>,
    ) -> Result<(), Error> {
        debug!("Subscribing to {c} on device {address}");

        let characteristic = self.get_characteristic(address, c).await?;

        let mut devices = self.connected_devices.write().await;
        let state = devices.get_mut(address).ok_or(Error::NoDeviceConnected)?;

        state.peripheral.subscribe(&characteristic).await?;

        state.notify_listeners.push(Listener {
            uuid: c,
            callback: callback.into(),
        });

        Ok(())
    }

    /// Unsubscribe from notifications for the given characteristic
    /// This will also remove the callback from the list of listeners
    /// # Errors
    /// Returns an error if no device is connected or the characteristic is not available
    /// or if the unsubscribe operation fails
    pub async fn unsubscribe(&self, address: &str, c: Uuid) -> Result<(), Error> {
        debug!("Unsubscribing from {c} on device {address}");

        let characteristic = self.get_characteristic(address, c).await?;

        let mut devices = self.connected_devices.write().await;
        let state = devices.get_mut(address).ok_or(Error::NoDeviceConnected)?;

        state.peripheral.unsubscribe(&characteristic).await?;

        state.notify_listeners.retain(|listener| listener.uuid != c);

        Ok(())
    }

    pub(super) async fn get_event_stream(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = CentralEvent> + Send>>, Error> {
        let events = self.adapter.events().await?;
        Ok(events)
    }

    /// Handle events from the adapter
    /// # Errors
    /// Returns an error if handling the event fails
    pub(crate) async fn handle_event(&self, event: CentralEvent) -> Result<(), Error> {
        match event {
            CentralEvent::DeviceDisconnected(id) => {
                debug!("Device disconnected: {:?}", id);
                self.handle_disconnect(id).await?;
            }
            CentralEvent::DeviceDiscovered(id) => {
                debug!("Device discovered: {:?}", id);
                self.handle_discover(id).await?;
            }
            _ => {}
        }

        Ok(())
    }

    /// Returns a list of connected device addresses
    pub async fn connected_devices(&self) -> Vec<String> {
        let devices = self.connected_devices.read().await;
        devices.keys().cloned().collect()
    }

    async fn send_connection_update(&self, address: &str, connected: bool) {
        let state = self.state.lock().await;
        for tx in &state.connection_update_channels {
            if let Err(e) = tx.send((address.to_string(), connected)).await {
                warn!("Failed to send connection update: {e}");
            }
        }
    }

    async fn send_scan_update(&self, scanning: bool) {
        let channels = {
            let state = self.state.lock().await;
            state.scan_update_channels.clone()
        };

        for tx in &channels {
            if let Err(e) = tx.send(scanning).await {
                warn!("Failed to send scan update: {e}");
            }
        }
        let _ = self.scanning_tx.send(scanning);
    }
}

async fn run_with_timeout<T: Send + Sync + 'static>(
    fut: impl Future<Output = Result<T, btleplug::Error>> + Send,
    cmd: &str,
) -> Result<T, Error> {
    tokio::time::timeout(Duration::from_secs(5), fut)
        .await
        .map_err(|_| Error::Timeout(cmd.to_string()))?
        .map_err(Error::Btleplug)
}

async fn filter_peripherals(discovered: &mut Vec<Peripheral>, filter: &ScanFilter) {
    if matches!(filter, ScanFilter::None) {
        return;
    }
    let mut remove = vec![];
    for p in discovered.iter().enumerate() {
        let Ok(Some(properties)) = p.1.properties().await else {
            // can't filter without properties
            remove.push(p.0);
            continue;
        };
        if properties.rssi.is_none() {
            // ignore not available devices
            remove.push(p.0);
            continue;
        }
        match filter {
            ScanFilter::None => unreachable!("Earyl return for no filter"),
            ScanFilter::Service(uuid) => {
                if !properties.services.iter().any(|s| s == uuid) {
                    remove.push(p.0);
                }
            }
            ScanFilter::AnyService(uuids) => {
                if !properties.services.iter().any(|s| uuids.contains(s)) {
                    remove.push(p.0);
                }
            }
            ScanFilter::AllServices(uuids) => {
                if !uuids.iter().all(|s| properties.services.contains(s)) {
                    remove.push(p.0);
                }
            }
            ScanFilter::ManufacturerData(key, value) => {
                if !properties
                    .manufacturer_data
                    .get(key)
                    .is_some_and(|v| v == value)
                {
                    remove.push(p.0);
                }
            }
            ScanFilter::ManufacturerDataMasked(key, value, maks) => {
                let Some(data) = properties.manufacturer_data.get(key) else {
                    remove.push(p.0);
                    continue;
                };
                if !data
                    .iter()
                    .zip(maks.iter())
                    .zip(value.iter())
                    .all(|((d, m), v)| (d & m) == (*v & m))
                {
                    remove.push(p.0);
                }
            }
        }
    }

    for i in remove.iter().rev() {
        discovered.swap_remove(*i);
    }
}

async fn listen_notify(peripheral: Peripheral, address: String, handler: &Handler) {
    debug!("Starting notification listener for device {address}");
    let mut notification_stream = match peripheral.notifications().await {
        Ok(stream) => stream,
        Err(e) => {
            error!("Failed to get notification stream for device {address}: {e}");
            return;
        }
    };

    while let Some(notification) = notification_stream.next().await {
        trace!(
            "Notification received from device {address}: {:?}",
            notification
        );
        let devices = handler.connected_devices.read().await;
        if let Some(state) = devices.get(&address) {
            for listener in &state.notify_listeners {
                if listener.uuid == notification.uuid {
                    let callback = listener.callback.clone();
                    let data = notification.value.clone();
                    async_runtime::spawn(async move {
                        callback.run(data).await;
                    });
                }
            }
        }
    }

    debug!("Notification listener for device {address} ended");
}
