import { useAppStore } from '../stores/app-store';
import { useToastStore } from '../stores/toast-store';
import { tauriBridge } from '../core/tauri-bridge';
import { GemaCastError } from '../core/error';
import { getPresetConfig } from '../core/presets';
import {
  loadLastMode,
  loadLastStreamer,
  rememberPcName,
  saveLastMode,
  saveLastStreamer,
} from '../core/persistence';
import { ConnectionMode, Status } from '../core/types';
import type { AudioSource, DiscoveredStreamer, Result } from '../core/types';
import { ok, err } from '../core/types';

const store = useAppStore;
const toast = useToastStore;

const TERMINAL_CONNECT_ERROR_CODES = [
  'pairing_cancelled',
  'pairing_rejected',
  'pairing_expired',
  'authentication_failed',
  'pairing_capacity_exhausted',
  'pairing_persistence_failed',
  'unauthorized',
  'streamer_offline',
];

export function isTerminalConnectError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  const normalized = message.toLowerCase();
  return TERMINAL_CONNECT_ERROR_CODES.some((code) => normalized.includes(code));
}

export function getPairingDecisionWarning(error: unknown): string | null {
  const message = error instanceof Error ? error.message : String(error);
  const normalized = message.toLowerCase();
  if (normalized.includes('pairing_cancelled') || normalized.includes('cancelled on the phone')) {
    return 'Pairing cancelled';
  }
  if (normalized.includes('pairing_rejected') || normalized.includes('rejected on the pc')) {
    return 'Pairing request rejected on the PC';
  }
  return null;
}

async function connectWithRetry(
  args: Parameters<typeof tauriBridge.connectToStreamer>[0],
  maxRetries: number,
  delayMs: number,
): Promise<void> {
  let lastError: unknown;
  for (let attempt = 0; attempt <= maxRetries; attempt++) {
    try {
      await tauriBridge.connectToStreamer(args);
      return;
    } catch (e) {
      lastError = e;
      if (isTerminalConnectError(e)) throw e;
      if (attempt < maxRetries) {
        console.warn(`Connection attempt ${attempt + 1} failed, retrying in ${delayMs}ms...`, e);
        await new Promise((r) => setTimeout(r, delayMs));
      }
    }
  }
  throw lastError;
}

export class ConnectionController {
  private disconnecting = false;

  async connect(streamer: DiscoveredStreamer): Promise<Result<true, GemaCastError>> {
    store.getState().patch({
      isLoading: true,
      status: Status.Connecting,
      isSuspended: false,
      connectingStreamerId: streamer.deviceId,
    });

    try {
      const state = store.getState();
      const ip = streamer.addr.split(':')[0];
      const settings = state.settings;
      const config = getPresetConfig(settings.bufferPreset, settings.customJitterConfig);

      const isManual = streamer.deviceId.startsWith('manual-');
      const connectionMode = isManual ? ConnectionMode.Wifi : settings.mode;
      const transport =
        connectionMode === ConnectionMode.Usb
          ? 'usb'
          : connectionMode === ConnectionMode.Wifi
            ? 'wifi'
            : null;

      const args = {
        ip,
        deviceId: state.deviceInfo.deviceId,
        deviceName: state.deviceInfo.deviceName,
        mode: connectionMode,
        exclusiveMode: settings.exclusiveMode,
        jitterConfig: config,
        bitratePreset: settings.bitratePreset,
        customBitrateKbps: settings.customBitrateKbps,
        transport,
      };

      await connectWithRetry(args, 0, 300);

      tauriBridge
        .establishWebsocket({ streamerIp: ip, deviceId: state.deviceInfo.deviceId })
        .catch((e) => console.warn('WebSocket setup failed (non-fatal):', e));

      saveLastStreamer(streamer);
      saveLastMode(connectionMode);
      rememberPcName(streamer.deviceId, streamer.deviceName);

      store.getState().dismissError();
      store.getState().patch({
        connectedStreamer: streamer,
        connectingStreamerId: null,
        lastConnectedStreamer: streamer,
        lastConnectedMode: connectionMode,
        status: Status.Connected,
        connectionHealth: 'ok',
        reconnectAttempts: 0,
        isLoading: false,
      });

      void this.fetchAudioSources(streamer);
      void this.refreshProcessList(streamer);

      tauriBridge
        .getNetworkLinkPair()
        .then((pair) => store.getState().setNetworkLinkPair(pair))
        .catch((e) => console.warn('Failed to fetch network link pair:', e));

      const gainDb = store.getState().settings.gainDb;
      if (gainDb !== 0) {
        tauriBridge.setAudioGain({ gainDb }).catch((e) => {
          console.warn('Failed to re-apply audio gain:', e);
        });
      }

      return ok(true);
    } catch (e) {
      const pairingWarning = getPairingDecisionWarning(e);
      const error = pairingWarning ? GemaCastError.from(e) : GemaCastError.failedToStartPlayback(e);
      if (pairingWarning) {
        store.getState().dismissError();
        toast.getState().show('warning', pairingWarning);
      } else {
        store.getState().displayError(error);
      }
      store.getState().patch({
        isLoading: false,
        status: Status.Listening,
        connectingStreamerId: null,
        lastConnectedStreamer: isTerminalConnectError(e)
          ? null
          : store.getState().lastConnectedStreamer,
      });
      return err(error);
    }
  }

