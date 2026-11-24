package org.projectseam.android

import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.os.SystemClock
import android.util.Base64
import java.io.Closeable
import java.io.DataInputStream
import java.io.IOException
import java.net.Inet4Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Proxy
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketTimeoutException
import java.nio.ByteBuffer
import java.security.MessageDigest
import java.security.SecureRandom
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.concurrent.CancellationException
import java.util.concurrent.ExecutionException
import java.util.concurrent.Executors
import java.util.concurrent.Future
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledThreadPoolExecutor
import java.util.concurrent.SynchronousQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicReference
import javax.net.ssl.SSLSocket

internal data class TrustedPeer(val id: String, val address: Inet4Address, val label: String)
internal data class TrustedPairPrompt(val code: String, val label: String)

internal class TrustedDevices(context: Context) : Closeable {
    private val app = context.applicationContext
    private val store = TrustedPeerStore(app.filesDir)
    private val lock = Any()
    private val random = SecureRandom()
    private val deadlines = ScheduledThreadPoolExecutor(1) { daemon("SEAM trust deadline", it) }.apply {
        removeOnCancelPolicy = true
    }
    private val workers = trustExecutor(MAX_INBOUND, "SEAM trust control")
    private val prompts = trustExecutor(2, "SEAM trust confirmation")
    private val records = LinkedHashMap<String, TrustedRecord>()
    private val calls = HashSet<Call>()
    private var identity: TrustedIdentity? = null
    private var loading = false
    private var stopped = false
    private var stopFailure: IOException? = null
    private var foreground: Foreground? = null
    private var window: Window? = null
    private var pairing: Call? = null
    private var grant: Grant? = null

    fun load() {
        synchronized(lock) {
            checkNotStopped()
            check(identity == null && !loading)
            if (!activeRuntime.compareAndSet(null, this)) throw IOException("Trusted devices unavailable")
            loading = true
        }
        try {
            val local = TrustedIdentity.load(store.exists())
            val saved = store.read(local.pin)
            synchronized(lock) {
                checkNotStopped()
                saved.forEach { records[it.id] = it }
                identity = local
            }
        } finally {
            synchronized(lock) {
                loading = false
                if (stopped || identity == null) activeRuntime.compareAndSet(this, null)
            }
        }
    }

    fun start(onPair: (TrustedPairPrompt) -> Boolean, onSaved: (ReceivedFile) -> Unit) {
        val local = synchronized(lock) { requireIdentity() }
        val listener = ServerSocket()
        var state: Foreground? = null
        var registered = false
        try {
            val manager = app.getSystemService(ConnectivityManager::class.java)
                ?: throw IOException("Connectivity manager unavailable")
            val network = manager.activeNetwork ?: throw IOException("Active private Wi-Fi network unavailable")
            if (!isWlan(manager.getNetworkCapabilities(network))) throw IOException("Active private Wi-Fi network unavailable")
            val address = wlanAddresses(manager.getLinkProperties(network)).firstOrNull()
                ?: throw IOException("Active private Wi-Fi address unavailable")
            val current = Foreground(manager, network, address, listener, onPair, onSaved).also { state = it }
            local.context(random, client = false) { throw CertificateException("Unexpected TLS identity") }
            synchronized(lock) {
                checkNotStopped()
                check(foreground == null)
                current.checkRoute()
                listener.reuseAddress = true
                listener.bind(InetSocketAddress(address, TRUST_PORT), MAX_INBOUND)
                foreground = current
                manager.registerDefaultNetworkCallback(current.callback)
                registered = true
            }
            while (true) accept(current, listener.accept())
        } catch (failure: Exception) {
            synchronized(lock) {
                if (!stopped) throw failure
                stopFailure?.let { throw it }
            }
        } finally {
            synchronized(lock) { state?.let(::disconnect) }
            closeResource(listener)
            if (registered) state?.let { it.manager.unregisterNetworkCallback(it.callback) }
        }
    }

