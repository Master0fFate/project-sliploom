package org.projectseam.android

import android.content.ActivityNotFoundException
import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.CancellationSignal
import android.os.OperationCanceledException
import android.os.Process
import android.view.View
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.core.content.IntentCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.google.zxing.client.android.Intents
import com.journeyapps.barcodescanner.CaptureActivity
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.Closeable
import java.io.IOException
import java.io.InputStream
import java.security.MessageDigest
import java.util.concurrent.atomic.AtomicLong

class MainActivity : ComponentActivity() {
    private val intake by viewModels<IntakeState> { viewModelFactory { initializer { IntakeState() } } }
    private var pendingIntent: Intent? = null
    private var foreground = false
    private val scanner = registerForActivityResult(ScanContract()) { result ->
        if (intake.finishScan() && pendingIntent == null && !intake.status.working) {
            intake.acceptScan(
                result.contents,
                result.originalIntent?.getBooleanExtra(Intents.Scan.MISSING_CAMERA_PERMISSION, false) == true,
            )
        }
    }
    private val picker = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) {
            intake.editInvitation("")
            if (!intake.cancel()) intake.select(uri, null)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) {
            pendingIntent = intent
        } else {
            intake.restore(savedInstanceState)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            window.decorView.importantForAutofill = View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
        }
        enableEdgeToEdge()
        setContent {
            val status = intake.status
            val cancel: () -> Unit = { intake.cancel() }
            BackHandler(enabled = status.working, onBack = cancel)
            IntakeScreen(
                status = status,
                sentBytes = intake.sentBytes,
                hasSource = intake.hasSource,
                invitation = intake.invitation,
                address = intake.parsedInvite?.address,
                scanMessage = intake.scanMessage,
                onScan = ::scanInvitation,
                onInvitationChanged = { value ->
                    if (foreground && intake.hasSource && !intake.status.working) intake.editInvitation(value)
                },
                onSend = {
                    if (foreground && supportsLanSend) intake.parsedInvite?.let { invite ->
                        intake.editInvitation("")
                        intake.start(applicationContext, invite)
                    }
                },
                onCheck = {
                    if (foreground) {
                        intake.editInvitation("")
                        intake.start(applicationContext, null)
                    }
                },
                onPick = {
                    if (foreground && !intake.status.working) picker.launch(arrayOf("image/*", "application/pdf"))
                },
                onCancel = cancel,
            )
        }
    }

    override fun onStart() {
        super.onStart()
        pendingIntent?.let(::receive)
        pendingIntent = null
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) {
            receive(intent)
        } else {
            pendingIntent = intent
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putBoolean(SHARE_SESSION, pendingIntent?.isShare ?: intake.hasShareSession)
        outState.putString(TRANSFER_SNAPSHOT, intake.transferSnapshot?.name)
    }

    override fun onResume() {
        super.onResume()
        foreground = true
    }

    override fun onPause() {
        foreground = false
        super.onPause()
    }

    override fun onStop() {
        if (!isChangingConfigurations) {
            intake.editInvitation("")
            intake.cancel()
        }
        super.onStop()
    }

    private fun scanInvitation() {
        if (!foreground || !supportsLanSend || !intake.hasSource || intake.status.working) return
        if (!packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY)) {
            intake.scanMessage = R.string.scan_unavailable
            return
        }
        if (!intake.beginScan()) return
        intake.editInvitation("")
        val options = ScanOptions()
            .setCaptureActivity(InvitationCaptureActivity::class.java)
            .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
            .setPrompt(getString(R.string.scan_prompt))
            .setOrientationLocked(false)
            .setBeepEnabled(false)
            .setBarcodeImageEnabled(false)
            .addExtra(Intents.Scan.SHOW_MISSING_CAMERA_PERMISSION_DIALOG, false)
        try {
            scanner.launch(options)
        } catch (_: ActivityNotFoundException) {
            intake.finishScan()
            intake.scanMessage = R.string.scan_unavailable
        } catch (_: SecurityException) {
            intake.finishScan()
            intake.scanMessage = R.string.scan_unavailable
        }
    }

    private fun receive(intent: Intent) {
        intake.editInvitation("")
        if (intake.cancel()) return
        intake.reset(intent.isShare)
        val type = intent.type.orEmpty()
        if (intent.action != Intent.ACTION_SEND || !type.isSupported()) {
            intake.status = if (intent.action == Intent.ACTION_MAIN) Status.Idle else Status.Unsupported
            return
        }

        val uri = try {
            intent.sharedUri()
        } catch (_: RuntimeException) {
            intake.status = Status.Unreadable
            return
        }
        if (uri == null) {
            intake.status = Status.Missing
            return
        }
        intake.select(uri, type)
    }
}

