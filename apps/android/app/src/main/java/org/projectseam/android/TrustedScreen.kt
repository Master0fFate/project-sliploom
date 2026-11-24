package org.projectseam.android

import android.content.Context
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.ModalBottomSheetProperties
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.Velocity
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
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.IOException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import kotlin.math.floor

internal sealed interface TrustSheet {
    data object EnterCode : TrustSheet
    data object Arming : TrustSheet
    class Invitation(val code: String, val qr: BitMatrix) : TrustSheet
    data object Joining : TrustSheet
    class Confirm(val prompt: TrustedPairPrompt) : TrustSheet
    class Forget(val peer: TrustedPeer) : TrustSheet
}

internal class TrustedState(
    private val context: Context,
    private val scope: CoroutineScope,
    private val canPair: () -> Boolean,
    private val shouldProbe: () -> Boolean,
    private val onSaved: () -> Unit,
) {
    private var session by mutableStateOf<Session?>(null)
    private var devices: TrustedDevices? = null
    private var lifecycle: Job? = null
    private var action by mutableStateOf<Job?>(null)
    private var actionSession: Session? = null
    var localReady by mutableStateOf(false)
        private set
    private var expiry: Job? = null
    private var pendingCode: String? = null
    private var pendingExpiry: Job? = null
    private var awaitingPeer: Set<String>? = null
    private var revision = 0
    var ready by mutableStateOf(false)
        private set
    var message by mutableStateOf<Int?>(null)
        private set
    var peers by mutableStateOf<List<TrustedPeer>>(emptyList())
        private set
    var reachable by mutableStateOf<List<TrustedPeer>>(emptyList())
        private set
    var sheet by mutableStateOf<TrustSheet?>(null)
        private set
    val busy: Boolean get() = action != null || sheet != null

    fun start() {
        if (!supportsLanReceive || lifecycle != null) return
        lifecycle = scope.launch {
            while (true) {
                val runtime = withContext(Dispatchers.IO) { trustResult { TrustedDevices(context) } }
                if (runtime == null) {
                    message = R.string.trusted_unavailable
                    delay(3000)
                    continue
                }
                devices = runtime
                try {
                    val known = withContext(Dispatchers.IO) {
                        trustResult {
                            runtime.load()
                            runtime.peers()
                        }
                    }
                    if (known != null) {
                        peers = known
                        localReady = true
                        val refresh = launch { refresh(runtime) }
                        try {
                            listen(runtime)
                        } finally {
                            refresh.cancel()
                        }
                    }
                } finally {
                    if (devices === runtime) closeRuntime()
                }
                if (message != R.string.trusted_pair_interrupted) message = R.string.trusted_unavailable
                delay(3000)
            }
        }
    }

    private suspend fun listen(runtime: TrustedDevices) {
        while (true) {
            val current = Session(runtime)
            session = current
            val interrupted = try {
                withContext(Dispatchers.IO) {
                    trustResult {
                        runtime.start(
                            onPair = { prompt -> confirm(current, prompt) },
                            onSaved = { scope.launch { if (session === current) onSaved() } },
                        )
                    }
                    runtime.isClosed()
                }
            } finally {
                if (session === current) closeSession(current)?.join()
            }
            if (interrupted) return
            delay(3000)
        }
    }

    fun stop() {
        closeRuntime()
        discardQueuedPair()
        lifecycle?.cancel()
        lifecycle = null
    }

    private fun closeRuntime() {
        devices?.close()
        devices = null
        session?.let(::closeSession)
        action?.cancel()
        action = null
        actionSession = null
        localReady = false
        peers = emptyList()
        sheet = null
    }

    private fun closeSession(current: Session): Job? {
        current.rejectAll()
        session = null
        val oldAction = action.takeIf { actionSession === current }
        if (oldAction != null) {
            oldAction.cancel()
            action = null
            actionSession = null
        }
        if (sheet == TrustSheet.Joining || sheet is TrustSheet.Confirm) message = R.string.trusted_pair_interrupted
        expiry?.cancel()
        expiry = null
        awaitingPeer = null
        if (sheet !is TrustSheet.Forget) sheet = null
        ready = false
        reachable = emptyList()
        return oldAction
    }

    private suspend fun refresh(runtime: TrustedDevices) {
        while (devices === runtime) {
            val current = session
            val version = revision
            val (known, online) = withContext(Dispatchers.IO) {
                trustResult { runtime.peers() } to (current != null && runtime.isReady())
            }
            if (devices !== runtime) return
            if (known != null && version == revision) peers = known
            if (session === current) {
                ready = online && known != null
                if (!ready) reachable = emptyList()
            }
            if (current != null && session === current && ready && known != null && version == revision) {
                if (message == R.string.trusted_unavailable) message = null
                awaitingPeer?.let { ids ->
                    if (known.any { it.id !in ids }) {
                        expiry?.cancel()
                        expiry = null
                        awaitingPeer = null
                        sheet = null
                        message = R.string.trusted_paired
                    }
                }
                resumePair()
                if (shouldProbe()) {
                    reachable = reachable.filter { it in known }
                    for (group in known.chunked(4)) {
                        val responding = withContext(Dispatchers.IO) {
                            coroutineScope {
                                group.map { peer ->
                                    async { peer.takeIf { trustResult { current.devices.ping(peer) } == true } }
                                }.map { it.await() }.filterNotNull()
                            }
                        }
                        if (session !== current || version != revision || !shouldProbe()) break
                        reachable = (reachable - group.toSet()) + responding
                    }
                }
            }
            delay(2000)
        }
    }

    fun queuePair(raw: String) {
        val code = raw.trim()
        if (!isTrustedCode(code)) {
            message = R.string.trusted_invalid_code
            return
        }
        discardQueuedPair()
        pendingCode = code
        pendingExpiry = scope.launch {
            delay(10_000)
            pendingCode = null
            pendingExpiry = null
            message = R.string.trusted_unavailable
        }
        resumePair()
    }

    fun discardQueuedPair() {
        pendingCode = null
        pendingExpiry?.cancel()
        pendingExpiry = null
    }

    private fun resumePair() {
        val code = pendingCode ?: return
        val current = session?.takeIf { ready && canPair() && !busy } ?: return
        discardQueuedPair()
        sheet = TrustSheet.Joining
        perform(current.devices, current, R.string.trusted_pair_failed, { it.pair(code) { prompt -> confirm(current, prompt) } }) { peer ->
            peers = (peers.filterNot { it.id == peer.id } + peer).sortedBy { it.label }
            message = R.string.trusted_paired
        }
    }

    fun arm() {
        val current = session?.takeIf { ready && canPair() && !busy } ?: return
        sheet = TrustSheet.Arming
        perform(current.devices, current, R.string.trusted_pair_failed, { devices ->
            val code = devices.armPair()
            try {
                require(isTrustedCode(code))
                val qr = QRCodeWriter().encode(code, BarcodeFormat.QR_CODE, 1, 1, mapOf(EncodeHintType.MARGIN to 4))
                TrustSheet.Invitation(code, qr)
            } catch (failure: Exception) {
                devices.cancelPair()
                throw failure
            }
        }) { invite ->
            sheet = invite
            expiry = scope.launch {
                delay(PAIR_INVITE_MILLIS)
                if (sheet === invite) {
                    dismiss()
                    message = R.string.trusted_pair_expired
                }
            }
        }
    }

    fun enterCode() {
        if (ready && canPair() && !busy) {
            message = null
            sheet = TrustSheet.EnterCode
        }
    }

    fun submitCode(code: String) {
        if (sheet == TrustSheet.EnterCode && canPair()) {
            sheet = null
            queuePair(code)
        }
    }

    fun requestForget(peer: TrustedPeer) {
        if (localReady && canPair() && !busy && peer in peers) sheet = TrustSheet.Forget(peer)
    }

    fun forget(peer: TrustedPeer) {
        val runtime = devices?.takeIf { localReady && canPair() && (sheet as? TrustSheet.Forget)?.peer == peer } ?: return
        perform(runtime, null, R.string.trusted_forget_failed, { it.forget(peer.id) }) {
            peers = peers.filterNot { it.id == peer.id }
            reachable = reachable.filterNot { it.id == peer.id }
        }
    }

    fun inviteOpener(peer: TrustedPeer): (() -> LanInvite)? {
        val current = session?.takeIf { ready && peer in reachable } ?: return null
        return current.devices.receiveOpener(peer)
    }

    fun dismiss() {
        when (sheet) {
            is TrustSheet.Confirm -> (sheet as? TrustSheet.Confirm)?.let { decide(it, false) }
            is TrustSheet.Invitation -> {
                val current = session ?: return
                expiry?.cancel()
                expiry = null
                sheet = null
                perform(current.devices, current, R.string.trusted_pair_failed, { it.cancelPair() }) {}
            }
            TrustSheet.Arming, TrustSheet.Joining -> {
                stop()
                start()
            }
            TrustSheet.EnterCode, is TrustSheet.Forget -> sheet = null
            null -> Unit
        }
    }

    fun decide(shown: TrustSheet.Confirm, approved: Boolean) {
        val current = session ?: return
        if (sheet !== shown) return
        val waiting = current.showing ?: return
        if (waiting.prompt !== shown.prompt) return
        val allow = approved && canPair()
        if (waiting.answer.complete(allow)) finishConfirmation(waiting, allow)
    }

    private fun finishConfirmation(waiting: PairDecision, allowed: Boolean) {
        if (waiting.outbound) {
            sheet = TrustSheet.Joining
        } else if (allowed) {
            sheet = TrustSheet.Joining
            awaitingPeer = waiting.peerIds
            expiry = scope.launch {
                delay(PAIR_CONFIRM_MILLIS)
                awaitingPeer = null
                sheet = null
                message = R.string.trusted_pair_failed
            }
        } else {
            sheet = null
            message = R.string.trusted_pair_not_confirmed
        }
    }

    fun scanFailed(status: Int) { message = status }

    private fun <T> perform(
        runtime: TrustedDevices,
        current: Session?,
        failure: Int,
        work: (TrustedDevices) -> T,
        onSuccess: (T) -> Unit,
    ) {
        if (action != null) return
        message = null
        actionSession = current
        action = scope.launch {
            val result = withContext(Dispatchers.IO) { trustResult { work(runtime) } }
            if (devices !== runtime || (current != null && session !== current)) return@launch
            action = null
            actionSession = null
            revision++
            sheet = null
            if (result == null) message = failure else onSuccess(result)
        }
    }

    private fun confirm(current: Session, prompt: TrustedPairPrompt): Boolean {
        val waiting = PairDecision(prompt)
        if (!current.begin(waiting)) return false
        scope.launch {
            val outbound = sheet == TrustSheet.Joining && action != null
            val allowed = outbound || sheet is TrustSheet.Invitation
            if (session === current && allowed && canPair() && !waiting.answer.isDone) {
                expiry?.cancel()
                expiry = null
                waiting.outbound = outbound
                waiting.peerIds = peers.mapTo(mutableSetOf()) { it.id }
                current.showing = waiting
                sheet = TrustSheet.Confirm(prompt)
            } else {
                waiting.answer.complete(false)
            }
        }
        return try {
            waiting.answer.get(PAIR_CONFIRM_MILLIS, TimeUnit.MILLISECONDS)
        } catch (_: TimeoutException) {
            false
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            false
        } finally {
            waiting.answer.complete(false)
            current.end(waiting)
            scope.launch {
                if (session === current && current.showing === waiting) {
                    current.showing = null
                    if ((sheet as? TrustSheet.Confirm)?.prompt === prompt) finishConfirmation(waiting, false)
                }
            }
        }
    }

    private class PairDecision(val prompt: TrustedPairPrompt) {
        val answer = CompletableFuture<Boolean>()
        var outbound = false
        var peerIds = emptySet<String>()
    }

    private class Session(val devices: TrustedDevices) {
        private val lock = Any()
        private var closed = false
        private var waiting: PairDecision? = null
        var showing: PairDecision? = null

        fun begin(decision: PairDecision): Boolean = synchronized(lock) {
            if (closed || waiting != null) false else {
                waiting = decision
                true
            }
        }

        fun end(decision: PairDecision) = synchronized(lock) {
            if (waiting === decision) waiting = null
        }

        fun rejectAll() = synchronized(lock) {
            closed = true
            waiting?.answer?.complete(false)
        }
    }
}

