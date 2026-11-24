package org.projectseam.android

import android.app.UiModeManager
import android.os.Build
import android.view.accessibility.AccessibilityManager
import androidx.annotation.RequiresApi
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.PagerState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.hideFromAccessibility
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.constrainHeight
import androidx.compose.ui.unit.constrainWidth
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.abs

internal val SliploomControlShape = RoundedCornerShape(12.dp)
private val PaneShape = RoundedCornerShape(24.dp)
private val Ink = Color(0xFF101010)
private val Paper = Color(0xFFFAFAFA)
private val DarkColors = darkColorScheme(
    background = Ink,
    onBackground = Paper,
    surface = Color(0xFF202020),
    surfaceContainerHigh = Color(0xFF3B3B3B),
    onSurface = Paper,
    onSurfaceVariant = Color(0xFFBDBDBD),
    primary = Color.White,
    onPrimary = Color.Black,
    outline = Color(0xFF808080),
    outlineVariant = Color(0xFF575757),
)
private val LightColors = lightColorScheme(
    background = Paper,
    onBackground = Ink,
    surface = Color.White,
    surfaceContainerHigh = Color(0xFFE9E9E9),
    onSurface = Ink,
    onSurfaceVariant = Color(0xFF505050),
    primary = Color.Black,
    onPrimary = Color.White,
    outline = Color(0xFF777777),
    outlineVariant = Color(0xFFB7B7B7),
)
private val SliploomTypography = Typography(
    headlineMedium = TextStyle(
        fontFamily = FontFamily.SansSerif, fontWeight = FontWeight.Medium,
        fontSize = 28.sp, lineHeight = 34.sp, letterSpacing = (-0.45).sp,
    ),
    bodyLarge = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 16.sp, lineHeight = 24.sp),
    bodyMedium = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 14.sp, lineHeight = 20.sp),
    labelLarge = TextStyle(
        fontFamily = FontFamily.SansSerif, fontWeight = FontWeight.Medium,
        fontSize = 16.sp, lineHeight = 22.sp,
    ),
)

@Composable
internal fun SliploomMaterial(overlay: @Composable () -> Unit, content: @Composable ColumnScope.(Modifier) -> Unit) {
    val dark = isSystemInDarkTheme()
    val themeContrast = Build.VERSION.SDK_INT >= 34 && raisedThemeContrast()
    val highContrastText = Build.VERSION.SDK_INT >= 36 && highContrastText()
    val opaque = themeContrast || highContrastText
    val colors = if (dark) DarkColors else LightColors
    val pane = if (opaque) {
        Modifier.background(colors.surface, PaneShape).border(1.dp, colors.outline, PaneShape)
    } else {
        val fill = if (dark) Color(0xBD262626) else Color(0xD9FFFFFF)
        val rim = remember(dark) {
            Brush.verticalGradient(
                0f to if (dark) Color(0xFF777777) else Color.White,
                0.42f to if (dark) Color(0xFF474747) else Color(0xFFE2E2E2),
                1f to if (dark) Color(0xFF393939) else Color(0xFFD0D0D0),
            )
        }
        Modifier.shadow(12.dp, PaneShape).background(fill, PaneShape).border(1.dp, rim, PaneShape)
    }.clip(PaneShape)
    MaterialTheme(colorScheme = colors, typography = SliploomTypography) {
        Surface(modifier = Modifier.fillMaxSize(), color = colors.background) {
            val backlight = when {
                opaque -> Modifier
                else -> Modifier.drawWithCache {
                    val brush = Brush.radialGradient(
                        colors = listOf(if (dark) Color(0xFF353535) else Color(0xFFE1E1E1), colors.background),
                        center = Offset(size.width * 0.5f, size.height * 0.5f),
                        radius = size.maxDimension.coerceAtLeast(1f) * 0.53f,
                    )
                    onDrawBehind { drawRect(brush) }
                }
            }
            BoxWithConstraints(
                modifier = Modifier.fillMaxSize().then(backlight).safeDrawingPadding().padding(vertical = 12.dp),
                contentAlignment = Alignment.Center,
            ) {
                Column(
                    modifier = Modifier.widthIn(max = 528.dp).fillMaxWidth().height(minOf(maxHeight, 424.dp)),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    SliploomWordmark()
                    content(pane)
                }
            }
        }
        overlay()
    }
}

