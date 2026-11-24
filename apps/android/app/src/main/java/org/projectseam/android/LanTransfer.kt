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
    val isBatch: Boolean,
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
            .put(byteArrayOf(0x53, 0x45, 0x41, 0x4d, if (isBatch) 0x04 else 0x02))
            .put(0x01).putInt(token.size).put(token).array()
        try {
            output.write(packet)
            output.flush()
        } finally {
            packet.fill(0)
        }
    }

    companion object {
        private const val BATCH_PREFIX = "SEAM4:"
        private const val LEGACY_PREFIX = "SEAM2:"
        private const val FLAGS = Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING

        fun parse(input: String): LanInvite? {
            val prefix = when {
                input.startsWith(BATCH_PREFIX) -> BATCH_PREFIX
                input.startsWith(LEGACY_PREFIX) -> LEGACY_PREFIX
                else -> return null
            }
            if (input.length != prefix.length + 94) return null
            val encoded = input.substring(prefix.length)
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
                return LanInvite(endpoint, prefix == BATCH_PREFIX, bytes.copyOfRange(6, 38), bytes.copyOfRange(38, 70))
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

    class LegacyItem(val input: InputStream, val length: Long, val isPdf: Boolean, val name: String?)

    data class BatchResult(val result: TransferResult, val committed: Int)

    fun sendLegacy(
        prepare: () -> LegacyItem,
        onAccepted: () -> Unit,
        onProgress: (Long) -> Unit,
    ): TransferResult {
        check(!invite.isBatch)
        var item: LegacyItem? = null
        return runTransfer(prepare = {
            val prepared = prepare()
            item = prepared
            when {
                prepared.length > TRANSFER_MAX_BYTES -> TransferResult.TooLarge
                prepared.length < -1 -> TransferResult.Failed
                else -> null
            }
        }) { _, output, response ->
            val prepared = checkNotNull(item)
            onAccepted()
            checkActive()
            writePacket(output, beginFrame(prepared.isPdf, prepared.length))
            progress()
            writeOptionalName(output, { prepared.name }, ::checkActive)
            pumpTransfer(
                prepared.input, prepared.length, output, response, ::checkActive, ::progress, ::startEnd, onProgress,
            )
        }
    }

    fun sendBatch(
        count: Int,
        queryLength: (Int) -> Long,
        displayName: (Int) -> String?,
        openItem: (Int) -> InputStream,
        closeItem: () -> Unit,
        onOffered: () -> Unit,
        onApproved: () -> Unit,
        onProgress: (Long) -> Unit,
        onCommitted: (Int) -> Unit,
    ): BatchResult {
        check(invite.isBatch)
        require(count in 1..SEND_BATCH_MAX_ITEMS)
        val lengths = LongArray(count)
        var committed = 0
        val result = runTransfer(prepare = {
            var reserved = 0L
            for (index in lengths.indices) {
                checkActive()
                val length = queryLength(index)
                progress()
                if (length < -1) return@runTransfer TransferResult.Failed
                if (length > TRANSFER_MAX_BYTES) return@runTransfer TransferResult.TooLarge
                reserved += if (length == -1L) TRANSFER_MAX_BYTES else length
                if (reserved > BATCH_MAX_BYTES) return@runTransfer TransferResult.TooLarge
                lengths[index] = length
            }
            null
        }) { secure, output, response ->
            val offer = ByteBuffer.allocate(HEADER_BYTES + Long.SIZE_BYTES * count)
                .put(0x09).putInt(Long.SIZE_BYTES * count)
            lengths.forEach { offer.putLong(it) }
            checkActive()
            writePacket(output, offer.array())
            setPhase(APPROVAL_WAIT_MS)
            secure.soTimeout = APPROVAL_WAIT_MS.toInt()
            onOffered()
            val reply = ByteArray(HEADER_BYTES)
            readFrameBytes(response, reply, ::checkActive)
            if (matchesHeader(reply, 0x07, 0)) return@runTransfer TransferResult.Declined
            requireHeader(reply, 0x0a, 0)
            progress()
            secure.soTimeout = IDLE_MS.toInt()
            onApproved()

            var committedBytes = 0L
            for (index in lengths.indices) {
                checkActive()
                try {
                    val name = batchName(displayName(index))
                    checkActive()
                    val input = openItem(index)
                    checkActive()
                    writePacket(output, ByteBuffer.allocate(HEADER_BYTES + name.size)
                        .put(0x08).putInt(name.size).put(name).array())
                    progress()
                    val item = pumpTransfer(
                        input, lengths[index], output, response, ::checkActive, ::progress, ::startEnd,
                        onProgress = { onProgress(committedBytes + it) }, index = index,
                    )
                    if (item !is TransferResult.Saved) return@runTransfer item
                    synchronized(lock) { endStarted = false }
                    committed++
                    committedBytes += item.bytes
                    onCommitted(committed)
                } finally {
                    closeItem()
                }
                if (committed != count) progress()
            }
            TransferResult.Saved(committedBytes)
        }
        return BatchResult(result, committed)
    }

    private fun runTransfer(
        prepare: () -> TransferResult?,
        transfer: (SSLSocket, OutputStream, InputStream) -> TransferResult,
    ): TransferResult {
        synchronized(lock) {
            if (started) return TransferResult.Failed
            started = true
            stopReason?.let { return TransferResult.Stopped(it, afterEnd = false) }
            val now = SystemClock.elapsedRealtime()
            totalDeadline = now + TOTAL_MS
            phaseDeadline = now + IDLE_MS
        }
        var raw: Socket? = null
        var tls: SSLSocket? = null
        var watchdog: Thread? = null
        try {
            val socket = Socket(Proxy.NO_PROXY).also { raw = it }
            watchdog = Thread({ watch(socket) }, "Sliploom transfer timeout").apply {
                isDaemon = true
                start()
            }
            prepare()?.let { return complete(it) }
            setPhase(AUTH_MS)
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
            return complete(transfer(secure, output, response))
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

    private fun progress() = setPhase(IDLE_MS)

    private fun setPhase(duration: Long) = synchronized(lock) {
        checkActiveLocked()
        phaseDeadline = SystemClock.elapsedRealtime() + duration
        lock.notifyAll()
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
private const val APPROVAL_WAIT_MS = 60_000L
internal const val SEND_BATCH_MAX_ITEMS = 32
private const val BATCH_MAX_BYTES = 256L * 1024 * 1024
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
    index: Int? = null,
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
    val receiptBytes = RECEIPT_BYTES + if (index == null) 0 else Short.SIZE_BYTES
    val receipt = ByteBuffer.allocate(receiptBytes).apply {
        if (index != null) putShort(index.toShort())
    }.putLong(sent).put(hash.digest()).array()
    val end = ByteBuffer.allocate(HEADER_BYTES + receiptBytes)
        .put(0x05).putInt(receiptBytes).put(receipt).array()
    startEnd()
    writePacket(output, end)
    progress()
    val header = ByteArray(HEADER_BYTES)
    readFrameBytes(response, header, checkActive, progress)
    requireHeader(header, 0x06, receiptBytes)
    progress()
    val committed = ByteArray(receiptBytes)
    readFrameBytes(response, committed, checkActive, progress)
    return if (MessageDigest.isEqual(receipt, committed)) TransferResult.Saved(sent) else TransferResult.Unconfirmed
}

private fun batchName(name: String?): ByteArray {
    if (name.isNullOrEmpty()) return ByteArray(0)
    val device = name.substringBefore('.').trimEnd(' ').uppercase()
    val reserved = device in setOf("CON", "PRN", "AUX", "NUL", "CLOCK$", "CONIN$", "CONOUT$") ||
        (device.startsWith("COM") || device.startsWith("LPT")) && device.length == 4 &&
        device.last() in "123456789¹²³"
    if (name.length <= MAX_NAME_BYTES && !reserved && name.first() !in ". " && name.last() !in ". " &&
        name.none {
            it in "/\\:\"<>|?*" || it.isISOControl() || it in '\u202a'..'\u202e' || it in '\u2066'..'\u206f' ||
                it in "\u061c\u200e\u200f"
        }
    ) {
        val encoded = try {
            name.encodeToByteArray(throwOnInvalidSequence = true)
        } catch (_: CharacterCodingException) {
            null
        }
        if (encoded != null && encoded.size <= MAX_NAME_BYTES) return encoded
    }
    val extension = name.substringAfterLast('.', "")
    return if (extension.length in 1..16 && extension.all { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' }) {
        "file.$extension".encodeToByteArray()
    } else {
        "file".encodeToByteArray()
    }
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
