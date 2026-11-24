package org.projectseam.android

import android.content.ActivityNotFoundException
import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.AssetFileDescriptor
import android.database.Cursor
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.CancellationSignal
import android.os.OperationCanceledException
import android.os.ParcelFileDescriptor
import android.os.Process
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.view.View
import android.view.WindowManager
import android.webkit.MimeTypeMap
import android.widget.Toast
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
import androidx.compose.foundation.pager.PagerState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import androidx.core.os.BundleCompat
import androidx.core.content.IntentCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import androidx.lifecycle.viewModelScope
import com.google.zxing.client.android.Intents
import com.journeyapps.barcodescanner.CaptureActivity
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import kotlinx.coroutines.CancellationException
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
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

class MainActivity : ComponentActivity() {
    private val intake by viewModels<IntakeState> { viewModelFactory { initializer { IntakeState() } } }
    private var pendingIntent: Intent?
        get() = intake.pendingIntent
        set(value) { intake.pendingIntent = value }
    private var intentDrain: Job? = null
    private var foreground by mutableStateOf(false)
    private var mode by mutableIntStateOf(SEND_PAGE)
    private val showsReceive: Boolean get() = mode == RECEIVE_PAGE
    private var showsHistory by mutableStateOf(false)
    private var historyRevision by mutableIntStateOf(0)
    private var scanningPair by mutableStateOf(false)
    private var savedNotice: Job? = null
    private var savedToast: Toast? = null
    private val trusted by lazy {
        TrustedState(
            applicationContext,
            lifecycleScope,
            ::canPair,
            { canUseSend && intake.hasSource && !intake.cableMode && !intake.status.working },
            ::trustedSaved,
        )
    }
    private var openingReceived: Job? = null
    private var pagerRevision by mutableIntStateOf(0)
    private val canSwitchMode: Boolean
        get() = foreground && pendingIntent == null && !FileOperations.busy &&
            !ReceiveOperations.busy && intake.receiver.phase == null && !showsHistory && !trusted.busy && !scanningPair
    private val canUseSend: Boolean get() = mode == SEND_PAGE && pendingIntent == null && !ReceiveOperations.busy
    private fun canPair(): Boolean = foreground && mode == TRUSTED_PAGE && pendingIntent == null &&
        !FileOperations.busy && !ReceiveOperations.busy && intake.receiver.phase == null && !showsHistory && !scanningPair
    private val scanner = registerForActivityResult(ScanContract()) { result ->
        if (intake.finishScan() && canUseSend && !intake.status.working) {
            intake.acceptScan(
                result.contents,
                result.originalIntent?.getBooleanExtra(Intents.Scan.MISSING_CAMERA_PERMISSION, false) == true,
            )
        }
    }
    private val pairScanner = registerForActivityResult(ScanContract()) { result ->
        val expected = scanningPair
        scanningPair = false
        if (expected && mode == TRUSTED_PAGE && pendingIntent == null) {
            val code = result.contents
            if (code != null) {
                trusted.queuePair(code)
            } else {
                val denied = result.originalIntent?.getBooleanExtra(Intents.Scan.MISSING_CAMERA_PERMISSION, false) == true
                trusted.scanFailed(if (denied) R.string.scan_unavailable else R.string.scan_no_result)
            }
        }
    }
    private val picker = registerForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty() && canUseSend && !intake.status.working) {
            intake.selectPicked(applicationContext, uris)
        }
    }

    private val saveDocument = registerForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
        val name = intake.pendingCopyName
        intake.pendingCopyName = null
        if (uri != null && name != null) intake.saveCopy(applicationContext, name, uri)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) {
            pendingIntent = intent.takeIf { it.isShare }
        } else {
            if (pendingIntent == null) {
                pendingIntent = BundleCompat.getParcelable(savedInstanceState, PENDING_SHARE, Intent::class.java)
            }
            intake.restore(savedInstanceState)
            intake.pendingCopyName = savedInstanceState.getString(PENDING_COPY)
            if (pendingIntent == null) {
                val previousPage = if (savedInstanceState.getBoolean(RECEIVE_MODE)) RECEIVE_PAGE else SEND_PAGE
                mode = savedInstanceState.getInt(TRANSFER_PAGE, previousPage).coerceIn(SEND_PAGE, TRUSTED_PAGE)
            }
            showsHistory = showsReceive && savedInstanceState.getBoolean(RECEIVE_HISTORY)
            scanningPair = mode == TRUSTED_PAGE && savedInstanceState.getBoolean(PAIR_SCAN)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            window.decorView.importantForAutofill = View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
        }
        enableEdgeToEdge()
        setContent {
            val status = intake.status
            val receiver = intake.receiver
            val cancel: () -> Unit = { intake.cancel() }
            val focus = LocalFocusManager.current
            if (mode == SEND_PAGE && intake.enteringCode) {
                DisposableEffect(window) {
                    window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
                    onDispose { window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE) }
                }
            }
            BackHandler(enabled = status.working, onBack = cancel)
            BackHandler(enabled = mode != SEND_PAGE && receiver.phase == null && !showsHistory && trusted.sheet == null) {
                if (canSwitchMode) {
                    focus.clearFocus()
                    chooseMode(SEND_PAGE)
                    pagerRevision++
                }
            }
            SeamMaterial(overlay = {
                if (showsReceive) {
                    val phase = receiver.phase
                    if (phase != null) {
                        ReceiveSheet(phase, receiver::close, ::openReceived, ::saveReceived, receiver.openFailed)
                    } else if (showsHistory) {
                        key(historyRevision) {
                            ReceivedFilesSheet({
                                openingReceived?.cancel()
                                showsHistory = false
                            }, ::openReceived, ::saveReceived, receiver.openFailed)
                        }
                    }
                } else if (mode == TRUSTED_PAGE) {
                    TrustedSheet(trusted)
                }
            }) { pane ->
                val revision = pagerRevision
                key(revision) {
                    val pager = remember { PagerState(currentPage = mode) { TRUSTED_PAGE + 1 } }
                    LaunchedEffect(pager, focus) {
                        snapshotFlow { pager.settledPage }.collect { page ->
                            if (revision == pagerRevision && page != mode) {
                                if (canSwitchMode) {
                                    focus.clearFocus()
                                    chooseMode(page)
                                } else {
                                    pager.requestScrollToPage(mode)
                                }
                            }
                        }
                    }
                    val isPageActive: (Int) -> Boolean = { page ->
                        revision == pagerRevision && foreground && pendingIntent == null &&
                            receiver.phase == null && !showsHistory && trusted.sheet == null && page == mode &&
                            !pager.isScrollInProgress && pager.settledPage == page
                    }
                    TransferModePager(pager, pane, canSwitchMode, isPageActive) { page ->
                        if (page == RECEIVE_PAGE) {
                            ReceiveScreen(
                                state = receiver,
                                canStart = !FileOperations.busy && !ReceiveOperations.busy && receiver.phase == null,
                                onStart = ::startReceive,
                                onHistory = {
                                    if (foreground && showsReceive && pendingIntent == null && !ReceiveOperations.busy && receiver.phase == null) {
                                        receiver.openFailed = false
                                        showsHistory = true
                                    }
                                },
                            )
                        } else if (page == TRUSTED_PAGE) {
                            TrustedScreen(trusted, ::scanPairing)
                        } else {
                            IntakeScreen(
                                status = status,
                                sentBytes = intake.sentBytes,
                                selectionCount = intake.selectionCount,
                                committed = intake.committed,
                                batchSummary = intake.batchSummary,
                                cableMode = intake.cableMode,
                                cableEligible = intake.cableEligible,
                                enteringCode = intake.enteringCode,
                                invitation = intake.invitation,
                                invite = intake.parsedInvite,
                                scanMessage = intake.scanMessage,
                                trustedPeers = trusted.reachable,
                                sendingTo = intake.sendingTo,
                                onSendToTrusted = { peer ->
                                    if (isPageActive(SEND_PAGE) && canUseSend && supportsLanSend &&
                                        intake.hasSource && !intake.cableMode && !intake.status.working && !FileOperations.busy
                                    ) {
                                        intake.clearInvitation()
                                        val open = trusted.inviteOpener(peer)
                                        if (open == null) intake.status = Status.TrustedUnavailable
                                        else intake.start(applicationContext, null, open, peer.label)
                                    }
                                },
                                onScan = ::scanInvitation,
                                onEnterCode = {
                                    if (foreground && canUseSend) intake.openCodeEntry()
                                },
                                onInvitationChanged = { value ->
                                    if (isPageActive(SEND_PAGE) && canUseSend && !intake.cableMode &&
                                        intake.hasSource && !intake.status.working
                                    ) {
                                        intake.editInvitation(value)
                                    }
                                },
                                onSend = {
                                    if (foreground && canUseSend && supportsLanSend && !intake.cableMode) {
                                        intake.parsedInvite?.let { invite ->
                                            intake.clearInvitation()
                                            intake.start(applicationContext, invite)
                                        }
                                    }
                                },
                                onCheck = {
                                    if (foreground && canUseSend && !intake.cableMode) {
                                        intake.clearInvitation()
                                        intake.start(applicationContext, null)
                                    }
                                },
                                onPick = {
                                    if (foreground && canUseSend && !intake.status.working) {
                                        picker.launch(arrayOf("*/*"))
                                    }
                                },
                                onUseCable = {
                                    if (foreground && canUseSend) intake.useCable(applicationContext)
                                },
                                onRetryCable = {
                                    if (foreground && canUseSend && intake.canRetryCable) intake.start(applicationContext, null)
                                },
                                onUseWifi = {
                                    if (foreground && canUseSend && supportsLanSend) intake.useWifi()
                                },
                                onCancel = cancel,
                            )
                        }
                    }
                }
            }
        }
    }

    override fun onStart() {
        super.onStart()
        routePendingIntent()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (!intent.isShare) return
        setIntent(intent)
        intake.cancelPickedType()
        pendingIntent = intent
        openingReceived?.cancel()
        showsHistory = false
        mode = SEND_PAGE
        scanningPair = false
        trusted.stop()
        if (foreground) trusted.start()
        pagerRevision++
        intake.receiver.close()
        if (lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) routePendingIntent()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putBoolean(SHARE_SESSION, pendingIntent?.isShare ?: intake.hasShareSession)
        pendingIntent?.let { pending ->
            try {
                val uris = pending.sharedUris()
                val saved = Intent(pending.action).setType(pending.type)
                if (pending.action == Intent.ACTION_SEND_MULTIPLE && uris.isNotEmpty()) {
                    saved.putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(uris))
                } else {
                    uris.singleOrNull()?.let { saved.putExtra(Intent.EXTRA_STREAM, it) }
                }
                outState.putParcelable(PENDING_SHARE, saved)
            } catch (_: RuntimeException) {
            }
        }
        outState.putString(TRANSFER_SNAPSHOT, intake.transferSnapshot?.name)
        intake.batchSummary?.let {
            outState.putInt(BATCH_COUNT, it.count)
            outState.putInt(BATCH_COMMITTED, it.committed)
            outState.putBoolean(BATCH_UNCONFIRMED, it.unconfirmed)
            outState.putBoolean(PREVIOUS_BATCH, it.previous)
        }
        outState.putInt(TRANSFER_PAGE, if (pendingIntent == null) mode else SEND_PAGE)
        outState.putBoolean(RECEIVE_HISTORY, showsHistory)
        outState.putBoolean(PAIR_SCAN, scanningPair)
        outState.putString(PENDING_COPY, intake.pendingCopyName)
    }

    override fun onResume() {
        super.onResume()
        foreground = true
        historyRevision++
        trusted.start()
    }

    override fun onPause() {
        foreground = false
        trusted.stop()
        intake.cancelTrustedSend()
        savedNotice?.cancel()
        savedNotice = null
        savedToast?.cancel()
        savedToast = null
        openingReceived?.cancel()
        intake.receiver.close()
        super.onPause()
    }

    override fun onStop() {
        if (!isChangingConfigurations) {
            intake.clearInvitation()
            intake.cancel()
        }
        super.onStop()
    }

    private fun chooseMode(page: Int) {
        if (!canSwitchMode) return
        if (page != SEND_PAGE) intake.clearInvitation()
        if (page != RECEIVE_PAGE) intake.receiver.close()
        if (page != TRUSTED_PAGE) trusted.discardQueuedPair()
        mode = page
    }

    private fun trustedSaved() {
        if (!foreground || savedNotice != null) return
        savedNotice = lifecycleScope.launch {
            delay(300)
            savedNotice = null
            if (foreground) {
                historyRevision++
                savedToast?.cancel()
                savedToast = Toast.makeText(this@MainActivity, R.string.trusted_received, Toast.LENGTH_LONG).also { it.show() }
            }
        }
    }

    private fun startReceive() {
        if (foreground && showsReceive && !showsHistory && pendingIntent == null && !FileOperations.busy) {
            intake.receiver.start(applicationContext)
        }
    }

    private fun openReceived(result: ReceivedFile) {
        if (!foreground || !showsReceive || pendingIntent != null) return
        val phase = intake.receiver.phase
        if (!showsHistory && phase !is ReceivePhase.Result) return
        openingReceived?.cancel()
        intake.receiver.openFailed = false
        openingReceived = lifecycleScope.launch {
            fun canOpen() = foreground && showsReceive && pendingIntent == null &&
                (if (phase == null) showsHistory else intake.receiver.phase === phase)
            try {
                val (uri, type) = withContext(Dispatchers.IO) {
                    val mime = result.mediaType ?: inspectReceivedMime(filesDir, result.file.name)
                    FileProvider.getUriForFile(this@MainActivity, "$packageName.received", result.file) to mime
                }
                if (!canOpen()) return@launch
                startActivity(Intent(Intent.ACTION_VIEW).apply {
                    setDataAndType(uri, type)
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                })
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: IOException) {
                if (canOpen()) intake.receiver.openFailed = true
            } catch (_: RuntimeException) {
                if (canOpen()) intake.receiver.openFailed = true
            }
        }
    }

    private fun saveReceived(result: ReceivedFile) {
        if (!foreground || !showsReceive || pendingIntent != null || intake.pendingCopyName != null) return
        if (!showsHistory && intake.receiver.phase !is ReceivePhase.Result) return
        openingReceived?.cancel()
        intake.pendingCopyName = result.file.name
        try {
            saveDocument.launch(result.file.name)
        } catch (_: RuntimeException) {
            intake.pendingCopyName = null
            Toast.makeText(this, R.string.receive_save_failed, Toast.LENGTH_LONG).show()
        }
    }

    private fun routePendingIntent() {
        val next = pendingIntent ?: return
        mode = SEND_PAGE
        intake.receiver.close()
        val receiving = ReceiveOperations.stop()
        val sending = FileOperations.stop()
        if (receiving != null || sending != null) {
            if (intentDrain == null) {
                intentDrain = lifecycleScope.launch {
                    receiving?.join()
                    sending?.join()
                    intentDrain = null
                    if (lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) routePendingIntent()
                }
            }
            return
        }
        pendingIntent = null
        receive(next)
    }

    private fun scanInvitation() {
        if (!foreground || !canUseSend || !supportsLanSend || intake.cableMode || !intake.hasSource || intake.status.working) return
        if (!packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY)) {
            intake.clearInvitation()
            intake.scanMessage = R.string.scan_unavailable
            return
        }
        if (!intake.beginScan()) return
        intake.clearInvitation()
        try {
            scanner.launch(scanOptions(R.string.scan_prompt))
        } catch (_: ActivityNotFoundException) {
            intake.finishScan()
            intake.scanMessage = R.string.scan_unavailable
        } catch (_: SecurityException) {
            intake.finishScan()
            intake.scanMessage = R.string.scan_unavailable
        }
    }

    private fun scanPairing() {
        if (!canPair() || !trusted.ready || trusted.busy) return
        if (!packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY)) {
            trusted.scanFailed(R.string.scan_unavailable)
            return
        }
        scanningPair = true
        try {
            pairScanner.launch(scanOptions(R.string.trusted_scan_prompt))
        } catch (_: ActivityNotFoundException) {
            scanningPair = false
            trusted.scanFailed(R.string.scan_unavailable)
        } catch (_: SecurityException) {
            scanningPair = false
            trusted.scanFailed(R.string.scan_unavailable)
        }
    }

    private fun scanOptions(prompt: Int) = ScanOptions()
        .setCaptureActivity(InvitationCaptureActivity::class.java)
        .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
        .setPrompt(getString(prompt))
        .setOrientationLocked(false)
        .setBeepEnabled(false)
        .setBarcodeImageEnabled(false)
        .addExtra(Intents.Scan.SHOW_MISSING_CAMERA_PERMISSION_DIALOG, false)

    private fun receive(intent: Intent) {
        val uris = try {
            intent.sharedUris()
        } catch (_: TooManySourcesException) {
            intake.reset(keepReceipt = true)
            intake.status = Status.TooManyFiles
            return
        } catch (_: RuntimeException) {
            intake.reset(keepReceipt = true)
            intake.status = Status.Unreadable
            return
        }
        intake.select(this, uris, intent.type, keepReceipt = true)
    }
}