    fun armPair(): String = synchronized(lock) {
        val state = requireForeground()
        val local = checkNotNull(identity)
        if (pairing != null || records.size >= MAX_TRUSTED_PEERS) throw pairUnavailable()
        clearWindow()
        val armed = Window(ByteArray(32).also(random::nextBytes), trustNow() + INVITE_MS)
        window = armed
        armed.expiry = deadlines.schedule({
            synchronized(lock) { if (window === armed) clearWindow() }
        }, INVITE_MS, TimeUnit.MILLISECONDS)
        val packed = ByteBuffer.allocate(70).put(state.address.address).putShort(TRUST_PORT.toShort())
            .put(local.pin).put(armed.nonce).array()
        try {
            PAIR_PREFIX + Base64.encodeToString(packed, BASE64_FLAGS)
        } finally {
            packed.fill(0)
        }
    }

    fun cancelPair() {
        synchronized(lock) {
            clearWindow()
            pairing?.abort()
            pairing = null
        }
    }

    fun isReady(): Boolean = synchronized(lock) {
        try {
            requireForeground()
            true
        } catch (_: IOException) {
            false
        }
    }

    fun isClosed(): Boolean = synchronized(lock) { stopped }

    fun peers(): List<TrustedPeer> = synchronized(lock) {
        requireIdentity()
        records.values.map(TrustedRecord::peer)
    }

    fun forget(id: String) {
        synchronized(lock) {
            requireIdentity()
            val record = records[id] ?: return
            persist(records.values.filterNot { it === record })
            records.remove(id)
            calls.filter { it.approved === record }.forEach { it.abort() }
            if (grant?.record === record) revokeGrant()
        }
    }

    fun pair(code: String, onPair: (TrustedPairPrompt) -> Boolean): TrustedPeer {
        val invite = PairInvite.parse(code)
        try {
            val (call, local) = synchronized(lock) {
                val state = requireForeground()
                val local = checkNotNull(identity)
                if (pairing != null || records.size >= MAX_TRUSTED_PEERS || invite.pin.trustHex() in records ||
                    MessageDigest.isEqual(local.pin, invite.pin)
                ) throw pairUnavailable()
                clearWindow()
                newCall(Socket(Proxy.NO_PROXY), state).apply { outbound = true }.also { pairing = it } to local
            }
            return useControl(call, invite.address, invite.pin) { wire ->
                val body = ByteArray(33)
                try {
                    invite.nonce.copyInto(body)
                    body[32] = TRUST_PLATFORM_ANDROID.toByte()
                    wire.request(PAIR, body)
                } finally {
                    body.fill(0)
                }
                if (wire.readByte() != ACK) throw pairUnavailable()
                val platform = wire.readByte()
                val label = trustPlatformLabel(platform) ?: throw pairUnavailable()
                synchronized(lock) {
                    call.checkActive()
                    call.setDeadline(PAIR_MS)
                }
                val prompt = TrustedPairPrompt(pairNumber(invite.nonce, invite.pin, local.pin), label)
                if (!confirm(call, prompt, onPair)) {
                    wire.reply(REJECT)
                    throw pairUnavailable()
                }
                wire.reply(ACK)
                if (wire.readByte() != ACK) throw pairUnavailable()
                synchronized(lock) { savePair(call, invite.pin, wire.address, platform).peer() }
            }
        } finally {
            invite.nonce.fill(0)
        }
    }

    fun ping(peer: TrustedPeer): Boolean = try {
        withPeer(peer) { wire, call ->
            wire.request(PING)
            wire.readByte() == ACK && synchronized(lock) {
                checkApproved(call, wire)
                true
            }
        }
    } catch (_: Exception) {
        false
    }

    fun receiveOpener(peer: TrustedPeer): (() -> LanInvite)? = synchronized(lock) {
        foreground?.let { state -> { openReceive(peer, state) } }
    }

    fun openReceive(peer: TrustedPeer): LanInvite = openReceive(peer, null)

