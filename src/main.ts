// Kefilex Desk — pairing window frontend.
//
// Minimal vanilla TS so we don't need a framework runtime in the
// release bundle. Three screens, swapped by the route hash:
//
//   default (no hash) → pairing screen (or "already paired" status)
//   #about            → about + version + log file link
//   #revoked          → "this device was revoked — re-pair"
//
// Talks to the Rust side via @tauri-apps/api's invoke().

import { invoke } from '@tauri-apps/api/core'

interface PairingStatus {
  paired: boolean
  device_label: string | null
  device_id: string | null
}

const app = document.querySelector<HTMLDivElement>('#app')!

async function render() {
  const route = window.location.hash || '#'
  if (route === '#about') {
    renderAbout()
    return
  }
  const status = await invoke<PairingStatus>('get_pairing_status')
  if (status.paired) {
    renderPaired(status)
  } else {
    renderUnpaired()
  }
}

function renderUnpaired() {
  app.innerHTML = `
    <h1>Pair this device</h1>
    <p>Open Kefilex Reception in your browser, click <em>Settings → Desk devices → Pair this device</em>, and enter the 6-digit code below.</p>
    <input id="code" class="kbd-input" inputmode="numeric" maxlength="6" placeholder="000 000" autofocus />
    <p class="muted">Optional: name this machine so you can tell devices apart later.</p>
    <input id="device-label" class="kbd-input" style="font-size:14px;letter-spacing:0" placeholder="Reception desk 1" />
    <div style="margin-top:16px;display:flex;gap:8px;">
      <button id="submit">Pair</button>
      <button id="cancel" class="secondary">Cancel</button>
    </div>
    <div id="status" class="status" hidden></div>
    <footer>
      <p>Kefilex Desk v${getPackageVersion()} · MIT licensed · <a href="https://github.com/Kefilab/kefilex-desk" target="_blank" rel="noopener">github.com/Kefilab/kefilex-desk</a></p>
    </footer>
  `
  app.querySelector<HTMLButtonElement>('#submit')!.addEventListener('click', onSubmit)
  app.querySelector<HTMLButtonElement>('#cancel')!.addEventListener('click', () => {
    window.close()
  })
  app.querySelector<HTMLInputElement>('#code')!.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') onSubmit()
  })
}

function renderPaired(status: PairingStatus) {
  app.innerHTML = `
    <h1>Paired</h1>
    <p>This machine is connected to Kefilex as <strong>${escapeHtml(status.device_label ?? 'Unnamed device')}</strong>.</p>
    <div class="status good">✓ Active. Kefilex Desk is watching for incoming calls in the background.</div>
    <p class="muted" style="margin-top:24px;">If you need to pair a different account or fix a stuck connection, click below. You'll need a fresh 6-digit code from Kefilex Reception.</p>
    <div style="display:flex;gap:8px;">
      <button id="unpair" class="secondary">Unpair this device</button>
      <button id="close" class="secondary">Close</button>
    </div>
    <footer>
      <p>Device ID: <code>${status.device_id?.slice(0, 12)}…</code></p>
      <p>Kefilex Desk v${getPackageVersion()} · <a href="https://github.com/Kefilab/kefilex-desk" target="_blank" rel="noopener">source</a></p>
    </footer>
  `
  app.querySelector<HTMLButtonElement>('#unpair')!.addEventListener('click', onUnpair)
  app.querySelector<HTMLButtonElement>('#close')!.addEventListener('click', () => window.close())
}

function renderAbout() {
  app.innerHTML = `
    <h1>About Kefilex Desk</h1>
    <p>Open-source companion app for the Kefilex law-firm SaaS. Watches OS notifications from any softphone (VXT, RingCentral, Aircall, Microsoft Teams, Zoom Phone, Dialpad, 8x8, Cisco Webex Calling) and pipes incoming-call events to the Kefilex Reception inbox in real time.</p>
    <p>Source code: <a href="https://github.com/Kefilab/kefilex-desk" target="_blank" rel="noopener">github.com/Kefilab/kefilex-desk</a></p>
    <p>Privacy: <a href="https://kefilex.com/privacy" target="_blank" rel="noopener">kefilex.com/privacy</a></p>
    <p>Version: ${getPackageVersion()}</p>
    <div style="margin-top:16px;">
      <button class="secondary" id="back">Back</button>
    </div>
  `
  app.querySelector<HTMLButtonElement>('#back')!.addEventListener('click', () => {
    window.location.hash = ''
    render()
  })
}

async function onSubmit() {
  const codeInput = app.querySelector<HTMLInputElement>('#code')!
  const labelInput = app.querySelector<HTMLInputElement>('#device-label')!
  const statusEl = app.querySelector<HTMLDivElement>('#status')!
  const submitBtn = app.querySelector<HTMLButtonElement>('#submit')!

  const code = codeInput.value.replace(/\D/g, '')
  if (code.length !== 6) {
    statusEl.hidden = false
    statusEl.className = 'status bad'
    statusEl.textContent = 'The pairing code is 6 digits.'
    return
  }
  const deviceLabel = labelInput.value.trim() || 'Unnamed Windows device'

  submitBtn.disabled = true
  statusEl.hidden = false
  statusEl.className = 'status'
  statusEl.textContent = 'Pairing…'

  try {
    await invoke('submit_pairing_code', { code, deviceLabel })
    statusEl.className = 'status good'
    statusEl.textContent = '✓ Paired. Reload to see the connected status.'
    setTimeout(() => render(), 1200)
  } catch (err) {
    statusEl.className = 'status bad'
    statusEl.textContent = `Failed: ${String(err)}`
    submitBtn.disabled = false
  }
}

async function onUnpair() {
  if (!confirm('Unpair this device? Kefilex will stop receiving calls from this machine until you pair again.')) {
    return
  }
  await invoke('clear_pairing')
  window.location.hash = ''
  render()
}

function getPackageVersion(): string {
  // Replaced at build time by Vite via the define option in vite.config.ts.
  // Falls back to "dev" during dev mode.
  return (import.meta as { env?: { VITE_APP_VERSION?: string } }).env?.VITE_APP_VERSION ?? 'dev'
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

window.addEventListener('hashchange', render)
render()