internal val LocalPageActive = staticCompositionLocalOf<() -> Boolean> { { true } }

@Composable
internal fun ColumnScope.TransferModePager(
    pager: PagerState,
    pane: Modifier,
    canSwitch: Boolean,
    isPageActive: (Int) -> Boolean,
    content: @Composable (Int) -> Unit,
) {
    HorizontalPager(
        state = pager,
        modifier = Modifier.fillMaxWidth().weight(1f),
        contentPadding = PaddingValues(horizontal = 24.dp, vertical = 12.dp),
        pageSpacing = 24.dp,
        beyondViewportPageCount = 1,
        userScrollEnabled = canSwitch,
    ) { page ->
        val active = { isPageActive(page) }
        CompositionLocalProvider(LocalPageActive provides active) {
            val accessibility = if (active()) Modifier else Modifier.clearAndSetSemantics { hideFromAccessibility() }
            BoxWithConstraints(Modifier.fillMaxWidth().fillMaxHeight().then(accessibility)) {
                Box(
                    modifier = Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).heightIn(min = maxHeight),
                    contentAlignment = Alignment.Center,
                ) {
                    Column(
                        modifier = Modifier.fillMaxWidth().then(pane).padding(24.dp),
                        verticalArrangement = Arrangement.spacedBy(24.dp),
                    ) { content(page) }
                }
            }
        }
    }
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val selected = MaterialTheme.colorScheme.onSurface
        val unselected = MaterialTheme.colorScheme.outline
        repeat(pager.pageCount) { dot ->
            Canvas(Modifier.size(6.dp)) {
                val stroke = 1.dp.toPx()
                val position = pager.currentPage + pager.currentPageOffsetFraction
                val active = (1f - abs(position - dot)).coerceIn(0f, 1f)
                drawCircle(
                    unselected,
                    radius = (size.minDimension - stroke) / 2,
                    alpha = 1f - active,
                    style = Stroke(stroke),
                )
                drawCircle(selected, alpha = active)
            }
        }
        val cue = if (canSwitch) Modifier else Modifier.alpha(0f).clearAndSetSemantics { hideFromAccessibility() }
        Text(
            stringResource(R.string.swipe_to_switch),
            modifier = cue,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

@Composable
internal fun TransferPageHeader(
    title: String,
    announce: Boolean = false,
    peerTitle: String? = null,
    details: @Composable () -> Unit,
) {
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        PeerSizedText(title, peerTitle) { text, visible ->
            val semantics = if (visible) Modifier.semantics {
                heading()
                if (announce) liveRegion = LiveRegionMode.Polite
            } else Modifier
            Text(text, semantics, color = MaterialTheme.colorScheme.onSurface, style = MaterialTheme.typography.headlineMedium)
        }
        details()
    }
}

@Composable
internal fun TransferIdleHeader(receiving: Boolean) {
    val sendTitle = stringResource(R.string.send_files)
    val receiveTitle = stringResource(R.string.receive_a_file)
    val sendHint = stringResource(if (supportsLanSend) R.string.share_hint else R.string.share_hint_cable)
    val receiveHint = stringResource(if (supportsLanReceive) R.string.receive_hint else R.string.receive_unavailable)
    TransferPageHeader(
        title = if (receiving) receiveTitle else sendTitle,
        announce = !receiving,
        peerTitle = if (receiving) sendTitle else receiveTitle,
    ) {
        PeerSizedText(
            text = if (receiving) receiveHint else sendHint,
            peerText = if (receiving) sendHint else receiveHint,
        ) { text, _ -> SliploomDetail(text) }
    }
}

