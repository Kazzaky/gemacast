import { describe, it, expect, beforeEach } from 'bun:test';
import {
  setupInvokeMock,
  invokeCalls,
  makeDeviceInfo,
  makeDiscoveredStreamer,
} from '../__tests__/setup';
import { useAppStore } from '../stores/app-store';
import { ConnectionMode, Status } from '../core/types';
import {
  connectionController,
  getPairingDecisionWarning,
  isTerminalConnectError,
} from './connection-controller';
import { ErrorCode } from '../core/error';
import { useToastStore } from '../stores/toast-store';

const connectToStreamer = connectionController.connect.bind(connectionController);
const disconnect = connectionController.disconnect.bind(connectionController);
const handleStreamerTimeout = connectionController.streamerTimedOut.bind(connectionController);
const handleForceDisconnect = connectionController.forceDisconnect.bind(connectionController);
const handleLinkLost = connectionController.linkLost.bind(connectionController);
const handleLinkRecovered = connectionController.linkRecovered.bind(connectionController);
const changeAudioSource = connectionController.changeAudioSource.bind(connectionController);
const reconnectOnAppOpen = connectionController.reconnectOnAppOpen.bind(connectionController);

beforeEach(() => {
  localStorage.clear();
  setupInvokeMock({
    connect_to_streamer: undefined,
    disconnect_from_streamer: undefined,
    kill_playback: undefined,
    notify_streaming_stopped: undefined,
    get_audio_sources: [[], { supportsProcessCapture: false }],
    get_process_list: [],
    probe_streamer: undefined,
    establish_websocket: undefined,
    change_audio_source: undefined,
    start_link_recovery: undefined,
    stop_link_recovery: undefined,
  });
  useAppStore.getState().init(makeDeviceInfo());
  useAppStore.getState().setStatus(Status.Listening);
  useToastStore.setState({ toasts: [] });
});

describe('connectToStreamer', () => {
  it('classifies pairing decisions as terminal', () => {
    expect(isTerminalConnectError('streamer rejected the request (pairing_rejected)')).toBe(true);
    expect(isTerminalConnectError(new Error('HTTP request failed'))).toBe(false);
  });

  it('maps pairing decisions to concise warnings', () => {
    expect(getPairingDecisionWarning('streamer rejected the request (pairing_rejected)')).toBe(
      'Pairing request rejected on the PC',
    );
    expect(getPairingDecisionWarning(new Error('pairing was cancelled on the phone'))).toBe(
      'Pairing cancelled',
    );
    expect(getPairingDecisionWarning('HTTP request failed')).toBeNull();
  });

  it('transitions through Connecting → Connected on success', async () => {
    const streamer = makeDiscoveredStreamer();
    const result = await connectToStreamer(streamer);
    expect(result.ok).toBe(true);
    expect(useAppStore.getState().status).toBe(Status.Connected);
    expect(useAppStore.getState().connectedStreamer?.deviceId).toBe(streamer.deviceId);
    expect(useAppStore.getState().isLoading).toBe(false);
  });

  it('invokes connect_to_streamer with correct IP', async () => {
    await connectToStreamer(makeDiscoveredStreamer({ addr: '10.0.0.1:9000' }));
    const call = invokeCalls.find((c) => c.cmd === 'connect_to_streamer');
    expect(call).toBeTruthy();
    expect((call?.args as Record<string, unknown>).ip).toBe('10.0.0.1');
  });

  it('saves lastConnectedStreamer on connect', async () => {
    const streamer = makeDiscoveredStreamer();
    await connectToStreamer(streamer);
    expect(useAppStore.getState().lastConnectedStreamer?.deviceId).toBe(streamer.deviceId);
  });

  it('returns err and reverts to Listening on IPC failure', async () => {
    setupInvokeMock({
      connect_to_streamer: () => {
        throw new Error('refused');
      },
    });
    const result = await connectToStreamer(makeDiscoveredStreamer());
    expect(result.ok).toBe(false);
    expect(useAppStore.getState().status).toBe(Status.Listening);
    expect(useAppStore.getState().error).not.toBeNull();
  });

  it('does not retry a rejected LAN pairing', async () => {
    let attempts = 0;
    setupInvokeMock({
      connect_to_streamer: () => {
        attempts += 1;
        throw new Error('streamer rejected the request (pairing_rejected)');
      },
    });

    await connectToStreamer(makeDiscoveredStreamer());

    expect(attempts).toBe(1);
  });

  it('shows a warning instead of a playback error when the PC rejects pairing', async () => {
    setupInvokeMock({
      connect_to_streamer: () => {
        throw new Error('streamer rejected the request (pairing_rejected)');
      },
    });

    const result = await connectToStreamer(makeDiscoveredStreamer());

    expect(result.ok).toBe(false);
    expect(useAppStore.getState().error).toBeNull();
    expect(useToastStore.getState().toasts).toHaveLength(1);
    expect(useToastStore.getState().toasts[0]).toMatchObject({
      type: 'warning',
      message: 'Pairing request rejected on the PC',
    });
  });

  it('shows a warning instead of a playback error when pairing is cancelled on the phone', async () => {
    setupInvokeMock({
      connect_to_streamer: () => {
        throw new Error('PC identity confirmation was cancelled on the phone');
      },
    });

    const result = await connectToStreamer(makeDiscoveredStreamer());

    expect(result.ok).toBe(false);
    expect(useAppStore.getState().error).toBeNull();
    expect(useToastStore.getState().toasts).toHaveLength(1);
    expect(useToastStore.getState().toasts[0]).toMatchObject({
      type: 'warning',
      message: 'Pairing cancelled',
    });
  });

  it('resets reconnectAttempts to 0 on connect', async () => {
    useAppStore.getState().patch({ reconnectAttempts: 3 });
    await connectToStreamer(makeDiscoveredStreamer());
    expect(useAppStore.getState().reconnectAttempts).toBe(0);
  });
});

