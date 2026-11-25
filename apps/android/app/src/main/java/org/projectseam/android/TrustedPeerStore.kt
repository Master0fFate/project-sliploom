package org.projectseam.android

import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.File
import java.io.IOException
import java.net.Inet4Address
import java.net.InetAddress
import java.security.MessageDigest

internal const val MAX_TRUSTED_PEERS = 16
private const val TRUST_PLATFORM_WINDOWS = 1
internal const val TRUST_PLATFORM_ANDROID = 2
private const val TRUST_PLATFORM_MACOS = 3
private const val TRUST_PLATFORM_LINUX = 4

internal fun trustPlatformLabel(platform: Int): String? = when (platform) {
    TRUST_PLATFORM_WINDOWS -> "Windows"
    TRUST_PLATFORM_ANDROID -> "Android"
    TRUST_PLATFORM_MACOS -> "macOS"
    TRUST_PLATFORM_LINUX -> "Linux"
    else -> null
}

internal class TrustedRecord(val pin: ByteArray, var address: Inet4Address, val platform: Int) {
    val id = pin.trustHex()
    fun peer() = TrustedPeer(id, address, checkNotNull(trustPlatformLabel(platform)))
}

internal class TrustedPeerStore(private val folder: File) {
    private val file = File(folder, "trusted-devices.v1")

    fun exists(): Boolean = try {
        val stat = Os.lstat(file.path)
        if (!OsConstants.S_ISREG(stat.st_mode)) throw IOException("Invalid paired device store")
        true
    } catch (failure: ErrnoException) {
        if (failure.errno != OsConstants.ENOENT) throw IOException("Cannot inspect paired device store", failure)
        false
    }

    fun read(owner: ByteArray): List<TrustedRecord> {
        if (!exists()) return emptyList()
        return DataInputStream(file.inputStream().buffered()).use { input ->
            if (input.readInt() != FORMAT || !MessageDigest.isEqual(owner, input.readExactly(32))) {
                throw IOException("Paired device store belongs to another identity")
            }
            val count = input.readUnsignedByte()
            if (count > MAX_TRUSTED_PEERS) throw IOException("Invalid paired device count")
            val ids = HashSet<String>()
            val records = List(count) {
                val pin = input.readExactly(32)
                val address = InetAddress.getByAddress(input.readExactly(4)) as Inet4Address
                val platform = input.readUnsignedByte()
                if (MessageDigest.isEqual(owner, pin) || !ids.add(pin.trustHex()) || !address.isPrivateV4() ||
                    trustPlatformLabel(platform) == null
                ) throw IOException("Invalid paired device record")
                TrustedRecord(pin, address, platform)
            }
            if (input.read() != -1) throw IOException("Invalid paired device store")
            records
        }
    }

    fun write(owner: ByteArray, records: Collection<TrustedRecord>) {
        val temp = File.createTempFile(".trust-", ".tmp", folder)
        var renamed = false
        try {
            temp.outputStream().use { stream ->
                val output = DataOutputStream(stream)
                output.writeInt(FORMAT)
                output.write(owner)
                output.writeByte(records.size)
                for (record in records) {
                    output.write(record.pin)
                    output.write(record.address.address)
                    output.writeByte(record.platform)
                }
                output.flush()
                stream.fd.sync()
            }
            val parent = Os.open(folder.path, OsConstants.O_RDONLY or OsConstants.O_NOFOLLOW or OsConstants.O_NONBLOCK, 0)
            try {
                if (!OsConstants.S_ISDIR(Os.fstat(parent).st_mode)) throw IOException("Invalid paired device store directory")
                Os.rename(temp.path, file.path)
                renamed = true
                Os.fsync(parent)
            } finally {
                Os.close(parent)
            }
        } catch (failure: ErrnoException) {
            if (renamed) throw UncertainWrite(failure)
            throw IOException("Cannot persist paired devices", failure)
        } finally {
            if (!renamed) temp.delete()
        }
    }

    class UncertainWrite(cause: Throwable) : IOException("Cannot confirm paired device store durability", cause)

    private companion object {
        const val FORMAT = 0x53454101
    }
}

private fun DataInputStream.readExactly(size: Int) = ByteArray(size).also(::readFully)