class InvitationCaptureActivity : CaptureActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        super.onCreate(savedInstanceState)
    }
}

class ReceivedFileProvider : FileProvider()

private const val SHARE_SESSION = "share_session"
private const val PENDING_SHARE = "pending_share"
private const val TRANSFER_SNAPSHOT = "transfer_snapshot"
private const val BATCH_COUNT = "batch_count"
private const val BATCH_COMMITTED = "batch_committed"
private const val BATCH_UNCONFIRMED = "batch_unconfirmed"
private const val PREVIOUS_BATCH = "previous_batch"
private const val SEND_PAGE = 0
private const val RECEIVE_PAGE = 1
private const val TRUSTED_PAGE = 2
private const val TRANSFER_PAGE = "transfer_page"
private const val RECEIVE_MODE = "receive_mode"
private const val PAIR_SCAN = "pair_scan"
private const val RECEIVE_HISTORY = "receive_history"
private const val PENDING_COPY = "pending_copy"
private const val CHECK_MAX_BYTES = 100L * 1024 * 1024
internal val supportsLanSend: Boolean get() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q

private enum class TransferSnapshot { Saved, Unconfirmed, InFlight }

private fun String.isLegacyType() = this == "application/pdf" || startsWith("image/") && length > 6

private class Source(val uri: Uri, val type: String?)
private class Selection(val sources: List<Source>) {
    val cableEligible: Boolean get() = sources.singleOrNull()?.type?.isLegacyType() == true
}