describe('disconnect', () => {
  it('transitions to Listening and clears connectedStreamer', async () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().patch({
      connectedStreamer: streamer,
      status: Status.Connected,
    });
    const result = await disconnect();
    expect(result.ok).toBe(true);
    expect(useAppStore.getState().connectedStreamer).toBeNull();
    expect(useAppStore.getState().status).toBe(Status.Listening);
  });

  it('invokes disconnect_from_streamer IPC', async () => {
    useAppStore.getState().patch({
      connectedStreamer: makeDiscoveredStreamer({ addr: '10.0.0.2:9000' }),
      status: Status.Connected,
    });
    await disconnect();
    expect(invokeCalls.some((c) => c.cmd === 'disconnect_from_streamer')).toBe(true);
  });

  it('still succeeds when no streamer is connected', async () => {
    const result = await disconnect();
    expect(result.ok).toBe(true);
  });

  it('resets metrics to all-null', async () => {
    useAppStore.getState().patch({
      connectedStreamer: makeDiscoveredStreamer(),
      status: Status.Connected,
    });
    useAppStore.getState().updateMetrics({ bufferMs: 10, networkRttMs: 12, jitterMs: 5 });
    await disconnect();
    const { metrics } = useAppStore.getState();
    expect(metrics.bufferMs).toBeNull();
    expect(metrics.networkRttMs).toBeNull();
    expect(metrics.jitterMs).toBeNull();
  });

  function disconnectWithPresenceMidFlight(streamer: ReturnType<typeof makeDiscoveredStreamer>) {
    const seen: Array<unknown> = [];
    setupInvokeMock({
      disconnect_from_streamer: () => {
        seen.push(useAppStore.getState().updateDiscoveredStreamer(streamer));
        return undefined;
      },
      kill_playback: undefined,
      notify_streaming_stopped: undefined,
      get_audio_sources: [[], { supportsProcessCapture: false }],
      get_process_list: [],
    });
    return seen;
  }

  it('does not auto-reconnect when a presence packet lands mid-disconnect', async () => {
    const streamer = makeDiscoveredStreamer({ deviceId: 'pc-1' });
    useAppStore.getState().patch({
      connectedStreamer: streamer,
      lastConnectedStreamer: streamer,
      status: Status.Connected,
      isSuspended: false,
    });

    const targets = disconnectWithPresenceMidFlight(streamer);
    await disconnect(true);

    expect(targets).toHaveLength(1);
    expect(targets[0]).toBeNull();
    expect(useAppStore.getState().lastConnectedStreamer).toBeNull();
    expect(useAppStore.getState().status).toBe(Status.Listening);
  });

  it('does not auto-reconnect a suspended session when a presence packet lands mid-disconnect', async () => {
    const streamer = makeDiscoveredStreamer({ deviceId: 'pc-1' });
    useAppStore.getState().patch({
      connectedStreamer: streamer,
      lastConnectedStreamer: streamer,
      status: Status.Connected,
      isSuspended: false,
    });

    const targets = disconnectWithPresenceMidFlight(streamer);
    // forgetStreamer=false keeps the streamer on purpose, so `isSuspended` is the
    // only thing holding the gate shut — it has to be set before the await too.
    await disconnect(false);

    expect(targets).toHaveLength(1);
    expect(targets[0]).toBeNull();
    expect(useAppStore.getState().lastConnectedStreamer?.deviceId).toBe(streamer.deviceId);
    expect(useAppStore.getState().isSuspended).toBe(true);
  });

  it('still auto-reconnects a genuine link drop, which the gate exists for', () => {
    const streamer = makeDiscoveredStreamer({ deviceId: 'pc-1' });
    useAppStore.getState().patch({
      connectedStreamer: null,
      lastConnectedStreamer: streamer,
      lastConnectedMode: ConnectionMode.Wifi,
      status: Status.Listening,
      isSuspended: false,
    });

    expect(useAppStore.getState().updateDiscoveredStreamer(streamer)).toMatchObject({
      deviceId: 'pc-1',
    });
  });
});

