import type { GemaCastError } from './error';

export type Ok<T> = {
  readonly ok: true;
  readonly value: T;
};

export type Err<E> = {
  readonly ok: false;
  readonly error: E;
};

export type Result<T, E = Error> = Ok<T> | Err<E>;

export function ok<T>(value: T): Ok<T> {
  return { ok: true, value };
}
export function err<E>(error: E): Err<E> {
  return { ok: false, error };
}

export type DeviceInfo = {
  deviceId: string;
  deviceName: string;
  ip: string;
};

export type DiscoveredStreamer = {
  deviceId: string;
  deviceName: string;
  addr: string;
  isOffline: boolean;
};

export enum Status {
  Idle = 'idle',
  Listening = 'listening',
  Connecting = 'connecting',
  Connected = 'connected',
  Playing = 'playing',
  Paused = 'paused',
  Reconnecting = 'reconnecting',
}

export function hasLiveSession(status: Status): boolean {
  return (
    status === Status.Connected ||
    status === Status.Playing ||
    status === Status.Paused ||
    status === Status.Reconnecting
  );
}

export type ConnectionHealth = 'ok' | 'degraded' | 'lost';

export type Metrics = {
  bufferMs: number | null;
  networkRttMs: number | null;
  jitterMs: number | null;
};

export enum ConnectionMode {
  Wifi = 'wifi',
  Usb = 'usb',
}

export type JitterConfig = {
  minDepthMs: number;
  comfortCapMs: number;
  peakDecayHalflifeMs: number;
  resumeThresholdPct: number;
  staticTargetMs?: number | null;
};

export type NetworkLink =
  'usbTether' | 'wifi5Ghz' | 'wifi2_4Ghz' | 'ethernet' | 'wifiUnknown' | 'unknown';

export type NetworkLinkPairInfo = {
  phone: NetworkLink;
  pc: NetworkLink;
  effective: NetworkLink;
  effectiveLabel: string;
};

export type SavedPreset = {
  name: string;
  config: JitterConfig;
};

export type PresetId =
  'nobuffer' | 'auto' | 'wired' | 'fast' | 'balanced' | 'stable' | 'resilient' | 'custom';

export type BitratePreset =
  '10' | '24' | '32' | '64' | '96' | '128' | '256' | '450' | '512' | 'raw' | 'custom';

export type AppSettings = {
  theme: 'light' | 'dark';
  mode: ConnectionMode;
  exclusiveMode: boolean;
  keepScreenOn: boolean;
  autoReconnect: boolean;
  bufferPreset: PresetId | string;
  customJitterConfig: JitterConfig;
  savedPresets: SavedPreset[];
  bitratePreset: BitratePreset;
  customBitrateKbps: number;
  gainDb: number;
  matchPcVolume: boolean;
};

export type AudioSource =
  { type: 'desktop' } | { type: 'process'; pid: number; name: string; hasAudioSession?: boolean };

export type StreamerCapabilities = {
  supportsProcessCapture: boolean;
  supportsVolumeSync: boolean;
};

export type ProcessInfo = {
  pid: number;
  name: string;
  hasAudioSession: boolean;
};

export type AppState = {
  deviceInfo: DeviceInfo;
  status: Status;
  discoveredStreamers: DiscoveredStreamer[];
  connectedStreamer: DiscoveredStreamer | null;
  connectingStreamerId: string | null;
  lastConnectedStreamer: DiscoveredStreamer | null;
  lastConnectedMode: ConnectionMode | null;
  error: GemaCastError | null;
  connectionHealth: ConnectionHealth;
  isNetworkAvailable: boolean;
  isLoading: boolean;
  isSuspended: boolean;
  reconnectAttempts: number;
  metrics: Metrics;
  settings: AppSettings;
  availableModes: { wifi: boolean; usb: boolean };
  audioSources: AudioSource[];
  currentAudioSource: AudioSource;
  streamerCapabilities: StreamerCapabilities | null;
  processList: ProcessInfo[];
  networkLinkPair: NetworkLinkPairInfo | null;
  exclusiveSupported: boolean;
  pcOutputVolume: number | null;
};
