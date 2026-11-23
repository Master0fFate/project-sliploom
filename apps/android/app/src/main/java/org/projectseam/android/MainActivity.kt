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
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
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
import java.util.concurrent.atomic.AtomicBoolean
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
                cableMode = intake.cableMode,
                invitation = intake.invitation,
                address = intake.parsedInvite?.address,
                scanMessage = intake.scanMessage,
                onScan = ::scanInvitation,
                onInvitationChanged = { value ->
                    if (foreground && !intake.cableMode && intake.hasSource && !intake.status.working) {
                        intake.editInvitation(value)
                    }
                },
                onSend = {
                    if (foreground && supportsLanSend && !intake.cableMode) intake.parsedInvite?.let { invite ->
                        intake.editInvitation("")
                        intake.start(applicationContext, invite)
                    }
                },
                onCheck = {
                    if (foreground && !intake.cableMode) {
                        intake.editInvitation("")
                        intake.start(applicationContext, null)
                    }
                },
                onPick = {
                    if (foreground && !intake.status.working) picker.launch(arrayOf("image/*", "application/pdf"))
                },
                onRetryCable = {
                    if (foreground && intake.canRetryCable) intake.start(applicationContext, null)
                },
                onUseWifi = {
                    if (foreground && supportsLanSend) intake.useWifi()
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
        if (!foreground || !supportsLanSend || intake.cableMode || !intake.hasSource || intake.status.working) return
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
        intake.select(uri, type, viaCable = true)
        if (intake.hasSource) intake.start(applicationContext, null)
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
        viaCable: Boolean,
        onProgress: (Long, Boolean) -> Unit,
        onStopping: (Status) -> Unit,
        onFinished: (Status, Long?) -> Unit,
    ): Operation? {
        if (busy) return null
        val operation = Operation(invite, viaCable, scope, onStopping)
        active = operation
        scope.launch {
            val progress = if (operation.sending) launch {
                while (true) {
                    delay(125)
                    onProgress(operation.sentBytes.get(), operation.cableApproved.get())
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
    viaCable: Boolean,
    private val scope: CoroutineScope,
    private val onStopping: (Status) -> Unit,
) {
    private val lock = Any()
    private val signal = CancellationSignal()
    private var resource: Closeable? = null
    private var stopReason: TransferStop? = null
    private var cleanupJob: Job? = null
    private val lan = invite?.let { LanTransfer(it, ::requestStop) }
    private val cable = if (viaCable) CableTransfer(::requestStop) else null
    val sending: Boolean get() = lan != null || cable != null
    val sentBytes = AtomicLong()
    val cableApproved = AtomicBoolean()
    val cancellation: Job? get() = synchronized(lock) { cleanupJob }

    fun cancel() {
        when {
            cable != null -> cable.cancel()
            lan != null -> lan.cancel()
            else -> requestStop(TransferStop.Cancelled)
        }
    }

    private fun requestStop(reason: TransferStop) {
        synchronized(lock) {
            if (stopReason != null) return
            stopReason = reason
            val toClose = takeResource()
            cleanupJob = scope.launch {
                onStopping(if (reason == TransferStop.TimedOut) Status.TimingOut else Status.Cancelling)
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
        TransferStop.TimedOut -> Status.SendTimedOut
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
        val isPdf = type == "application/pdf"
        if (cable != null) {
            try {
                return sendStatus(cable.send(
                    isPdf,
                    prepare = {
                        checkActive()
                        val descriptor = resolver.openAssetFileDescriptor(uri, "r", signal) ?: throw IOException()
                        if (!register(descriptor)) {
                            cleanup { descriptor.close() }
                            throw OperationCanceledException()
                        }
                        CableTransfer.Payload(descriptor.declaredLength) {
                            checkActive()
                            val input = descriptor.createInputStream()
                            if (!register(input)) {
                                cleanup { input.close() }
                                throw OperationCanceledException()
                            }
                            input
                        }
                    },
                    onApproved = { cableApproved.set(true) },
                    onProgress = { sentBytes.set(it) },
                ))
            } finally {
                closeSource()
            }
        }
        val descriptor = resolver.openAssetFileDescriptor(uri, "r", signal) ?: return sourceFailure()
        var localResource: Closeable? = descriptor
        try {
            checkActive()
            val length = descriptor.declaredLength
            if (sending && length > TRANSFER_MAX_BYTES) return Status.SendTooLarge
            if (!sending && length > CHECK_MAX_BYTES) return Status.CheckTooLarge
            val input = descriptor.createInputStream()
            localResource = input
            if (!register(input)) return stoppedBeforeTransfer()
            localResource = null
            checkActive()
            val network = lan ?: return checkFile(input)
            return sendStatus(network.send(input, length, isPdf) { sentBytes.set(it) })
        } finally {
            if (localResource != null) cleanup { localResource?.close() } else closeSource()
        }
    }

    private fun closeSource() {
        val toClose = synchronized(lock) { takeResource() }
        cleanup { toClose?.close() }
    }

    private fun sendStatus(result: TransferResult): Status = when (result) {
        is TransferResult.Saved -> {
            sentBytes.set(result.bytes)
            Status.Saved
        }
        TransferResult.Unconfirmed -> Status.Unconfirmed
        TransferResult.TooLarge -> Status.SendTooLarge
        TransferResult.Failed -> Status.SendFailed
        TransferResult.TlsUnavailable -> Status.TlsUnavailable
        TransferResult.CableUnavailable -> Status.CableUnavailable
        TransferResult.Declined -> Status.Declined
        is TransferResult.Stopped -> when (result.reason) {
            TransferStop.Cancelled -> if (result.afterEnd) Status.UnconfirmedAfterCancel else Status.SendCancelled
            TransferStop.TimedOut -> if (result.afterEnd) Status.UnconfirmedAfterTimeout else Status.SendTimedOut
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
    var cableMode by mutableStateOf(false)
        private set
    val canRetryCable: Boolean
        get() = cableMode && hasSource && status.canRetryCable
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
        cableMode = false
        source = null
        sentBytes = null
        status = Status.Idle
    }

    fun select(uri: Uri, type: String?, viaCable: Boolean = false) {
        reset(true)
        when {
            uri.scheme != ContentResolver.SCHEME_CONTENT || uri.authority.isNullOrBlank() -> status = Status.Unsupported
            FileOperations.busy -> status = Status.Busy
            else -> {
                source = Source(uri, type)
                cableMode = viaCable
                status = Status.Ready
            }
        }
    }

    fun useWifi() {
        if (!cableMode || !hasSource || status != Status.CableUnavailable) return
        cableMode = false
        sentBytes = null
        status = Status.Ready
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
            context, selected, invite, cableMode,
            onProgress = { count, approved ->
                if (status == Status.WaitingForComputer && approved) status = Status.Sending
                if (status != Status.WaitingForComputer) sentBytes = count
            },
            onStopping = { status = it },
        ) { result, count ->
            operation = null
            if (result == Status.Unsupported || result == Status.GrantLost || result == Status.Saved || result.unconfirmed) {
                source = null
            }
            sentBytes = count?.takeUnless { cableMode && it == 0L && result != Status.Saved }
            status = result
        }
        operation = attempt
        if (attempt == null) {
            status = Status.Busy
        } else {
            sentBytes = if (attempt.sending && !cableMode) 0L else null
            status = when {
                cableMode -> Status.WaitingForComputer
                attempt.sending -> Status.Sending
                else -> Status.Reading
            }
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
    val canCancel: Boolean get() = this == WaitingForComputer || this == Reading || this == Sending
    val working: Boolean get() = canCancel || this == Cancelling || this == TimingOut
    val canRetryCable: Boolean
        get() = this == CableUnavailable || this == SendTimedOut || this == SendCancelled ||
            this == SendFailed || this == Declined
    val unconfirmed: Boolean get() = this == Unconfirmed || this == UnconfirmedAfterCancel || this == UnconfirmedAfterTimeout

    data object Idle : Status(R.string.share_hint)
    data object Ready : Status(R.string.file_ready)
    data object WaitingForComputer : Status(R.string.waiting_for_computer)
    data object CableUnavailable : Status(R.string.open_seam_on_computer)
    data object Declined : Status(R.string.send_declined)
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
    cableMode: Boolean,
    invitation: String,
    address: String?,
    scanMessage: Int?,
    onScan: () -> Unit,
    onInvitationChanged: (String) -> Unit,
    onSend: () -> Unit,
    onCheck: () -> Unit,
    onPick: () -> Unit,
    onRetryCable: () -> Unit,
    onUseWifi: () -> Unit,
    onCancel: () -> Unit,
) {
    val focus = LocalFocusManager.current
    val scan = { focus.clearFocus(); onScan() }
    val headline = when (status) {
        Status.Idle -> R.string.send_a_file
        Status.Ready -> R.string.file_selected
        Status.TlsUnavailable -> R.string.sending_unavailable
        Status.Interrupted -> R.string.save_status_unknown
        Status.UnconfirmedAfterCancel, Status.UnconfirmedAfterTimeout -> R.string.save_unconfirmed
        Status.Cancelling -> R.string.cancelling_headline
        Status.TimingOut -> R.string.timing_out_headline
        Status.GrantLost -> R.string.file_access_lost
        is Status.Read -> R.string.file_checked
        else -> status.message
    }
    val detail = when (status) {
        Status.Idle, Status.TlsUnavailable, Status.Interrupted, Status.GrantLost, is Status.Read -> status.message
        Status.UnconfirmedAfterCancel -> R.string.send_cancelled_short
        Status.UnconfirmedAfterTimeout -> R.string.timing_out_headline
        Status.Cancelling, Status.TimingOut -> R.string.waiting_for_stop
        Status.Ready -> if (supportsLanSend) R.string.file_ready else R.string.tls_unavailable
        Status.WaitingForComputer -> R.string.accept_on_computer
        Status.CableUnavailable -> R.string.connect_usb
        else -> null
    }
    SeamMaterial(bottomBiased = cableMode && status.working) {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                text = stringResource(headline),
                modifier = Modifier.semantics {
                    heading()
                    liveRegion = LiveRegionMode.Polite
                },
                color = MaterialTheme.colorScheme.onSurface,
                style = MaterialTheme.typography.headlineMedium,
            )
            detail?.let { SeamDetail(stringResource(it)) }
            if (status is Status.Read) {
                SeamDetail(stringResource(R.string.byte_count, status.count))
                SeamDetail(stringResource(R.string.sha256, status.digest))
            }
            if (sentBytes != null) {
                SeamDetail(stringResource(R.string.bytes_sent, sentBytes))
            }
            if (status.unconfirmed || status == Status.Interrupted) {
                SeamDetail(stringResource(R.string.check_computer))
            }
        }
        when {
            status.canCancel -> SeamAction(stringResource(R.string.cancel), onCancel)
            status.working -> Unit
            cableMode && hasSource -> {
                if (status.canRetryCable) {
                    SeamAction(stringResource(R.string.try_cable_again), onRetryCable)
                }
                if (status == Status.CableUnavailable && supportsLanSend) {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                        SeamAction(stringResource(R.string.use_wifi), onUseWifi, primary = false)
                    }
                }
            }
            !hasSource -> SeamAction(stringResource(R.string.pick_file), onPick)
            supportsLanSend && status != Status.TlsUnavailable -> {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(
                        modifier = Modifier.fillMaxWidth(),
                        value = invitation,
                        onValueChange = onInvitationChanged,
                        label = { Text(stringResource(R.string.invitation)) },
                        singleLine = true,
                        shape = SeamControlShape,
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
                    )
                    if (address != null) {
                        SeamDetail(stringResource(R.string.destination, address))
                        SeamAction(stringResource(R.string.send), { focus.clearFocus(); onSend() })
                    } else {
                        val feedback = scanMessage ?: R.string.invalid_invitation.takeIf { invitation.isNotEmpty() }
                        feedback?.let {
                            SeamDetail(stringResource(it), Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                        }
                        SeamAction(stringResource(R.string.scan_invitation), scan)
                    }
                }
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (address != null) {
                        SeamAction(stringResource(R.string.scan_invitation), scan, primary = false)
                    }
                    SeamAction(stringResource(R.string.check_file), onCheck, primary = false)
                    SeamAction(stringResource(R.string.pick_file), onPick, primary = false)
                }
            }
            else -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (!supportsLanSend && status != Status.Ready && status != Status.TlsUnavailable) {
                    SeamDetail(stringResource(R.string.tls_unavailable))
                }
                SeamAction(stringResource(R.string.check_file), onCheck)
                SeamAction(stringResource(R.string.pick_file), onPick, primary = false)
            }
        }
    }
}
