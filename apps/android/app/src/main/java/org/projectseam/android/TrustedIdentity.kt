package org.projectseam.android

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.io.IOException
import java.net.Socket
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.MessageDigest
import java.security.Principal
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.security.spec.ECGenParameterSpec
import javax.net.ssl.KeyManager
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLEngine
import javax.net.ssl.SSLSocket
import javax.net.ssl.X509ExtendedKeyManager
import javax.net.ssl.X509TrustManager
import javax.security.auth.x500.X500Principal

internal const val TRUST_TLS = "TLSv1.3"
private const val STORE = "AndroidKeyStore"
private const val ALIAS = "project-seam-trust-v1"

internal fun trustSha256(bytes: ByteArray): ByteArray = MessageDigest.getInstance("SHA-256").digest(bytes)

internal fun ByteArray.trustHex(): String = buildString(size * 2) {
    val hex = "0123456789abcdef"
    for (byte in this@trustHex) {
        append(hex[(byte.toInt() and 0xff) ushr 4])
        append(hex[byte.toInt() and 0x0f])
    }
}

internal class TrustedIdentity private constructor(
    private val key: PrivateKey,
    private val cert: X509Certificate,
) {
    val pin: ByteArray = trustSha256(cert.encoded)
    val id: String = pin.trustHex()

    private val manager = object : X509ExtendedKeyManager() {
        private fun isEc(keyType: String?) = keyType == "EC" || keyType?.startsWith("EC_") == true
        override fun getClientAliases(keyType: String?, issuers: Array<out Principal>?): Array<String>? =
            if (isEc(keyType)) arrayOf(ALIAS) else null
        override fun chooseClientAlias(
            keyType: Array<out String>?, issuers: Array<out Principal>?, socket: Socket?,
        ): String? = if (keyType?.any(::isEc) == true) ALIAS else null
        override fun getServerAliases(keyType: String?, issuers: Array<out Principal>?): Array<String>? =
            getClientAliases(keyType, issuers)
        override fun chooseServerAlias(keyType: String?, issuers: Array<out Principal>?, socket: Socket?): String? =
            if (isEc(keyType)) ALIAS else null
        override fun chooseEngineClientAlias(
            keyType: Array<out String>?, issuers: Array<out Principal>?, engine: SSLEngine?,
        ): String? = chooseClientAlias(keyType, issuers, null)
        override fun chooseEngineServerAlias(keyType: String?, issuers: Array<out Principal>?, engine: SSLEngine?): String? =
            chooseServerAlias(keyType, issuers, null)
        override fun getCertificateChain(alias: String?): Array<X509Certificate>? =
            if (alias == ALIAS) arrayOf(cert) else null
        override fun getPrivateKey(alias: String?): PrivateKey? = if (alias == ALIAS) key else null
    }

    fun context(random: SecureRandom, client: Boolean, verify: (ByteArray) -> Unit): SSLContext {
        cert.checkValidity()
        val trust = object : X509TrustManager {
            private fun verifyLeaf(chain: Array<out X509Certificate>?) {
                val leaf = chain?.firstOrNull() ?: throw CertificateException("Missing TLS identity")
                leaf.checkValidity()
                verify(trustSha256(leaf.encoded))
            }

            override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) {
                if (client) throw CertificateException("Unexpected TLS client identity")
                verifyLeaf(chain)
            }

            override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) {
                if (!client) throw CertificateException("Unexpected TLS server identity")
                verifyLeaf(chain)
            }

            override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
        }
        return SSLContext.getInstance(TRUST_TLS).apply {
            init(arrayOf<KeyManager>(manager), arrayOf(trust), random)
            if (TRUST_TLS !in supportedSSLParameters.protocols) throw IOException("TLS 1.3 unavailable")
        }
    }

    fun authenticatedPeer(socket: SSLSocket): X509Certificate {
        val session = socket.session
        if (session.protocol != TRUST_TLS) throw IOException("TLS 1.3 required")
        val local = session.localCertificates?.firstOrNull() as? X509Certificate
            ?: throw IOException("Mutual TLS required")
        local.checkValidity()
        if (!MessageDigest.isEqual(pin, trustSha256(local.encoded))) throw IOException("Unexpected local identity")
        val peer = session.peerCertificates.firstOrNull() as? X509Certificate
            ?: throw IOException("Mutual TLS required")
        peer.checkValidity()
        return peer
    }

    companion object {
        @Synchronized
        fun load(hasRecords: Boolean): TrustedIdentity {
            val store = KeyStore.getInstance(STORE).apply { load(null) }
            if (!store.containsAlias(ALIAS)) {
                if (hasRecords) throw IOException("Paired device identity unavailable")
                KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, STORE).apply {
                    initialize(KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN or KeyProperties.PURPOSE_VERIFY)
                        .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
                        .setDigests(KeyProperties.DIGEST_NONE, KeyProperties.DIGEST_SHA256, KeyProperties.DIGEST_SHA384,
                            KeyProperties.DIGEST_SHA512)
                        .setCertificateSubject(X500Principal("CN=SEAM"))
                        .build())
                }.generateKeyPair()
            }
            val key = store.getKey(ALIAS, null) as? PrivateKey ?: throw IOException("Paired device key unavailable")
            val cert = store.getCertificate(ALIAS) as? X509Certificate ?: throw IOException("Paired device certificate unavailable")
            if (key.algorithm != "EC" || key.encoded != null) throw IOException("Invalid paired device key")
            cert.checkValidity()
            return TrustedIdentity(key, cert)
        }
    }
}
