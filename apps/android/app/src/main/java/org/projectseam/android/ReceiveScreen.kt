package org.projectseam.android

import android.content.Context
import android.os.Build
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.ModalBottomSheetProperties
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.SecureFlagPolicy
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.common.BitMatrix
import com.google.zxing.qrcode.QRCodeWriter
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.IOException
import java.net.SocketTimeoutException
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference
import kotlin.math.floor

internal val supportsLanReceive: Boolean get() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q

internal sealed interface ReceivePhase {
    data object Starting : ReceivePhase
    class Waiting(val code: String, val qr: BitMatrix) : ReceivePhase
    data object Connected : ReceivePhase
    data class Receiving(val count: Int, val saved: Int, val bytes: Long) : ReceivePhase
    data object Cancelling : ReceivePhase
    data class Result(val message: Int, val files: List<ReceivedFile>, val offered: Int?) : ReceivePhase
}

internal class ReceiveState {
    var phase by mutableStateOf<ReceivePhase?>(null)
        private set
    var lastResult by mutableStateOf<ReceivePhase.Result?>(null)
        private set
    var openFailed by mutableStateOf(false)
    private var attempt: ReceiveOperations.Attempt? = null

    fun start(context: Context) {
        if (!supportsLanReceive || phase != null || ReceiveOperations.busy) return
        lastResult = null
        phase = ReceivePhase.Starting
        openFailed = false
        attempt = ReceiveOperations.start(context, onPhase = {
            if (phase != null && phase != ReceivePhase.Cancelling) phase = it
        }) { result ->
            attempt = null
            if (result.files.isNotEmpty()) lastResult = result
            if (phase != null) phase = result
        }
    }

    fun cancel() {
        val receiving = attempt ?: return
        if (phase == null || phase == ReceivePhase.Cancelling) return
        phase = ReceivePhase.Cancelling
        receiving.stop()
    }

    fun close() {
        phase = null
        attempt?.stop()
    }
}

internal object ReceiveOperations {
    private val scope = CoroutineScope(Dispatchers.Main)
    private var active by mutableStateOf<Attempt?>(null)
    val busy: Boolean get() = active != null

    fun stop(): Job? = active?.let {
        it.stop()
        it.finished
    }

    fun start(context: Context, onPhase: (ReceivePhase) -> Unit, onFinished: (ReceivePhase.Result) -> Unit): Attempt {
        check(active == null)
        val attempt = Attempt(scope)
        active = attempt
        attempt.finished = scope.launch {
            val progress = launch {
                while (true) {
                    delay(150)
                    attempt.progress()?.let(onPhase)
                }
            }
            val message = try {
                attempt.run(context, onPhase)
            } finally {
                progress.cancel()
                attempt.closing?.join()
                active = null
            }
            onFinished(attempt.result(message))
        }
        return attempt
    }

    internal class Attempt(private val scope: CoroutineScope) {
        private val lock = Any()
        private var listener: LanReceive? = null
        private val stopRequested = AtomicBoolean()
        val stopped: Boolean get() = stopRequested.get()
        private val bytes = AtomicLong()
        private val saved = AtomicReference<List<ReceivedFile>>(emptyList())
        @Volatile private var connected = false
        @Volatile private var acceptedCount = 0
        lateinit var finished: Job
        var closing: Job? = null
            private set

        fun stop() {
            if (!stopRequested.compareAndSet(false, true)) return
            val opened = takeListener() ?: return
            closing = scope.launch(Dispatchers.IO) { close(opened) }
        }

        fun progress(): ReceivePhase? {
            if (stopped) return null
            val count = acceptedCount
            return when {
                count > 0 -> ReceivePhase.Receiving(count, saved.get().size, bytes.get())
                connected -> ReceivePhase.Connected
                else -> null
            }
        }

        fun result(message: Int): ReceivePhase.Result {
            val status = if (stopped && message != R.string.receive_batch_complete) R.string.receive_batch_cancelled else message
            return ReceivePhase.Result(status, saved.get(), acceptedCount.takeIf { it > 0 })
        }

