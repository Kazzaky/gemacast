import { invoke } from '@tauri-apps/api/core';
import type {
  AudioSource,
  BitratePreset,
  ConnectionMode,
  JitterConfig,
  NetworkLinkPairInfo,
  ProcessInfo,
  StreamerCapabilities,
} from './types';

export function resolveBitrate(preset: BitratePreset, customKbps: number): number | null {
  if (preset === 'raw') return null;
  const kbps = preset === 'custom' ? customKbps : Number(preset);
  if (!Number.isInteger(kbps) || kbps < 6 || kbps > 512) {
    throw new Error('Bitrate must be a whole number from 6 to 512 Kbps');
  }
  return kbps * 1000;
}

export type ConnectArgs = {
  ip: string;
  deviceId: string;
  deviceName: string;
  mode: ConnectionMode | string;
  exclusiveMode: boolean;
  jitterConfig: JitterConfig;
  bitratePreset: BitratePreset;
  customBitrateKbps: number;
  transport: string | null;
};

export type DisconnectArgs = {
  ip: string;
  deviceId: string;
};

export type PlaybackArgs = {
  ip: string | null;
  deviceId: string;
  deviceName?: string;
};

export const tauriBridge = {
  checkForUpdate: () =>
    invoke<{ version: string; downloadUrl: string; sha256: string } | null>('check_for_update'),

  downloadUpdate: (args: { url: string; sha256: string }) =>
    invoke<string>('download_update', args),

  installApk: (args: { path: string }) => invoke('install_apk', args),

  cleanupStaleUpdates: () => invoke('cleanup_stale_updates'),

  connectToStreamer: (args: ConnectArgs) =>
    invoke('connect_to_streamer', {
      ip: args.ip,
      deviceId: args.deviceId,
      deviceName: args.deviceName,
      mode: args.mode,
      exclusiveMode: args.exclusiveMode,
      jitterConfig: args.jitterConfig,
      bitrate: resolveBitrate(args.bitratePreset, args.customBitrateKbps),
      transport: args.transport,
    }),

  disconnectFromStreamer: (args: DisconnectArgs) => invoke('disconnect_from_streamer', args),

  startAudioPlayback: (args: PlaybackArgs) => invoke('start_audio_playback', args),

  stopAudioPlayback: (args: Omit<PlaybackArgs, 'deviceName'>) =>
    invoke('stop_audio_playback', args),

  killPlayback: () => invoke('kill_playback'),

  notifyStreamingStopped: () => invoke('notify_streaming_stopped'),

  startListeningForStreamers: (args: { deviceId: string; mode: ConnectionMode }) =>
    invoke('start_listening_for_streamers', args),

  stopListeningForStreamers: () => invoke('stop_listening_for_streamers'),

  getNetworkState: () =>
    invoke<{
      localIp: string;
      networkId: string;
      modes: { wifi: boolean; usb: boolean };
    }>('get_network_state'),

  forgetPcIdentity: (pcId: string) => invoke('forget_pc_identity', { pcId }),

  getPairedPcIds: () => invoke<string[]>('get_paired_pc_ids'),

  getLocalIp: () => invoke<string>('get_local_ip'),

  getNetworkIdentifier: () => invoke<string>('get_network_identifier'),

  getConnectionStatus: () =>
    invoke<{ wifi: boolean; usb: boolean }>('get_connection_status'),

  getAudioSources: (args: { ip: string }) =>
    invoke<[AudioSource[], StreamerCapabilities]>('get_audio_sources', args),

  changeAudioSource: (args: { ip: string; deviceId: string; source: AudioSource }) =>
    invoke('change_audio_source', args),

  getProcessList: (args: { ip: string }) => invoke<ProcessInfo[]>('get_process_list', args),

  updateJitterConfig: (args: { jitterConfig: JitterConfig }) =>
    invoke('update_jitter_config', args),

  changeAudioBitrate: (args: { ip: string; deviceId: string; bitrate: number | null }) =>
    invoke('change_audio_bitrate', args),

  probeStreamer: (args: { ip: string; deviceId: string }) => invoke('probe_streamer', args),

  startLinkRecovery: (args: { ip: string; deviceId: string }) =>
    invoke('start_link_recovery', args),

  stopLinkRecovery: () => invoke('stop_link_recovery'),

  establishWebsocket: (args: { streamerIp: string; deviceId: string }) =>
    invoke('establish_websocket', args),

  setAudioGain: (args: { gainDb: number }) => invoke('set_audio_gain', args),

  setMatchPcVolume: (args: { enabled: boolean }) => invoke('set_match_pc_volume', args),

  restartSession: (args: { exclusiveMode: boolean }) => invoke('restart_session', args),

  getNetworkLinkPair: () => invoke<NetworkLinkPairInfo | null>('get_network_link_pair'),

  checkExclusiveSupport: () => invoke<boolean>('check_exclusive_support'),
};