    private fun openReceive(peer: TrustedPeer, expected: Foreground?): LanInvite = withPeer(peer, expected) { wire, call ->
        wire.request(OPEN)
        if (wire.readByte() != ACK) throw IOException("Trusted device is not ready to receive")
        val encoded = wire.read(100)
        try {
            if (encoded.any { (it.toInt() and 0xff) !in 0x21..0x7e }) throw badControl()
            val invite = LanInvite.parse(String(encoded, Charsets.US_ASCII)) ?: throw badControl()
            if (!invite.isBatch || invite.endpoint.address != wire.address) throw badControl()
            synchronized(lock) { checkApproved(call, wire) }
            invite
        } finally {
            encoded.fill(0)
        }
    }

    override fun close() = synchronized(lock) { stop(null) }

    private fun persist(replacement: Collection<TrustedRecord>) {
        try {
            store.write(checkNotNull(identity).pin, replacement)
        } catch (failure: TrustedPeerStore.UncertainWrite) {
            stop(failure)
            throw failure
        }
    }

    private fun stop(failure: IOException?) {
        if (stopped) return
        stopped = true
        stopFailure = failure
        foreground?.let(::disconnect)
        deadlines.shutdownNow()
        prompts.shutdownNow()
        workers.shutdown()
        if (!loading) activeRuntime.compareAndSet(this, null)
    }

    private fun disconnect(state: Foreground) {
        if (foreground !== state) return
        foreground = null
        val resources = ArrayList<Closeable>()
        resources.add(state.listener)
        calls.forEach { it.abort { resource -> resources.add(resource) } }
        calls.clear()
        clearWindow()
        pairing = null
        grant?.receive?.let { resources.add(it) }
        grant = null
        cleanup.execute { resources.forEach(::closeResource) }
    }

    private fun requireIdentity(): TrustedIdentity {
        checkNotStopped()
        return identity ?: throw IOException("Trusted devices unavailable")
    }

    private fun checkNotStopped() {
        if (stopped) throw stopFailure ?: IOException("Trusted devices closed")
    }

    private fun requireForeground(): Foreground {
        checkNotStopped()
        return (foreground ?: throw IOException("Trusted devices listener unavailable")).also { it.checkRoute() }
    }

    private fun clearWindow(except: Call? = null) {
        val old = window ?: return
        window = null
        old.nonce.fill(0)
        old.expiry?.cancel(false)
        calls.filter { it !== except && it.admitted === old }.forEach { it.abort() }
    }

    private fun revokeGrant() {
        val active = grant ?: return
        grant = null
        active.receive?.let(::closeResource)
    }

    private fun checkGrant(active: Grant) {
        if (requireForeground() !== active.state || grant !== active || records[active.record.id] !== active.record) {
            throw IOException("Trusted receive permission revoked")
        }
    }

    private fun checkApproved(call: Call, wire: Control): TrustedRecord {
        call.checkActive()
        wire.peer.checkValidity()
        val record = call.approved ?: throw IOException("Peer not enrolled")
        if (records[record.id] !== record || !MessageDigest.isEqual(record.pin, wire.pin)) {
            throw IOException("Peer not enrolled")
        }
        return record
    }

    private fun newCall(raw: Socket, state: Foreground): Call = Call(raw, state).also {
        calls.add(it)
        it.setDeadline(CONTROL_MS)
    }

    private fun accept(state: Foreground, raw: Socket) {
        if (!raw.inetAddress.isPrivateV4()) {
            closeResource(raw)
            return
        }
        synchronized(lock) {
            if (stopped || foreground !== state) {
                closeResource(raw)
                return
            }
            val call = newCall(raw, state)
            try {
                workers.execute { serve(call) }
            } catch (_: RejectedExecutionException) {
                call.finish()
            }
        }
    }

