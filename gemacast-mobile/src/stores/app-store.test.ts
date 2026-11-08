import { describe, it, expect, beforeEach } from 'bun:test';
import { makeDeviceInfo, makeDiscoveredStreamer } from '../__tests__/setup';
import { useAppStore } from './app-store';
import { ConnectionMode, Status } from '../core/types';
import { GemaCastError, ErrorCode } from '../core/error';

beforeEach(() => {
  useAppStore.getState().init(makeDeviceInfo());
});

describe('app-store — initialization', () => {
  it('initializes with Idle status', () => {
    expect(useAppStore.getState().status).toBe(Status.Idle);
  });

  it('stores device info', () => {
    expect(useAppStore.getState().deviceInfo.deviceName).toBe('Test Phone');
    expect(useAppStore.getState().deviceInfo.ip).toBe('192.168.1.100');
  });

  it('starts with null connected streamer', () => {
    expect(useAppStore.getState().connectedStreamer).toBeNull();
  });
});

describe('app-store — status transitions', () => {
  it('transitions to Listening', () => {
    useAppStore.getState().setStatus(Status.Listening);
    expect(useAppStore.getState().status).toBe(Status.Listening);
  });

  it('transitions through Connecting → Connected', () => {
    useAppStore.getState().setStatus(Status.Connecting);
    expect(useAppStore.getState().status).toBe(Status.Connecting);
    useAppStore.getState().setStatus(Status.Connected);
    expect(useAppStore.getState().status).toBe(Status.Connected);
  });
});

describe('app-store — discovered streamers', () => {
  it('adds a new streamer', () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().updateDiscoveredStreamer(streamer);
    expect(useAppStore.getState().discoveredStreamers).toHaveLength(1);
    expect(useAppStore.getState().discoveredStreamers[0].deviceId).toBe(streamer.deviceId);
  });

  it('updates an existing streamer', () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().updateDiscoveredStreamer(streamer);
    const updated = { ...streamer, deviceName: 'New Name' };
    useAppStore.getState().updateDiscoveredStreamer(updated);
    expect(useAppStore.getState().discoveredStreamers).toHaveLength(1);
    expect(useAppStore.getState().discoveredStreamers[0].deviceName).toBe('New Name');
  });

  it('removes an offline streamer', () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().updateDiscoveredStreamer(streamer);
    useAppStore.getState().updateDiscoveredStreamer({ ...streamer, isOffline: true });
    expect(useAppStore.getState().discoveredStreamers).toHaveLength(0);
  });

  it('clears connected streamer when it goes offline', () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().setConnectedStreamer(streamer);
    useAppStore.getState().updateDiscoveredStreamer(streamer);
    useAppStore.getState().updateDiscoveredStreamer({ ...streamer, isOffline: true });
    expect(useAppStore.getState().connectedStreamer).toBeNull();
    expect(useAppStore.getState().status).toBe(Status.Listening);
  });

  it('returns auto-reconnect target when last connected streamer reappears', () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().patch({
      status: Status.Listening,
      lastConnectedStreamer: streamer,
      lastConnectedMode: ConnectionMode.Wifi,
      isSuspended: false,
    });
    const result = useAppStore.getState().updateDiscoveredStreamer(streamer);
    expect(result?.deviceId).toBe(streamer.deviceId);
  });

  it('does not auto-reconnect when suspended', () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().patch({
      status: Status.Listening,
      lastConnectedStreamer: streamer,
      lastConnectedMode: ConnectionMode.Wifi,
      isSuspended: true,
    });
    const result = useAppStore.getState().updateDiscoveredStreamer(streamer);
    expect(result).toBeNull();
  });

  it('does not auto-reconnect when the toggle is off', () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().patch({
      status: Status.Listening,
      lastConnectedStreamer: streamer,
      lastConnectedMode: ConnectionMode.Wifi,
      isSuspended: false,
      settings: { ...useAppStore.getState().settings, autoReconnect: false },
    });
    const result = useAppStore.getState().updateDiscoveredStreamer(streamer);
    expect(result).toBeNull();
  });

  it('does not auto-reconnect on a different mode than the last connection used', () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().patch({
      status: Status.Listening,
      lastConnectedStreamer: streamer,
      lastConnectedMode: ConnectionMode.Usb,
      isSuspended: false,
    });
    const result = useAppStore.getState().updateDiscoveredStreamer(streamer);
    expect(result).toBeNull();
  });

  it('does not auto-reconnect after an explicit disconnect cleared the target', () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().patch({
      status: Status.Listening,
      lastConnectedStreamer: null,
      lastConnectedMode: ConnectionMode.Wifi,
      isSuspended: false,
    });
    const result = useAppStore.getState().updateDiscoveredStreamer(streamer);
    expect(result).toBeNull();
  });
});

describe('app-store — error handling', () => {
  it('displays a GemaCastError', () => {
    const error = GemaCastError.streamerTimeout();
    useAppStore.getState().displayError(error);
    expect(useAppStore.getState().error?.code).toBe(ErrorCode.NETWORK_STREAMER_TIMEOUT);
  });

  it('displays a string error by wrapping it', () => {
    useAppStore.getState().displayError('something broke');
    expect(useAppStore.getState().error).toBeInstanceOf(GemaCastError);
    expect(useAppStore.getState().error?.code).toBe(ErrorCode.UNKNOWN_ERROR);
  });

  it('dismisses error', () => {
    useAppStore.getState().displayError(GemaCastError.streamerTimeout());
    useAppStore.getState().dismissError();
    expect(useAppStore.getState().error).toBeNull();
  });
});

describe('app-store — metrics', () => {
  it('updates metrics by patch', () => {
    useAppStore.getState().updateMetrics({ bufferMs: 50, jitterMs: 4 });
    const { metrics } = useAppStore.getState();
    expect(metrics.bufferMs).toBe(50);
    expect(metrics.jitterMs).toBe(4);
  });

  it('merges successive metric patches', () => {
    useAppStore.getState().updateMetrics({ bufferMs: 50 });
    useAppStore.getState().updateMetrics({ networkRttMs: 18 });
    const { metrics } = useAppStore.getState();
    expect(metrics.bufferMs).toBe(50);
    expect(metrics.networkRttMs).toBe(18);
  });

  it('resets metrics', () => {
    useAppStore.getState().updateMetrics({ bufferMs: 50, networkRttMs: 18, jitterMs: 4 });
    useAppStore.getState().resetMetrics();
    const { metrics } = useAppStore.getState();
    expect(metrics.bufferMs).toBeNull();
    expect(metrics.networkRttMs).toBeNull();
    expect(metrics.jitterMs).toBeNull();
  });
});

describe('app-store — settings', () => {
  it('updates settings and persists', () => {
    useAppStore.getState().updateSettings({ bitratePreset: '256' });
    expect(useAppStore.getState().settings.bitratePreset).toBe('256');
    const saved = JSON.parse(localStorage.getItem('gemacast_settings')!);
    expect(saved.bitratePreset).toBe('256');
  });
});