  async disconnect(forgetStreamer = true): Promise<Result<true, GemaCastError>> {
    const state = store.getState();

    if (state.status === Status.Listening || state.status === Status.Idle || this.disconnecting) {
      return ok(true);
    }

    this.disconnecting = true;

    const streamer = state.connectedStreamer;
    const retainedStreamer = streamer ?? state.lastConnectedStreamer;

    store.getState().patch({
      status: Status.Listening,
      lastConnectedStreamer: forgetStreamer ? null : retainedStreamer,
      isSuspended: !forgetStreamer,
    });
    store.getState().setLoading(true);

    try {
      if (!streamer) {
        store.getState().patch({
          connectedStreamer: null,
          lastConnectedStreamer: forgetStreamer ? null : retainedStreamer,
          status: Status.Listening,
          connectionHealth: 'ok',
          reconnectAttempts: 0,
          isLoading: false,
          isSuspended: !forgetStreamer,
          networkLinkPair: null,
          pcOutputVolume: null,
        });
        store.getState().resetMetrics();
        tauriBridge.notifyStreamingStopped().catch(console.warn);
        if (forgetStreamer) toast.getState().show('info', 'Disconnected');
        return ok(true);
      }

      try {
        const ip = streamer.addr.split(':')[0];
        await tauriBridge.disconnectFromStreamer({
          ip,
          deviceId: state.deviceInfo.deviceId,
        });
        await new Promise((r) => setTimeout(r, 150));
        tauriBridge.killPlayback().catch(console.warn);
      } catch (e) {
        console.warn('disconnect_from_streamer IPC failed:', e);
        await new Promise((r) => setTimeout(r, 150));
        tauriBridge.killPlayback().catch(console.warn);
      }

      store.getState().patch({
        connectedStreamer: null,
        lastConnectedStreamer: forgetStreamer ? null : retainedStreamer,
        status: Status.Listening,
        connectionHealth: 'ok',
        reconnectAttempts: 0,
        isLoading: false,
        isSuspended: !forgetStreamer,
        audioSources: [],
        currentAudioSource: { type: 'desktop' },
        streamerCapabilities: null,
        processList: [],
        networkLinkPair: null,
        pcOutputVolume: null,
      });
      store.getState().resetMetrics();
      if (forgetStreamer) toast.getState().show('info', 'Disconnected');
      return ok(true);
    } finally {
      this.disconnecting = false;
    }
  }

  streamerTimedOut(deviceId: string) {
    const state = store.getState();
    const list = state.discoveredStreamers.filter((s) => s.deviceId !== deviceId);
    store.getState().setDiscoveredStreamers(list);

    if (state.connectedStreamer?.deviceId === deviceId) {
      store.getState().displayError(GemaCastError.streamerTimeout());
      store.getState().patch({
        connectionHealth: 'lost',
        status: Status.Listening,
        connectedStreamer: null,
      });
      store.getState().resetMetrics();
      tauriBridge.killPlayback().catch(console.warn);
    }
  }

  forceDisconnect(forgetStreamer = true) {
    const state = store.getState();
    if (
      state.status === Status.Listening ||
      state.status === Status.Idle ||
      state.status === Status.Connecting
    ) {
      return;
    }

    store.getState().patch({
      connectedStreamer: null,
      lastConnectedStreamer: forgetStreamer ? null : state.lastConnectedStreamer,
      status: Status.Listening,
      connectionHealth: 'ok',
      reconnectAttempts: 0,
      isSuspended: !forgetStreamer,
    });
    store.getState().resetMetrics();
    tauriBridge.notifyStreamingStopped().catch(console.warn);
    tauriBridge.killPlayback().catch(console.warn);
  }