private const val PAIR_INVITE_MILLIS = 120_000L
private const val PAIR_CONFIRM_MILLIS = 60_000L
private const val TRUST_CODE_LENGTH = 100
private fun isTrustedCode(code: String): Boolean = code.length == TRUST_CODE_LENGTH && code.startsWith("SEAM5:") &&
    code.substring(6).all { it in 'A'..'Z' || it in 'a'..'z' || it in '0'..'9' || it == '-' || it == '_' }

private inline fun <T> trustResult(work: () -> T): T? = try {
    work()
} catch (cancelled: CancellationException) {
    throw cancelled
} catch (_: Exception) {
    null
}

@Composable
internal fun TrustedScreen(state: TrustedState, onScan: () -> Unit) {
    TransferPageHeader(stringResource(R.string.trusted_devices)) {
        SeamDetail(stringResource(R.string.trusted_hint))
    }
    val peers = state.peers
    if (peers.isNotEmpty()) {
        TrustedDeviceList(peers, enabled = state.localReady && !state.busy, onDevice = state::requestForget)
    } else if (state.localReady) {
        SeamDetail(stringResource(R.string.trusted_empty))
    }
    val status = state.message ?: when {
        !supportsLanReceive -> R.string.receive_unavailable
        !state.ready -> R.string.trusted_unavailable
        else -> null
    }
    status?.let { SeamDetail(stringResource(it), Modifier.semantics { liveRegion = LiveRegionMode.Polite }) }
    if (supportsLanReceive) {
        val enabled = state.ready && !state.busy
        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(2.dp)) {
            SeamAction(stringResource(R.string.trusted_pair_new), state::arm, enabled = enabled)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterHorizontally)) {
                SeamAction(stringResource(R.string.trusted_scan_qr), onScan, primary = false, enabled = enabled)
                SeamAction(stringResource(R.string.enter_code), state::enterCode, primary = false, enabled = enabled)
            }
        }
    }
}