describe('reconnectOnAppOpen', () => {
  const ALL_MODES = { wifi: true, usb: true };

  function mockBridge(modes: { wifi: boolean; usb: boolean }) {
    setupInvokeMock({
      get_connection_status: modes,
      connect_to_streamer: undefined,
      disconnect_from_streamer: undefined,
      establish_websocket: undefined,
      kill_playback: undefined,
      notify_streaming_stopped: undefined,
      get_audio_sources: [[], { supportsProcessCapture: false }],
      get_process_list: [],
    });
  }

  async function connectThenDisconnect(streamer: ReturnType<typeof makeDiscoveredStreamer>) {
    useAppStore.getState().setDiscoveredStreamers([streamer]);
    await connectToStreamer(streamer);
    await disconnect(true);
  }

  it('does nothing when no PC has ever been connected', async () => {
    mockBridge(ALL_MODES);
    useAppStore.getState().setDiscoveredStreamers([makeDiscoveredStreamer({ deviceId: 'pc-1' })]);

    await reconnectOnAppOpen();

    expect(invokeCalls.some((c) => c.cmd === 'connect_to_streamer')).toBe(false);
    expect(useAppStore.getState().lastConnectedStreamer).toBeNull();
  });

  it('reconnects to the saved PC after an explicit disconnect and reopen', async () => {
    const streamer = makeDiscoveredStreamer({ deviceId: 'pc-1' });
    mockBridge(ALL_MODES);
    await connectThenDisconnect(streamer);
    expect(useAppStore.getState().lastConnectedStreamer).toBeNull();

    mockBridge(ALL_MODES);
    await reconnectOnAppOpen();

    expect(useAppStore.getState().status).toBe(Status.Connected);
    expect(useAppStore.getState().connectedStreamer?.deviceId).toBe('pc-1');
  });

  it('does not reconnect when the saved mode is unavailable', async () => {
    const streamer = makeDiscoveredStreamer({ deviceId: 'pc-1' });
    mockBridge(ALL_MODES);
    await connectThenDisconnect(streamer);

    mockBridge({ wifi: false, usb: true });
    await reconnectOnAppOpen();

    expect(invokeCalls.some((c) => c.cmd === 'connect_to_streamer')).toBe(false);
    expect(useAppStore.getState().status).toBe(Status.Listening);
  });

  it('switches to the saved mode instead of connecting on the wrong one', async () => {
    const streamer = makeDiscoveredStreamer({ deviceId: 'pc-1' });
    mockBridge(ALL_MODES);
    useAppStore.getState().updateSettings({ mode: ConnectionMode.Usb });
    await connectThenDisconnect(streamer);
    useAppStore.getState().updateSettings({ mode: ConnectionMode.Wifi });

    mockBridge(ALL_MODES);
    await reconnectOnAppOpen();

    expect(useAppStore.getState().settings.mode).toBe(ConnectionMode.Usb);
    expect(invokeCalls.some((c) => c.cmd === 'connect_to_streamer')).toBe(false);
    expect(useAppStore.getState().lastConnectedStreamer?.deviceId).toBe('pc-1');
  });

  it('does nothing when the toggle is off', async () => {
    const streamer = makeDiscoveredStreamer({ deviceId: 'pc-1' });
    mockBridge(ALL_MODES);
    await connectThenDisconnect(streamer);
    useAppStore.getState().updateSettings({ autoReconnect: false });

    mockBridge(ALL_MODES);
    await reconnectOnAppOpen();

    expect(invokeCalls.some((c) => c.cmd === 'connect_to_streamer')).toBe(false);
    expect(useAppStore.getState().lastConnectedStreamer).toBeNull();
  });

  it('does nothing while a session is live', async () => {
    const streamer = makeDiscoveredStreamer({ deviceId: 'pc-1' });
    mockBridge(ALL_MODES);
    useAppStore.getState().setDiscoveredStreamers([streamer]);
    await connectToStreamer(streamer);

    mockBridge(ALL_MODES);
    await reconnectOnAppOpen();

    expect(invokeCalls.some((c) => c.cmd === 'connect_to_streamer')).toBe(false);
  });
});

