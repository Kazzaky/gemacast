import type { AppSettings, DiscoveredStreamer, JitterConfig, SavedPreset } from './types';
import { ConnectionMode } from './types';
import { JITTER_PRESETS } from './presets';

const LS_LAST_STREAMER = 'gemacast_last_streamer';
const LS_LAST_MODE = 'gemacast_last_mode';
const LS_SETTINGS = 'gemacast_settings';
const LS_DEVICE_ID = 'gemacast_device_id';
const LS_PC_NAMES = 'gemacast_pc_names';

/**
 * Cap on the remembered-name cache. Bounded because every manually-entered IP
 * also lands here, and the map is otherwise append-only for the life of the
 * install. Oldest insertions are evicted first.
 */
const MAX_REMEMBERED_PC_NAMES = 64;

const DEFAULT_AUTO_CONFIG = JITTER_PRESETS.find((p) => p.id === 'auto')!.config!;

export const DEFAULT_SETTINGS: AppSettings = {
  theme: 'dark',
  mode: ConnectionMode.Wifi,
  exclusiveMode: true,
  keepScreenOn: false,
  autoReconnect: true,
  bufferPreset: 'auto',
  customJitterConfig: DEFAULT_AUTO_CONFIG,
  savedPresets: [],
  bitratePreset: '128',
  customBitrateKbps: 128,
  gainDb: 0,
  matchPcVolume: false,
};

function sanitizeJitterConfig(value: unknown, fallback: JitterConfig): JitterConfig {
  if (!value || typeof value !== 'object') return { ...fallback };
  const config = value as Partial<JitterConfig>;
  const staticTargetMs = config.staticTargetMs;
  if (
    staticTargetMs != null &&
    (!Number.isInteger(staticTargetMs) || staticTargetMs < 0 || staticTargetMs > 5000)
  ) {
    return { ...fallback };
  }
  return { ...fallback, ...config };
}

function sanitizeSettings(value: unknown): AppSettings {
  if (!value || typeof value !== 'object') return { ...DEFAULT_SETTINGS };
  const incoming = value as Partial<AppSettings>;
  const customBitrateKbps =
    Number.isInteger(incoming.customBitrateKbps) &&
    incoming.customBitrateKbps! >= 6 &&
    incoming.customBitrateKbps! <= 512
      ? incoming.customBitrateKbps!
      : DEFAULT_SETTINGS.customBitrateKbps;
  const savedPresets = Array.isArray(incoming.savedPresets)
    ? incoming.savedPresets
        .filter((preset): preset is SavedPreset => Boolean(preset?.name && preset.config))
        .map((preset) => ({
          ...preset,
          config: sanitizeJitterConfig(preset.config, DEFAULT_AUTO_CONFIG),
        }))
    : [];
  return {
    ...DEFAULT_SETTINGS,
    ...incoming,
    customBitrateKbps,
    customJitterConfig: sanitizeJitterConfig(incoming.customJitterConfig, DEFAULT_AUTO_CONFIG),
    savedPresets,
  };
}

export function loadLastStreamer(): DiscoveredStreamer | null {
  try {
    const raw = localStorage.getItem(LS_LAST_STREAMER);
    return raw ? (JSON.parse(raw) as DiscoveredStreamer) : null;
  } catch {
    return null;
  }
}

export function saveLastStreamer(streamer: DiscoveredStreamer | null) {
  if (streamer) {
    localStorage.setItem(LS_LAST_STREAMER, JSON.stringify(streamer));
  } else {
    localStorage.removeItem(LS_LAST_STREAMER);
  }
}

export function loadLastMode(): ConnectionMode | null {
  try {
    const raw = localStorage.getItem(LS_LAST_MODE);
    return raw === ConnectionMode.Wifi || raw === ConnectionMode.Usb
      ? raw
      : null;
  } catch {
    return null;
  }
}

export function saveLastMode(mode: ConnectionMode | null) {
  if (mode) {
    localStorage.setItem(LS_LAST_MODE, mode);
  } else {
    localStorage.removeItem(LS_LAST_MODE);
  }
}

export function loadSettings(): AppSettings {
  try {
    const raw = localStorage.getItem(LS_SETTINGS);
    if (raw) {
      return sanitizeSettings(JSON.parse(raw));
    }
  } catch {
    // Ignore JSON parse errors
  }
  return DEFAULT_SETTINGS;
}

function readPcNames(): Record<string, string> {
  try {
    const raw = localStorage.getItem(LS_PC_NAMES);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};
    const entries = Object.entries(parsed as Record<string, unknown>).filter(
      (entry): entry is [string, string] => typeof entry[1] === 'string' && entry[1].length > 0,
    );
    return Object.fromEntries(entries);
  } catch {
    return {};
  }
}

export function loadPcNames(): Record<string, string> {
  return readPcNames();
}

/**
 * Cache one PC's display name. No-ops when the name carries no information
 * (empty, or identical to the id we would fall back to anyway), so a streamer
 * discovered without a name cannot overwrite a good cached one.
 */
export function rememberPcName(deviceId: string, deviceName: string) {
  if (!deviceId || !deviceName || deviceName === deviceId) return;

  const names = readPcNames();
  if (names[deviceId] === deviceName) return;

  // Re-insert last so eviction below drops the least recently written.
  delete names[deviceId];
  names[deviceId] = deviceName;

  const keys = Object.keys(names);
  const pruned =
    keys.length > MAX_REMEMBERED_PC_NAMES
      ? Object.fromEntries(
          keys.slice(keys.length - MAX_REMEMBERED_PC_NAMES).map((k) => [k, names[k]]),
        )
      : names;

  try {
    localStorage.setItem(LS_PC_NAMES, JSON.stringify(pruned));
  } catch {
    // A full quota must not break a connection.
  }
}

/** Drop a cached name — paired with forgetting the PC's identity natively. */
export function forgetPcName(deviceId: string) {
  const names = readPcNames();
  if (!(deviceId in names)) return;
  delete names[deviceId];
  try {
    localStorage.setItem(LS_PC_NAMES, JSON.stringify(names));
  } catch {
    // Ignore quota errors.
  }
}

export function saveSettings(settings: AppSettings) {
  localStorage.setItem(LS_SETTINGS, JSON.stringify(settings));
}

export function generateUuid(): string {
  if (typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
    const arr = new Uint8Array(1);
    const r = crypto.getRandomValues
      ? (crypto.getRandomValues(arr), arr[0] % 16)
      : (Math.random() * 16) | 0;
    const v = c === 'x' ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

export function getOrCreateDeviceId(): string {
  let deviceId = localStorage.getItem(LS_DEVICE_ID);
  if (!deviceId) {
    deviceId = generateUuid();
    localStorage.setItem(LS_DEVICE_ID, deviceId);
  }
  return deviceId;
}
