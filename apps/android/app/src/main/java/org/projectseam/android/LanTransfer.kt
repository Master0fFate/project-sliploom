package org.projectseam.android

import android.os.SystemClock
import android.util.Base64
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Proxy
import java.net.Socket
import java.nio.ByteBuffer
import java.security.MessageDigest
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

    internal fun tlsFactory() = SSLContext.getInstance(TLS_VERSION).apply {
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

internal sealed interface LanResult {
    data class Saved(val bytes: Long) : LanResult
    data object Unconfirmed : LanResult
    data object TooLarge : LanResult
    data object Failed : LanResult
    data object Cancelled : LanResult
}

internal class LanTransfer(private val invite: LanInvite) {
    private val lock = Object()
    private var started = false
    private var finished = false
    private var cancelled = false
    private var endStarted = false
    private var totalDeadline = 0L
    private var phaseDeadline = 0L

    fun cancel() {
        synchronized(lock) {
            cancelled = true
            lock.notifyAll()
        }
    }

    fun send(
        input: InputStream,
        declaredLength: Long,
        isPdf: Boolean,
        onProgress: (Long) -> Unit,
    ): LanResult {
        synchronized(lock) {
            if (started) return LanResult.Failed
            started = true
            if (cancelled) return LanResult.Cancelled
            if (declaredLength > MAX_BYTES) return LanResult.TooLarge
            if (declaredLength < -1) return LanResult.Failed
            val now = SystemClock.elapsedRealtime()
            totalDeadline = now + TOTAL_MS
            phaseDeadline = now + AUTH_MS
        }
        var raw: Socket? = null
        var tls: SSLSocket? = null
        return try {
            val socket = Socket(Proxy.NO_PROXY).also { raw = it }
            Thread({ watch(socket) }, "SEAM transfer timeout").apply {
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
            secure.enabledProtocols = arrayOf(TLS_VERSION)
            secure.soTimeout = AUTH_MS.toInt()
            secure.startHandshake()
            if (secure.session.protocol != TLS_VERSION) throw IOException()
            val output = secure.outputStream
            val response = secure.inputStream
            checkActive()
            invite.writeAuth(output)
            val header = ByteArray(HEADER_BYTES)
            readExactly(response, header, trackIdle = false)
            requireHeader(header, 0x02, 0)
            progress()
            secure.soTimeout = IDLE_MS.toInt()
            transfer(input, declaredLength, isPdf, onProgress, output, response, header)
        } catch (_: Exception) {
            stopped()
        } finally {
            synchronized(lock) {
                finished = true
                lock.notifyAll()
            }
            closeQuietly(raw)
            closeQuietly(tls)
        }
    }

    private fun transfer(
        input: InputStream,
        declaredLength: Long,
        isPdf: Boolean,
        onProgress: (Long) -> Unit,
        output: OutputStream,
        response: InputStream,
        header: ByteArray,
    ): LanResult {
        val begin = ByteBuffer.allocate(HEADER_BYTES + 9)
            .put(0x03).putInt(9).put(if (isPdf) 0x02 else 0x01).putLong(declaredLength).array()
        checkActive()
        writePacket(output, begin)
        progress()

        val hash = sha256()
        val packet = ByteBuffer.allocate(HEADER_BYTES + CHUNK_BYTES)
        val data = packet.array()
        val limit = if (declaredLength == -1L) MAX_BYTES else declaredLength
        var sent = 0L
        while (true) {
            checkActive()
            val requested = minOf(CHUNK_BYTES.toLong(), limit - sent + 1).toInt()
            var read = input.read(data, HEADER_BYTES, requested)
            if (read == 0) {
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
            if (next > MAX_BYTES) return stopped(LanResult.TooLarge)
            if (next > limit) return stopped()
            packet.put(0, 0x04).putInt(1, read)
            hash.update(data, HEADER_BYTES, read)
            writePacket(output, data, HEADER_BYTES + read)
            sent = next
            progress()
            onProgress(sent)
        }
        if (declaredLength != -1L && sent != declaredLength) return stopped()
        val receipt = ByteBuffer.allocate(RECEIPT_BYTES).putLong(sent).put(hash.digest()).array()
        val end = ByteBuffer.allocate(HEADER_BYTES + RECEIPT_BYTES)
            .put(0x05).putInt(RECEIPT_BYTES).put(receipt).array()
        synchronized(lock) {
            checkActiveLocked()
            endStarted = true
        }
        writePacket(output, end)
        progress()
        readExactly(response, header, trackIdle = true)
        requireHeader(header, 0x06, RECEIPT_BYTES)
        progress()
        val committed = ByteArray(RECEIPT_BYTES)
        readExactly(response, committed, trackIdle = true)
        return if (MessageDigest.isEqual(receipt, committed)) LanResult.Saved(sent) else LanResult.Unconfirmed
    }

    private fun readExactly(input: InputStream, buffer: ByteArray, trackIdle: Boolean) {
        var offset = 0
        while (offset < buffer.size) {
            checkActive()
            val count = input.read(buffer, offset, buffer.size - offset)
            if (count <= 0) throw IOException()
            offset += count
            if (trackIdle && offset < buffer.size) progress()
        }
    }

    private fun stopped(beforeEnd: LanResult = LanResult.Failed): LanResult = synchronized(lock) {
        when {
            endStarted -> LanResult.Unconfirmed
            cancelled -> LanResult.Cancelled
            else -> beforeEnd
        }
    }

    private fun checkActive() = synchronized(lock) { checkActiveLocked() }

    private fun checkActiveLocked() {
        if (cancelled || SystemClock.elapsedRealtime() >= minOf(totalDeadline, phaseDeadline)) {
            throw IOException()
        }
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
            while (!finished && !cancelled) {
                val remaining = minOf(totalDeadline, phaseDeadline) - SystemClock.elapsedRealtime()
                if (remaining <= 0) break
                lock.wait(remaining)
            }
            if (finished) return
        }
        closeQuietly(socket)
    }
}

private const val TLS_VERSION = "TLSv1.3"
private const val AUTH_MS = 5_000L
private const val IDLE_MS = 10_000L
private const val TOTAL_MS = 300_000L
private const val MAX_BYTES = 67_108_864L
private const val CHUNK_BYTES = 65_536
private const val HEADER_BYTES = 5
private const val RECEIPT_BYTES = 40

private fun sha256(): MessageDigest = MessageDigest.getInstance("SHA-256")

private fun writePacket(output: OutputStream, bytes: ByteArray, length: Int = bytes.size) {
    output.write(bytes, 0, length)
    output.flush()
}

private fun requireHeader(header: ByteArray, type: Byte, length: Int) {
    if (header[0] != type || ByteBuffer.wrap(header).getInt(1) != length) throw IOException()
}

private fun closeQuietly(socket: Socket?) {
    try {
        socket?.close()
    } catch (_: IOException) {
    }
}