describe('handleStreamerTimeout', () => {
  it('removes the timed-out streamer from the list', () => {
    const streamer = makeDiscoveredStreamer({ deviceId: 'gone' });
    useAppStore.getState().setDiscoveredStreamers([streamer]);
    handleStreamerTimeout('gone');
    expect(useAppStore.getState().discoveredStreamers).toHaveLength(0);
  });

  it('enters Listening when connected streamer times out', () => {
    const streamer = makeDiscoveredStreamer({ deviceId: 'pc-1' });
    useAppStore.getState().patch({
      connectedStreamer: streamer,
      lastConnectedStreamer: streamer,
      discoveredStreamers: [streamer],
      status: Status.Connected,
    });
    handleStreamerTimeout('pc-1');
    expect(useAppStore.getState().status).toBe(Status.Listening);
    expect(useAppStore.getState().connectedStreamer).toBeNull();
  });

  it('sets a streamerTimeout error', () => {
    const streamer = makeDiscoveredStreamer({ deviceId: 'pc-1' });
    useAppStore.getState().patch({
      connectedStreamer: streamer,
      discoveredStreamers: [streamer],
    });
    handleStreamerTimeout('pc-1');
    expect(useAppStore.getState().error?.code).toBe(ErrorCode.NETWORK_STREAMER_TIMEOUT);
  });
});

describe('handleForceDisconnect', () => {
  it('clears connectedStreamer and moves to Listening', () => {
    useAppStore.getState().patch({
      connectedStreamer: makeDiscoveredStreamer(),
      status: Status.Playing,
    });
    handleForceDisconnect();
    expect(useAppStore.getState().connectedStreamer).toBeNull();
    expect(useAppStore.getState().status).toBe(Status.Listening);
  });

  it('forgets lastConnectedStreamer when forgetStreamer=true', () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().patch({
      lastConnectedStreamer: streamer,
      status: Status.Connected,
    });
    handleForceDisconnect(true);
    expect(useAppStore.getState().lastConnectedStreamer).toBeNull();
  });

  it('retains lastConnectedStreamer when forgetStreamer=false', () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().patch({
      connectedStreamer: streamer,
      lastConnectedStreamer: streamer,
      status: Status.Connected,
    });
    handleForceDisconnect(false);
    expect(useAppStore.getState().lastConnectedStreamer?.deviceId).toBe(streamer.deviceId);
  });
});

