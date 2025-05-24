<script setup lang="ts">
// This starter template is using Vue 3 <script setup> SFCs
// Check out https://vuejs.org/api/sfc-script-setup.html#script-setup
import { BleDevice, getConnectionUpdates, startScan, sendString, readString, unsubscribe, subscribeString, stopScan, connect, disconnect, getScanningUpdates } from '@mnlphlp/plugin-blec'
import { onMounted, ref } from 'vue';
import BleDev from './components/BleDev.vue'
import { invoke } from '@tauri-apps/api/core'

const devices = ref<BleDevice[]>([])
const device = ref<BleDevice | null>(null)
const connected = ref(false)
const scanning = ref(false)

onMounted(async () => {
  await getConnectionUpdates((state) => connected.value = state)
  await getScanningUpdates((state) => {
    console.log('Scanning:', state)
    scanning.value = state
  })
})

const sendData = ref('')
const recvData = ref('')
const characteristicUuid = ref('51FF12BB-3ED8-46E5-B4F9-D64E2FEC021B')
const rustTest = ref(false)

const notifyData = ref('')
async function subscribe() {
  if (notifyData.value) {
    unsubscribe(characteristicUuid, device.address)
    notifyData.value = ''
  } else {
    subscribeString(characteristicUuid, device.address, (data: string) => notifyData.value = data)
  }
}

async function test() {
  try {
    let resp = await invoke<boolean>('test')
    rustTest.value = resp
  } catch (e) {
    console.error(e)
  }
}

const showServices = ref(false);
</script>

<template>
  <div class="container">
    <h1>Welcome to the blec plugin!</h1>

    <button v-if="scanning" :onclick="stopScan" style="margin-bottom: 5px;">
      Stop Scan
      <div class="lds-ring">
        <div></div>
        <div></div>
        <div></div>
        <div></div>
      </div>
    </button>
    <button v-else :onclick="() => startScan((dev: BleDevice[]) => devices = dev, 5000)" style="margin-bottom: 5px;">
      Start Scan
    </button>
    <div v-if="connected">
      <p>Connected</p>
      <div class="row">
        <button :onclick="disconnect" style="margin-bottom: 5px;">Disconnect</button>
      </div>
      <div>
        {{ rustTest ? 'Rust test successful' : '' }}
        <br />
        <button :onclick="test" style="margin-bottom: 5px;">Test Rust communication</button>
      </div>
      <div class="row">
        <input v-model="characteristicUuid" placeholder="Characteristic UUID" class="uuid-input" />
        <button class="ml" :onclick="async () => {
            try {
              recvData = await readString(device.address, characteristicUuid)
              console.log(`Read data (string): ${recvData}`)
              if (typeof recvData === 'string') {
                const hex = Array.from(recvData, c => c.charCodeAt(0).toString(16).padStart(2, '0')).join(' ')
                console.log(`Read data (hex): ${hex}`)
              } else {
                const str = new TextDecoder('utf-8').decode(recvData)
                const hex = Array.from(str, c => c.charCodeAt(0).toString(16).padStart(2, '0')).join(' ')
                console.log(`Read data (string): ${str}`)
                console.log(`Read data (hex): ${hex}`)
              }
            } catch (e) {
              console.error(`Error reading data: ${e}`)
            }
        }">Read</button>
      </div>
      <div class="row">
        <input v-model="sendData" placeholder="Data to send" class="uuid-input" />
        <button class="ml" :onclick="() => sendString(device.address, characteristicUuid, sendData, 'withoutResponse')">Send</button>
      </div>
      <div class="row">
        <input v-model="characteristicUuid" placeholder="Characteristic UUID" class="uuid-input" />
        <button class="ml" :onclick="subscribe">{{ notifyData ? "Unsubscribe" : "Subscribe" }}</button>
      </div>
    </div>
    <div v-else>
      <label id="show-services-label" for="show-services">Show Services</label>
      <input v-model="showServices" type="checkbox" id="show-services" />
      <div v-for="bleDevice in devices" class="row">
        <BleDev :key="bleDevice.address" :device="bleDevice"
          :show-services="showServices" 
          :onclick="async () => {
            device = bleDevice;
            await connect(bleDevice.address, () => {
              console.log('disconnected');
              device = null;
            });
            console.log('connect command returned');
          }"
        />
      </div>
    </div>
    <!-- Received Data Section -->
    <div style="margin-top: 2em; text-align: center;">
      <h2>Received Data</h2>
      <div v-if="recvData">
      <strong>Read (String):</strong>
      {{
        typeof recvData === 'string'
        ? recvData
        : new TextDecoder('utf-8').decode(recvData)
      }}
      <br />
      <strong>Read (Hex):</strong>
      {{
        (() => {
        const str = typeof recvData === 'string' ? recvData : new TextDecoder('utf-8').decode(recvData)
        return Array.from(str, c => c.charCodeAt(0).toString(16).padStart(2, '0')).join(' ')
        })()
      }}
      </div>
      <div v-if="notifyData">
      <strong>Notification (String):</strong>
      {{ notifyData }}
      <br />
      <strong>Notification (Hex):</strong>
      {{
        Array.from(notifyData, c => c.charCodeAt(0).toString(16).padStart(2, '0')).join(' ')
      }}
      </div>
      <div v-if="!recvData && !notifyData">
      <em>No data received yet.</em>
      </div>
    </div>
  </div>