    private fun serve(call: Call) {
        var active: Grant? = null
        try {
            useControl(call) { wire ->
                if (!wire.read(MAGIC.size).contentEquals(MAGIC)) throw badControl()
                when (wire.readByte()) {
                    PAIR -> servePair(call, wire)
                    PING -> {
                        val ready = synchronized(lock) {
                            if (call.approved == null) false else {
                                rememberAddress(checkApproved(call, wire), wire.address)
                                grant == null && app.filesDir.usableSpace >= MIN_RECEIVE_SPACE
                            }
                        }
                        wire.reply(if (ready) ACK else REJECT)
                    }
                    OPEN -> active = serveOpen(call, wire)
                    else -> wire.reply(REJECT)
                }
            }
            active?.let(::receiveBatch)
        } catch (_: Exception) {
        } finally {
            active?.let(::finishGrant)
        }
    }

    private fun servePair(call: Call, wire: Control) {
        val nonce = wire.read(32)
        try {
            val platform = wire.readByte()
            val label = trustPlatformLabel(platform)
            val local = checkNotNull(identity)
            val accepted = synchronized(lock) {
                call.checkActive()
                wire.peer.checkValidity()
                val armed = window
                if (armed == null || trustNow() >= armed.expires || !MessageDigest.isEqual(armed.nonce, nonce)) {
                    false
                } else if (call.approved == null && call.admitted !== armed) {
                    false
                } else {
                    clearWindow(except = call)
                    if (pairing != null || records.size >= MAX_TRUSTED_PEERS || wire.pin.trustHex() in records ||
                        MessageDigest.isEqual(local.pin, wire.pin) || label == null
                    ) false else {
                        pairing = call
                        call.setDeadline(PAIR_MS)
                        true
                    }
                }
            }
            if (!accepted || label == null) {
                wire.reply(REJECT)
                return
            }
            wire.reply(ACK, TRUST_PLATFORM_ANDROID)
            val prompt = TrustedPairPrompt(pairNumber(nonce, local.pin, wire.pin), label)
            if (!confirm(call, prompt, call.state.onPair) || wire.readByte() != ACK) {
                wire.reply(REJECT)
                return
            }
            try {
                synchronized(lock) { savePair(call, wire.pin, wire.address, platform) }
            } catch (_: IOException) {
                wire.reply(REJECT)
                return
            }
            wire.reply(ACK)
        } finally {
            nonce.fill(0)
        }
    }

    private fun savePair(call: Call, pin: ByteArray, address: Inet4Address, platform: Int): TrustedRecord {
        call.checkActive()
        call.peer?.checkValidity() ?: throw pairUnavailable()
        if (pairing !== call || records.size >= MAX_TRUSTED_PEERS) throw pairUnavailable()
        val record = TrustedRecord(pin, address, platform)
        if (record.id in records || record.id == identity?.id) throw pairUnavailable()
        persist(records.values + record)
        records[record.id] = record
        call.approved = record
        return record
    }

    private fun confirm(call: Call, prompt: TrustedPairPrompt, ask: (TrustedPairPrompt) -> Boolean): Boolean {
        val task = synchronized(lock) {
            call.checkActive()
            if (pairing !== call) throw pairUnavailable()
            try {
                prompts.submit<Boolean> {
                    synchronized(lock) { call.checkActive() }
                    ask(prompt)
                }.also { call.prompt = it }
            } catch (_: RejectedExecutionException) {
                return false
            }
        }
        return try {
            task.get((call.deadline - trustNow()).coerceAtLeast(0), TimeUnit.MILLISECONDS)
        } catch (_: CancellationException) {
            false
        } catch (_: TimeoutException) {
            false
        } catch (_: ExecutionException) {
            false
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            false
        } finally {
            task.cancel(true)
            synchronized(lock) { call.prompt = null }
        }
    }

    private fun rememberAddress(record: TrustedRecord, address: Inet4Address) {
        if (record.address == address) return
        val updated = TrustedRecord(record.pin, address, record.platform)
        persist(records.values.map { if (it === record) updated else it })
        record.address = address
    }