private class TooManySourcesException : IllegalArgumentException()

private data class BatchSummary(
    val count: Int,
    val committed: Int,
    val unconfirmed: Boolean,
    val previous: Boolean = false,
) {
    val unsent: Int get() = count - committed - if (unconfirmed) 1 else 0
}

private fun Context.hasReadGrant(uri: Uri) = checkUriPermission(
    uri, Process.myPid(), Process.myUid(), Intent.FLAG_GRANT_READ_URI_PERMISSION,
) == PackageManager.PERMISSION_GRANTED

private val Intent.isShare: Boolean
    get() = action == Intent.ACTION_SEND || action == Intent.ACTION_SEND_MULTIPLE

private fun Intent.sharedUris(): List<Uri> {
    val multiple = action == Intent.ACTION_SEND_MULTIPLE
    val extra = if (!hasExtra(Intent.EXTRA_STREAM)) {
        null
    } else if (multiple) {
        val list: List<*> = requireNotNull(IntentCompat.getParcelableArrayListExtra(this, Intent.EXTRA_STREAM, Uri::class.java))
        if (list.size > SEND_BATCH_MAX_ITEMS) throw TooManySourcesException()
        require(list.isNotEmpty())
        list.map { requireNotNull(it as? Uri) }
    } else {
        listOf(requireNotNull(IntentCompat.getParcelableExtra(this, Intent.EXTRA_STREAM, Uri::class.java)))
    }
    val clip = clipData?.let { data ->
        if (data.itemCount > SEND_BATCH_MAX_ITEMS) throw TooManySourcesException()
        require(data.itemCount > 0 && (multiple || data.itemCount == 1))
        List(data.itemCount) { index ->
            val item = data.getItemAt(index)
            require(item.intent == null)
            requireNotNull(item.uri)
        }
    }
    require(extra == null || clip == null || extra == clip)
    return extra ?: clip.orEmpty()
}

