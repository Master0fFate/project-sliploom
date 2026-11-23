package org.projectseam.android

import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.os.CancellationSignal
import android.os.OperationCanceledException
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
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.Closeable
import java.io.IOException
import java.security.MessageDigest

class MainActivity : ComponentActivity() {
    private val intake by viewModels<IntakeState> { viewModelFactory { initializer { IntakeState() } } }
    private var pendingIntent: Intent? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) {
            pendingIntent = intent
        } else if (savedInstanceState.getBoolean(SHARE_SESSION) && !intake.hasShareSession) {
            intake.hasShareSession = true
            intake.status = Status.Retry
        }
        enableEdgeToEdge()
        setContent {
            val status = intake.status
            val cancel: () -> Unit = { intake.cancelRead() }
            BackHandler(enabled = status == Status.Reading, onBack = cancel)
            IntakeScreen(status, cancel)
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

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putBoolean(SHARE_SESSION, pendingIntent?.isShare ?: intake.hasShareSession)
    }

    override fun onStop() {
        intake.cancelRead()
        super.onStop()
    }

    private fun receive(intent: Intent) {
        if (intake.cancelRead()) return
        intake.hasShareSession = intent.isShare
        val type = intent.type.orEmpty()
        if (intent.action != Intent.ACTION_SEND ||
            !(type == "application/pdf" || type.startsWith("image/") && type.length > 6)
        ) {
            intake.status = if (intent.action == Intent.ACTION_MAIN) Status.Idle else Status.Unsupported
            return
        }

        val uri = try {
            intent.sharedUri()
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

        intake.read(applicationContext, uri)
    }
}

private const val SHARE_SESSION = "share_session"
private const val MAX_BYTES = 100L * 1024 * 1024

private val Intent.isShare: Boolean
    get() = action == Intent.ACTION_SEND || action == Intent.ACTION_SEND_MULTIPLE

private fun Intent.sharedUri(): Uri? {
    val extraUri = if (hasExtra(Intent.EXTRA_STREAM)) {
        requireNotNull(IntentCompat.getParcelableExtra(this, Intent.EXTRA_STREAM, Uri::class.java))
    } else {
        null
    }
    val clipUri = clipData?.let { clip ->
        require(clip.itemCount == 1)
        val item = clip.getItemAt(0)
        require(item.intent == null)
        requireNotNull(item.uri)
    }
    require(extraUri == null || clipUri == null || extraUri == clipUri)
    return extraUri ?: clipUri
}

private object ShareReader {
    private val scope = CoroutineScope(Dispatchers.Main)
    private var active: ReadOperation? = null

    fun start(context: Context, uri: Uri, onFinished: (Status) -> Unit): ReadOperation? {
        if (active != null) return null
        val operation = ReadOperation()
        active = operation
        scope.launch {
            val result = withContext(Dispatchers.IO) {
                try {
                    operation.read(context, uri)
                } catch (_: IOException) {
                    Status.Unreadable
                } catch (_: RuntimeException) {
                    Status.Unreadable
                }
            }
            val cancellation = operation.cancellation
            cancellation?.join()
            active = null
            onFinished(if (cancellation == null) result else Status.Cancelled)
        }
        return operation
    }

    fun cancel(operation: ReadOperation) {
        operation.cancel(scope)
    }
}

private class ReadOperation {
    private val lock = Any()
    private val signal = CancellationSignal()
    private var resource: Closeable? = null
    private var cancelled = false
    var cancellation: Job? = null
        private set

    fun cancel(scope: CoroutineScope) {
        if (cancellation != null) return
        val toClose = synchronized(lock) {
            cancelled = true
            takeResource()
        }
        cancellation = scope.launch(Dispatchers.IO) {
            try {
                signal.cancel()
            } catch (_: RuntimeException) {
            } finally {
                try {
                    toClose?.close()
                } catch (_: IOException) {
                } catch (_: RuntimeException) {
                }
            }
        }
    }

    fun read(context: Context, uri: Uri): Status {
        checkActive()
        val grant = context.checkUriPermission(uri, Process.myPid(), Process.myUid(), Intent.FLAG_GRANT_READ_URI_PERMISSION)
        if (grant != PackageManager.PERMISSION_GRANTED) return Status.Unreadable
        checkActive()
        val descriptor = context.contentResolver.openAssetFileDescriptor(uri, "r", signal)
            ?: return Status.Unreadable
        if (!register(descriptor)) {
            descriptor.close()
            return Status.Cancelled
        }
        try {
            checkActive()
            if (descriptor.declaredLength > MAX_BYTES) return Status.TooLarge
            val input = descriptor.createInputStream()
            if (!register(input)) {
                input.close()
                return Status.Cancelled
            }
            val digest = MessageDigest.getInstance("SHA-256")
            val buffer = ByteArray(64 * 1024)
            var total = 0L
            while (true) {
                checkActive()
                val size = input.read(buffer, 0, minOf(buffer.size.toLong(), MAX_BYTES - total + 1).toInt())
                checkActive()
                if (size == -1) return Status.Read(total, digest.digest().toHexString())
                if (size == 0) return Status.Unreadable
                total += size
                if (total > MAX_BYTES) return Status.TooLarge
                digest.update(buffer, 0, size)
            }
        } finally {
            val toClose = synchronized(lock) { takeResource() }
            toClose?.close()
        }
    }

    private fun checkActive() {
        if (synchronized(lock) { cancelled }) throw OperationCanceledException()
    }

    private fun register(opened: Closeable): Boolean = synchronized(lock) {
        if (cancelled) return false
        resource = opened
        true
    }

    private fun takeResource(): Closeable? = resource.also { resource = null }
}

private class IntakeState : ViewModel() {
    var status by mutableStateOf<Status>(Status.Idle)
    var hasShareSession = false
    private var operation: ReadOperation? = null

    fun read(context: Context, uri: Uri) {
        operation = ShareReader.start(context, uri) { result ->
            operation = null
            status = result
        }
        status = if (operation == null) Status.Busy else Status.Reading
    }

    fun cancelRead(): Boolean {
        val active = operation ?: return false
        status = Status.Cancelling
        ShareReader.cancel(active)
        return true
    }
}

private sealed class Status(val message: Int) {
    data object Idle : Status(R.string.share_hint)
    data object Reading : Status(R.string.reading)
    data object Busy : Status(R.string.reader_busy)
    data object Retry : Status(R.string.share_again)
    data object Unsupported : Status(R.string.unsupported)
    data object Missing : Status(R.string.missing_file)
    data object Unreadable : Status(R.string.unreadable)
    data object TooLarge : Status(R.string.too_large)
    data object Cancelling : Status(R.string.cancelling)
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
