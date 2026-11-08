package com.apir.gemacast

import android.app.AlertDialog
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import android.os.PowerManager
import android.os.SystemClock
import android.provider.Settings
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.widget.CheckBox
import android.widget.FrameLayout
import androidx.activity.enableEdgeToEdge
import androidx.annotation.Keep
import androidx.core.content.FileProvider
import java.io.File
import java.math.BigInteger
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import org.json.JSONArray
import org.json.JSONObject

class MainActivity : TauriActivity() {
    companion object {
        private const val DEVICE_AUTH_KEY_ALIAS = "gemacast_device_auth_p256_v1"
        private const val P256_COORDINATE_SIZE = 32
        private const val TRUSTED_PC_PREFERENCES = "gemacast_trusted_pcs_v1"
        private const val APP_STATE_PREFERENCES = "gemacast_app_state_v1"
        private const val BATTERY_PROMPT_SUPPRESSED_KEY = "battery_prompt_suppressed"
    }
    private var gemaCastService: GemaCastService? = null
    private var webViewSystemBars: WebViewSystemBars? = null
    private var serviceBound = false
    @Volatile private var pendingPcId: String? = null
    @Volatile private var pendingPcFingerprint: String? = null
    private val pcIdentityConfirmationState = PcIdentityConfirmationState()
    private var pcIdentityConfirmationDialog: AlertDialog? = null
    private var pcIdentityConfirmationDialogKey: PcIdentityConfirmationKey? = null

    /** Guards the battery-optimization prompt so it shows at most once per launch. */
    private var batteryOptimizationPrompted = false

    private val serviceConnection =
            object : ServiceConnection {
                override fun onServiceConnected(name: ComponentName?, service: IBinder?) {
                    val binder = service as GemaCastService.LocalBinder
                    gemaCastService = binder.getService()
                    serviceBound = true
                }

                override fun onServiceDisconnected(name: ComponentName?) {
                    gemaCastService = null
                    serviceBound = false
                }
            }

    private var multicastLock: android.net.wifi.WifiManager.MulticastLock? = null