private object FileOperations {
    private val scope = CoroutineScope(Dispatchers.Main)
    private var active by mutableStateOf<Operation?>(null)
    private var running: Job? = null
    val busy: Boolean get() = active != null

    fun stop(): Job? = running.also { if (it != null) active?.cancel() }

    fun start(
        context: Context,
        selection: Selection,
        invite: LanInvite?,
        openPrivateInvite: (() -> LanInvite)?,
        viaCable: Boolean,
        onProgress: (Long, SendPhase, Int) -> Unit,
        onStopping: (Status) -> Unit,
        onFinished: (Status, Long?, BatchSummary?) -> Unit,
    ): Operation? {
        if (busy || ReceiveOperations.busy) return null
        val operation = Operation(invite, openPrivateInvite, viaCable, scope, onStopping)
        active = operation
        running = scope.launch {
            val progress = if (operation.sending) launch {
                while (true) {
                    delay(125)
                    onProgress(operation.sentBytes.get(), operation.phase.get(), operation.committed.get())
                }
            } else null
            val result = withContext(Dispatchers.IO) { operation.run(context, selection) }
            progress?.cancel()
            val cancellation = operation.cancellation
            cancellation?.join()
            active = null
            running = null
            onFinished(
                if (!operation.sending && cancellation != null) Status.Cancelled else result,
                operation.sentBytes.get().takeIf { viaCable || operation.phase.get() == SendPhase.Sending },
                operation.batchSummary,
            )
        }
        return operation
    }
}

private enum class SendPhase { Connecting, Approval, Sending }

