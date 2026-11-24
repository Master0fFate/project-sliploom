package org.projectseam.android

import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.os.SystemClock
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.util.Base64
import org.bouncycastle.asn1.x500.X500Name
import org.bouncycastle.asn1.x509.Extension
import org.bouncycastle.asn1.x509.GeneralName
import org.bouncycastle.asn1.x509.GeneralNames
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder
import java.io.Closeable
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.math.BigInteger
import java.net.Inet4Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketTimeoutException
import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.MessageDigest
import java.security.SecureRandom
import java.security.cert.CertificateFactory
import java.security.spec.ECGenParameterSpec
import java.util.Date
import javax.net.ssl.KeyManagerFactory
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket

internal data class ReceivedFile(val file: File, val mediaType: String?, val bytes: Long)

internal fun publishedReceivedFiles(filesDir: File): List<ReceivedFile> {
    val folder = File(filesDir, "received")
    val stat = try {
        Os.lstat(folder.path)
    } catch (failure: ErrnoException) {
        if (failure.errno == OsConstants.ENOENT) return emptyList()
        throw IOException("Cannot inspect receive directory", failure)
    }
    if (!OsConstants.S_ISDIR(stat.st_mode)) throw IOException("Invalid receive directory")
    val entries = folder.listFiles() ?: throw IOException("Cannot list receive directory")
    return entries.asSequence().filterNot { it.name.startsWith('.') }.mapNotNull { file ->
        val entry = try {
            Os.lstat(file.path)
        } catch (failure: ErrnoException) {
            if (failure.errno == OsConstants.ENOENT) return@mapNotNull null
            throw IOException("Cannot inspect received file", failure)
        }
        if (OsConstants.S_ISREG(entry.st_mode)) ReceivedFile(file, null, entry.st_size) to entry.st_mtime else null
    }.sortedWith(compareByDescending<Pair<ReceivedFile, Long>> { it.second }.thenBy { it.first.file.name })
        .map { it.first }.toList()
}

internal fun <T> withPublishedReceivedFile(filesDir: File, name: String, read: (java.io.FileDescriptor) -> T): T {
    val folder = File(filesDir, "received")
    if (name.isEmpty() || name.startsWith('.') || '/' in name || '\u0000' in name) {
        throw IOException("Invalid received file name")
    }
    try {
        if (!OsConstants.S_ISDIR(Os.lstat(folder.path).st_mode)) throw IOException("Invalid receive directory")
        val fd = Os.open(File(folder, name).path, OsConstants.O_NOFOLLOW or OsConstants.O_RDONLY or OsConstants.O_NONBLOCK, 0)
        try {
            if (!OsConstants.S_ISREG(Os.fstat(fd).st_mode)) throw IOException("Invalid received file")
            return read(fd)
        } finally {
            Os.close(fd)
        }
    } catch (failure: ErrnoException) {
        throw IOException("Cannot access received file", failure)
    }
}

internal fun inspectReceivedMime(filesDir: File, name: String): String = withPublishedReceivedFile(filesDir, name) { fd ->
    val prefix = ByteArray(16)
    var count = 0
    while (count < prefix.size) {
        val read = Os.read(fd, prefix, count, prefix.size - count)
        if (read <= 0) break
        count += read
    }
    receivedMime(prefix, count.toLong())
}

