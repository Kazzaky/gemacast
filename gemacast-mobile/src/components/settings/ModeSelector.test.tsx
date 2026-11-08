import { describe, it, expect, beforeEach } from 'bun:test';
import { render, screen, cleanup } from '@testing-library/react';
import { useAppStore } from '../../stores/app-store';
import { ConnectionMode } from '../../core/types';
import { ModeSelector } from './ModeSelector';

beforeEach(() => {
  cleanup();
  useAppStore.getState().init({
    deviceId: 'test',
    deviceName: 'Test',
    ip: '127.0.0.1',
  });
});

describe('ModeSelector', () => {
  it('renders WiFi / USB options', () => {
    render(<ModeSelector />);
    expect(screen.getByText('WiFi')).toBeTruthy();
    expect(screen.getByText('USB')).toBeTruthy();
  });

  it('disables USB when modes.usb is false', () => {
    useAppStore.getState().setAvailableModes({ wifi: true, usb: false });
    render(<ModeSelector />);
    const usbInput = screen.getByLabelText('USB') as HTMLInputElement;
    expect(usbInput.disabled).toBe(true);
  });

  it('current mode is selected', () => {
    useAppStore.getState().updateSettings({ mode: ConnectionMode.Wifi });
    render(<ModeSelector />);
    const wifiInput = screen.getByLabelText('WiFi') as HTMLInputElement;
    expect(wifiInput.checked).toBe(true);
  });
});
