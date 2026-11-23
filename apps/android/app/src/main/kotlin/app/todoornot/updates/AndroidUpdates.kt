package app.todoornot.updates

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInfo
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.Build
import android.os.SystemClock
import androidx.core.content.IntentCompat
import app.todoornot.BuildConfig
import java.io.File
import java.io.IOException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONException

internal data class UpdateState(
    val currentVersion: String = BuildConfig.VERSION_NAME,
    val checkedAt: Long = 0,
    val checking: Boolean = false,
    val offer: UpdateManifest? = null,
    val progress: Int? = null,
    val ready: Boolean = false,
    val downloadBlocked: Boolean = false,
    val dismissed: Boolean = false,
    val installing: Boolean = false,
    val permissionNeeded: Boolean = false,
    val confirmation: Intent? = null,
    val problem: String? = null,
    val status: String? = null,
)

/** The updater is independent of pairing. All disk writes and installer callbacks serialize. */
internal class AndroidUpdates(private val app: Context, private val feed: String = BuildConfig.UPDATE_FEED) {
    val enabled = feed.isNotEmpty()
    private val lock = Mutex()
    private val prefs = app.getSharedPreferences("app_updates", Context.MODE_PRIVATE)
    private val directory = File(app.noBackupFilesDir, "app-updates")
    private val part = File(directory, "download.part.apk")
    private val apk = File(directory, "update.apk")
    private val installer get() = app.packageManager.packageInstaller
    private var loaded = false
    private var installId: Int? = null
    private var installed = BuildConfig.VERSION_CODE.toLong()
    private val mutableState = MutableStateFlow(UpdateState())
    val state = mutableState.asStateFlow()
    private var view: UpdateState
        get() = mutableState.value
        set(value) { mutableState.value = value }

    private fun abandon(id: Int) { runCatching { installer.abandonSession(id) } }

    private fun restore() {
        if (loaded) return
        loaded = true
        val own = app.packageManager.getPackageInfo(app.packageName, 0)
        installed = own.longVersionCode
        val oldSession = prefs.getInt("session", -1)
        part.delete()
        if (!enabled || prefs.getString("feed", null) != feed) {
            apk.delete()
            if (oldSession != -1) abandon(oldSession)
            installId = null
            prefs.edit().clear().putString("feed", feed).commit()
        }
        val offer = prefs.getString("manifest", null)?.let(::parse)?.takeIf { newer(it.versionCode, installed) }
        val blocked = failureFor(offer)
        if (blocked != null) apk.delete()
        if (offer == null) {
            apk.delete()
            prefs.edit().remove("manifest").apply()
        }
        view = UpdateState(
            currentVersion = own.versionName ?: BuildConfig.VERSION_NAME,
            checkedAt = prefs.getLong("checked", 0),
            offer = offer,
            ready = offer != null && blocked == null && apk.isFile,
            downloadBlocked = blocked != null,
            problem = blocked,
            dismissed = offer?.versionCode == prefs.getLong("dismissed", 0),
        )
        // A still-live PackageInstaller session from a prior process can deliver a result. If
        // the user retries first, abandon it and create a fresh session/confirmation.
        if (!view.ready) UpdateAlerts.clear(app)
    }

    suspend fun activate() = withContext(Dispatchers.IO) {
        lock.withLock { restore() }
    }

    fun startupDue(now: Long) = enabled && checkDue(now, prefs.getLong("attempt", 0))