internal class LanReceive private constructor(
    private val manager: ConnectivityManager,
    private val network: Network,
    private val address: Inet4Address,
    private val folder: File,
    private val listener: ServerSocket,
    private val tls: SSLContext,
    private val secret: ByteArray,
    val invitation: String,
    private val publishWhileAllowed: ((() -> Unit) -> Unit)?,
) : Closeable {
    private val lock = Object()
    private val expires = elapsed() + INVITE_MS
    private var registered = false
    private var started = false
    private var authorized = false
    private var stopped: IOException? = null
    private var current: Attempt? = null

    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            if (network != this@LanReceive.network) networkLost()
        }

        override fun onLost(network: Network) {
            if (network == this@LanReceive.network) networkLost()
        }

        override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
            if (network == this@LanReceive.network && !isWlan(capabilities)) networkLost()
        }

        override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) {
            if (network == this@LanReceive.network && address !in wlanAddresses(properties)) networkLost()
        }

        override fun onBlockedStatusChanged(network: Network, blocked: Boolean) {
            if (network == this@LanReceive.network && blocked) networkLost()
        }
    }

    private fun startWatching() {
        synchronized(lock) {
            manager.registerDefaultNetworkCallback(callback)
            registered = true
        }
        checkNetwork()
        synchronized(lock) { stopped?.let { throw it } }
        Thread(::watch, "Sliploom receive deadline").apply { isDaemon = true }.start()
    }

    private fun checkNetwork() {
        if (manager.activeNetwork != network || !isWlan(manager.getNetworkCapabilities(network)) ||
            address !in wlanAddresses(manager.getLinkProperties(network))
        ) {
            val failure = IOException("Active private Wi-Fi network unavailable")
            stop(failure)
            throw failure
        }
    }

    private fun networkLost() = stop(IOException("Wi-Fi network lost"))

    override fun close() = stop(IOException("Receive session closed"))

    private fun stop(reason: IOException) {
        val unregister = synchronized(lock) {
            if (stopped != null) return
            stopped = reason
            secret.fill(0)
            closeQuietly(listener)
            current?.raw?.abort()
            lock.notifyAll()
            registered.also { registered = false }
        }
        if (unregister) manager.unregisterNetworkCallback(callback)
    }

    private fun watch() {
        try {
            synchronized(lock) {
                while (stopped == null) {
                    val attempt = current
                    val deadline = when {
                        authorized -> checkNotNull(attempt).deadline
                        attempt != null && !attempt.timedOut -> attempt.deadline
                        else -> expires
                    }
                    val remaining = deadline - elapsed()
                    if (remaining > 0) {
                        lock.wait(remaining)
                    } else if (authorized || elapsed() >= expires) {
                        throw SocketTimeoutException("Receive session timed out")
                    } else if (attempt != null) {
                        attempt.timedOut = true
                        attempt.raw.abort()
                    }
                }
            }
        } catch (failure: SocketTimeoutException) {
            stop(failure)
        } catch (_: InterruptedException) {
            close()
        }
    }

    fun awaitBatch(
        onConnected: () -> Unit,
        onOffer: (Int) -> Unit,
        onSaved: (ReceivedFile) -> Unit,
        onProgress: (Long) -> Unit,
    ) {
        synchronized(lock) {
            check(!started)
            started = true
        }
        try {
            repeat(MAX_ATTEMPTS) {
                checkWaiting()
                val raw = listener.accept()
                var secure: SSLSocket? = null
                try {
                    if (!raw.inetAddress.isPrivateV4()) return@repeat
                    val attempt = synchronized(lock) {
                        checkWaiting()
                        Attempt(raw, minOf(expires, elapsed() + AUTH_MS)).also {
                            current = it
                            lock.notifyAll()
                        }
                    }
                    val wire = try {
                        val socket = (tls.socketFactory.createSocket(
                            raw, raw.inetAddress.hostAddress, raw.port, false,
                        ) as SSLSocket).also { secure = it }
                        socket.useClientMode = false
                        socket.enabledProtocols = arrayOf(RECEIVE_TLS)
                        socket.soTimeout = AUTH_MS.toInt()
                        checkAttempt(attempt)
                        socket.startHandshake()
                        if (socket.session.protocol != RECEIVE_TLS) throw IOException("TLS 1.3 required")
                        Wire(socket, attempt).also { authenticate(it, attempt) }
                    } catch (_: IOException) {
                        checkWaiting()
                        return@repeat
                    }
                    onConnected()
                    wire.readTimeout(RECEIVE_IDLE_MS)
                    wire.write(0x02)
                    receiveBatch(wire, onOffer, onSaved, onProgress)
                    return
                } finally {
                    synchronized(lock) {
                        if (!authorized) {
                            current = null
                            lock.notifyAll()
                        }
                    }
                    raw.abort()
                    closeQuietly(secure)
                }
            }
            checkWaiting()
            throw IOException("Invitation attempt limit reached")
        } catch (failure: IOException) {
            synchronized(lock) { throw stopped ?: failure }
        } finally {
            close()
        }
    }

    private fun checkWaiting() = synchronized(lock) {
        stopped?.let { throw it }
        if (elapsed() >= expires) throw SocketTimeoutException("Invitation expired")
    }

    private fun checkAttempt(attempt: Attempt, madeProgress: Boolean = false) = synchronized(lock) {
        stopped?.let { throw it }
        val now = elapsed()
        if (attempt.timedOut || now >= attempt.deadline) throw SocketTimeoutException("Receive attempt timed out")
        if (authorized && madeProgress) {
            attempt.idle = now + RECEIVE_IDLE_MS
            lock.notifyAll()
        }
    }

    private fun authenticate(wire: Wire, attempt: Attempt) {
        if (!wire.read(5).contentEquals(byteArrayOf(0x53, 0x45, 0x41, 0x4d, 0x04))) badFrame()
        val presented = wire.readFrame(0x01, 32)
        try {
            checkNetwork()
            synchronized(lock) {
                checkAttempt(attempt)
                if (!MessageDigest.isEqual(secret, presented)) badFrame()
                authorized = true
                secret.fill(0)
                val now = elapsed()
                attempt.total = now + RECEIVE_TOTAL_MS
                attempt.idle = now + RECEIVE_IDLE_MS
                closeQuietly(listener)
                lock.notifyAll()
            }
        } finally {
            presented.fill(0)
        }
    }

    private fun receiveBatch(
        wire: Wire,
        onOffer: (Int) -> Unit,
        onSaved: (ReceivedFile) -> Unit,
        onProgress: (Long) -> Unit,
    ) {
        val (type, length) = wire.header()
        if (type != 0x09 || length !in Long.SIZE_BYTES..RECEIVE_MAX_ITEMS * Long.SIZE_BYTES ||
            length % Long.SIZE_BYTES != 0
        ) rejectOffer(wire)
        val offer = ByteBuffer.wrap(wire.read(length))
        val lengths = LongArray(length / Long.SIZE_BYTES) { offer.long }
        var upperBound = 0L
        for (declared in lengths) {
            if (declared !in -1L..RECEIVE_MAX_BYTES) rejectOffer(wire)
            upperBound += if (declared == -1L) RECEIVE_MAX_BYTES else declared
            if (upperBound > RECEIVE_MAX_BATCH_BYTES) rejectOffer(wire)
        }
        checkNetwork()
        wire.check()
        onOffer(lengths.size)
        wire.write(0x0a)

        val data = ByteArray(65_536)
        var completedBytes = 0L
        for ((index, declared) in lengths.withIndex()) {
            completedBytes += receiveItem(wire, index, declared, data, onSaved) { onProgress(completedBytes + it) }
        }
    }

    private fun rejectOffer(wire: Wire): Nothing {
        try {
            wire.write(0x07)
        } catch (_: IOException) {
        }
        badFrame()
    }

    private fun receiveItem(
        wire: Wire,
        index: Int,
        declared: Long,
        data: ByteArray,
        onSaved: (ReceivedFile) -> Unit,
        onProgress: (Long) -> Unit,
    ): Long {
        val (type, length) = wire.header()
        if (type != 0x08 || length !in 0..RECEIVE_NAME_BYTES) badFrame()
        val name = try {
            wire.read(length).decodeToString(throwOnInvalidSequence = true)
        } catch (_: CharacterCodingException) {
            badFrame()
        }
        val limit = if (declared == -1L) RECEIVE_MAX_BYTES else declared
        val prefix = ByteArray(12)
        val digest = MessageDigest.getInstance("SHA-256")
        var bytes = 0L
        wire.check()
        val staging = File.createTempFile(".receive-", ".tmp", folder)
        try {
            val end = staging.outputStream().use { output ->
                var frame = wire.header()
                while (frame.first == 0x04) {
                    val chunk = frame.second
                    if (chunk !in 1..data.size || bytes + chunk > limit) badFrame()
                    wire.readInto(data, chunk)
                    output.write(data, 0, chunk)
                    digest.update(data, 0, chunk)
                    if (bytes < prefix.size) {
                        val offset = bytes.toInt()
                        data.copyInto(prefix, offset, 0, minOf(prefix.size - offset, chunk))
                    }
                    bytes += chunk
                    onProgress(bytes)
                    frame = wire.header()
                }
                if (frame != (0x05 to RECEIVE_RECEIPT_BYTES)) badFrame()
                val receipt = wire.read(RECEIVE_RECEIPT_BYTES)
                val fields = ByteBuffer.wrap(receipt)
                if (fields.short.toInt() != index || fields.long != bytes ||
                    (declared != -1L && declared != bytes) ||
                    !MessageDigest.isEqual(receipt.copyOfRange(fields.position(), RECEIVE_RECEIPT_BYTES), digest.digest())
                ) badFrame()
                wire.check()
                output.fd.sync()
                receipt
            }
            checkNetwork()
            val target = publish(staging, destinationName(name), wire)
            onSaved(ReceivedFile(target, receivedMime(prefix, bytes), bytes))
            staging.delete()
            syncDirectory(folder)
            wire.write(0x06, end)
            return bytes
        } finally {
            staging.delete()
        }
    }

    private fun publish(staging: File, name: String, wire: Wire): File {
        val dot = name.lastIndexOf('.').let { if (it < 0) name.length else it }
        val stem = name.substring(0, dot)
        val extension = name.substring(dot)
        for (number in 1..1_000) {
            val filename = if (number == 1) name else "$stem ($number)$extension"
            val target = File(folder, filename)
            try {
                val publish = {
                    synchronized(lock) {
                        wire.check()
                        Os.link(staging.path, target.path)
                    }
                }
                if (publishWhileAllowed != null) publishWhileAllowed.invoke(publish) else publish()
            } catch (failure: ErrnoException) {
                if (failure.errno == OsConstants.EEXIST) continue
                throw IOException("Cannot publish received file", failure)
            }
            return target
        }
        throw IOException("No available receive filename")
    }

    private class Attempt(val raw: Socket, var total: Long) {
        var idle = Long.MAX_VALUE
        var timedOut = false
        val deadline get() = minOf(total, idle)
    }

    private inner class Wire(private val socket: SSLSocket, private val attempt: Attempt) {
        private val input: InputStream = socket.inputStream
        private val header = ByteArray(HEADER_BYTES)

        fun check() = checkAttempt(attempt)

        fun readTimeout(milliseconds: Long) {
            socket.soTimeout = milliseconds.toInt()
        }

        fun read(length: Int): ByteArray = ByteArray(length).also { readInto(it, length) }

        fun readInto(data: ByteArray, length: Int) {
            var offset = 0
            while (offset < length) {
                check()
                val count = input.read(data, offset, length - offset)
                if (count <= 0) badFrame()
                offset += count
                checkAttempt(attempt, madeProgress = true)
            }
        }

        fun header(): Pair<Int, Int> {
            readInto(header, header.size)
            return (header[0].toInt() and 0xff) to ByteBuffer.wrap(header).getInt(1)
        }

        fun readFrame(type: Int, length: Int): ByteArray {
            if (header() != (type to length)) badFrame()
            return read(length)
        }

        fun write(type: Int, payload: ByteArray = ByteArray(0)) {
            check()
            writePacket(socket.outputStream, ByteBuffer.allocate(HEADER_BYTES + payload.size)
                .put(type.toByte()).putInt(payload.size).put(payload).array())
            checkAttempt(attempt, madeProgress = true)
        }
    }

    companion object {
        fun start(context: Context): LanReceive {
            val manager = context.getSystemService(ConnectivityManager::class.java)
                ?: throw IOException("Connectivity manager unavailable")
            val network = manager.activeNetwork ?: throw IOException("Active private Wi-Fi network unavailable")
            if (!isWlan(manager.getNetworkCapabilities(network))) throw IOException("Active private Wi-Fi network unavailable")
            val address = wlanAddresses(manager.getLinkProperties(network)).firstOrNull()
                ?: throw IOException("Active private Wi-Fi address unavailable")
            return startBound(context, manager, network, address, null)
        }

        fun start(
            context: Context, manager: ConnectivityManager, network: Network, address: Inet4Address,
            publishWhileAllowed: (() -> Unit) -> Unit,
        ): LanReceive = startBound(context, manager, network, address, publishWhileAllowed)

        private fun startBound(
            context: Context, manager: ConnectivityManager, network: Network, address: Inet4Address,
            publishWhileAllowed: ((() -> Unit) -> Unit)?,
        ): LanReceive {
            val folder = receiveDirectory(context.filesDir)
            val random = SecureRandom()
            val (tls, pin) = receiverTls(address, random)
            val secret = ByteArray(32).also(random::nextBytes)
            val listener = ServerSocket()
            var session: LanReceive? = null
            try {
                listener.bind(InetSocketAddress(address, 0), MAX_ATTEMPTS)
                val packed = ByteBuffer.allocate(70).put(address.address).putShort(listener.localPort.toShort())
                    .put(pin).put(secret).array()
                val invite = try {
                    "SEAM4:" + Base64.encodeToString(packed, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
                } finally {
                    packed.fill(0)
                }
                val receive = LanReceive(manager, network, address, folder, listener, tls, secret, invite, publishWhileAllowed)
                session = receive
                receive.startWatching()
                return receive
            } catch (failure: Throwable) {
                if (session != null) {
                    session.close()
                } else {
                    secret.fill(0)
                    closeQuietly(listener)
                }
                throw failure
            }
        }
    }
}

