package org.projectseam.android

import android.content.ContentResolver
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.os.Process
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.core.content.IntentCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.runInterruptible
import java.io.IOException
import java.security.MessageDigest

class MainActivity : ComponentActivity() {
    private val intake by viewModels<IntakeState> { viewModelFactory { initializer { IntakeState() } } }
    private var pendingIntent: Intent? = null
    private var readJob: Job? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) pendingIntent = intent
        enableEdgeToEdge()
        setContent {
            val status = intake.status
            BackHandler(enabled = status == Status.Reading, onBack = ::cancelRead)
            IntakeScreen(status, ::cancelRead)
        }
    }

    override fun onStart() {
        super.onStart()
        pendingIntent?.let(::receive)
        pendingIntent = null
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) {
            receive(intent)
        } else {
            pendingIntent = intent
        }
    }

    override fun onStop() {
        cancelRead()
        super.onStop()
    }

    private fun cancelRead() {
        readJob?.cancel()
        readJob = null
        if (intake.status == Status.Reading) intake.status = Status.Cancelled
    }

    private fun receive(intent: Intent) {
        cancelRead()
        val type = intent.type.orEmpty()
        if (intent.action != Intent.ACTION_SEND ||
            !(type == "application/pdf" || type.startsWith("image/") && type.length > 6)
        ) {
            intake.status = if (intent.action == Intent.ACTION_MAIN) Status.Idle else Status.Unsupported
            return
        }

        val uri = try {
            IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)
        } catch (_: RuntimeException) {
            intake.status = Status.Unreadable
            return
        }
        if (uri == null) {
            intake.status = Status.Missing
            return
        }
        if (uri.scheme != ContentResolver.SCHEME_CONTENT || uri.authority.isNullOrBlank()) {
            intake.status = Status.Unsupported
            return
        }

        intake.status = Status.Reading
        readJob = lifecycleScope.launch {
            val context = coroutineContext
            val result = try {
                runInterruptible(Dispatchers.IO) {
                    val grant = checkUriPermission(uri, Process.myPid(), Process.myUid(), Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    if (grant == PackageManager.PERMISSION_GRANTED) {
                        readStream(contentResolver, uri) { context.ensureActive() }
                    } else {
                        Status.Unreadable
                    }
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: IOException) {
                Status.Unreadable
            } catch (_: RuntimeException) {
                Status.Unreadable
            }
            context.ensureActive()
            intake.status = result
        }
    }
}

private const val MAX_BYTES = 100L * 1024 * 1024

private fun readStream(resolver: ContentResolver, uri: Uri, checkActive: () -> Unit): Status {
    val digest = MessageDigest.getInstance("SHA-256")
    val buffer = ByteArray(64 * 1024)
    var total = 0L
    val stream = resolver.openInputStream(uri) ?: return Status.Unreadable
    stream.use { input ->
        while (true) {
            checkActive()
            val size = input.read(buffer, 0, minOf(buffer.size.toLong(), MAX_BYTES - total + 1).toInt())
            if (size == -1) return Status.Read(total, digest.digest().toHexString())
            total += size
            if (total > MAX_BYTES) return Status.TooLarge
            digest.update(buffer, 0, size)
        }
    }
}

private class IntakeState : ViewModel() {
    var status by mutableStateOf<Status>(Status.Idle)
}

private sealed class Status(val message: Int) {
    data object Idle : Status(R.string.share_hint)
    data object Reading : Status(R.string.reading)
    data object Unsupported : Status(R.string.unsupported)
    data object Missing : Status(R.string.missing_file)
    data object Unreadable : Status(R.string.unreadable)
    data object TooLarge : Status(R.string.too_large)
    data object Cancelled : Status(R.string.cancelled)
    data class Read(val count: Long, val digest: String) : Status(R.string.file_read)
}

@Composable
private fun IntakeScreen(status: Status, onCancel: () -> Unit) {
    MaterialTheme(colorScheme = if (isSystemInDarkTheme()) darkColorScheme() else lightColorScheme()) {
        Surface(modifier = Modifier.fillMaxSize()) {
            Column(
                modifier = Modifier
                    .safeDrawingPadding()
                    .verticalScroll(rememberScrollState())
                    .padding(24.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(
                    text = stringResource(R.string.app_name),
                    modifier = Modifier.semantics { heading() },
                    style = MaterialTheme.typography.headlineMedium,
                )
                Text(
                    text = stringResource(status.message),
                    modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                )
                if (status is Status.Read) {
                    Text(stringResource(R.string.byte_count, status.count))
                    Text(stringResource(R.string.sha256, status.digest))
                }
                if (status == Status.Reading) {
                    Button(onClick = onCancel) { Text(stringResource(R.string.cancel)) }
                }
            }
        }
    }
}