    private fun serveOpen(call: Call, wire: Control): Grant? {
        val active = synchronized(lock) {
            if (call.approved == null) null else {
                val record = checkApproved(call, wire)
                rememberAddress(record, wire.address)
                if (grant != null || app.filesDir.usableSpace < MIN_RECEIVE_SPACE) null else {
                    Grant(call.state, record).also { grant = it }
                }
            }
        }
        if (active == null) {
            wire.reply(REJECT)
            return null
        }
        var receive: LanReceive? = null
        var sending = false
        var offered = false
        try {
            val state = call.state
            val session = LanReceive.start(app, state.manager, state.network, state.address) { publish ->
                synchronized(lock) {
                    checkGrant(active)
                    publish()
                }
            }.also { receive = it }
            synchronized(lock) {
                checkApproved(call, wire)
                checkGrant(active)
                active.receive = session
            }
            val encoded = session.invitation.toByteArray(Charsets.US_ASCII)
            try {
                if (encoded.size != 100) throw badControl()
                sending = true
                wire.reply(ACK, encoded)
            } finally {
                encoded.fill(0)
            }
            offered = true
            return active
        } catch (failure: Exception) {
            if (sending) throw failure
            wire.reply(REJECT)
            return null
        } finally {
            if (!offered) {
                closeResource(receive)
                finishGrant(active)
            }
        }
    }

    private fun receiveBatch(active: Grant) {
        val receive = synchronized(lock) {
            checkGrant(active)
            checkNotNull(active.receive)
        }
        receive.awaitBatch(
            onConnected = { synchronized(lock) { checkGrant(active) } },
            onOffer = { synchronized(lock) { checkGrant(active) } },
            onSaved = active.state.onSaved,
            onProgress = {},
        )
    }

    private fun finishGrant(active: Grant) = synchronized(lock) {
        if (grant === active) revokeGrant()
    }

    private fun <T> withPeer(peer: TrustedPeer, expected: Foreground? = null, exchange: (Control, Call) -> T): T {
        val (call, address, pin) = synchronized(lock) {
            val state = requireForeground()
            if (expected != null && expected !== state) throw IOException("Trusted connection closed")
            val record = records[peer.id] ?: throw IOException("Peer not enrolled")
            if (calls.count { it.outbound } >= MAX_TRUSTED_PEERS) throw IOException("Too many active peer connections")
            val pending = newCall(Socket(Proxy.NO_PROXY), state).apply {
                approved = record
                outbound = true
            }
            Triple(pending, record.address, record.pin)
        }
        return useControl(call, address, pin) { wire ->
            synchronized(lock) { checkApproved(call, wire) }
            exchange(wire, call)
        }
    }

    private fun <T> useControl(
        call: Call, destination: Inet4Address? = null, expected: ByteArray? = null, exchange: (Control) -> T,
    ): T {
        var tls: SSLSocket? = null
        try {
            val local = checkNotNull(identity)
            val context = local.context(random, client = destination != null) { pin ->
                synchronized(lock) {
                    call.checkActive()
                    val id = pin.trustHex()
                    if (id == local.id) throw CertificateException("Unexpected TLS identity")
                    if (expected != null) {
                        if (!MessageDigest.isEqual(pin, expected)) throw CertificateException("Unexpected TLS identity")
                        if (call.approved != null && records[id] !== call.approved) throw CertificateException("Peer not enrolled")
                    } else {
                        call.approved = records[id]
                        if (call.approved == null) {
                            call.admitted = window?.takeIf { trustNow() < it.expires }
                                ?: throw CertificateException("Peer not enrolled")
                        }
                    }
                    call.verified = pin
                }
            }
            val raw = call.raw
            if (destination != null) {
                call.state.network.bindSocket(raw)
                raw.bind(InetSocketAddress(call.state.address, 0))
                raw.connect(InetSocketAddress(destination, TRUST_PORT), call.remaining())
            }
            val address = raw.inetAddress as? Inet4Address ?: throw badControl()
            if (!address.isPrivateV4()) throw badControl()
            val secure = (context.socketFactory.createSocket(raw, address.hostAddress, raw.port, false) as SSLSocket)
                .also { tls = it }
            secure.useClientMode = destination != null
            secure.enabledProtocols = arrayOf(TRUST_TLS)
            if (!secure.useClientMode) secure.needClientAuth = true
            secure.soTimeout = call.remaining()
            secure.startHandshake()
            val peer = local.authenticatedPeer(secure)
            val pin = trustSha256(peer.encoded)
            synchronized(lock) {
                call.checkActive()
                val verified = call.verified ?: throw badControl()
                if (!MessageDigest.isEqual(pin, verified)) throw badControl()
                call.peer = peer
            }
            return exchange(Control(call, secure, address, peer, pin))
        } finally {
            synchronized(lock) { call.finish() }
            closeResource(tls)
        }
    }