class InvitationCaptureActivity : CaptureActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        super.onCreate(savedInstanceState)
    }
}

private const val SHARE_SESSION = "share_session"
private const val TRANSFER_SNAPSHOT = "transfer_snapshot"
private const val CHECK_MAX_BYTES = 100L * 1024 * 1024
private val supportsLanSend: Boolean get() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q

private enum class TransferSnapshot { Saved, Unconfirmed, InFlight }

private fun String.isSupported() = this == "application/pdf" || startsWith("image/") && length > 6

private class Source(val uri: Uri, val type: String?)

private val Intent.isShare: Boolean
    get() = action == Intent.ACTION_SEND || action == Intent.ACTION_SEND_MULTIPLE

private fun Intent.sharedUri(): Uri? {
    val extraUri = if (hasExtra(Intent.EXTRA_STREAM)) {
        requireNotNull(IntentCompat.getParcelableExtra(this, Intent.EXTRA_STREAM, Uri::class.java))
    } else {
        null
    }
    val clipUri = clipData?.let { clip ->
        require(clip.itemCount == 1)
        val item = clip.getItemAt(0)
        require(item.intent == null)
        requireNotNull(item.uri)
    }
    require(extraUri == null || clipUri == null || extraUri == clipUri)
    return extraUri ?: clipUri
}

private object FileOperations {
    private val scope = CoroutineScope(Dispatchers.Main)
    private var active: Operation? = null
    val busy: Boolean get() = active != null

    fun start(
        context: Context,
        source: Source,
        invite: LanInvite?,
        onProgress: (Long) -> Unit,
        onStopping: (Status) -> Unit,
        onFinished: (Status, Long?) -> Unit,
    ): Operation? {
        if (busy) return null
        val operation = Operation(invite, scope, onStopping)
        active = operation
        scope.launch {
            val progress = if (operation.sending) launch {
                while (true) {
                    delay(125)
                    onProgress(operation.sentBytes.get())
                }
            } else null
            val result = withContext(Dispatchers.IO) { operation.run(context, source) }
            progress?.cancel()
            val cancellation = operation.cancellation
            cancellation?.join()
            active = null
            onFinished(
                if (!operation.sending && cancellation != null) Status.Cancelled else result,
                operation.sentBytes.get().takeIf { operation.sending },
            )
        }
        return operation
    }
}