  async reconnectOnAppOpen() {
    const state = store.getState();
    if (!state.settings.autoReconnect) return;
    if (state.status !== Status.Listening && state.status !== Status.Idle) return;

    const streamer = loadLastStreamer();
    const mode = loadLastMode();
    if (!streamer || !mode) return;

    const modes = await tauriBridge.getConnectionStatus().catch(() => state.availableModes);
    if (!modes[mode]) return;

    store.getState().patch({
      lastConnectedStreamer: streamer,
      lastConnectedMode: mode,
      isSuspended: false,
    });

    if (mode !== state.settings.mode) {
      store.getState().updateSettings({ mode });
      return;
    }

    const onList = state.discoveredStreamers.find((s) => s.deviceId === streamer.deviceId);
    if (onList) await this.connect(onList);
  }

  async linkLost() {
    const state = store.getState();
    if (
      state.status === Status.Listening ||
      state.status === Status.Idle ||
      state.status === Status.Connecting
    ) {
      return;
    }

    const streamer = state.connectedStreamer ?? state.lastConnectedStreamer;

    store.getState().patch({
      connectedStreamer: null,
      lastConnectedStreamer: streamer,
      status: Status.Listening,
      connectionHealth: 'lost',
      reconnectAttempts: 0,
      isSuspended: true,
    });
    store.getState().resetMetrics();

    await tauriBridge.notifyStreamingStopped().catch(console.warn);
    await tauriBridge.killPlayback().catch(console.warn);

    if (!streamer) return;

    const ip = streamer.addr.split(':')[0];
    await tauriBridge
      .startLinkRecovery({ ip, deviceId: state.deviceInfo.deviceId })
      .catch((e) => console.warn('Failed to start link recovery:', e));
  }

  async linkRecovered(deviceRegistered: boolean | null) {
    const state = store.getState();
    if (state.status === Status.Connected || state.status === Status.Connecting) return;

    const streamer = state.lastConnectedStreamer;
    if (!streamer) return;

    console.info(`Link recovered (PC still had us registered: ${deviceRegistered})`);
    toast.getState().show('info', 'Connection restored — reconnecting');
    await this.connect(streamer);
  }

  linkRecoveryGaveUp() {
    store.getState().patch({ connectionHealth: 'lost' });
    toast.getState().show('warning', 'Could not reach the PC — tap to reconnect');
  }

  async changeAudioSource(source: AudioSource): Promise<Result<true, GemaCastError>> {
    const state = store.getState();
    const streamer = state.connectedStreamer;
    if (!streamer) return err(GemaCastError.from('No streamer connected'));

    try {
      const ip = streamer.addr.split(':')[0];
      await tauriBridge.changeAudioSource({
        ip,
        deviceId: state.deviceInfo.deviceId,
        source,
      });
      store.getState().setCurrentAudioSource(source);
      toast.getState().show('success', 'Audio source changed');
      return ok(true);
    } catch (e) {
      console.error('Failed to change source:', e);
      return err(GemaCastError.from(e));
    }
  }

  killPlayback() {
    tauriBridge.killPlayback().catch(console.warn);
  }

  private async fetchAudioSources(streamer: DiscoveredStreamer) {
    try {
      const ip = streamer.addr.split(':')[0];
      const result = await tauriBridge.getAudioSources({ ip });
      store.getState().patch({
        audioSources: result[0],
        streamerCapabilities: result[1],
      });
    } catch (e) {
      console.warn('Failed to fetch audio sources:', e);
      store.getState().patch({
        audioSources: [{ type: 'desktop' }],
        streamerCapabilities: { supportsProcessCapture: false, supportsVolumeSync: false },
      });
    }
  }

  async refreshProcessList(streamer: DiscoveredStreamer) {
    try {
      const ip = streamer.addr.split(':')[0];
      const processes = await tauriBridge.getProcessList({ ip });
      store.getState().setProcessList(processes);
    } catch (e) {
      console.warn('Failed to fetch process list:', e);
      store.getState().setProcessList([]);
    }
  }
}

export const connectionController = new ConnectionController();