    private inner class Call(val raw: Socket, val state: Foreground) {
        var outbound = false
        var approved: TrustedRecord? = null
        var admitted: Window? = null
        var verified: ByteArray? = null
        var peer: X509Certificate? = null
        var prompt: Future<Boolean>? = null
        var deadline: Long = 0
            private set
        private var expiredTask: Future<*>? = null
        private var cancelled = false

        fun setDeadline(duration: Long) {
            expiredTask?.cancel(false)
            val expiresAt = trustNow() + duration
            deadline = expiresAt
            expiredTask = deadlines.schedule({
                synchronized(lock) { if (deadline == expiresAt && !cancelled) abort() }
            }, duration, TimeUnit.MILLISECONDS)
        }

        fun remaining(): Int = synchronized(lock) {
            checkActive()
            (deadline - trustNow()).coerceAtLeast(1).toInt()
        }

        fun checkActive() {
            if (cancelled || requireForeground() !== state) throw IOException("Trusted connection closed")
            if (trustNow() >= deadline) throw SocketTimeoutException("Trusted connection timed out")
        }

        fun abort(close: (Closeable) -> Unit = ::closeResource) {
            if (cancelled) return
            cancelled = true
            expiredTask?.cancel(false)
            prompt?.cancel(true)
            close(raw)
        }

        fun finish() {
            abort()
            if (pairing === this) pairing = null
            calls.remove(this)
        }
    }

    private class Control(
        private val call: Call,
        private val tls: SSLSocket,
        val address: Inet4Address,
        val peer: X509Certificate,
        val pin: ByteArray,
    ) {
        private val input = DataInputStream(tls.inputStream)

        fun readByte(): Int {
            tls.soTimeout = call.remaining()
            return input.readUnsignedByte()
        }

        fun read(count: Int): ByteArray {
            tls.soTimeout = call.remaining()
            return ByteArray(count).also(input::readFully)
        }

        fun request(op: Int, body: ByteArray = byteArrayOf()) = send(MAGIC + op.toByte() + body)
        fun reply(code: Int) = send(byteArrayOf(code.toByte()))
        fun reply(code: Int, platform: Int) = send(byteArrayOf(code.toByte(), platform.toByte()))
        fun reply(code: Int, body: ByteArray) = send(byteArrayOf(code.toByte()) + body)

        private fun send(bytes: ByteArray) {
            try {
                call.remaining()
                writePacket(tls.outputStream, bytes)
            } finally {
                bytes.fill(0)
            }
        }
    }

    private inner class Foreground(
        val manager: ConnectivityManager,
        val network: Network,
        val address: Inet4Address,
        val listener: ServerSocket,
        val onPair: (TrustedPairPrompt) -> Boolean,
        val onSaved: (ReceivedFile) -> Unit,
    ) {
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                if (network != this@Foreground.network) lost()
            }

            override fun onLost(network: Network) {
                if (network == this@Foreground.network) lost()
            }