private const val RECEIVE_TLS = "TLSv1.3"
private const val INVITE_MS = 120_000L
private const val AUTH_MS = 5_000L
private const val RECEIVE_IDLE_MS = IDLE_MS
private const val RECEIVE_TOTAL_MS = TOTAL_MS
private const val RECEIVE_MAX_BYTES = TRANSFER_MAX_BYTES
private const val RECEIVE_MAX_ITEMS = 32
private const val RECEIVE_MAX_BATCH_BYTES = 256L * 1024 * 1024
private const val RECEIVE_RECEIPT_BYTES = 42
private const val RECEIVE_NAME_BYTES = 180
private const val MAX_ATTEMPTS = 8

private fun elapsed() = SystemClock.elapsedRealtime()

private fun badFrame(): Nothing = throw IOException("Invalid Sliploom receive envelope")

private fun closeQuietly(resource: Closeable?) {
    try {
        resource?.close()
    } catch (_: IOException) {
    }
}

private fun Socket.abort() {
    try {
        shutdownInput()
    } catch (_: IOException) {
    }
    try {
        shutdownOutput()
    } catch (_: IOException) {
    }
    closeQuietly(this)
}

internal fun InetAddress.isPrivateV4(): Boolean {
    if (this !is Inet4Address) return false
    val first = address[0].toInt() and 255
    val second = address[1].toInt() and 255
    return first == 10 || (first == 172 && second in 16..31) || (first == 192 && second == 168)
}