describe('handleLinkLost', () => {
  it('starts link recovery for the streamer that was lost', async () => {
    const streamer = makeDiscoveredStreamer({ addr: '10.0.0.7:9000' });
    useAppStore.getState().patch({ connectedStreamer: streamer, status: Status.Playing });

    await handleLinkLost();

    const call = invokeCalls.find((c) => c.cmd === 'start_link_recovery');
    expect(call).toBeTruthy();
    expect((call?.args as Record<string, unknown>).ip).toBe('10.0.0.7');
  });

  it('never forgets the streamer — recovery has to have something to reconnect to', async () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().patch({
      connectedStreamer: streamer,
      lastConnectedStreamer: streamer,
      status: Status.Playing,
    });

    await handleLinkLost();

    expect(useAppStore.getState().connectedStreamer).toBeNull();
    expect(useAppStore.getState().lastConnectedStreamer?.deviceId).toBe(streamer.deviceId);
    expect(useAppStore.getState().status).toBe(Status.Listening);
    expect(useAppStore.getState().isSuspended).toBe(true);
  });

  it('tears the session down before arming the prober', async () => {
    useAppStore
      .getState()
      .patch({ connectedStreamer: makeDiscoveredStreamer(), status: Status.Playing });

    await handleLinkLost();

    // kill_playback cancels recovery on the Rust side, so a prober armed
    // before it would be aborted by the very teardown it followed.
    const killIndex = invokeCalls.findIndex((c) => c.cmd === 'kill_playback');
    const startIndex = invokeCalls.findIndex((c) => c.cmd === 'start_link_recovery');
    expect(killIndex).toBeGreaterThanOrEqual(0);
    expect(startIndex).toBeGreaterThan(killIndex);
  });

  it('does nothing when there was no live session to lose', async () => {
    useAppStore.getState().patch({ connectedStreamer: null, status: Status.Listening });

    await handleLinkLost();

    expect(invokeCalls.find((c) => c.cmd === 'start_link_recovery')).toBeUndefined();
  });
});

describe('handleLinkRecovered', () => {
  it('reconnects to the streamer the link was lost from', async () => {
    const streamer = makeDiscoveredStreamer({ addr: '10.0.0.8:9000' });
    useAppStore.getState().patch({
      lastConnectedStreamer: streamer,
      status: Status.Listening,
      isSuspended: true,
    });

    await handleLinkRecovered(true);

    expect(useAppStore.getState().status).toBe(Status.Connected);
    const call = invokeCalls.find((c) => c.cmd === 'connect_to_streamer');
    expect((call?.args as Record<string, unknown>).ip).toBe('10.0.0.8');
  });

  it('reconnects the same way whether or not the PC still had us registered', async () => {
    const streamer = makeDiscoveredStreamer();
    useAppStore.getState().patch({ lastConnectedStreamer: streamer, status: Status.Listening });
    await handleLinkRecovered(false);

    // `deviceRegistered` is observability only today: both answers take the
    // full handshake, so neither can skip it.
    expect(invokeCalls.filter((c) => c.cmd === 'connect_to_streamer')).toHaveLength(1);
    expect(useAppStore.getState().status).toBe(Status.Connected);
  });

  it('does not reconnect on top of a session the user already restored', async () => {
    useAppStore.getState().patch({
      lastConnectedStreamer: makeDiscoveredStreamer(),
      connectedStreamer: makeDiscoveredStreamer(),
      status: Status.Connected,
    });

    await handleLinkRecovered(true);

    expect(invokeCalls.find((c) => c.cmd === 'connect_to_streamer')).toBeUndefined();
  });
});

describe('changeAudioSource', () => {
  it('returns err when no streamer connected', async () => {
    const result = await changeAudioSource({ type: 'desktop' });
    expect(result.ok).toBe(false);
  });

  it('invokes change_audio_source IPC on success', async () => {
    setupInvokeMock({ change_audio_source: undefined });
    useAppStore.getState().patch({
      connectedStreamer: makeDiscoveredStreamer({ addr: '10.0.0.5:9000' }),
      status: Status.Connected,
    });
    const result = await changeAudioSource({ type: 'desktop' });
    expect(result.ok).toBe(true);
    const call = invokeCalls.find((c) => c.cmd === 'change_audio_source');
    expect(call).toBeTruthy();
    expect((call?.args as Record<string, unknown>).ip).toBe('10.0.0.5');
  });

  it('returns err on IPC failure', async () => {
    setupInvokeMock({
      change_audio_source: () => {
        throw new Error('denied');
      },
    });
    useAppStore.getState().patch({
      connectedStreamer: makeDiscoveredStreamer(),
      status: Status.Connected,
    });
    const result = await changeAudioSource({ type: 'desktop' });
    expect(result.ok).toBe(false);
  });
});
