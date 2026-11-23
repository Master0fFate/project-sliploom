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
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
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
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.BiasAlignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

internal val SeamControlShape = RoundedCornerShape(12.dp)
private val PaneShape = RoundedCornerShape(24.dp)
private val Ink = Color(0xFF101010)
private val Paper = Color(0xFFFAFAFA)
private val DarkColors = darkColorScheme(
    background = Ink,
    onBackground = Paper,
    surface = Color(0xFF202020),
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
    onSurface = Ink,
    onSurfaceVariant = Color(0xFF505050),
    primary = Color.Black,
    onPrimary = Color.White,
    outline = Color(0xFF777777),
    outlineVariant = Color(0xFFB7B7B7),
)
private val SeamTypography = Typography(
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
internal fun SeamMaterial(bottomBiased: Boolean, content: @Composable ColumnScope.() -> Unit) {
    val dark = isSystemInDarkTheme()
    val themeContrast = Build.VERSION.SDK_INT >= 34 && raisedThemeContrast()
    val highContrastText = Build.VERSION.SDK_INT >= 36 && highContrastText()
    val opaque = themeContrast || highContrastText
    val colors = if (dark) DarkColors else LightColors
    MaterialTheme(colorScheme = colors, typography = SeamTypography) {
        Surface(modifier = Modifier.fillMaxSize(), color = colors.background) {
            val backlight = when {
                opaque -> Modifier
                else -> Modifier.drawWithCache {
                    val brush = Brush.radialGradient(
                        colors = listOf(if (dark) Color(0xFF353535) else Color(0xFFE1E1E1), colors.background),
                        center = Offset(size.width * 0.45f, size.height * if (bottomBiased) 0.74f else 0.47f),
                        radius = size.maxDimension.coerceAtLeast(1f) * 0.53f,
                    )
                    onDrawBehind { drawRect(brush) }
                }
            }
            BoxWithConstraints(Modifier.fillMaxSize().then(backlight).safeDrawingPadding()) {
                Box(
                    modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState())
                        .heightIn(min = maxHeight).padding(horizontal = 24.dp, vertical = 24.dp),
                    contentAlignment = BiasAlignment(0f, if (bottomBiased) 1f else 0.18f),
                ) {
                    Column(
                        modifier = Modifier.widthIn(max = 480.dp).fillMaxWidth(),
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(24.dp),
                    ) {
                        SeamWordmark()
                        val pane = if (opaque) {
                            Modifier.background(colors.surface, PaneShape).border(1.dp, colors.outline, PaneShape)
                        } else {
                            val fill = if (dark) Color(0xBD262626) else Color(0xD9FFFFFF)
                            val rim = Brush.verticalGradient(
                                0f to if (dark) Color(0xFF777777) else Color.White,
                                0.42f to if (dark) Color(0xFF474747) else Color(0xFFE2E2E2),
                                1f to if (dark) Color(0xFF393939) else Color(0xFFD0D0D0),
                            )
                            Modifier.shadow(12.dp, PaneShape).background(fill, PaneShape).border(1.dp, rim, PaneShape)
                        }
                        Column(
                            modifier = Modifier.fillMaxWidth().then(pane).clip(PaneShape).padding(24.dp),
                            verticalArrangement = Arrangement.spacedBy(24.dp),
                            content = content,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun SeamWordmark() {
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
            text = stringResource(R.string.app_name),
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.SemiBold,
            letterSpacing = 2.sp,
        )
    }
}

@Composable
internal fun SeamAction(label: String, onClick: () -> Unit, primary: Boolean = true) {
    if (primary) {
        Button(
            onClick = onClick,
            modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
            shape = SeamControlShape,
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 12.dp),
        ) {
            Text(label)
        }
    } else {
        TextButton(
            onClick = onClick,
            modifier = Modifier.heightIn(min = 48.dp),
            shape = SeamControlShape,
            contentPadding = PaddingValues(horizontal = 8.dp, vertical = 8.dp),
        ) {
            Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
internal fun SeamDetail(text: String, modifier: Modifier = Modifier) {
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
