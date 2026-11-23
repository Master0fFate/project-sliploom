package org.projectseam.android

import android.os.SystemClock
import android.util.Base64
import kotlinx.coroutines.CancellationException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Proxy
import java.net.Socket
import java.net.SocketTimeoutException
import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.security.MessageDigest
import java.security.NoSuchAlgorithmException
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket
import javax.net.ssl.X509TrustManager

internal class LanInvite private constructor(
    internal val endpoint: InetSocketAddress,
    private val pin: ByteArray,
    private val token: ByteArray,
) {
    private val host = checkNotNull(endpoint.address.hostAddress)
    val address: String = "$host:${endpoint.port}"

    internal fun tlsFactory() = tlsContext().apply {
        init(emptyArray(), arrayOf(object : X509TrustManager {
            override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
                val leaf = chain.firstOrNull() ?: throw CertificateException()
                if (!MessageDigest.isEqual(pin, sha256().digest(leaf.encoded))) {
                    throw CertificateException()
                }
                leaf.checkValidity()
                val matches = leaf.subjectAlternativeNames?.any { san ->
                    san.getOrNull(0) == 7 && when (val ip = san.getOrNull(1)) {
                        is String -> ip == host
                        is ByteArray -> ip.contentEquals(endpoint.address.address)
                        else -> false
                    }
                } == true
                if (!matches) throw CertificateException()
            }

            override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) {
                throw CertificateException()
            }

            override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
        }), null)
        if (TLS_VERSION !in supportedSSLParameters.protocols) throw TlsUnavailableException()
    }.socketFactory

    internal fun writeAuth(output: OutputStream) {
        val packet = ByteBuffer.allocate(5 + HEADER_BYTES + token.size)
            .put(byteArrayOf(0x53, 0x45, 0x41, 0x4d, 0x02))
            .put(0x01).putInt(token.size).put(token).array()
        try {
            output.write(packet)
            output.flush()
        } finally {
            packet.fill(0)
        }
    }

    companion object {
        private const val PREFIX = "SEAM2:"
        private const val FLAGS = Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING

        fun parse(input: String): LanInvite? {
            if (input.length != PREFIX.length + 94 || !input.startsWith(PREFIX)) return null
            val encoded = input.substring(PREFIX.length)
            val bytes = try {
                Base64.decode(encoded, FLAGS)
            } catch (_: IllegalArgumentException) {
                return null
            }
            try {
                if (bytes.size != 70 || Base64.encodeToString(bytes, FLAGS) != encoded) return null
                val first = bytes[0].toInt() and 0xff
                val second = bytes[1].toInt() and 0xff
                if (first != 10 && !(first == 172 && second in 16..31) &&
                    !(first == 192 && second == 168)
                ) return null
                val port = ((bytes[4].toInt() and 0xff) shl 8) or (bytes[5].toInt() and 0xff)
                if (port == 0) return null
                val endpoint = InetSocketAddress(InetAddress.getByAddress(bytes.copyOfRange(0, 4)), port)
                return LanInvite(endpoint, bytes.copyOfRange(6, 38), bytes.copyOfRange(38, 70))
            } finally {
                bytes.fill(0)
            }
        }
    }
}

internal sealed interface TransferResult {
    data class Saved(val bytes: Long) : TransferResult
    data object Unconfirmed : TransferResult
    data object TooLarge : TransferResult
    data object Failed : TransferResult
    data object TlsUnavailable : TransferResult
    data object CableUnavailable : TransferResult
    data object Declined : TransferResult
    data class Stopped(val reason: TransferStop, val afterEnd: Boolean) : TransferResult
}

internal enum class TransferStop { Cancelled, TimedOut }

internal class LanTransfer(private val invite: LanInvite, private val onStop: (TransferStop) -> Unit) {
    private val lock = Object()
    private var started = false
    private var finished = false
    private var stopReason: TransferStop? = null
    private var endStarted = false
    private var totalDeadline = 0L
    private var phaseDeadline = 0L

    fun cancel() = synchronized(lock) {
        stopLocked(if (totalDeadline != 0L && remainingMs() <= 0) TransferStop.TimedOut else TransferStop.Cancelled)
    }