private class Operation(
    invite: LanInvite?,
    private val scope: CoroutineScope,
    private val onStopping: (Status) -> Unit,
) {
    private val lock = Any()
    private val signal = CancellationSignal()
    private var resource: Closeable? = null
    private var stopReason: LanStop? = null
    private var cleanupJob: Job? = null
    private val transfer = invite?.let { LanTransfer(it, ::requestStop) }
    val sending: Boolean get() = transfer != null
    val sentBytes = AtomicLong()
    val cancellation: Job? get() = synchronized(lock) { cleanupJob }

    fun cancel() {
        if (transfer != null) transfer.cancel() else requestStop(LanStop.Cancelled)
    }

    private fun requestStop(reason: LanStop) {
        synchronized(lock) {
            if (stopReason != null) return
            stopReason = reason
            val toClose = takeResource()
            cleanupJob = scope.launch {
                onStopping(if (reason == LanStop.TimedOut) Status.TimingOut else Status.Cancelling)
                launch(Dispatchers.IO) { cleanup { signal.cancel() } }
                launch(Dispatchers.IO) { cleanup { toClose?.close() } }
            }
        }
    }

    fun run(context: Context, source: Source): Status = try {
        openAndRun(context, source)
    } catch (_: IOException) {
        sourceFailure()
    } catch (_: RuntimeException) {
        sourceFailure()
    }

    private fun sourceFailure(): Status = when {
        synchronized(lock) { stopReason != null } -> stoppedBeforeTransfer()
        sending -> Status.SendFailed
        else -> Status.Unreadable
    }

    private fun stoppedBeforeTransfer() = when (synchronized(lock) { stopReason }) {
        LanStop.TimedOut -> Status.SendTimedOut
        else -> if (sending) Status.SendCancelled else Status.Cancelled
    }

    private fun openAndRun(context: Context, source: Source): Status {
        checkActive()
        val uri = source.uri
        val grant = context.checkUriPermission(uri, Process.myPid(), Process.myUid(), Intent.FLAG_GRANT_READ_URI_PERMISSION)
        checkActive()
        if (grant != PackageManager.PERMISSION_GRANTED) return Status.GrantLost
        val resolver = context.contentResolver
        val type = source.type ?: resolver.getType(uri)
        if (type == null || !type.isSupported()) return Status.Unsupported
        checkActive()
        val descriptor = resolver.openAssetFileDescriptor(uri, "r", signal) ?: return sourceFailure()
        var localResource: Closeable? = descriptor
        try {
            checkActive()
            val length = descriptor.declaredLength
            if (sending && length > LAN_MAX_BYTES) return Status.SendTooLarge
            if (!sending && length > CHECK_MAX_BYTES) return Status.CheckTooLarge
            val input = descriptor.createInputStream()
            localResource = input
            if (!register(input)) return stoppedBeforeTransfer()
            localResource = null
            checkActive()
            val network = transfer ?: return checkFile(input)
            return send(network, input, length, type == "application/pdf")
        } finally {
            val toClose = localResource ?: synchronized(lock) { takeResource() }
            cleanup { toClose?.close() }
        }
    }

    private fun send(network: LanTransfer, input: InputStream, length: Long, isPdf: Boolean): Status =
        when (val result = network.send(input, length, isPdf) { sentBytes.set(it) }) {
            is LanResult.Saved -> {
                sentBytes.set(result.bytes)
                Status.Saved
            }
            LanResult.Unconfirmed -> Status.Unconfirmed
            LanResult.TooLarge -> Status.SendTooLarge
            LanResult.Failed -> Status.SendFailed
            LanResult.TlsUnavailable -> Status.TlsUnavailable
            is LanResult.Stopped -> when (result.reason) {
                LanStop.Cancelled -> if (result.afterEnd) Status.UnconfirmedAfterCancel else Status.SendCancelled
                LanStop.TimedOut -> if (result.afterEnd) Status.UnconfirmedAfterTimeout else Status.SendTimedOut
            }
        }

    private fun checkFile(input: InputStream): Status {
        val digest = MessageDigest.getInstance("SHA-256")
        val buffer = ByteArray(64 * 1024)
        var total = 0L
        while (true) {
            checkActive()
            val size = input.read(buffer, 0, minOf(buffer.size.toLong(), CHECK_MAX_BYTES - total + 1).toInt())
            checkActive()
            if (size == -1) return Status.Read(total, digest.digest().toHexString())
            if (size == 0) return Status.Unreadable
            total += size
            if (total > CHECK_MAX_BYTES) return Status.CheckTooLarge
            digest.update(buffer, 0, size)
        }
    }

    private inline fun cleanup(action: () -> Unit) {
        try {
            action()
        } catch (_: IOException) {
        } catch (_: RuntimeException) {
        }
    }

    private fun checkActive() {
        if (synchronized(lock) { stopReason != null }) throw OperationCanceledException()
    }

    private fun register(opened: Closeable): Boolean = synchronized(lock) {
        if (stopReason != null) return false
        resource = opened
        true
    }

    private fun takeResource(): Closeable? = resource.also { resource = null }
}