private class Operation(
    private val invite: LanInvite?,
    private val openPrivateInvite: (() -> LanInvite)?,
    viaCable: Boolean,
    private val scope: CoroutineScope,
    private val onStopping: (Status) -> Unit,
) {
    private val lock = Any()
    private val signal = CancellationSignal()
    private var resource: Closeable? = null
    private var cursorResource: Cursor? = null
    private var stopReason: TransferStop? = null
    private var cleanupJob: Job? = null
    private var lan = invite?.let { LanTransfer(it, ::requestStop) }
    private val cable = if (viaCable) CableTransfer(::requestStop) else null
    val trusted: Boolean get() = openPrivateInvite != null
    val sending: Boolean get() = trusted || invite != null || cable != null
    val sentBytes = AtomicLong()
    val phase = AtomicReference(SendPhase.Connecting)
    val committed = AtomicInteger()
    val cancellation: Job? get() = synchronized(lock) { cleanupJob }
    var batchSummary: BatchSummary? = null
        private set
    private var sourceError: Status? = null

    fun cancel() {
        if (cable != null) {
            cable.cancel()
        } else {
            val network = synchronized(lock) {
                lan ?: run {
                    requestStop(TransferStop.Cancelled)
                    return
                }
            }
            network.cancel()
        }
    }

    private fun requestStop(reason: TransferStop) {
        synchronized(lock) {
            if (stopReason != null) return
            stopReason = reason
            val toClose = takeResource()
            val cursor = takeCursor()
            cleanupJob = scope.launch {
                onStopping(if (reason == TransferStop.TimedOut) Status.TimingOut else Status.Cancelling)
                launch(Dispatchers.IO) { cleanup { signal.cancel() } }
                launch(Dispatchers.IO) { cleanup { toClose?.close() } }
                launch(Dispatchers.IO) { cleanup { cursor?.close() } }
            }
        }
    }

    fun run(context: Context, selection: Selection): Status = try {
        openAndRun(context, selection)
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (_: IOException) {
        sourceFailure()
    } catch (_: RuntimeException) {
        sourceFailure()
    } finally {
        closeSource()
    }

    private fun sourceFailure(): Status = when (synchronized(lock) { stopReason }) {
        TransferStop.TimedOut -> Status.SendTimedOut
        TransferStop.Cancelled -> if (sending) Status.SendCancelled else Status.Cancelled
        null -> sourceError ?: if (sending) Status.SendFailed else Status.Unreadable
    }

    private fun openAndRun(context: Context, selection: Selection): Status {
        val sources = selection.sources
        val destination = openPrivateInvite?.let { open ->
            checkActive()
            sourceError = Status.TrustedUnavailable
            open().also { granted ->
                if (!granted.isBatch) throw IOException()
                synchronized(lock) {
                    checkActive()
                    lan = LanTransfer(granted, ::requestStop)
                }
                sourceError = null
            }
        } ?: invite
        val network = synchronized(lock) { lan }
        if (network != null && destination?.isBatch == true) {
            val batch = network.sendBatch(
                sources.size,
                queryLength = { index ->
                    val source = sources[index]
                    checkFileType(context, source)
                    queryColumn(context, source.uri, OpenableColumns.SIZE) { cursor, column ->
                        if (cursor.getType(column) == Cursor.FIELD_TYPE_INTEGER) cursor.getLong(column).takeIf { it >= 0 }
                        else null
                    } ?: -1L
                },
                displayName = { queryName(context, sources[it].uri) },
                openItem = { openDescriptor(context, sources[it].uri).let { openStream(it::createInputStream) } },
                closeItem = ::closeSource,
                onOffered = { if (!trusted) phase.set(SendPhase.Approval) },
                onApproved = { phase.set(SendPhase.Sending) },
                onProgress = { sentBytes.set(it) },
                onCommitted = { committed.set(it) },
            )
            val status = sendStatus(batch.result)
            batchSummary = BatchSummary(sources.size, batch.committed, status.unconfirmed)
            return when (status) {
                Status.Saved -> Status.BatchSaved
                Status.Declined -> Status.BatchDeclined
                Status.SendTooLarge -> Status.BatchTooLarge
                Status.SendCancelled -> Status.BatchCancelled
                Status.SendTimedOut -> Status.BatchTimedOut
                else -> status
            }
        }

        val source = sources.singleOrNull() ?: return Status.LegacyIncompatible
        checkGrant(context, source.uri)
        val uri = source.uri
        if (network != null) {
            return sendStatus(network.sendLegacy(
                prepare = {
                    val reportedType = checkFileType(context, source)
                    val name = queryName(context, uri)
                    val type = reportedType ?: name?.substringAfterLast('.', "")?.lowercase()?.let {
                        MimeTypeMap.getSingleton().getMimeTypeFromExtension(it)
                    }
                    if (type?.isLegacyType() != true) reject(Status.LegacyIncompatible)
                    val descriptor = openDescriptor(context, uri)
                    val length = descriptor.declaredLength
                    if (length > TRANSFER_MAX_BYTES) reject(Status.SendTooLarge)
                    val input = openStream(descriptor::createInputStream)
                    LanTransfer.LegacyItem(input, length, type == "application/pdf", name)
                },
                onAccepted = { phase.set(SendPhase.Sending) },
                onProgress = { sentBytes.set(it) },
            ))
        }
        if (cable != null) {
            val type = source.type?.takeIf { it.isLegacyType() } ?: return Status.LegacyIncompatible
            return sendStatus(cable.send(
                isPdf = type == "application/pdf",
                prepare = {
                    val actualType = checkFileType(context, source)
                    if (actualType?.isLegacyType() != true || (type == "application/pdf") != (actualType == "application/pdf")) {
                        reject(Status.LegacyIncompatible)
                    }
                    val descriptor = openDescriptor(context, uri)
                    CableTransfer.Payload(descriptor.declaredLength) { openStream(descriptor::createInputStream) }
                },
                displayName = { queryName(context, uri) },
                onApproved = { phase.set(SendPhase.Sending) },
                onProgress = { sentBytes.set(it) },
            ))
        }
        checkFileType(context, source)
        val descriptor = openDescriptor(context, uri)
        if (descriptor.declaredLength > CHECK_MAX_BYTES) return Status.CheckTooLarge
        return checkFile(openStream(descriptor::createInputStream))
    }

    private fun reject(status: Status): Nothing {
        sourceError = status
        throw IOException()
    }

    private fun checkGrant(context: Context, uri: Uri) {
        checkActive()
        if (!context.hasReadGrant(uri)) reject(Status.GrantLost)
    }

    private fun checkFileType(context: Context, source: Source): String? {
        val mime = queryColumn(
            context, source.uri, DocumentsContract.Document.COLUMN_MIME_TYPE, read = Cursor::getString,
        ) ?: source.type
        if (mime == DocumentsContract.Document.MIME_TYPE_DIR) reject(Status.Unsupported)
        return mime
    }

    private fun openDescriptor(context: Context, uri: Uri): AssetFileDescriptor {
        checkGrant(context, uri)
        val descriptor = try {
            context.contentResolver.openAssetFileDescriptor(uri, "r", signal) ?: throw IOException()
        } catch (_: SecurityException) {
            reject(Status.GrantLost)
        }
        attach(descriptor)
        return descriptor
    }

    private fun openStream(open: () -> InputStream): InputStream {
        checkActive()
        return open().also(::attach)
    }

    private fun queryName(context: Context, uri: Uri): String? =
        queryColumn(context, uri, OpenableColumns.DISPLAY_NAME, read = Cursor::getString)

    private fun <T> queryColumn(
        context: Context,
        uri: Uri,
        column: String,
        read: (Cursor, Int) -> T?,
    ): T? {
        checkGrant(context, uri)
        return try {
            val cursor = context.contentResolver.query(uri, arrayOf(column), null, null, null, signal) ?: return null
            attach(cursor)
            try {
                val index = cursor.getColumnIndex(column)
                if (index >= 0 && cursor.moveToFirst() && !cursor.isNull(index)) read(cursor, index) else null
            } finally {
                val toClose = synchronized(lock) { takeCursor() }
                cleanup { toClose?.close() }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: RuntimeException) {
            checkGrant(context, uri)
            null
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
        TransferResult.Failed -> sourceError ?: Status.SendFailed
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

    private fun attach(opened: Closeable) {
        val attached = synchronized(lock) {
            if (stopReason != null) false else {
                if (opened is Cursor) cursorResource = opened else resource = opened
                true
            }
        }
        if (!attached) {
            cleanup { opened.close() }
            throw OperationCanceledException()
        }
    }

    private fun takeResource(): Closeable? = resource.also { resource = null }
    private fun takeCursor(): Cursor? = cursorResource.also { cursorResource = null }
}

private class IntakeState : ViewModel() {
    val receiver = ReceiveState()
    var pendingCopyName: String? = null
    var pendingIntent by mutableStateOf<Intent?>(null)
    var status by mutableStateOf<Status>(Status.Idle)
    var sendingTo by mutableStateOf<String?>(null)
        private set
    var hasShareSession = false
    var sentBytes by mutableStateOf<Long?>(null)
        private set
    var committed by mutableIntStateOf(0)
        private set
    var batchSummary by mutableStateOf<BatchSummary?>(null)
        private set
    private var selection by mutableStateOf<Selection?>(null)
    private var pickedTypeLookup: Job? = null
    private var scanSelection: Selection? = null
    private var operation: Operation? = null
    val selectionCount: Int get() = selection?.sources?.size ?: 0
    val hasSource: Boolean get() = selection != null
    val cableEligible: Boolean get() = selection?.cableEligible == true
    var cableMode by mutableStateOf(false)
        private set
    val canRetryCable: Boolean
        get() = cableMode && hasSource && status.canRetryCable
    var enteringCode by mutableStateOf(false)
        private set
    var invitation by mutableStateOf("")
        private set
    var parsedInvite by mutableStateOf<LanInvite?>(null)
        private set
    var scanMessage by mutableStateOf<Int?>(null)
    val transferSnapshot: TransferSnapshot?
        get() = when {
            operation?.sending == true || status == Status.Interrupted -> TransferSnapshot.InFlight
            status == Status.Saved || status == Status.BatchSaved -> TransferSnapshot.Saved
            status.unconfirmed -> TransferSnapshot.Unconfirmed
            else -> null
        }

    fun restore(state: Bundle) {
        if (hasShareSession) return
        val count = state.getInt(BATCH_COUNT)
        val received = state.getInt(BATCH_COMMITTED)
        val unconfirmed = state.getBoolean(BATCH_UNCONFIRMED)
        if (count in 1..SEND_BATCH_MAX_ITEMS && received in 0..count && (!unconfirmed || received < count)) {
            batchSummary = BatchSummary(count, received, unconfirmed, state.getBoolean(PREVIOUS_BATCH))
        }
        val restored = when (state.getString(TRANSFER_SNAPSHOT)) {
            TransferSnapshot.Saved.name -> if (received == count && count > 0) Status.BatchSaved else Status.Saved
            TransferSnapshot.Unconfirmed.name -> Status.Unconfirmed
            TransferSnapshot.InFlight.name -> Status.Interrupted
            else -> if (state.getBoolean(SHARE_SESSION)) Status.Retry else return
        }
        hasShareSession = true
        status = restored
    }

    fun openCodeEntry() {
        if (hasSource && !cableMode && !status.working && parsedInvite == null) enteringCode = true
    }

    fun clearInvitation() {
        enteringCode = false
        invitation = ""
        parsedInvite = null
        scanMessage = null
    }

    fun editInvitation(value: String) {
        if (!enteringCode) return
        val parsed = LanInvite.parse(value)
        if (parsed == null) {
            invitation = value
            scanMessage = null
        } else {
            clearInvitation()
            parsedInvite = parsed
        }
    }

    fun acceptScan(contents: String?, missingPermission: Boolean) {
        clearInvitation()
        parsedInvite = contents?.let(LanInvite::parse)
        scanMessage = when {
            parsedInvite != null -> null
            contents != null -> R.string.scan_invalid
            missingPermission -> R.string.scan_unavailable
            else -> R.string.scan_no_result
        }
    }

    fun reset(keepReceipt: Boolean) {
        cancelPickedType()
        clearInvitation()
        hasShareSession = true
        cableMode = false
        selection = null
        sentBytes = null
        committed = 0
        batchSummary = batchSummary?.takeIf { keepReceipt && (it.committed > 0 || it.unconfirmed) }
            ?.copy(previous = true)
        status = Status.Idle
    }

    fun select(context: Context, uris: List<Uri>, type: String? = null, keepReceipt: Boolean = false) {
        reset(keepReceipt)
        status = when {
            uris.isEmpty() -> Status.Missing
            uris.size > SEND_BATCH_MAX_ITEMS -> Status.TooManyFiles
            type == DocumentsContract.Document.MIME_TYPE_DIR || uris.any { uri ->
                uri.scheme != ContentResolver.SCHEME_CONTENT || uri.authority.isNullOrBlank() ||
                    DocumentsContract.isTreeUri(uri) && uri.pathSegments.getOrNull(2) != "document"
            } -> Status.Unsupported
            uris.any { !context.hasReadGrant(it) } -> Status.GrantLost
            FileOperations.busy -> Status.Busy
            else -> {
                selection = Selection(uris.map { Source(it, type.takeIf { uris.size == 1 }) })
                Status.Ready
            }
        }
    }

    fun selectPicked(context: Context, uris: List<Uri>) {
        select(context, uris)
        val selected = selection ?: return
        val uri = selected.sources.singleOrNull()?.uri ?: return
        pickedTypeLookup = viewModelScope.launch {
            val type = withContext(Dispatchers.IO) {
                try {
                    if (context.hasReadGrant(uri)) context.contentResolver.getType(uri) else null
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: RuntimeException) {
                    null
                }
            }
            if (type != null && selection === selected) {
                val resolved = Selection(listOf(Source(uri, type)))
                selection = resolved
                if (scanSelection === selected) scanSelection = resolved
            }
        }
    }

    fun cancelPickedType() {
        pickedTypeLookup?.cancel()
        pickedTypeLookup = null
    }

    fun useCable(context: Context) {
        if (!cableEligible || cableMode || status.working || parsedInvite != null) return
        clearInvitation()
        cableMode = true
        start(context, null)
    }

    fun useWifi() {
        if (!cableMode || !hasSource || status.working) return
        cableMode = false
        sentBytes = null
        status = Status.Ready
    }

    fun beginScan(): Boolean {
        if (scanSelection != null) return false
        scanSelection = selection ?: return false
        return true
    }

    fun finishScan(): Boolean {
        val scannedFor = scanSelection
        scanSelection = null
        return scannedFor != null && scannedFor === selection
    }

    fun saveCopy(context: Context, name: String, uri: Uri) {
        viewModelScope.launch {
            val message = try {
                withContext(Dispatchers.IO) {
                    if (uri.scheme != ContentResolver.SCHEME_CONTENT || !DocumentsContract.isDocumentUri(context, uri)) {
                        throw IOException("Invalid document destination")
                    }
                    withPublishedReceivedFile(context.filesDir, name) { fd ->
                        ParcelFileDescriptor.AutoCloseInputStream(ParcelFileDescriptor.dup(fd)).use { input ->
                            val output = context.contentResolver.openOutputStream(uri, "wt") ?: throw IOException("Document unavailable")
                            output.use { input.copyTo(it) }
                        }
                    }
                }
                R.string.receive_copy_saved
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: IOException) {
                R.string.receive_save_failed
            } catch (_: RuntimeException) {
                R.string.receive_save_failed
            }
            Toast.makeText(context, message, Toast.LENGTH_LONG).show()
        }
    }

    fun start(
        context: Context,
        invite: LanInvite?,
        openPrivateInvite: (() -> LanInvite)? = null,
        recipient: String? = null,
    ) {
        val selected = selection ?: return
        if (operation != null) return
        if (selected.sources.size != 1 && invite?.isBatch != true && openPrivateInvite == null) {
            if (invite != null) status = Status.LegacyIncompatible
            return
        }
        val attempt = FileOperations.start(
            context, selected, invite, openPrivateInvite, cableMode,
            onProgress = { count, phase, received ->
                if (status.waitingForReceiver) {
                    status = when (phase) {
                        SendPhase.Connecting -> status
                        SendPhase.Approval -> Status.WaitingForApproval
                        SendPhase.Sending -> Status.Sending
                    }
                }
                if (phase == SendPhase.Sending) sentBytes = count
                committed = received
            },
            onStopping = { status = it },
        ) { result, count, summary ->
            operation = null
            sendingTo = null
            if (result == Status.Unsupported || result == Status.GrantLost || result == Status.Saved ||
                result.unconfirmed || (summary?.committed ?: 0) > 0
            ) {
                selection = null
            }
            sentBytes = count?.takeUnless { cableMode && it == 0L && result != Status.Saved }
            batchSummary = summary
            status = result
        }
        operation = attempt
        sendingTo = recipient.takeIf { attempt != null }
        if (attempt == null) {
            status = Status.Busy
        } else {
            sentBytes = null
            committed = 0
            batchSummary = null
            status = when {
                cableMode -> Status.WaitingForComputer
                attempt.sending -> Status.Connecting
                else -> Status.Reading
            }
        }
    }

    fun cancel(): Boolean {
        val active = operation ?: return false
        active.cancel()
        return true
    }

    fun cancelTrustedSend() {
        operation?.takeIf { it.trusted }?.cancel()
    }

    override fun onCleared() {
        receiver.close()
        cancel()
    }
}

private sealed class Status(val message: Int) {
    val waitingForReceiver: Boolean get() = this == WaitingForComputer || this == Connecting || this == WaitingForApproval
    val canCancel: Boolean get() = waitingForReceiver || this == Reading || this == Sending
    val working: Boolean get() = canCancel || this == Cancelling || this == TimingOut
    val canRetryCable: Boolean
        get() = this == Busy || this == CableUnavailable || this == SendTimedOut || this == SendCancelled ||
            this == SendFailed || this == Declined
    val unconfirmed: Boolean get() = this == Unconfirmed || this == UnconfirmedAfterCancel || this == UnconfirmedAfterTimeout

    data object Idle : Status(R.string.share_hint)
    data object Ready : Status(R.string.file_selected)
    data object WaitingForComputer : Status(R.string.waiting_for_computer)
    data object Connecting : Status(R.string.connecting)
    data object WaitingForApproval : Status(R.string.waiting_for_approval)
    data object CableUnavailable : Status(R.string.open_seam_on_computer)
    data object Declined : Status(R.string.send_declined)
    data object Sending : Status(R.string.sending)
    data object Unconfirmed : Status(R.string.save_unconfirmed)
    data object UnconfirmedAfterCancel : Status(R.string.save_unconfirmed_cancelled)
    data object UnconfirmedAfterTimeout : Status(R.string.save_unconfirmed_timeout)
    data object Interrupted : Status(R.string.send_interrupted)
    data object SendFailed : Status(R.string.send_failed)
    data object TrustedUnavailable : Status(R.string.trusted_send_unavailable)
    data object SendCancelled : Status(R.string.send_cancelled)
    data object SendTimedOut : Status(R.string.send_timed_out)
    data object TlsUnavailable : Status(R.string.tls_unavailable)
    data object Saved : Status(R.string.file_saved)
    data object BatchSaved : Status(R.string.batch_saved)
    data object BatchDeclined : Status(R.string.batch_declined)
    data object BatchCancelled : Status(R.string.batch_cancelled)
    data object BatchTimedOut : Status(R.string.batch_timed_out)
    data object BatchTooLarge : Status(R.string.batch_too_large)
    data object LegacyIncompatible : Status(R.string.legacy_wifi_incompatible)
    data object TooManyFiles : Status(R.string.too_many_files)
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
    selectionCount: Int,
    committed: Int,
    batchSummary: BatchSummary?,
    cableMode: Boolean,
    cableEligible: Boolean,
    enteringCode: Boolean,
    invitation: String,
    invite: LanInvite?,
    scanMessage: Int?,
    trustedPeers: List<TrustedPeer>,
    sendingTo: String?,
    onSendToTrusted: (TrustedPeer) -> Unit,
    onScan: () -> Unit,
    onEnterCode: () -> Unit,
    onInvitationChanged: (String) -> Unit,
    onSend: () -> Unit,
    onCheck: () -> Unit,
    onPick: () -> Unit,
    onUseCable: () -> Unit,
    onRetryCable: () -> Unit,
    onUseWifi: () -> Unit,
    onCancel: () -> Unit,
) {
    val focus = LocalFocusManager.current
    val scan = { focus.clearFocus(); onScan() }
    val hasSource = selectionCount != 0
    val showWifi = hasSource && !cableMode && !status.working && supportsLanSend && status != Status.TlsUnavailable
    val destination = invite?.takeIf { showWifi }
    val legacy = destination?.isBatch == false
    val headline = if (destination != null) R.string.ready_to_send else when (status) {
        Status.TlsUnavailable -> R.string.sending_unavailable
        Status.Interrupted -> R.string.save_status_unknown
        Status.UnconfirmedAfterCancel, Status.UnconfirmedAfterTimeout -> R.string.save_unconfirmed
        Status.Cancelling -> R.string.cancelling_headline
        Status.TimingOut -> R.string.timing_out_headline
        Status.GrantLost -> R.string.file_access_lost
        Status.TrustedUnavailable -> R.string.trusted_not_ready
        is Status.Read -> R.string.file_checked
        else -> status.message
    }
    val detail = when (status) {
        Status.TlsUnavailable, Status.Interrupted, Status.GrantLost, Status.TrustedUnavailable, is Status.Read -> status.message
        Status.UnconfirmedAfterCancel -> R.string.send_cancelled_short
        Status.UnconfirmedAfterTimeout -> R.string.timing_out_headline
        Status.Cancelling, Status.TimingOut -> R.string.waiting_for_stop
        Status.Ready -> R.string.tls_unavailable.takeUnless { supportsLanSend }
        Status.WaitingForComputer -> R.string.accept_on_computer
        Status.WaitingForApproval -> R.string.approve_on_receiver
        Status.CableUnavailable -> R.string.connect_usb
        else -> null
    }
    if (status == Status.Idle) {
        TransferIdleHeader(receiving = false)
        SeamAction(stringResource(R.string.pick_file), onPick)
    } else {
        TransferPageHeader(stringResource(headline), announce = true) {
            if (hasSource && (batchSummary == null || batchSummary.previous)) {
                SeamDetail(pluralStringResource(R.plurals.selection_count, selectionCount, selectionCount))
            }
            if (sendingTo != null) SeamDetail(stringResource(R.string.destination, sendingTo))
            if (destination != null) {
                SeamDetail(stringResource(R.string.destination, destination.address))
                SeamDetail(stringResource(if (legacy) R.string.legacy_wifi_hint else R.string.send_connect_hint))
            } else {
                detail?.let { SeamDetail(stringResource(it)) }
                if (status is Status.Read) {
                    SeamDetail(stringResource(R.string.byte_count, status.count))
                    SeamDetail(stringResource(R.string.sha256, status.digest))
                }
                if (sentBytes != null) {
                    SeamDetail(stringResource(R.string.bytes_sent, sentBytes))
                }
            }
            if (batchSummary != null) {
                val label = if (batchSummary.previous) R.string.previous_batch_committed else R.string.batch_committed
                SeamDetail(stringResource(label, batchSummary.committed, batchSummary.count))
                if (batchSummary.unconfirmed) {
                    SeamDetail(stringResource(R.string.batch_item_unconfirmed, batchSummary.committed + 1))
                }
                if (batchSummary.unsent != 0) SeamDetail(stringResource(R.string.batch_unsent, batchSummary.unsent))
            } else if (status.working && committed != 0) {
                SeamDetail(stringResource(R.string.batch_committed, committed, selectionCount))
            }
            if (status.unconfirmed || status == Status.Interrupted || batchSummary?.unconfirmed == true) {
                SeamDetail(stringResource(R.string.check_receiver))
            }
        }
        when {
            status.canCancel -> SeamAction(stringResource(R.string.cancel), onCancel)
            status.working -> Unit
            cableMode && hasSource -> {
                if (status.canRetryCable) {
                    SeamAction(stringResource(R.string.try_cable_again), onRetryCable)
                }
                if (supportsLanSend) {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                        SeamAction(stringResource(R.string.use_wifi), onUseWifi, primary = false)
                    }
                }
                SeamAction(stringResource(R.string.pick_file), onPick, primary = false)
            }
            !hasSource -> SeamAction(stringResource(R.string.pick_file), onPick)
            showWifi -> {
                if (destination != null) {
                    if (!legacy || selectionCount == 1) {
                        SeamAction(
                            stringResource(if (legacy) R.string.send_legacy_wifi else R.string.send),
                            { focus.clearFocus(); onSend() },
                        )
                    }
                } else {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        SeamDetail(stringResource(R.string.scan_qr_hint))
                        val feedback = scanMessage ?: R.string.invalid_invitation.takeIf { invitation.isNotEmpty() }
                        feedback?.let {
                            Text(
                                text = stringResource(it),
                                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                                color = MaterialTheme.colorScheme.error,
                                style = MaterialTheme.typography.bodyMedium,
                            )
                        }
                        SeamAction(stringResource(R.string.scan_invitation), scan)
                        if (enteringCode) {
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
                        }
                    }
                }
                if (trustedPeers.isNotEmpty()) TrustedDestinationPicker(trustedPeers, onSendToTrusted)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (destination != null) {
                        SeamAction(stringResource(R.string.scan_again), scan, primary = false)
                    } else if (!enteringCode) {
                        SeamAction(stringResource(R.string.enter_code), onEnterCode, primary = false)
                    }
                    if (destination == null && cableEligible) {
                        SeamAction(stringResource(R.string.use_debug_usb), onUseCable, primary = false)
                    }
                    if (selectionCount == 1) SeamAction(stringResource(R.string.check_file), onCheck, primary = false)
                    SeamAction(stringResource(R.string.pick_file), onPick, primary = false)
                }
            }
            else -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (!supportsLanSend && status != Status.Ready && status != Status.TlsUnavailable) {
                    SeamDetail(stringResource(R.string.tls_unavailable))
                }
                if (cableEligible) SeamAction(stringResource(R.string.use_debug_usb), onUseCable)
                if (selectionCount == 1) SeamAction(stringResource(R.string.check_file), onCheck, primary = !cableEligible)
                SeamAction(stringResource(R.string.pick_file), onPick, primary = false)
            }
        }
    }
}