        suspend fun run(context: Context, onPhase: (ReceivePhase) -> Unit): Int = withContext(Dispatchers.IO) {
            var waiting = false
            try {
                if (stopped) return@withContext R.string.receive_batch_cancelled
                val opened = LanReceive.start(context)
                val registered = synchronized(lock) {
                    if (stopped) false else {
                        listener = opened
                        true
                    }
                }
                if (!registered) {
                    close(opened)
                    return@withContext R.string.receive_batch_cancelled
                }
                val code = opened.invitation
                val qr = QRCodeWriter().encode(code, BarcodeFormat.QR_CODE, 1, 1, mapOf(EncodeHintType.MARGIN to 4))
                withContext(Dispatchers.Main) {
                    if (!stopped) onPhase(ReceivePhase.Waiting(code, qr))
                }
                if (stopped) return@withContext R.string.receive_batch_cancelled
                waiting = true
                opened.awaitBatch(
                    onConnected = { connected = true },
                    onOffer = { count ->
                        if (stopped) throw IOException("Receive session closed")
                        acceptedCount = count
                    },
                    onSaved = { file -> saved.set(saved.get() + file) },
                    onProgress = { bytes.set(it) },
                )
                R.string.receive_batch_complete
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: SocketTimeoutException) {
                R.string.receive_timed_out
            } catch (_: Exception) {
                if (waiting) R.string.receive_batch_failed else R.string.receive_start_failed
            } finally {
                takeListener()?.let(::close)
            }
        }

        private fun takeListener(): LanReceive? = synchronized(lock) { listener.also { listener = null } }

        private fun close(opened: LanReceive) {
            try {
                opened.close()
            } catch (_: Exception) {
            }
        }
    }
}

@Composable
internal fun ReceiveScreen(state: ReceiveState, canStart: Boolean, onStart: () -> Unit, onHistory: () -> Unit) {
    TransferPageHeader(
        title = stringResource(R.string.receive_batch_title),
        peerTitle = stringResource(R.string.send_files),
    ) {
        SliploomDetail(stringResource(if (supportsLanReceive) R.string.receive_batch_hint else R.string.receive_unavailable))
    }
    if (supportsLanReceive) {
        SliploomAction(stringResource(R.string.receive_once), onStart, enabled = canStart)
    }
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        SliploomAction(
            stringResource(R.string.received_files), onHistory, primary = false,
            enabled = state.phase == null && !ReceiveOperations.busy,
        )
    }
    if (state.phase == null) state.lastResult?.let { BatchStatus(it) }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ReceivedFilesSheet(
    onDismiss: () -> Unit,
    onOpen: (ReceivedFile) -> Unit,
    onSave: (ReceivedFile) -> Unit,
    openFailed: Boolean,
) {
    val context = LocalContext.current.applicationContext
    var files by remember { mutableStateOf<List<ReceivedFile>?>(null) }
    var loadFailed by remember { mutableStateOf(false) }
    LaunchedEffect(context) {
        try {
            files = withContext(Dispatchers.IO) { publishedReceivedFiles(context.filesDir) }
        } catch (_: IOException) {
            loadFailed = true
        }
    }
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        properties = ModalBottomSheetProperties(securePolicy = SecureFlagPolicy.SecureOn),
    ) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                stringResource(R.string.received_files),
                modifier = Modifier.semantics { heading() },
                style = MaterialTheme.typography.headlineMedium,
            )
            if (openFailed) ReceiveStatus(stringResource(R.string.receive_open_failed))
            val loaded = files
            when {
                loadFailed -> ReceiveStatus(stringResource(R.string.received_files_load_failed))
                loaded == null -> LinearProgressIndicator(Modifier.fillMaxWidth())
                loaded.isEmpty() -> ReceiveStatus(stringResource(R.string.received_files_empty))
                else -> LazyColumn(Modifier.weight(1f, fill = false), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    items(loaded, key = { it.file.name }) { ReceivedFileRow(it, onOpen, onSave) }
                }
            }
            SliploomAction(stringResource(R.string.close_receive), onDismiss, primary = false)
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ReceiveSheet(
    phase: ReceivePhase,
    onDismiss: () -> Unit,
    onCancel: () -> Unit,
    onOpen: (ReceivedFile) -> Unit,
    onSave: (ReceivedFile) -> Unit,
    openFailed: Boolean,
) {
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        properties = ModalBottomSheetProperties(securePolicy = SecureFlagPolicy.SecureOn),
    ) {
        Column(
            modifier = Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(start = 24.dp, end = 24.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                stringResource(R.string.receive_once),
                modifier = Modifier.semantics { heading() },
                style = MaterialTheme.typography.headlineMedium,
            )
            when (phase) {
                ReceivePhase.Starting -> {
                    ReceiveStatus(stringResource(R.string.receive_starting))
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                }
                is ReceivePhase.Waiting -> {
                    var showCode by remember(phase) { mutableStateOf(false) }
                    ReceiveStatus(stringResource(R.string.receive_scan_hint))
                    SliploomDetail(stringResource(R.string.receive_batch_code_hint))
                    InviteQr(phase.qr, Modifier.align(Alignment.CenterHorizontally))
                    SliploomDetail(stringResource(R.string.receive_batch_keep_open))
                    SliploomAction(
                        stringResource(if (showCode) R.string.hide_receive_code else R.string.show_receive_code),
                        { showCode = !showCode },
                        primary = false,
                    )
                    if (showCode) Text(phase.code, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodyMedium)
                }
                ReceivePhase.Connected -> {
                    ReceiveStatus(stringResource(R.string.receive_batch_connected))
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                }
                is ReceivePhase.Receiving -> {
                    ReceiveStatus(stringResource(R.string.receive_batch_receiving))
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                    SliploomDetail(stringResource(R.string.receive_batch_saved_count, phase.saved, phase.count))
                    SliploomDetail(stringResource(R.string.bytes_received, phase.bytes))
                }
                ReceivePhase.Cancelling -> {
                    ReceiveStatus(stringResource(R.string.receive_batch_cancelling))
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                }
                is ReceivePhase.Result -> BatchResult(phase, onOpen, onSave, openFailed)
            }
            if (phase != ReceivePhase.Cancelling) {
                val isResult = phase is ReceivePhase.Result
                SliploomAction(
                    stringResource(if (isResult) R.string.close_receive else R.string.cancel),
                    if (isResult) onDismiss else onCancel,
                    primary = false,
                )
            }
        }
    }
}