private class IntakeState : ViewModel() {
    var status by mutableStateOf<Status>(Status.Idle)
    var hasShareSession = false
    var sentBytes by mutableStateOf<Long?>(null)
        private set
    private var source by mutableStateOf<Source?>(null)
    private var scanSource: Source? = null
    private var operation: Operation? = null
    val hasSource: Boolean get() = source != null
    var invitation by mutableStateOf("")
        private set
    var parsedInvite by mutableStateOf<LanInvite?>(null)
        private set
    var scanMessage by mutableStateOf<Int?>(null)
    val transferSnapshot: TransferSnapshot?
        get() = when {
            operation?.sending == true || status == Status.Interrupted -> TransferSnapshot.InFlight
            status == Status.Saved -> TransferSnapshot.Saved
            status.unconfirmed -> TransferSnapshot.Unconfirmed
            else -> null
        }

    fun restore(state: Bundle) {
        if (hasShareSession) return
        val restored = when (state.getString(TRANSFER_SNAPSHOT)) {
            TransferSnapshot.Saved.name -> Status.Saved
            TransferSnapshot.Unconfirmed.name -> Status.Unconfirmed
            TransferSnapshot.InFlight.name -> Status.Interrupted
            else -> if (state.getBoolean(SHARE_SESSION)) Status.Retry else return
        }
        hasShareSession = true
        status = restored
    }

    fun editInvitation(value: String) {
        scanMessage = null
        invitation = value
        parsedInvite = if (value.isEmpty()) null else LanInvite.parse(value)
    }

    fun acceptScan(contents: String?, missingPermission: Boolean) {
        editInvitation("")
        parsedInvite = contents?.let(LanInvite::parse)
        scanMessage = when {
            parsedInvite != null -> null
            contents != null -> R.string.scan_invalid
            missingPermission -> R.string.scan_unavailable
            else -> R.string.scan_no_result
        }
    }

    fun reset(shareSession: Boolean) {
        hasShareSession = shareSession
        source = null
        sentBytes = null
        status = Status.Idle
    }

    fun select(uri: Uri, type: String?) {
        reset(true)
        when {
            uri.scheme != ContentResolver.SCHEME_CONTENT || uri.authority.isNullOrBlank() -> status = Status.Unsupported
            FileOperations.busy -> status = Status.Busy
            else -> {
                source = Source(uri, type)
                status = Status.Ready
            }
        }
    }

    fun beginScan(): Boolean {
        if (scanSource != null) return false
        scanSource = source ?: return false
        return true
    }

    fun finishScan(): Boolean {
        val scannedFor = scanSource
        scanSource = null
        return scannedFor != null && scannedFor === source
    }

    fun start(context: Context, invite: LanInvite?) {
        val selected = source ?: return
        if (operation != null) return
        val attempt = FileOperations.start(
            context, selected, invite, onProgress = { sentBytes = it }, onStopping = { status = it },
        ) { result, count ->
            operation = null
            if (result == Status.Unsupported || result == Status.GrantLost || result == Status.Saved || result.unconfirmed) {
                source = null
            }
            sentBytes = count
            status = result
        }
        operation = attempt
        if (attempt == null) {
            status = Status.Busy
        } else {
            sentBytes = if (attempt.sending) 0L else null
            status = if (attempt.sending) Status.Sending else Status.Reading
        }
    }

    fun cancel(): Boolean {
        val active = operation ?: return false
        active.cancel()
        return true
    }

    override fun onCleared() {
        cancel()
    }
}

private sealed class Status(val message: Int) {
    val working: Boolean get() = this == Reading || this == Sending || this == Cancelling || this == TimingOut
    val unconfirmed: Boolean get() = this == Unconfirmed || this == UnconfirmedAfterCancel || this == UnconfirmedAfterTimeout

