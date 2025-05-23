import { Channel, invoke } from '@tauri-apps/api/core'

export type BleDevice = {
  address: string;
  name: string;
  rssi: number;
  isConnected: boolean;
  services: string[];
  manufacturerData: Record<number, Uint8Array>;
  serviceData: Record<string, Uint8Array>;
};

/**
  * Scan for BLE devices
  * @param handler - A function that will be called with an array of devices found during the scan
  * @param timeout - The scan timeout in milliseconds
*/
export async function startScan(handler: (devices: BleDevice[]) => void, timeout: Number) {
  if (!timeout) {
    timeout = 10000;
  }
  let onDevices = new Channel<BleDevice[]>();
  onDevices.onmessage = handler;
  await invoke<BleDevice[]>('plugin:blec|scan', {
    timeout,
    onDevices
  })
}

/**
  * Stop scanning for BLE devices
*/
export async function stopScan() {
  await invoke('plugin:blec|stop_scan')
}

/**
  * Check if necessary permissions are granted
  * @returns true if permissions are granted, false otherwise
  */
export async function checkPermissions(): Promise<boolean> {
  return await invoke<boolean>('plugin:blec|check_permissions')
}

/**
  * Register a handler to receive updates when the connection state changes
  * @param handler A function that will be called with the device address and connection state
  */
export async function getConnectionUpdates(handler: (address: string, connected: boolean) => void) {
  let connection_chan = new Channel<[string, boolean]>()
  connection_chan.onmessage = (data: [string, boolean]) => {
    handler(data[0], data[1])
  }
  await invoke('plugin:blec|connection_state', { update: connection_chan })
}

/**
 * Backward compatibility for connection updates
 * @param handler A function that will be called with connection state (true if any device is connected)
 */
export async function getAnyConnectionUpdates(handler: (connected: boolean) => void) {
  let connected = false;
  await getConnectionUpdates((address, isConnected) => {
    if (isConnected) {
      connected = true;
    } else {
      // We need to check if any other device is still connected
      invoke<string[]>('plugin:blec|get_connected_devices').then((devices: string[]) => {
        connected = devices.length > 0;
        handler(connected);
      });
      return;
    }
    handler(connected);
  });
}

/**
 * Register a handler to receive updates when the scanning state changes
 */
export async function getScanningUpdates(handler: (scanning: boolean) => void) {
  let scanning_chan = new Channel<boolean>()
  scanning_chan.onmessage = handler
  await invoke('plugin:blec|scanning_state', { update: scanning_chan })
}

/**
  * Disconnect from a specific BLE device
  * @param address The address of the device to disconnect from
  */
export async function disconnectDevice(address: string) {
  await invoke('plugin:blec|disconnect', { address })
}

/**
  * Disconnect from all connected devices
  */
export async function disconnectAll() {
  await invoke('plugin:blec|disconnect')
}

/**
  * Disconnect from the first connected device (backward compatibility)
  */
export async function disconnect() {
  await disconnectAll()
}

/**
  * Connect to a BLE device
  * @param address The address of the device to connect to
  * @param onDisconnect A function that will be called when the device disconnects
*/
export async function connect(address: string, onDisconnect: (() => void) | null) {
  let disconnectChannel = new Channel()
  if (onDisconnect) {
    disconnectChannel.onmessage = onDisconnect
  }
  await invoke('plugin:blec|connect', {
    address: address,
    onDisconnect: disconnectChannel
  })
}

/**
 * Write a Uint8Array to a BLE characteristic on a specific device
 * @param characteristic UUID of the characteristic to write to
 * @param data Data to write to the characteristic
 * @param writeType Whether to write with response
 * @param address Optional address of the device to write to. If not provided, writes to the first connected device
 */
export async function send(
  characteristic: string, 
  data: Uint8Array, 
  writeType: 'withResponse' | 'withoutResponse' = 'withResponse',
  address: string
) {
  await invoke('plugin:blec|send', {
    characteristic,
    data,
    writeType,
    address
  })
}

/**
 * Write a string to a BLE characteristic on a specific device
 * @param characteristic UUID of the characteristic to write to
 * @param data String data to write to the characteristic
 * @param writeType Whether to write with response
 * @param address Optional address of the device to write to. If not provided, writes to the first connected device
 */
export async function sendString(
  characteristic: string, 
  data: string, 
  writeType: 'withResponse' | 'withoutResponse' = 'withResponse',
  address: string
) {
  await invoke('plugin:blec|send_string', {
    characteristic,
    data,
    writeType,
    address
  })
}

/**
 * Read bytes from a BLE characteristic on a specific device
 * @param characteristic UUID of the characteristic to read from
 * @param address Optional address of the device to read from. If not provided, reads from the first connected device
 */
export async function read(characteristic: string, address: string): Promise<Uint8Array> {
  let res = await invoke<Uint8Array>('plugin:blec|read', {
    characteristic,
    address
  })
  return res
}

/**
 * Read a string from a BLE characteristic on a specific device
 * @param characteristic UUID of the characteristic to read from
 * @param address Optional address of the device to read from. If not provided, reads from the first connected device
 */
export async function readString(characteristic: string, address: string): Promise<string> {
  let res = await invoke<string>('plugin:blec|read_string', {
    characteristic,
    address
  })
  return res
}

/**
 * Unsubscribe from a BLE characteristic on a specific device
 * @param characteristic UUID of the characteristic to unsubscribe from
 * @param address Optional address of the device to unsubscribe from. If not provided, unsubscribes from the first connected device
 */
export async function unsubscribe(characteristic: string, address: string) {
  await invoke('plugin:blec|unsubscribe', {
    characteristic,
    address
  })
}

/**
 * Subscribe to a BLE characteristic on a specific device
 * @param characteristic UUID of the characteristic to subscribe to
 * @param handler Callback function that will be called with the data received for every notification
 * @param address Optional address of the device to subscribe to. If not provided, subscribes to the first connected device
 */
export async function subscribe(characteristic: string, handler: (data: Uint8Array) => void, address: string) {
  let onData = new Channel<Uint8Array>()
  onData.onmessage = handler;
  await invoke('plugin:blec|subscribe', {
    characteristic,
    onData,
    address
  })
}

/**
 * Subscribe to a BLE characteristic on a specific device. Converts the received data to a string
 * @param characteristic UUID of the characteristic to subscribe to
 * @param handler Callback function that will be called with the data received for every notification
 * @param address Optional address of the device to subscribe to. If not provided, subscribes to the first connected device
 */
export async function subscribeString(characteristic: string, address: string, handler: (data: string) => void) {
  let onData = new Channel<string>()
  onData.onmessage = handler;
  await invoke('plugin:blec|subscribe_string', {
    characteristic,
    address,
    onData
  })
}
