# Tauri Plugin blec

A BLE-Client plugin based on [btlelug](https://github.com/deviceplug/btleplug).

The main difference to using btleplug directly is that this uses the tauri plugin system for android.
All other platforms use the btleplug implementation.

## Docs

~~- [Rust docs](https://docs.rs/crate/tauri-plugin-blec/latest)~~  
~~- [JavaScript docs](https://mnlphlp.github.io/tauri-plugin-blec/)~~

### Frontend (JS/TS)

- `startScan(handler, timeout)`
- `stopScan()`
- `checkPermissions()`
- `getConnectionUpdates(handler)`
- `getAnyConnectionUpdates(handler)`
- `getScanningUpdates(handler)`
- `connect(address, onDisconnect)`
- `disconnectDevice(address)`
- `disconnectAll()`
- `disconnect()`
- `send(address, characteristic, data, writeType)`
- `sendString(address, characteristic, data, writeType)`
- `read(address, characteristic)`
- `readString(address, characteristic)`
- `subscribe(address, characteristic, handler)`
- `subscribeString(address, characteristic, handler)`
- `unsubscribe(address, characteristic)`
- `get_connected_devices()`

### Backend (Rust)

- `scan`
- `stop_scan`
- `connect`
- `disconnect`
- `disconnect_all`
- `connection_state`
- `scanning_state`
- `send`
- `send_string`
- `read`
- `read_string`
- `subscribe`
- `subscribe_string`
- `unsubscribe`
- `check_permissions`
- `get_connected_devices`

## Installation

### Install the rust part of the plugin

`cargo add tauri-plugin-blec`

Or manually add it to the `src-tauri/Cargo.toml`

```toml
[dependencies]
tauri-plugin-blec = "0.4"
```

### Install the js bindings

use your preferred JavaScript package manager to add `@mnlphlp/plugin-blec`:

`yarn add @mnlphlp/plugin-blec`

`npm add @mnlphlp/plugin-blec`

### Register the plugin in Tauri

`src-tauri/src/lib.rs`

```rs
tauri::Builder::default()
    .plugin(tauri_plugin_blec::init())
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
```

### Allow calls from Frontend

Add `blec:default` to the permissions in your capabilities file.

[Explanation about capabilities](https://v2.tauri.app/security/capabilities/)

### IOS Setup

Add an entry to the info.plist of your app:

```xml
<key>NSBluetoothAlwaysUsageDescription</key>
<string>The App uses Bluetooth to communicate with BLE devices</string>
```

Add the CoreBluetooth Framework in your xcode procjet:

- open with `tauri ios dev --open`
- click on your project to open settings
- Add Framework under General -> Frameworks,Libraries and Embedded Content

## Usage in Frontend

See [examples/plugin-blec-example](examples/plugin-blec-example) for a full working example that scans for devices, connects and sends/receives data.
In order to use it run [examples/test-server](examples/test-server) on another device and connect to that server.

Short example:

```ts
import { connect, sendString, readString, subscribeString, disconnectDevice } from '@mnlphlp/plugin-blec'
// get address by scanning for devices and selecting the desired one
let address = ...
// connect and run a callback on disconnect
await connect(address, () => console.log(`Disconnected from ${address}`))
// send some text to a characteristic
const CHARACTERISTIC_UUID = '51FF12BB-3ED8-46E5-B4F9-D64E2FEC021B'
await sendString(address, CHARACTERISTIC_UUID, 'Test', 'withResponse')
// read a string from a characteristic
const value = await readString(address, CHARACTERISTIC_UUID)
// subscribe to notifications
const unsub = await subscribeString(address, CHARACTERISTIC_UUID, data => {
  console.log(`Notification from ${address}:`, data)
})
// disconnect from a specific device
await disconnectDevice(address)
```

## Usage in Backend

The plugin can also be used from the rust backend.

The handler returned by `get_handler()` is the same that is used by the frontend commands.
This means if you connect from the frontend you can send data from rust without having to call connect on the backend.

```rs
use uuid::{uuid, Uuid};
use tauri_plugin_blec::models::WriteType;

const DEVICE_ADDRESS: &str = "00:00:00:00:00:00";
const CHARACTERISTIC_UUID: Uuid = uuid!("51FF12BB-3ED8-46E5-B4F9-D64E2FEC021B");
const DATA: [u8; 500] = [0; 500];
let handler = tauri_plugin_blec::get_handler().unwrap();
handler
    .send_data(DEVICE_ADDRESS, CHARACTERISTIC_UUID, &DATA, WriteType::WithResponse)
    .await
    .unwrap();
```