    fun send(
        input: InputStream,
        declaredLength: Long,
        isPdf: Boolean,
        displayName: () -> String?,
        onProgress: (Long) -> Unit,
    ): TransferResult {
        synchronized(lock) {
            if (started) return TransferResult.Failed
            started = true
            stopReason?.let { return TransferResult.Stopped(it, afterEnd = false) }
            val now = SystemClock.elapsedRealtime()
            totalDeadline = now + TOTAL_MS
            phaseDeadline = now + AUTH_MS
        }
        var raw: Socket? = null
        var tls: SSLSocket? = null
        var watchdog: Thread? = null
        try {
            if (declaredLength > TRANSFER_MAX_BYTES) return complete(TransferResult.TooLarge)
            if (declaredLength < -1) return complete(TransferResult.Failed)
            val socket = Socket(Proxy.NO_PROXY).also { raw = it }
            watchdog = Thread({ watch(socket) }, "SEAM transfer timeout").apply {
                isDaemon = true
                start()
            }
            val factory = invite.tlsFactory()
            checkActive()
            socket.connect(invite.endpoint, AUTH_MS.toInt())
            checkActive()
            val secure = (factory.createSocket(
                socket, invite.endpoint.address.hostAddress, invite.endpoint.port, true,
            ) as SSLSocket).also { tls = it }
            secure.useClientMode = true
            if (TLS_VERSION !in secure.supportedProtocols) throw TlsUnavailableException()
            secure.enabledProtocols = arrayOf(TLS_VERSION)
            secure.soTimeout = AUTH_MS.toInt()
            secure.startHandshake()
            if (secure.session.protocol != TLS_VERSION) throw IOException()
            val output = secure.outputStream
            val response = secure.inputStream
            checkActive()
            invite.writeAuth(output)
            val header = ByteArray(HEADER_BYTES)
            readFrameBytes(response, header, ::checkActive)
            requireHeader(header, 0x02, 0)
            progress()
            secure.soTimeout = IDLE_MS.toInt()
            checkActive()
            writePacket(output, beginFrame(isPdf, declaredLength))
            progress()
            writeOptionalName(output, displayName, ::checkActive)
            return complete(pumpTransfer(
                input, declaredLength, output, response, ::checkActive, ::progress, ::startEnd, onProgress,
            ))
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: SocketTimeoutException) {
            return complete(TransferResult.Failed, socketTimedOut = true)
        } catch (_: TlsUnavailableException) {
            return complete(TransferResult.TlsUnavailable)
        } catch (_: Exception) {
            return complete(TransferResult.Failed)
        } finally {
            synchronized(lock) {
                finished = true
                lock.notifyAll()
            }
            closeQuietly(raw)
            closeQuietly(tls)
            watchdog?.join()
        }
    }

    private fun startEnd() = synchronized(lock) {
        checkActiveLocked()
        endStarted = true
    }

    private fun complete(result: TransferResult, socketTimedOut: Boolean = false): TransferResult = synchronized(lock) {
        if (result !is TransferResult.Saved && (socketTimedOut || remainingMs() <= 0)) {
            stopLocked(TransferStop.TimedOut)
        }
        finished = true
        lock.notifyAll()
        when {
            result is TransferResult.Saved -> result
            stopReason != null -> TransferResult.Stopped(checkNotNull(stopReason), endStarted)
            endStarted -> TransferResult.Unconfirmed
            else -> result
        }
    }

    private fun stopLocked(reason: TransferStop) {
        if (finished || stopReason != null) return
        stopReason = reason
        lock.notifyAll()
        onStop(reason)
    }

    private fun remainingMs() = minOf(totalDeadline, phaseDeadline) - SystemClock.elapsedRealtime()

    private fun checkActive() = synchronized(lock) { checkActiveLocked() }

    private fun checkActiveLocked() {
        if (remainingMs() <= 0) stopLocked(TransferStop.TimedOut)
        if (stopReason != null) throw IOException()
    }

    private fun progress() {
        synchronized(lock) {
            checkActiveLocked()
            phaseDeadline = SystemClock.elapsedRealtime() + IDLE_MS
            lock.notifyAll()
        }
    }

    private fun watch(socket: Socket) {
        synchronized(lock) {
            while (!finished && stopReason == null) {
                val remaining = remainingMs()
                if (remaining <= 0) {
                    stopLocked(TransferStop.TimedOut)
                    break
                }
                lock.wait(remaining)
            }
            if (finished) return
        }
        closeQuietly(socket)
    }
}

private const val TLS_VERSION = "TLSv1.3"
private const val AUTH_MS = 5_000L
internal const val IDLE_MS = 10_000L
internal const val TOTAL_MS = 300_000L
internal const val TRANSFER_MAX_BYTES = 64L * 1024 * 1024
private const val CHUNK_BYTES = 65_536
internal const val HEADER_BYTES = 5
private const val RECEIPT_BYTES = 40
private const val MAX_NAME_BYTES = 180

private class TlsUnavailableException : IOException()