internal fun isWlan(capabilities: NetworkCapabilities?): Boolean = capabilities != null &&
    capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) &&
    capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN) &&
    !capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN) &&
    !capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)

internal fun wlanAddresses(properties: LinkProperties?): List<Inet4Address> =
    properties?.linkAddresses.orEmpty().mapNotNull { (it.address as? Inet4Address)?.takeIf { ip -> ip.isPrivateV4() } }

private fun receiverTls(address: Inet4Address, random: SecureRandom): Pair<SSLContext, ByteArray> {
    val tls = try {
        SSLContext.getInstance(RECEIVE_TLS)
    } catch (failure: java.security.NoSuchAlgorithmException) {
        throw IOException("TLS 1.3 unavailable", failure)
    }
    val keys = KeyPairGenerator.getInstance("EC").apply {
        initialize(ECGenParameterSpec("secp256r1"), random)
    }.generateKeyPair()
    val now = System.currentTimeMillis()
    val name = X500Name("CN=SEAM")
    val der = JcaX509v3CertificateBuilder(
        name, BigInteger(128, random).setBit(127), Date(now - 120_000), Date(now + 58 * 60_000), name, keys.public,
    ).addExtension(
        Extension.subjectAlternativeName, false,
        GeneralNames(GeneralName(GeneralName.iPAddress, address.hostAddress)),
    ).build(JcaContentSignerBuilder("SHA256withECDSA").setSecureRandom(random).build(keys.private)).encoded
    val cert = CertificateFactory.getInstance("X.509").generateCertificate(der.inputStream())
    val store = KeyStore.getInstance(KeyStore.getDefaultType()).apply {
        load(null, null)
        setKeyEntry("seam", keys.private, CharArray(0), arrayOf(cert))
    }
    val managers = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm()).apply {
        init(store, CharArray(0))
    }
    tls.init(managers.keyManagers, null, random)
    if (RECEIVE_TLS !in tls.supportedSSLParameters.protocols) throw IOException("TLS 1.3 unavailable")
    return tls to MessageDigest.getInstance("SHA-256").digest(cert.encoded)
}

