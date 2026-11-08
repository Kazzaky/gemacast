import { listen } from '@tauri-apps/api/event';
import { tauriBridge } from '../core/tauri-bridge';
import { useToastStore } from '../stores/toast-store';
import { useUpdateStore } from '../stores/update-store';

export class UpdateController {
  start(): () => void {
    document.addEventListener('visibilitychange', this.handleVisibilityChange);
    return () => document.removeEventListener('visibilitychange', this.handleVisibilityChange);
  }

  async check(): Promise<void> {
    const currentStatus = useUpdateStore.getState().status;
    useUpdateStore.getState().setChecking();
    if (currentStatus !== 'ready' && currentStatus !== 'installing') {
      await tauriBridge.cleanupStaleUpdates().catch(() => undefined);
    }

    try {
      const update = await tauriBridge.checkForUpdate();
      if (update) {
        useUpdateStore.getState().setAvailable(update.version, update.downloadUrl, update.sha256);
      } else {
        useUpdateStore.getState().setUpToDate();
      }
    } catch (error) {
      this.reportError('Update check failed', error);
    }
  }

  async download(): Promise<void> {
    const { status, downloadUrl, sha256, version } = useUpdateStore.getState();
    if (status !== 'available' || !downloadUrl || !version || !sha256) return;

    const unlisten = await listen<number>('update-progress', (event) => {
      if (useUpdateStore.getState().status === 'downloading') {
        useUpdateStore.getState().setDownloading(event.payload);
      }
    });
    useUpdateStore.getState().setDownloading(0);

    try {
      const apkPath = await tauriBridge.downloadUpdate({ url: downloadUrl, sha256 });
      useUpdateStore.getState().setReady(version, apkPath);
    } catch (error) {
      this.reportError('Download failed', error);
    } finally {
      unlisten();
    }
  }

  async install(): Promise<void> {
    const { status, apkPath } = useUpdateStore.getState();
    if (status !== 'ready' || !apkPath) return;

    useUpdateStore.getState().setInstalling();
    try {
      await tauriBridge.installApk({ path: apkPath });
    } catch (error) {
      this.reportError('Install failed', error);
    }
  }

  retry(): void {
    useUpdateStore.getState().reset();
    void this.check();
  }

  appResumed(): void {
    useUpdateStore.getState().handleAppResume();
  }

  private readonly handleVisibilityChange = (): void => {
    if (document.visibilityState === 'visible') this.appResumed();
  };

  private reportError(summary: string, error: unknown): void {
    const message = error instanceof Error ? error.message : String(error);
    useUpdateStore.getState().setError(message);
    useToastStore.getState().show('error', summary, message);
  }
}

export const updateController = new UpdateController();

export function useUpdateController() {
  const state = useUpdateStore();
  return {
    state,
    checkForUpdates: () => updateController.check(),
    startDownload: () => updateController.download(),
    installUpdate: () => updateController.install(),
    retry: () => updateController.retry(),
  };
}