    /** True to ask WorkManager to retry with backoff. No installer is ever started by a worker. */
    suspend fun run(mode: UpdateMode, expectedCode: Long?, retrying: Boolean, mayRetry: Boolean): Boolean = lock.withLock {
        if (!enabled) return@withLock false
        restore()
        if (mode.check && (mode.userAsked || retrying || startupDue(System.currentTimeMillis()))) {
            if (view.installing) return@withLock false
            val now = System.currentTimeMillis()
            prefs.edit().putLong("attempt", now).apply()
            view = view.copy(checking = true, problem = null)
            try {
                val raw = fetchIndex(feed)
                val manifest = raw?.let { parse(it) ?: throw FeedProblem("The update information isn't valid.") }
                val offer = manifest?.takeIf { newer(it.versionCode, installed) && it.minSdk <= Build.VERSION.SDK_INT }
                if (offer != view.offer) {
                    apk.delete()
                    UpdateAlerts.clear(app)
                }
                val keep = offer == view.offer && view.ready
                val editor = prefs.edit().putString("manifest", raw.takeIf { offer != null }).putLong("checked", now)
                if (mode.userAsked && failureFor(offer) != null) editor.remove("failed").remove("failedMessage")
                editor.apply()
                val blocked = failureFor(offer)
                view = view.copy(
                    checkedAt = now, offer = offer, ready = keep, downloadBlocked = blocked != null, problem = blocked,
                    dismissed = offer?.versionCode == prefs.getLong("dismissed", 0),
                    permissionNeeded = keep && view.permissionNeeded,
                    status = when {
                        manifest == null -> "No Android release is available yet."
                        newer(manifest.versionCode, installed) && offer == null -> "This update needs a newer Android version."
                        offer == null -> "You're up to date."
                        else -> null
                    },
                )
            } catch (problem: FeedProblem) {
                view = view.copy(problem = problem.messageForOwner)
                return@withLock problem.retry
            } catch (_: IOException) {
                view = view.copy(problem = "Couldn't check for updates. Check your connection.")
                return@withLock true
            } finally {
                view = view.copy(checking = false)
            }
        }
        val offer = view.offer ?: return@withLock false
        if (view.ready || view.installing || view.downloadBlocked) return@withLock false
        if (mode.check) {
            UpdateWork.enqueue(app, UpdateMode.WIFI)
            return@withLock false
        }
        if (mode == UpdateMode.DOWNLOAD_NOW && expectedCode != offer.versionCode) return@withLock false
        val connectivity = app.getSystemService(ConnectivityManager::class.java) ?: return@withLock true
        val network = connectivity.activeNetwork ?: return@withLock true
        if (!connectivity.allowed(network, mode)) return@withLock true
        view = view.copy(progress = 0, problem = null)
        try {
            if (!directory.isDirectory && !directory.mkdirs()) throw IOException()
            // Pin the socket to this network so a default-network handoff can't spend mobile data.
            // Recheck occasionally in case the same network changes its metered capability.
            var recheck = SystemClock.elapsedRealtime() + 1_000L
            fetchApk(feed, offer, part, network, allowed = {
                val elapsed = SystemClock.elapsedRealtime()
                if (elapsed < recheck) true else {
                    recheck = elapsed + 1_000L
                    connectivity.allowed(network, mode)
                }
            }) { if (view.progress != it) view = view.copy(progress = it) }
            currentCoroutineContext().ensureActive()
            if (!verify(part, offer)) return@withLock false
            if (!part.renameTo(apk)) throw IOException()
            view = view.copy(ready = true)
            UpdateAlerts.ready(app, offer)
            false
        } catch (problem: FeedProblem) {
            val retry = problem.retry && mayRetry
            if (!retry && problem.rememberFailure) reject(offer, problem.messageForOwner)
            else view = view.copy(problem = problem.messageForOwner)
            retry
        } catch (_: IOException) {
            val problem = "Couldn't download the update. Check your connection and free space."
            if (mayRetry) view = view.copy(problem = problem) else reject(offer, problem)
            mayRetry
        } finally {
            part.delete()
            view = view.copy(progress = null)
        }
    }