            override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
                if (network == this@Foreground.network && !isWlan(capabilities)) lost()
            }

            override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) {
                if (network == this@Foreground.network && address !in wlanAddresses(properties)) lost()
            }

            override fun onBlockedStatusChanged(network: Network, blocked: Boolean) {
                if (network == this@Foreground.network && blocked) lost()
            }
        }

        fun checkRoute() {
            if (manager.activeNetwork != network || !isWlan(manager.getNetworkCapabilities(network)) ||
                address !in wlanAddresses(manager.getLinkProperties(network))
            ) {
                val failure = IOException("Active private Wi-Fi network changed")
                disconnect(this)
                throw failure
            }
        }

        private fun lost() = synchronized(lock) { disconnect(this) }
    }

    private class Window(val nonce: ByteArray, val expires: Long) {
        var expiry: Future<*>? = null
    }

    private class Grant(val state: Foreground, val record: TrustedRecord) {
        var receive: LanReceive? = null
    }

    private companion object {
        val activeRuntime = AtomicReference<TrustedDevices?>()
        val cleanup = Executors.newSingleThreadExecutor { daemon("SEAM trust cleanup", it) }
    }
}

private class PairInvite(val address: Inet4Address, val pin: ByteArray, val nonce: ByteArray) {
    companion object {
        fun parse(code: String): PairInvite {
            if (!code.startsWith(PAIR_PREFIX) || code.length != 100) throw badControl()
            val encoded = code.substring(PAIR_PREFIX.length)
            val packed = try {
                Base64.decode(encoded, BASE64_FLAGS)
            } catch (_: IllegalArgumentException) {
                throw badControl()
            }
            try {
                if (packed.size != 70 || Base64.encodeToString(packed, BASE64_FLAGS) != encoded) throw badControl()
                val address = InetAddress.getByAddress(packed.copyOfRange(0, 4)) as Inet4Address
                if (!address.isPrivateV4() || (ByteBuffer.wrap(packed).getShort(4).toInt() and 0xffff) != TRUST_PORT) {
                    throw badControl()
                }
                return PairInvite(address, packed.copyOfRange(6, 38), packed.copyOfRange(38, 70))
            } finally {
                packed.fill(0)
            }
        }
    }
}

private const val PAIR_PREFIX = "SEAM5:"
private const val BASE64_FLAGS = Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING
private const val TRUST_PORT = 48162
private const val INVITE_MS = 120_000L
private const val PAIR_MS = 60_000L
private const val CONTROL_MS = 5_000L
private const val MAX_INBOUND = 8
private const val MIN_RECEIVE_SPACE = 512L * 1024 * 1024
private const val PAIR = 0x01
private const val PING = 0x02
private const val OPEN = 0x03
private const val ACK = 0x06
private const val REJECT = 0x15
private val MAGIC = byteArrayOf(0x53, 0x45, 0x41, 0x4d, 0x05)

private fun pairNumber(nonce: ByteArray, server: ByteArray, client: ByteArray): String {
    val hash = MessageDigest.getInstance("SHA-256")
    hash.update("SEAM5 pair\u0000".toByteArray(Charsets.US_ASCII))
    hash.update(nonce)
    hash.update(server)
    hash.update(client)
    val value = ByteBuffer.wrap(hash.digest()).int.toLong() and 0xffff_ffffL
    return (value % 1_000_000).toString().padStart(6, '0')
}

private fun trustNow() = SystemClock.elapsedRealtime()
private fun badControl() = IOException("Invalid trusted device message")
private fun pairUnavailable() = IOException("Trusted pairing was not approved")
private fun daemon(name: String, task: Runnable) = Thread(task, name).apply { isDaemon = true }
private fun trustExecutor(max: Int, name: String) = ThreadPoolExecutor(
    0, max, 15, TimeUnit.SECONDS, SynchronousQueue(), { daemon(name, it) },
)

private fun closeResource(resource: Closeable?) {
    try {
        resource?.close()
    } catch (_: IOException) {
    }
}
