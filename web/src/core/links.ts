const REPO_URL = 'https://github.com/Kazzaky/gemacast'
const LATEST_DOWNLOAD_URL = `${REPO_URL}/releases/latest/download`

export const links = {
  repo: REPO_URL,
  releases: `${REPO_URL}/releases/latest`,
  issues: `${REPO_URL}/issues`,
  contact: 'mailto:echa.apriliyanto.dev@gmail.com',
  androidApk: `${LATEST_DOWNLOAD_URL}/gemacast-mobile.apk`,
  windowsInstaller: `${LATEST_DOWNLOAD_URL}/gemacast-pc-x86_64-pc-windows-msvc.msi`,
  windowsPortable: `${LATEST_DOWNLOAD_URL}/gemacast-pc-x86_64-pc-windows-msvc.zip`,
  macosDmg: `${LATEST_DOWNLOAD_URL}/gemacast-pc-universal-apple-darwin.dmg`,
  linuxArchive: `${LATEST_DOWNLOAD_URL}/gemacast-pc-x86_64-unknown-linux-gnu.tar.xz`,
} as const