    private fun ConnectivityManager.allowed(network: Network, mode: UpdateMode): Boolean {
        val caps = getNetworkCapabilities(network) ?: return false
        return mayDownload(caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED), mode.userAsked)
    }

    private fun verify(file: File, release: UpdateManifest): Boolean {
        @Suppress("DEPRECATION") // Int flags also work on our API 29 floor.
        val own = app.packageManager.getPackageInfo(app.packageName, PackageManager.GET_SIGNING_CERTIFICATES).identity()
        val failure = verifyApk(file, release, own) {
            @Suppress("DEPRECATION")
            app.packageManager.getPackageArchiveInfo(file.absolutePath, PackageManager.GET_SIGNING_CERTIFICATES)?.identity()
        }
        if (failure == null) return true
        reject(release, failureMessage(failure))
        return false
    }

    private fun failureFor(offer: UpdateManifest?): String? =
        if (offer != null && offer.fingerprint == prefs.getString("failed", null))
            prefs.getString("failedMessage", "This download failed. Check now to try again.")
        else null

    private fun reject(release: UpdateManifest, problem: String) {
        prefs.edit().putString("failed", release.fingerprint).putString("failedMessage", problem).apply()
        apk.delete()
        UpdateAlerts.clear(app)
        view = view.copy(ready = false, downloadBlocked = true, permissionNeeded = false, problem = problem)
    }

    private fun failureMessage(failure: VerificationFailure): String = when (failure) {
        VerificationFailure.SIZE, VerificationFailure.HASH -> "The download didn't match the published update."
        VerificationFailure.PACKAGE, VerificationFailure.VERSION, VerificationFailure.SDK -> "The file isn't a compatible update for this app."
        VerificationFailure.SIGNATURE -> "The update wasn't signed with a trusted key for this app."
    }

    suspend fun dismiss(code: Long) = withContext(Dispatchers.IO) {
        lock.withLock {
            if (view.offer?.versionCode != code) return@withLock
            prefs.edit().putLong("dismissed", code).apply()
            view = view.copy(dismissed = true)
        }
    }

    /** Only called from the app's explicit Install button. */
    suspend fun install(expectedCode: Long) = withContext(Dispatchers.IO) {
        lock.withLock {
            if (!enabled) return@withLock
            restore()
            val release = view.offer?.takeIf { it.versionCode == expectedCode } ?: return@withLock
            if (!view.ready || view.installing) return@withLock
            if (!app.packageManager.canRequestPackageInstalls()) {
                view = view.copy(permissionNeeded = true, problem = null)
                return@withLock
            }
            var id: Int? = null
            try {
                view = view.copy(installing = true, permissionNeeded = false, problem = null)
                if (!verify(apk, release)) {
                    view = view.copy(installing = false)
                    return@withLock
                }
                prefs.getInt("session", -1).takeIf { it != -1 }?.let(::abandon)
                val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
                    setAppPackageName(app.packageName)
                    setSize(release.size)
                    if (Build.VERSION.SDK_INT >= 31) {
                        val alreadyInstaller = app.packageManager.getInstallSourceInfo(app.packageName).installingPackageName == app.packageName
                        setRequireUserAction(if (alreadyInstaller) PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED
                            else PackageInstaller.SessionParams.USER_ACTION_REQUIRED)
                    }
                }
                id = installer.createSession(params)
                installer.openSession(id).use { staged ->
                    val job = currentCoroutineContext()
                    val failure = staged.openWrite("base.apk", 0, release.size).use { output ->
                        val result = apk.inputStream().use { verifyBytes(it, release, output) { job.ensureActive() } }
                        if (result == null) staged.fsync(output)
                        result
                    }
                    if (failure != null) {
                        abandon(id)
                        clearSession()
                        reject(release, failureMessage(failure))
                        view = view.copy(installing = false)
                        return@withLock
                    }
                    job.ensureActive()
                    val result = Intent(app, UpdateInstallReceiver::class.java)
                        .setData(Uri.parse("todo-update://install/$id"))
                    val pending = PendingIntent.getBroadcast(app, id, result, PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
                    if (!prefs.edit().putInt("session", id).putString("sessionFingerprint", release.fingerprint).commit()) throw IOException()
                    installId = id
                    staged.commit(pending.intentSender)
                }
            } catch (error: Exception) {
                id?.let(::abandon)
                clearSession()
                view = view.copy(installing = false, problem = "Android couldn't start the installation. Try again.")
                if (error is CancellationException) throw error
            }
        }
    }

    private fun clearSession() {
        prefs.edit().remove("session").remove("sessionFingerprint").apply()
        installId = null
    }

    suspend fun installationResult(intent: Intent) = withContext(Dispatchers.IO) {
        lock.withLock {
            restore()
            val id = intent.data?.lastPathSegment?.toIntOrNull() ?: return@withLock
            if (id != prefs.getInt("session", -1)) return@withLock
            when (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
                PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                    val confirm = IntentCompat.getParcelableExtra(intent, Intent.EXTRA_INTENT, Intent::class.java)
                    if (confirm != null && id == installId) {
                        view = view.copy(confirmation = confirm)
                        return@withLock
                    }
                    // Can't resume the in-memory confirmation after process death; let Install retry.
                    abandon(id)
                }
                PackageInstaller.STATUS_SUCCESS -> {
                    apk.delete()
                    view = view.copy(ready = false, offer = null, status = "Update installed.")
                    prefs.edit().remove("manifest").apply()
                    UpdateAlerts.clear(app)
                }
                PackageInstaller.STATUS_FAILURE_INCOMPATIBLE,
                PackageInstaller.STATUS_FAILURE_CONFLICT,
                PackageInstaller.STATUS_FAILURE_INVALID -> {
                    abandon(id)
                    val problem = "Android couldn't use this update. Check for a newer release."
                    val offer = view.offer
                    if (offer != null && offer.fingerprint == prefs.getString("sessionFingerprint", null)) reject(offer, problem)
                }
                else -> {
                    abandon(id)
                    view = view.copy(problem = "The update wasn't installed. You can try again.")
                }
            }
            clearSession()
            view = view.copy(installing = false, confirmation = null)
        }
    }

    /** The resumed app presents Android's own confirmation only after the owner taps Continue. */
    fun openConfirmation(start: (Intent) -> Unit) {
        val confirmation = view.confirmation ?: return
        try {
            start(confirmation)
        } catch (_: android.content.ActivityNotFoundException) {
            view = view.copy(problem = "Couldn't open Android's confirmation.")
        }
    }

    private fun parse(raw: String): UpdateManifest? = try {
        UpdateManifest.parse(raw)
    } catch (_: IllegalArgumentException) {
        null
    } catch (_: JSONException) {
        null
    }
}

private fun PackageInfo.identity(): ApkIdentity {
    val signerInfo = signingInfo
    return ApkIdentity(
        packageName, longVersionCode, applicationInfo?.minSdkVersion ?: 0,
        signerInfo?.apkContentsSigners.orEmpty().map { sha256(it.toByteArray()) }.toSet(),
        signerInfo?.signingCertificateHistory.orEmpty().map { sha256(it.toByteArray()) }.toSet(),
    )
}
