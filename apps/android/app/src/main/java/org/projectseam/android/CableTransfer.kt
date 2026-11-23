package org.projectseam.android

import android.net.LocalSocket
import android.net.LocalSocketAddress
import android.os.SystemClock
import kotlinx.coroutines.CancellationException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

internal class CableTransfer(private val onStop: (TransferStop) -> Unit) {
    class Payload(val length: Long, val open: () -> InputStream)

    private val lock = Object()
    private var started = false
    private var finished = false
    private var stopReason: TransferStop? = null
    private var endStarted = false
    private var totalDeadline = 0L
    private var phaseDeadline = 0L

    fun cancel() = synchronized(lock) {
        stopLocked(if (started && remainingMs() <= 0) TransferStop.TimedOut else TransferStop.Cancelled)
    }

    fun send(
        isPdf: Boolean,
        prepare: () -> Payload,
        displayName: () -> String?,
        onApproved: () -> Unit,
        onProgress: (Long) -> Unit,
    ): TransferResult {
        synchronized(lock) {
            if (started) return TransferResult.Failed
            stopReason?.let { return TransferResult.Stopped(it, afterEnd = false) }
            started = true
            val now = SystemClock.elapsedRealtime()
            totalDeadline = now + TOTAL_MS
            phaseDeadline = now + IDLE_MS
        }
        val socket = LocalSocket()
        val watchdog = Thread({ watch(socket) }, "SEAM cable timeout").apply {
            isDaemon = true
            start()
        }
        try {
            return complete(offer(socket, isPdf, prepare, displayName, onApproved, onProgress))
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: IOException) {
            return complete(TransferResult.Failed)
        } catch (_: RuntimeException) {
            return complete(TransferResult.Failed)
        } finally {
            synchronized(lock) {
                finished = true
                lock.notifyAll()
            }
            watchdog.join()
        }
    }

    private fun offer(
        socket: LocalSocket,
        isPdf: Boolean,
        prepare: () -> Payload,
        displayName: () -> String?,
        onApproved: () -> Unit,
        onProgress: (Long) -> Unit,
    ): TransferResult {
        val output: OutputStream
        val response: InputStream
        try {
            checkActive()
            socket.connect(LocalSocketAddress(CABLE_NAME, LocalSocketAddress.Namespace.ABSTRACT))
            if (socket.peerCredentials.uid != ADB_SHELL_UID) return TransferResult.CableUnavailable
            output = socket.outputStream
            response = socket.inputStream
        } catch (_: IOException) {
            return TransferResult.CableUnavailable
        }
        checkActive()
        val payload = prepare()
        if (payload.length > TRANSFER_MAX_BYTES) return TransferResult.TooLarge
        if (payload.length < -1) return TransferResult.Failed
        val header = ByteArray(HEADER_BYTES)
        setPhase(APPROVAL_MS)
        try {
            writePacket(output, byteArrayOf(0x53, 0x45, 0x41, 0x4D, 0x03) + beginFrame(isPdf, payload.length))
            readFrameBytes(response, header, ::checkActive)
        } catch (_: IOException) {
            return TransferResult.CableUnavailable
        }
        if (matchesHeader(header, 0x07, 0)) return TransferResult.Declined
        if (!matchesHeader(header, 0x02, 0)) return TransferResult.Failed
        progress()
        onApproved()
        writeOptionalName(output, displayName, ::checkActive)
        val input = payload.open()
        return pumpTransfer(
            input, payload.length, output, response, ::checkActive, ::progress, ::startEnd, onProgress,
        )
    }

    private fun startEnd() = synchronized(lock) {
        checkActiveLocked()
        endStarted = true
    }

    private fun complete(result: TransferResult): TransferResult = synchronized(lock) {
        if (result !is TransferResult.Saved && remainingMs() <= 0) stopLocked(TransferStop.TimedOut)
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

    private fun setPhase(durationMs: Long) = synchronized(lock) {
        checkActiveLocked()
        phaseDeadline = SystemClock.elapsedRealtime() + durationMs
        lock.notifyAll()
    }

    private fun watch(socket: LocalSocket) {
        synchronized(lock) {
            while (!finished && stopReason == null) {
                val remaining = remainingMs()
                if (remaining <= 0) {
                    stopLocked(TransferStop.TimedOut)
                    break
                }
                lock.wait(remaining)
            }
        }
        runCatching { socket.shutdownInput() }
        runCatching { socket.shutdownOutput() }
        runCatching { socket.close() }
    }
}

private const val CABLE_NAME = "org.projectseam.android.cable.v1"
private const val ADB_SHELL_UID = 2000
private const val APPROVAL_MS = 45_000L