    private fun acquireMulticastLock() {
        try {
            val wifiManager =
                    applicationContext.getSystemService(Context.WIFI_SERVICE) as
                            android.net.wifi.WifiManager
            multicastLock =
                    wifiManager.createMulticastLock("GemaCast::DiscoveryMulticastLock").also {
                        it.setReferenceCounted(false)
                        it.acquire()
                    }
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    private fun releaseMulticastLock() {
        try {
            multicastLock?.let { if (it.isHeld) it.release() }
            multicastLock = null
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    private fun isStreamingActive(): Boolean {
        return File(cacheDir, ".streaming_active").exists()
    }

    @Keep
    fun getTransportType(): String {
        return try {
            val connectivityManager =
                    getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
            val activeNetwork = connectivityManager.activeNetwork
            val caps = connectivityManager.getNetworkCapabilities(activeNetwork)

            val activeTransports = mutableSetOf<String>()
            if (caps != null) {
                if (caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) {
                    // Include WiFi frequency for band detection (2.4GHz vs 5GHz)
                    // WifiInfo.getFrequency() available since API 21, Tauri requires API 24+
                    try {
                        val wifiManager =
                                applicationContext.getSystemService(Context.WIFI_SERVICE) as
                                        android.net.wifi.WifiManager
                        val wifiInfo = wifiManager.connectionInfo
                        val freq = wifiInfo?.frequency ?: 0
                        if (freq > 0) {
                            activeTransports.add("WIFI:$freq")
                        } else {
                            activeTransports.add("WIFI")
                        }
                    } catch (e: Exception) {
                        activeTransports.add("WIFI")
                    }
                }
                if (caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET))
                        activeTransports.add("ETHERNET")
            }

            val networkType =
                    if (activeTransports.isEmpty()) "NONE" else activeTransports.joinToString(",")

            // Tethering state. Needed because when this phone shares its
            // connection, its *active* network is the upstream (often cellular),
            // so the checks above report neither WIFI nor ETHERNET — and a Wi-Fi
            // hotspot would otherwise be indistinguishable from a USB cable.
            val tether =
                    try {
                        val ifaces =
                                java.net.NetworkInterface.getNetworkInterfaces()
                                        ?.toList()
                                        ?.filter { it.isUp && !it.isLoopback }
                                        ?.map { iface ->
                                            TetherClassifier.Iface(
                                                    iface.name ?: "",
                                                    iface.inetAddresses
                                                            .toList()
                                                            .filterIsInstance<
                                                                    java.net.Inet4Address>()
                                                            .mapNotNull { it.hostAddress },
                                            )
                                        }
                                        ?: emptyList()
                        TetherClassifier.classify(ifaces)
                    } catch (e: Exception) {
                        TetherClassifier.Tether.NONE
                    }

            val intentFilter = android.content.IntentFilter("android.hardware.usb.action.USB_STATE")
            val tetherStatus =
                    when (tether) {
                        TetherClassifier.Tether.HOTSPOT -> "|HOTSPOT"
                        TetherClassifier.Tether.USB -> "|USB_TETHER"
                        TetherClassifier.Tether.NONE -> ""
                    }

            "${networkType}${tetherStatus}"
        } catch (e: Exception) {
            "ERROR: ${e.message}"
        }
    }

    private fun getOrCreateDeviceAuthKeyPair(): java.security.KeyPair {
        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val existingPrivateKey = keyStore.getKey(DEVICE_AUTH_KEY_ALIAS, null)
        val existingCertificate = keyStore.getCertificate(DEVICE_AUTH_KEY_ALIAS)
        if (existingPrivateKey != null && existingCertificate != null) {
            return java.security.KeyPair(
                    existingCertificate.publicKey,
                    existingPrivateKey as java.security.PrivateKey
            )
        }

        val generator =
                KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore")
        val spec =
                KeyGenParameterSpec.Builder(
                                DEVICE_AUTH_KEY_ALIAS,
                                KeyProperties.PURPOSE_SIGN or KeyProperties.PURPOSE_VERIFY,
                        )
                        .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
                        .setDigests(KeyProperties.DIGEST_SHA256)
                        .setUserAuthenticationRequired(false)
                        .build()
        generator.initialize(spec)
        return generator.generateKeyPair()
    }

    private fun fixedUnsigned(value: BigInteger): ByteArray {
        val encoded = value.toByteArray()
        val unsigned =
                if (encoded.size == P256_COORDINATE_SIZE + 1 && encoded[0].toInt() == 0) {
                    encoded.copyOfRange(1, encoded.size)
                } else {
                    encoded
                }
        require(unsigned.size <= P256_COORDINATE_SIZE) { "P-256 coordinate is too large" }
        return ByteArray(P256_COORDINATE_SIZE - unsigned.size) + unsigned
    }

    @Keep
    fun getDeviceAuthPublicKey(): String {
        return try {
            val publicKey = getOrCreateDeviceAuthKeyPair().public as ECPublicKey
            val sec1 =
                    byteArrayOf(0x04) +
                            fixedUnsigned(publicKey.w.affineX) +
                            fixedUnsigned(publicKey.w.affineY)
            Base64.encodeToString(sec1, Base64.NO_WRAP)
        } catch (e: Exception) {
            e.printStackTrace()
            "ERROR: ${e.message}"
        }
    }

    @Keep
    fun signDeviceAuthTranscript(transcriptBase64: String): String {
        return try {
            val transcript = Base64.decode(transcriptBase64, Base64.DEFAULT)
            val signature =
                    Signature.getInstance("SHA256withECDSA").apply {
                        initSign(getOrCreateDeviceAuthKeyPair().private)
                        update(transcript)
                    }
            Base64.encodeToString(signature.sign(), Base64.NO_WRAP)
        } catch (e: Exception) {
            e.printStackTrace()
            "ERROR: ${e.message}"
        }
    }

    @Keep
    fun getTrustedPcFingerprint(pcId: String): String {
        return try {
            getSharedPreferences(TRUSTED_PC_PREFERENCES, Context.MODE_PRIVATE).getString(pcId, "")
                    ?: ""
        } catch (e: Exception) {
            e.printStackTrace()
            "ERROR: ${e.message}"
        }
    }

    @Keep
    fun getTrustedPcIds(): String {
        return try {
            val ids = JSONArray()
            getSharedPreferences(TRUSTED_PC_PREFERENCES, Context.MODE_PRIVATE)
                    .all
                    .keys
                    .asSequence()
                    .sorted()
                    .forEach(ids::put)
            ids.toString()
        } catch (e: Exception) {
            e.printStackTrace()
            "ERROR: ${e.message}"
        }
    }

    private fun pcIdentityConfirmationKey(request: JSONObject): PcIdentityConfirmationKey {
        val pcId = request.getString("pcId")
        val fingerprint = request.getString("fingerprint").lowercase()
        val pairingCode = request.getString("pairingCode")
        require(fingerprint.matches(Regex("[0-9a-f]{64}"))) { "Invalid PC certificate fingerprint" }
        require(pairingCode.matches(Regex("[0-9]{6}"))) { "Invalid pairing code" }
        return PcIdentityConfirmationKey(pcId, fingerprint, pairingCode)
    }

    @Keep
    fun confirmPcIdentity(payload: String): String {
        return try {
            val request = JSONObject(payload)
            val key = pcIdentityConfirmationKey(request)
            val pcName = request.getString("pcName")
            val requiresApproval = request.optBoolean("requiresApproval", true)

            val stored =
                    getSharedPreferences(TRUSTED_PC_PREFERENCES, Context.MODE_PRIVATE)
                            .getString(key.pcId, null)
            if (stored != null && !stored.equals(key.fingerprint, ignoreCase = true)) {
                return "ERROR: The paired PC certificate changed. Forget this PC before pairing again."
            }
            if (stored != null && !requiresApproval) {
                return "TRUSTED"
            }

            when (pcIdentityConfirmationState.begin(key, SystemClock.elapsedRealtime())) {
                PcIdentityConfirmationState.BeginResult.APPROVED -> "APPROVED"
                PcIdentityConfirmationState.BeginResult.REJECTED -> "REJECTED"
                PcIdentityConfirmationState.BeginResult.PENDING -> "PENDING"
                PcIdentityConfirmationState.BeginResult.BUSY ->
                        "ERROR: Another PC pairing confirmation is already pending"
                PcIdentityConfirmationState.BeginResult.STARTED -> {
                    runOnUiThread {
                        pcIdentityConfirmationDialog?.dismiss()
                        val shortFingerprint = key.fingerprint.take(16).chunked(4).joinToString(" ")
                        val dialog =
                                AlertDialog.Builder(this)
                                        .setTitle("Verify Gemacast PC")
                                        .setMessage(
                                                "Connect to $pcName?\n\n" +
                                                        "Pairing code:  ${key.pairingCode}\n\n" +
                                                        "After tapping Continue, confirm that the PC shows the same code.\n\n" +
                                                        "PC certificate: $shortFingerprint"
                                        )
                                        .setPositiveButton("Continue") { _, _ ->
                                            if (pcIdentityConfirmationState.complete(
                                                            key,
                                                            approved = true,
                                                            nowMillis =
                                                                    SystemClock.elapsedRealtime(),
                                                    )
                                            ) {
                                                pendingPcId = key.pcId
                                                pendingPcFingerprint = key.fingerprint
                                            }
                                        }
                                        .setNegativeButton("Cancel") { _, _ ->
                                            pcIdentityConfirmationState.complete(
                                                    key,
                                                    approved = false,
                                                    nowMillis = SystemClock.elapsedRealtime(),
                                            )
                                        }
                                        .create()
                        dialog.setOnCancelListener {
                            pcIdentityConfirmationState.complete(
                                    key,
                                    approved = false,
                                    nowMillis = SystemClock.elapsedRealtime(),
                            )
                        }
                        dialog.setOnDismissListener {
                            if (pcIdentityConfirmationDialogKey == key) {
                                pcIdentityConfirmationDialog = null
                                pcIdentityConfirmationDialogKey = null
                            }
                        }
                        pcIdentityConfirmationDialog = dialog
                        pcIdentityConfirmationDialogKey = key
                        dialog.show()
                    }
                    "PENDING"
                }
            }
        } catch (e: Exception) {
            e.printStackTrace()
            "ERROR: ${e.message}"
        }
    }

    @Keep
    fun pollPcIdentityConfirmation(payload: String): String {
        return try {
            val key = pcIdentityConfirmationKey(JSONObject(payload))
            pcIdentityConfirmationState.poll(key, SystemClock.elapsedRealtime()).name
        } catch (e: Exception) {
            e.printStackTrace()
            "ERROR: ${e.message}"
        }
    }

    @Keep
    fun cancelPcIdentityConfirmation(payload: String): String {
        return try {
            val key = pcIdentityConfirmationKey(JSONObject(payload))
            pcIdentityConfirmationState.cancel(key)
            runOnUiThread {
                if (pcIdentityConfirmationDialogKey == key) {
                    pcIdentityConfirmationDialog?.dismiss()
                    pcIdentityConfirmationDialog = null
                    pcIdentityConfirmationDialogKey = null
                }
            }
            "OK"
        } catch (e: Exception) {
            e.printStackTrace()
            "ERROR: ${e.message}"
        }
    }

    @Keep
    fun rememberPcIdentity(payload: String): String {
        return try {
            val request = JSONObject(payload)
            val pcId = request.getString("pcId")
            val fingerprint = request.getString("fingerprint").lowercase()
            require(fingerprint.matches(Regex("[0-9a-f]{64}"))) {
                "Invalid PC certificate fingerprint"
            }
            val existing =
                    getSharedPreferences(TRUSTED_PC_PREFERENCES, Context.MODE_PRIVATE)
                            .getString(pcId, null)
            if (existing != null && !existing.equals(fingerprint, ignoreCase = true)) {
                return "ERROR: Refusing to replace an existing PC certificate pin"
            }
            if (existing == null && (pendingPcId != pcId || pendingPcFingerprint != fingerprint)) {
                return "ERROR: PC certificate was not confirmed by the phone user"
            }
            val saved =
                    getSharedPreferences(TRUSTED_PC_PREFERENCES, Context.MODE_PRIVATE)
                            .edit()
                            .putString(pcId, fingerprint)
                            .commit()
            if (!saved) {
                return "ERROR: Android could not persist the PC certificate pin"
            }
            pendingPcId = null
            pendingPcFingerprint = null
            "OK"
        } catch (e: Exception) {
            e.printStackTrace()
            "ERROR: ${e.message}"
        }
    }

    @Keep
    fun forgetPcIdentity(pcId: String): String {
        return try {
            require(pcId.isNotBlank()) { "PC ID cannot be empty" }
            val preferences = getSharedPreferences(TRUSTED_PC_PREFERENCES, Context.MODE_PRIVATE)
            if (!preferences.edit().remove(pcId).commit()) {
                return "ERROR: Android could not remove the PC certificate pin"
            }
            if (pendingPcId == pcId) {
                pendingPcId = null
                pendingPcFingerprint = null
            }
            pcIdentityConfirmationState.cancelAll()
            runOnUiThread {
                if (pcIdentityConfirmationDialogKey?.pcId == pcId) {
                    pcIdentityConfirmationDialog?.dismiss()
                    pcIdentityConfirmationDialog = null
                    pcIdentityConfirmationDialogKey = null
                }
            }
            "OK"
        } catch (e: Exception) {
            e.printStackTrace()
            "ERROR: ${e.message}"
        }
    }

    @Keep
    fun syncServiceState(action: String, isExclusive: Boolean) {
        try {
            val intent =
                    Intent(this, GemaCastService::class.java).apply {
                        this.action = action
                        putExtra("EXCLUSIVE_MODE", isExclusive)
                    }
            startService(intent)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    /**
     * Install an APK from the given file path using the system package installer.
     *
     * Called from Rust via JNI. This method MUST live in Kotlin (not in JNI Rust code) because
     * `with_webview`/`jni_handle().exec()` runs on a native thread whose class loader is the boot
     * class loader. The boot class loader cannot see application-level classes like
     * `androidx.core.content.FileProvider`, causing `NoClassDefFoundError`. By keeping all
     * FileProvider/Intent logic in Kotlin, the app's own class loader is used and the class is
     * found normally.
     */
    @Keep
    fun installApk(path: String): String? {
        return try {
            val file = File(path)
            if (!file.exists()) {
                return "APK file does not exist at path: $path"
            }
            val authority = "${packageName}.fileprovider"
            val contentUri = FileProvider.getUriForFile(applicationContext, authority, file)

            val intent =
                    Intent(Intent.ACTION_VIEW).apply {
                        setDataAndType(contentUri, "application/vnd.android.package-archive")
                        addFlags(
                                Intent.FLAG_GRANT_READ_URI_PERMISSION or
                                        Intent.FLAG_ACTIVITY_NEW_TASK
                        )
                    }
            startActivity(intent)
            null // Success
        } catch (e: Exception) {
            e.printStackTrace()
            "Exception in installApk: ${e.message}"
        }
    }

    /**
     * Finish this activity and drop its task record, then let the caller kill the process.
     *
     * Called from Rust while handling `ExitRequested`. Without it the process exits with the task
     * record still in place, and because the activity is `launchMode="singleTask"` a relaunch can
     * be routed into that stale record instead of starting cold. Removing the task also fires
     * `GemaCastService.onTaskRemoved`, which is where the service's own teardown lives.
     *
     * Deliberately does not kill the process — Rust owns that, so there is exactly one place that
     * ends the app.
     */
    @Keep
    fun finishAndRemoveAppTask(): String {
        return try {
            val finished = java.util.concurrent.CountDownLatch(1)
            runOnUiThread {
                try {
                    finishAndRemoveTask()
                } catch (e: Exception) {
                    e.printStackTrace()
                } finally {
                    finished.countDown()
                }
            }
            // Bounded on purpose: the caller kills the process straight after this
            // returns, so a busy main thread must not be able to stall the exit.
            finished.await(1, java.util.concurrent.TimeUnit.SECONDS)
            "OK"
        } catch (e: Exception) {
            e.printStackTrace()
            "ERROR: ${e.message}"
        }
    }

    private fun promptBatteryOptimization() {
        if (batteryOptimizationPrompted) return
        batteryOptimizationPrompted = true
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return
        try {
            val powerManager = getSystemService(POWER_SERVICE) as PowerManager
            if (powerManager.isIgnoringBatteryOptimizations(packageName)) return

            val appState = getSharedPreferences(APP_STATE_PREFERENCES, Context.MODE_PRIVATE)
            if (appState.getBoolean(BATTERY_PROMPT_SUPPRESSED_KEY, false)) return

            val dontShowAgain = CheckBox(this).apply { text = "Do not show this again" }

            val inset = (24 * resources.displayMetrics.density).toInt()
            val checkboxRow =
                    FrameLayout(this).apply {
                        setPadding(inset, inset / 2, inset, 0)
                        addView(dontShowAgain)
                    }

            AlertDialog.Builder(this)
                    .setTitle("Battery Optimization")
                    .setMessage(
                            "To prevent audio from stuttering, Gemacast needs to be excluded from battery optimizations. Please disable battery optimization for Gemacast in the setting."
                    )
                    .setView(checkboxRow)
                    .setPositiveButton("Allow") { _, _ ->
                        try {
                            val intent =
                                    Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
                                            .apply { data = Uri.parse("package:$packageName") }
                            startActivity(intent)
                        } catch (e: Exception) {
                            e.printStackTrace()
                        }
                    }
                    .setNegativeButton("Not Now", null)
                    .setOnDismissListener {
                        if (dontShowAgain.isChecked) {
                            appState.edit().putBoolean(BATTERY_PROMPT_SUPPRESSED_KEY, true).apply()
                        }
                    }
                    .show()
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        webViewSystemBars = WebViewSystemBars(findViewById(android.R.id.content)).also { it.install() }

        promptBatteryOptimization()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) webViewSystemBars?.refresh()
    }

    override fun onStart() {
        super.onStart()
        Intent(this, GemaCastService::class.java).also { intent ->
            bindService(intent, serviceConnection, Context.BIND_AUTO_CREATE)
        }
    }

    override fun onPause() {
        releaseMulticastLock()
        super.onPause() // MUST be called to prevent SuperNotCalledException
    }

    override fun onStop() {
        if (serviceBound) {
            unbindService(serviceConnection)
            serviceBound = false
            gemaCastService = null
        }
        super.onStop() // MUST be called to prevent SuperNotCalledException
    }

    override fun onResume() {
        super.onResume()
        webViewSystemBars?.refresh()
        acquireMulticastLock()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
    }

    override fun onDestroy() {
        pcIdentityConfirmationState.cancelAll()
        pcIdentityConfirmationDialog?.dismiss()
        pcIdentityConfirmationDialog = null
        pcIdentityConfirmationDialogKey = null
        super.onDestroy()
    }
}