</template>

<style scoped>
.logo.vite:hover {
  filter: drop-shadow(0 0 2em #747bff);
}

.logo.vue:hover {
  filter: drop-shadow(0 0 2em #249b73);
}

:root {
  font-family: Inter, Avenir, Helvetica, Arial, sans-serif;
  font-size: 16px;
  line-height: 24px;
  font-weight: 400;

  color: #0f0f0f;
  background-color: #f6f6f6;

  font-synthesis: none;
  text-rendering: optimizeLegibility;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  -webkit-text-size-adjust: 100%;
}

.container {
  margin: 0;
  padding-top: 10vh;
  display: flex;
  flex-direction: column;
  justify-content: center;
  text-align: center;
}

.logo {
  height: 6em;
  padding: 1.5em;
  will-change: filter;
  transition: 0.75s;
}

.logo.tauri:hover {
  filter: drop-shadow(0 0 2em #24c8db);
}

.row {
  display: flex;
  justify-content: center;
  margin-bottom: 5px;
}

.ml {
  margin-left: 5px;
  min-width: 35%;
}

a {
  font-weight: 500;
  color: #646cff;
  text-decoration: inherit;
}

a:hover {
  color: #535bf2;
}

h1 {
  text-align: center;
}

input,
button {
  border-radius: 8px;
  border: 1px solid transparent;
  padding: 0.6em 1.2em;
  font-size: 1em;
  font-weight: 500;
  font-family: inherit;
  color: #0f0f0f;
  background-color: #ffffff;
  transition: border-color 0.25s;
  box-shadow: 0 2px 2px rgba(0, 0, 0, 0.2);
}

button {
  cursor: pointer;
}

button:hover {
  border-color: #396cd8;
}

button:active {
  border-color: #396cd8;
  background-color: #e8e8e8;
}

input,
button {
  outline: none;
}

#greet-input {
  margin-right: 5px;
}

#show-services-label {
  margin-top: 5px;
  font-size: 1.3em;
}
#show-services {
  margin: 5px;
  height: 2em;
  width: 2em;
}

:root {
  color: #f6f6f6;
  background-color: #2f2f2f;
}

a:hover {
  color: #24c8db;
}

input,
button {
  color: #ffffff;
  background-color: #0f0f0f98;
}

button:active {
  background-color: #0f0f0f69;
}


.lds-ring,
.lds-ring div {
  box-sizing: border-box;
}

.lds-ring {
  display: inline-block;
  position: relative;
  width: 15px;
  height: 15px;
}

.lds-ring div {
  box-sizing: border-box;
  display: block;
  position: absolute;
  width: 14px;
  height: 14px;
  margin: 2px;
  border: 2px solid currentColor;
  border-radius: 50%;
  animation: lds-ring 1.2s cubic-bezier(0.5, 0, 0.5, 1) infinite;
  border-color: currentColor transparent transparent transparent;
}

.lds-ring div:nth-child(1) {
  animation-delay: -0.45s;
}

.lds-ring div:nth-child(2) {
  animation-delay: -0.3s;
}

.lds-ring div:nth-child(3) {
  animation-delay: -0.15s;
}

@keyframes lds-ring {
  0% {
    transform: rotate(0deg);
  }

  100% {
    transform: rotate(360deg);
  }
}

.uuid-input {
  margin-left: 5px;
  width: 250px;
}
</style>