@Composable
private fun ReceiveStatus(message: String) {
    SliploomDetail(message, Modifier.semantics { liveRegion = LiveRegionMode.Polite })
}

@Composable
private fun BatchResult(
    result: ReceivePhase.Result,
    onOpen: (ReceivedFile) -> Unit,
    onSave: (ReceivedFile) -> Unit,
    openFailed: Boolean,
) {
    BatchStatus(result)
    if (openFailed) ReceiveStatus(stringResource(R.string.receive_open_failed))
    result.files.forEach { ReceivedFileRow(it, onOpen, onSave) }
}

@Composable
private fun BatchStatus(result: ReceivePhase.Result) {
    ReceiveStatus(stringResource(result.message))
    if (result.message == R.string.receive_batch_cancelled && result.files.isEmpty()) {
        SliploomDetail(stringResource(R.string.receive_batch_none_saved))
    } else {
        result.offered?.let { SliploomDetail(stringResource(R.string.receive_batch_saved_count, result.files.size, it)) }
    }
}

@Composable
private fun ReceivedFileRow(file: ReceivedFile, onOpen: (ReceivedFile) -> Unit, onSave: (ReceivedFile) -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        SliploomDetail(file.file.name)
        SliploomDetail(stringResource(R.string.receive_batch_size, file.bytes))
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            SliploomAction(stringResource(R.string.open_received_file), { onOpen(file) }, primary = false)
            SliploomAction(stringResource(R.string.save_received_copy), { onSave(file) }, primary = false)
        }
    }
}

@Composable
private fun InviteQr(qr: BitMatrix, modifier: Modifier) {
    val label = stringResource(R.string.receive_qr_description)
    Canvas(modifier.widthIn(max = 248.dp).fillMaxWidth().aspectRatio(1f).background(Color.White)
        .semantics { contentDescription = label }) {
        val module = floor(size.minDimension / qr.width)
        val start = floor((size.minDimension - module * qr.width) / 2f)
        for (y in 0 until qr.height) {
            for (x in 0 until qr.width) {
                if (qr[x, y]) drawRect(Color.Black, Offset(start + x * module, start + y * module), Size(module, module))
            }
        }
    }
}