private object DeviceListScrollBoundary : NestedScrollConnection {
    override fun onPostScroll(consumed: Offset, available: Offset, source: NestedScrollSource): Offset =
        Offset(available.x, 0f)

    override suspend fun onPostFling(consumed: Velocity, available: Velocity): Velocity =
        Velocity(available.x, 0f)
}

@Composable
private fun TrustedDeviceList(peers: List<TrustedPeer>, enabled: Boolean, onDevice: (TrustedPeer) -> Unit) {
    if (peers.size == 1) {
        val peer = peers.single()
        TrustedDeviceCard(peer, enabled) { onDevice(peer) }
        return
    }
    val pager = rememberPagerState { peers.size }
    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        HorizontalPager(
            state = pager,
            modifier = Modifier.fillMaxWidth().clipToBounds().nestedScroll(DeviceListScrollBoundary),
            pageSpacing = 8.dp,
            key = { peers[it].id },
        ) { page ->
            val peer = peers[page]
            TrustedDeviceCard(peer, enabled && pager.settledPage == page && !pager.isScrollInProgress) { onDevice(peer) }
        }
        Text(
            stringResource(R.string.trusted_device_progress, pager.currentPage.coerceAtMost(peers.lastIndex) + 1, peers.size),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

@Composable
private fun TrustedDeviceCard(peer: TrustedPeer, enabled: Boolean, onClick: () -> Unit) {
    val active = LocalPageActive.current
    Row(
        modifier = Modifier.fillMaxWidth().heightIn(min = 72.dp)
            .clip(SeamControlShape)
            .background(MaterialTheme.colorScheme.surface)
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, SeamControlShape)
            .focusProperties { canFocus = active() }
            .clickable(
                enabled = enabled,
                onClickLabel = stringResource(R.string.trusted_review_forget),
                role = Role.Button,
            ) { if (active()) onClick() }
            .padding(horizontal = 16.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(peer.label, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                peer.address.hostAddress.orEmpty(),
                color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium,
                maxLines = 1, overflow = TextOverflow.Ellipsis,
            )
        }
        val cue = MaterialTheme.colorScheme.onSurfaceVariant
        Canvas(Modifier.size(12.dp)) {
            val forward = if (layoutDirection == LayoutDirection.Ltr) 1 else -1
            val start = size.width * (0.5f - forward * 0.125f)
            val tip = Offset(size.width * (0.5f + forward * 0.125f), size.height * 0.5f)
            drawLine(cue, Offset(start, size.height * 0.25f), tip, strokeWidth = 1.5.dp.toPx())
            drawLine(cue, tip, Offset(start, size.height * 0.75f), strokeWidth = 1.5.dp.toPx())
        }
    }
}

@Composable
internal fun TrustedDestinationPicker(peers: List<TrustedPeer>, onSend: (TrustedPeer) -> Unit) {
    var menuPeers by remember { mutableStateOf<List<TrustedPeer>?>(null) }
    val focus = LocalFocusManager.current
    val colors = MaterialTheme.colorScheme
    Box {
        SeamAction(
            stringResource(R.string.trusted_choose),
            { focus.clearFocus(); menuPeers = peers },
            primary = false,
        )
        DropdownMenu(
            expanded = menuPeers != null,
            onDismissRequest = { menuPeers = null },
            modifier = Modifier.widthIn(max = 304.dp),
            shape = SeamControlShape,
            containerColor = colors.surfaceContainerHigh,
            shadowElevation = 8.dp,
            border = BorderStroke(1.dp, colors.outlineVariant),
        ) {
            menuPeers.orEmpty().forEach { peer ->
                DropdownMenuItem(
                    text = {
                        Column {
                            Text(peer.label, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            SeamDetail(peer.address.hostAddress.orEmpty())
                        }
                    },
                    trailingIcon = {
                        Text(stringResource(R.string.send), color = colors.onSurface, style = MaterialTheme.typography.labelLarge)
                    },
                    onClick = { menuPeers = null; onSend(peer) },
                )
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun TrustedSheet(state: TrustedState) {
    val sheet = state.sheet ?: return
    val invitation = sheet as? TrustSheet.Invitation
    val showingQr = sheet == TrustSheet.Arming || invitation != null
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(
        onDismissRequest = state::dismiss,
        modifier = if (showingQr) Modifier.fillMaxHeight(0.72f) else Modifier,
        sheetState = sheetState,
        properties = ModalBottomSheetProperties(securePolicy = SecureFlagPolicy.SecureOn),
    ) {
        if (showingQr) {
            PairingQrSheet(invitation, state::dismiss, Modifier.weight(1f))
            return@ModalBottomSheet
        }
        Column(
            modifier = Modifier.fillMaxWidth().verticalScroll(rememberScrollState())
                .padding(start = 24.dp, end = 24.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            val title = when (sheet) {
                TrustSheet.EnterCode -> R.string.trusted_enter_code
                is TrustSheet.Confirm -> R.string.trusted_verify_title
                is TrustSheet.Forget -> R.string.trusted_forget
                else -> R.string.trusted_pair_new
            }
            Text(stringResource(title), Modifier.semantics { heading() }, style = MaterialTheme.typography.headlineMedium)
            when (sheet) {
                TrustSheet.Joining -> {
                    SeamDetail(stringResource(R.string.trusted_preparing), Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                    SeamAction(stringResource(R.string.cancel), state::dismiss, primary = false)
                }
                TrustSheet.Arming, is TrustSheet.Invitation -> Unit
                TrustSheet.EnterCode -> {
                    var code by remember { mutableStateOf("") }
                    val focus = LocalFocusManager.current
                    SeamDetail(stringResource(R.string.trusted_code_hint))
                    OutlinedTextField(
                        modifier = Modifier.fillMaxWidth(),
                        value = code,
                        onValueChange = { if (it.length <= TRUST_CODE_LENGTH + 8) code = it },
                        label = { Text(stringResource(R.string.trusted_enter_code)) },
                        singleLine = true,
                        shape = SeamControlShape,
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
                    )
                    SeamAction(
                        stringResource(R.string.trusted_continue),
                        { focus.clearFocus(); state.submitCode(code) }, enabled = isTrustedCode(code.trim()),
                    )
                    SeamAction(stringResource(R.string.cancel), state::dismiss, primary = false)
                }
                is TrustSheet.Confirm -> {
                    SeamDetail(stringResource(R.string.trusted_compare, sheet.prompt.label))
                    Text(sheet.prompt.code, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.headlineMedium)
                    SeamDetail(stringResource(R.string.trusted_receive_permission))
                    SeamAction(stringResource(R.string.trusted_confirm_yes), { state.decide(sheet, true) })
                    SeamAction(stringResource(R.string.trusted_confirm_no), { state.decide(sheet, false) }, primary = false)
                }
                is TrustSheet.Forget -> {
                    SeamDetail(stringResource(R.string.trusted_forget_hint, sheet.peer.label))
                    SeamAction(stringResource(R.string.trusted_forget), { state.forget(sheet.peer) })
                    SeamAction(stringResource(R.string.cancel), state::dismiss, primary = false)
                }
            }
        }
    }
}

@Composable
private fun PairingQrSheet(invitation: TrustSheet.Invitation?, onCancel: () -> Unit, modifier: Modifier) {
    var showCode by remember(invitation) { mutableStateOf(false) }
    val codeLocation = remember { BringIntoViewRequester() }
    Column(modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, bottom = 12.dp)) {
        BoxWithConstraints(Modifier.fillMaxWidth().weight(1f)) {
            val qrSize = minOf(maxHeight, (maxHeight - 112.dp).coerceIn(144.dp, 248.dp))
            Column(
                modifier = Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(bottom = 8.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(
                    stringResource(R.string.trusted_pair_new),
                    Modifier.semantics { heading() }, style = MaterialTheme.typography.headlineMedium,
                )
                if (invitation == null) {
                    SeamDetail(stringResource(R.string.trusted_preparing), Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                } else {
                    SeamDetail(stringResource(R.string.trusted_qr_hint))
                    PairQr(invitation.qr, qrSize, Modifier.align(Alignment.CenterHorizontally))
                    if (showCode) {
                        Text(
                            invitation.code,
                            Modifier.fillMaxWidth().bringIntoViewRequester(codeLocation),
                            fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodyMedium,
                        )
                        LaunchedEffect(codeLocation) { codeLocation.bringIntoView() }
                    }
                }
            }
        }
        FlowRow(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.End,
        ) {
            if (invitation != null) {
                SeamAction(
                    stringResource(if (showCode) R.string.trusted_hide_code else R.string.trusted_show_code),
                    { showCode = !showCode }, primary = false,
                )
            }
            SeamAction(stringResource(R.string.cancel), onCancel, primary = false)
        }
    }
}

@Composable
private fun PairQr(qr: BitMatrix, maxSize: Dp, modifier: Modifier) {
    val label = stringResource(R.string.trusted_qr_description)
    Canvas(modifier.widthIn(max = maxSize).fillMaxWidth().aspectRatio(1f).background(Color.White)
        .semantics { contentDescription = label }) {
        val module = floor(size.minDimension / qr.width)
        val start = floor((size.minDimension - module * qr.width) / 2)
        for (y in 0 until qr.height) {
            for (x in 0 until qr.width) {
                if (qr[x, y]) drawRect(Color.Black, Offset(start + x * module, start + y * module), Size(module, module))
            }
        }
    }
}