@Composable
private fun PeerSizedText(text: String, peerText: String?, label: @Composable (String, Boolean) -> Unit) {
    if (peerText == null) {
        label(text, true)
    } else {
        Layout(content = {
            label(text, true)
            label(peerText, false)
        }) { measurables, constraints ->
            val visible = measurables[0].measure(constraints)
            val peer = measurables[1].measure(constraints)
            layout(constraints.constrainWidth(visible.width), constraints.constrainHeight(maxOf(visible.height, peer.height))) {
                visible.placeRelative(0, 0)
            }
        }
    }
}

@Composable
private fun SliploomWordmark() {
    val ink = MaterialTheme.colorScheme.onBackground
    Row(
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Canvas(Modifier.size(width = 36.dp, height = 16.dp)) {
            val top = size.height * 0.26f
            val bottom = size.height * 0.74f
            val start = 2.dp.toPx()
            val end = size.width - start
            val stroke = 1.75.dp.toPx()
            val bridge = Path().apply {
                moveTo(start, top)
                lineTo(size.width * 0.42f, top)
                lineTo(size.width * 0.62f, bottom)
                lineTo(end, bottom)
            }
            drawPath(bridge, ink, style = Stroke(stroke, cap = StrokeCap.Round, join = StrokeJoin.Round))
            drawLine(ink, Offset(start, bottom), Offset(size.width * 0.36f, bottom), stroke, StrokeCap.Round)
            drawLine(ink, Offset(size.width * 0.68f, top), Offset(end, top), stroke, StrokeCap.Round)
        }
        Text(
            text = stringResource(R.string.app_name).uppercase(),
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.SemiBold,
            letterSpacing = 2.sp,
        )
    }
}

@Composable
internal fun SliploomAction(label: String, onClick: () -> Unit, primary: Boolean = true, enabled: Boolean = true) {
    val active = LocalPageActive.current
    val click = { if (active()) onClick() }
    val action = Modifier.heightIn(min = 48.dp).focusProperties { canFocus = active() }
    if (primary) {
        Button(
            onClick = click,
            enabled = enabled,
            modifier = action.fillMaxWidth(),
            shape = SliploomControlShape,
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
        ) {
            Text(label)
        }
    } else {
        TextButton(
            onClick = click,
            enabled = enabled,
            modifier = action,
            shape = SliploomControlShape,
            contentPadding = PaddingValues(horizontal = 8.dp, vertical = 8.dp),
        ) {
            Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
internal fun SliploomDetail(text: String, modifier: Modifier = Modifier) {
    Text(text, modifier, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)
}

@RequiresApi(34)
@Composable
private fun raisedThemeContrast(): Boolean {
    val context = LocalContext.current
    val manager = remember(context) { context.getSystemService(UiModeManager::class.java) } ?: return false
    var raised by remember(manager) { mutableStateOf(manager.contrast >= 0.5f) }
    DisposableEffect(manager) {
        val listener = UiModeManager.ContrastChangeListener { raised = it >= 0.5f }
        manager.addContrastChangeListener(context.mainExecutor, listener)
        raised = manager.contrast >= 0.5f
        onDispose { manager.removeContrastChangeListener(listener) }
    }
    return raised
}

@RequiresApi(36)
@Composable
private fun highContrastText(): Boolean {
    val context = LocalContext.current
    val manager = remember(context) { context.getSystemService(AccessibilityManager::class.java) } ?: return false
    var enabled by remember(manager) { mutableStateOf(manager.isHighContrastTextEnabled) }
    DisposableEffect(manager) {
        val listener = AccessibilityManager.HighContrastTextStateChangeListener { enabled = it }
        manager.addHighContrastTextStateChangeListener(context.mainExecutor, listener)
        enabled = manager.isHighContrastTextEnabled
        onDispose { manager.removeHighContrastTextStateChangeListener(listener) }
    }
    return enabled
}