private fun receiveDirectory(filesDir: File): File = File(filesDir, "received").also { folder ->
    try {
        Os.mkdir(folder.path, OsConstants.S_IRWXU)
    } catch (failure: ErrnoException) {
        if (failure.errno != OsConstants.EEXIST) throw IOException("Cannot create receive directory", failure)
    }
    syncDirectory(folder)
    syncDirectory(filesDir)
}

private fun syncDirectory(folder: File) {
    try {
        val fd = Os.open(folder.path, OsConstants.O_NOFOLLOW or OsConstants.O_RDONLY or OsConstants.O_NONBLOCK, 0)
        try {
            if (!OsConstants.S_ISDIR(Os.fstat(fd).st_mode)) throw IOException("Invalid receive directory")
            Os.fsync(fd)
        } finally {
            Os.close(fd)
        }
    } catch (failure: ErrnoException) {
        throw IOException("Cannot sync receive directory", failure)
    }
}

private fun receivedMime(prefix: ByteArray, bytes: Long): String {
    fun has(vararg signature: Int, offset: Int = 0): Boolean = bytes >= offset + signature.size &&
        signature.indices.all { (prefix[offset + it].toInt() and 255) == signature[it] }
    fun ascii(text: String, offset: Int = 0): Boolean = bytes >= offset + text.length &&
        text.indices.all { prefix[offset + it].toInt() == text[it].code }
    return when {
        ascii("%PDF-") -> "application/pdf"
        has(137, 80, 78, 71, 13, 10, 26, 10) -> "image/png"
        has(255, 216, 255) -> "image/jpeg"
        ascii("GIF87a") || ascii("GIF89a") -> "image/gif"
        ascii("RIFF") && ascii("WEBP", 8) -> "image/webp"
        ascii("BM") -> "image/bmp"
        has(73, 73, 42, 0) || has(73, 73, 43, 0) || has(77, 77, 0, 42) || has(77, 77, 0, 43) -> "image/tiff"
        has(0, 0, 1, 0) -> "image/vnd.microsoft.icon"
        ascii("ftypavif", 4) || ascii("ftypavis", 4) -> "image/avif"
        ascii("ftypheic", 4) || ascii("ftypheix", 4) -> "image/heic"
        else -> "application/octet-stream"
    }
}

private fun destinationName(name: String): String {
    val device = name.substringBefore('.').trimEnd(' ').uppercase()
    val reserved = device in setOf("CON", "PRN", "AUX", "NUL", "CLOCK$", "CONIN$", "CONOUT$") ||
        (device.length == 4 && (device.startsWith("COM") || device.startsWith("LPT")) && device.last() in "123456789¹²³")
    if (name.isNotEmpty() && name.first() !in ". " && name.last() !in ". " && !reserved && portableCharacters(name)) {
        return name
    }
    val extension = name.substringAfterLast('.', "")
    return if (extension.isNotEmpty() && extension.last() != ' ' && portableCharacters(extension) &&
        extension.encodeToByteArray().size <= RECEIVE_NAME_BYTES - "file.".length
    ) "file.$extension" else "file"
}

private fun portableCharacters(name: String): Boolean = name.none {
    it.isISOControl() || it in "/\\:\"<>|?*" || it in "\u061c\u200e\u200f" ||
        it in '\u202a'..'\u202e' || it in '\u2066'..'\u206f'
}