    data object Idle : Status(R.string.share_hint)
    data object Ready : Status(R.string.file_ready)
    data object Sending : Status(R.string.sending)
    data object Unconfirmed : Status(R.string.save_unconfirmed)
    data object UnconfirmedAfterCancel : Status(R.string.save_unconfirmed_cancelled)
    data object UnconfirmedAfterTimeout : Status(R.string.save_unconfirmed_timeout)
    data object Interrupted : Status(R.string.send_interrupted)
    data object SendFailed : Status(R.string.send_failed)
    data object SendCancelled : Status(R.string.send_cancelled)
    data object SendTimedOut : Status(R.string.send_timed_out)
    data object TlsUnavailable : Status(R.string.tls_unavailable)
    data object Saved : Status(R.string.saved_on_computer)
    data object Reading : Status(R.string.reading)
    data object Busy : Status(R.string.reader_busy)
    data object Retry : Status(R.string.share_again)
    data object Unsupported : Status(R.string.unsupported)
    data object Missing : Status(R.string.missing_file)
    data object Unreadable : Status(R.string.unreadable)
    data object GrantLost : Status(R.string.grant_lost)
    data object CheckTooLarge : Status(R.string.check_too_large)
    data object SendTooLarge : Status(R.string.send_too_large)
    data object Cancelling : Status(R.string.cancelling)
    data object TimingOut : Status(R.string.timing_out)
    data object Cancelled : Status(R.string.cancelled)
    data class Read(val count: Long, val digest: String) : Status(R.string.file_read)
}

@Composable
private fun IntakeScreen(
    status: Status,
    sentBytes: Long?,
    hasSource: Boolean,
    invitation: String,
    address: String?,
    scanMessage: Int?,
    onScan: () -> Unit,
    onInvitationChanged: (String) -> Unit,
    onSend: () -> Unit,
    onCheck: () -> Unit,
    onPick: () -> Unit,
    onCancel: () -> Unit,
) {
    val focus = LocalFocusManager.current
    MaterialTheme(colorScheme = if (isSystemInDarkTheme()) darkColorScheme() else lightColorScheme()) {
        Surface(modifier = Modifier.fillMaxSize()) {
            Column(
                modifier = Modifier
                    .safeDrawingPadding()
                    .imePadding()
                    .verticalScroll(rememberScrollState())
                    .padding(24.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(
                    text = stringResource(R.string.app_name),
                    modifier = Modifier.semantics { heading() },
                    style = MaterialTheme.typography.headlineMedium,
                )
                Text(
                    text = stringResource(if (status == Status.Ready && !supportsLanSend) R.string.file_selected else status.message),
                    modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                )
                if (status is Status.Read) {
                    Text(stringResource(R.string.byte_count, status.count))
                    Text(stringResource(R.string.sha256, status.digest))
                }
                if (sentBytes != null) {
                    Text(stringResource(R.string.bytes_sent, sentBytes))
                }
                if (status.unconfirmed || status == Status.Interrupted) {
                    Text(stringResource(R.string.check_computer))
                }
                if (status == Status.Reading || status == Status.Sending) {
                    Button(onClick = onCancel) { Text(stringResource(R.string.cancel)) }
                }
                if (!status.working) {
                    if (hasSource) {
                        if (supportsLanSend && status != Status.TlsUnavailable) {
                            TextButton(onClick = { focus.clearFocus(); onScan() }) {
                                Text(stringResource(R.string.scan_invitation))
                            }
                            scanMessage?.let {
                                Text(stringResource(it), modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                            }
                            OutlinedTextField(
                                modifier = Modifier.fillMaxWidth(),
                                value = invitation,
                                onValueChange = onInvitationChanged,
                                label = { Text(stringResource(R.string.invitation)) },
                                singleLine = true,
                                visualTransformation = PasswordVisualTransformation(),
                                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
                            )
                            if (address != null) {
                                Text(stringResource(R.string.destination, address))
                                Button(onClick = { focus.clearFocus(); onSend() }) {
                                    Text(stringResource(R.string.send))
                                }
                            } else if (invitation.isNotEmpty()) {
                                Text(stringResource(R.string.invalid_invitation))
                            }
                        } else if (!supportsLanSend) {
                            Text(stringResource(R.string.tls_unavailable))
                        }
                        TextButton(onClick = onCheck) { Text(stringResource(R.string.check_file)) }
                    }
                    TextButton(onClick = onPick) { Text(stringResource(R.string.pick_file)) }
                }
            }
        }
    }
}