private fun tlsContext() = try {
    SSLContext.getInstance(TLS_VERSION)
} catch (_: NoSuchAlgorithmException) {
    throw TlsUnavailableException()
}

private fun sha256(): MessageDigest = MessageDigest.getInstance("SHA-256")

internal fun beginFrame(isPdf: Boolean, declaredLength: Long): ByteArray = ByteBuffer.allocate(HEADER_BYTES + 9)
    .put(0x03).putInt(9).put(if (isPdf) 0x02 else 0x01).putLong(declaredLength).array()

internal fun writeOptionalName(output: OutputStream, displayName: () -> String?, checkActive: () -> Unit) {
    checkActive()
    val name = displayName()
    checkActive()
    if (name == null || name.length !in 1..MAX_NAME_BYTES) return
    if (name.any { it == '/' || it == '\\' || it == ':' || it.isISOControl() }) return
    val bytes = try {
        name.encodeToByteArray(throwOnInvalidSequence = true)
    } catch (_: CharacterCodingException) {
        return
    }
    if (bytes.size !in 1..MAX_NAME_BYTES) return
    writePacket(output, ByteBuffer.allocate(HEADER_BYTES + bytes.size)
        .put(0x08).putInt(bytes.size).put(bytes).array())
    checkActive()
}

internal fun pumpTransfer(
    input: InputStream,
    declaredLength: Long,
    output: OutputStream,
    response: InputStream,
    checkActive: () -> Unit,
    progress: () -> Unit,
    startEnd: () -> Unit,
    onProgress: (Long) -> Unit,
): TransferResult {
    val hash = sha256()
    val packet = ByteBuffer.allocate(HEADER_BYTES + CHUNK_BYTES)
    val data = packet.array()
    val limit = if (declaredLength == -1L) TRANSFER_MAX_BYTES else declaredLength
    var sent = 0L
    while (true) {
        checkActive()
        val requested = minOf(CHUNK_BYTES.toLong(), limit - sent + 1).toInt()
        var read = input.read(data, HEADER_BYTES, requested)
        if (read == 0) {
            checkActive()
            val byte = input.read()
            read = if (byte == -1) -1 else {
                data[HEADER_BYTES] = byte.toByte()
                1
            }
        }
        checkActive()
        if (read == -1) break
        if (read !in 1..requested) throw IOException()
        val next = sent + read
        if (next > TRANSFER_MAX_BYTES) return TransferResult.TooLarge
        if (next > limit) return TransferResult.Failed
        packet.put(0, 0x04).putInt(1, read)
        hash.update(data, HEADER_BYTES, read)
        writePacket(output, data, HEADER_BYTES + read)
        sent = next
        progress()
        onProgress(sent)
    }
    if (declaredLength != -1L && sent != declaredLength) return TransferResult.Failed
    val receipt = ByteBuffer.allocate(RECEIPT_BYTES).putLong(sent).put(hash.digest()).array()
    val end = ByteBuffer.allocate(HEADER_BYTES + RECEIPT_BYTES)
        .put(0x05).putInt(RECEIPT_BYTES).put(receipt).array()
    startEnd()
    writePacket(output, end)
    progress()
    val header = ByteArray(HEADER_BYTES)
    readFrameBytes(response, header, checkActive, progress)
    requireHeader(header, 0x06, RECEIPT_BYTES)
    progress()
    val committed = ByteArray(RECEIPT_BYTES)
    readFrameBytes(response, committed, checkActive, progress)
    return if (MessageDigest.isEqual(receipt, committed)) TransferResult.Saved(sent) else TransferResult.Unconfirmed
}

internal fun readFrameBytes(
    input: InputStream,
    buffer: ByteArray,
    checkActive: () -> Unit,
    onPartial: () -> Unit = {},
) {
    var offset = 0
    while (offset < buffer.size) {
        checkActive()
        val count = input.read(buffer, offset, buffer.size - offset)
        if (count <= 0) throw IOException()
        offset += count
        if (offset < buffer.size) onPartial()
    }
}

internal fun writePacket(output: OutputStream, bytes: ByteArray, length: Int = bytes.size) {
    output.write(bytes, 0, length)
    output.flush()
}

internal fun matchesHeader(header: ByteArray, type: Byte, length: Int): Boolean =
    header[0] == type && ByteBuffer.wrap(header).getInt(1) == length

private fun requireHeader(header: ByteArray, type: Byte, length: Int) {
    if (!matchesHeader(header, type, length)) throw IOException()
}

private fun closeQuietly(socket: Socket?) {
    try {
        socket?.close()
    } catch (_: IOException) {
    }
}
