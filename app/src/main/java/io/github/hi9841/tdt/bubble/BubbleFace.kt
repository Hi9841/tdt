package io.github.hi9841.tdt.bubble

import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.Easing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.hi9841.tdt.DictationState
import io.github.hi9841.tdt.R
import io.github.hi9841.tdt.ui.theme.Motion
import kotlin.math.PI
import kotlin.math.sin
import io.github.hi9841.tdt.audio.SpeechEnvelope

/** What the pill says after a take: "Inserted 412 ms", "Copied", "No speech heard", or an error. */
data class Notice(val label: String, val meta: String? = null, val error: Boolean = false)

/** The bubble's height in every state, so the window does not jump up or down when it changes. */
val BubbleHeight = 52.dp
/** The width of the ✕ at the inner end of the listening pill; a tap there cancels the take. */
val CancelWidth = 48.dp
private val ButtonSize = 40.dp
private val BarWidth = 2.5.dp
private val BarStep = 5.dp
/** The desktop live pill's bars (3 to 13 px there), scaled up for a phone. */
private val BarLow = 4.dp
private val BarHigh = 20.dp
private const val BARS = 11
private val WaveWidth = BarStep * BARS
/** The listening pill's width, fixed, so its window does not change size while the wave moves. */
val ListenWidth = 4.dp + ButtonSize + 10.dp + 14.dp + 8.dp + WaveWidth + 10.dp + ButtonSize + 4.dp
/** The tab's width at rest. */
val TabWidth = 44.dp

private val PillBlack = Color(0xFF0A0B0F)
private val Edge = Color(0x1FFFFFFF)
private val Ink = Color(0xFFEFF0F2)
private val Soft = Color(0xFF9A9CA5)
private val Danger = Color(0xFFFF8FA3)

/**
 * The bubble, after the TDT desktop pill and Aqua Voice: a black tab attached to the screen edge with the orb.
 * Tapped, it pops open into a pill with ✕, the orb and a live wave, and ✓ next to the edge; then three dots
 * while the words are worked out; then a short notice. Tucked, it is a thin handle on the edge.
 */
@Composable
fun BubbleFace(
    state: DictationState,
    wave: SpeechEnvelope,
    notice: Notice?,
    onRight: Boolean,
    tucked: Boolean,
    visible: Boolean,
    pressed: Boolean,
) {
    // Slides out of its edge when it shows and back into it when it goes, in the desktop panel's times.
    val away = remember { Animatable(1f) }
    LaunchedEffect(visible) {
        // Both ways ease out: the bubble answers at once, and the exit is the quicker one.
        away.animateTo(if (visible) 0f else 1f, tween(if (visible) ENTER_MILLIS else EXIT_MILLIS, easing = Motion.EaseOut))
    }
    // Gives a little under the finger, so a touch feels heard.
    val press by animateFloatAsState(
        if (pressed) 0.94f else 1f,
        tween(Motion.PRESS_MILLIS, easing = Motion.EaseOut),
        label = "press",
    )
    // The desktop pill's size change: 320 ms with a small overshoot (hud.rs PILL_MOTION_MS, ease_pop).
    val pop = tween<androidx.compose.ui.unit.IntSize>(POP_MILLIS, easing = EasePop)
    val shape = edgeShape(onRight, if (tucked) 6.dp else BubbleHeight / 2)
    Row(
        Modifier
            .graphicsLayer {
                translationX = away.value * size.width * if (onRight) 1f else -1f
                alpha = 1f - away.value * 0.6f
                scaleX = press
                scaleY = press
                // Shrink toward the edge the bubble hangs on, not toward its middle.
                transformOrigin = TransformOrigin(if (onRight) 1f else 0f, 0.5f)
            }
            .height(BubbleHeight)
            .clip(shape)
            .background(if (tucked) PillBlack.copy(alpha = 0.8f) else PillBlack)
            .border(1.dp, Edge, shape)
            .animateContentSize(pop),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        when {
            tucked -> Handle()
            state is DictationState.Recording -> Listening(wave, onRight)
            state == DictationState.Processing -> Working(onRight)
            notice != null -> Message(notice, onRight)
            else -> Rest(onRight)
        }
    }
}

/** Rounded on the side away from the edge only, so the bubble reads as attached to the edge. */
private fun edgeShape(onRight: Boolean, radius: Dp): Shape = if (onRight) {
    RoundedCornerShape(topStart = radius, bottomStart = radius)
} else {
    RoundedCornerShape(topEnd = radius, bottomEnd = radius)
}

@Composable
private fun Orb(size: Dp) = Image(painterResource(R.drawable.orb), contentDescription = null, Modifier.size(size))

@Composable
private fun RowScope.Rest(onRight: Boolean) {
    // 12 + 24 + 8 = TabWidth.
    Spacer(Modifier.width(if (onRight) 12.dp else 8.dp))
    Orb(24.dp)
    Spacer(Modifier.width(if (onRight) 8.dp else 12.dp))
}

@Composable
private fun Handle() {
    Box(Modifier.width(12.dp), contentAlignment = Alignment.Center) {
        Box(
            Modifier
                .size(3.dp, 22.dp)
                .clip(CircleShape)
                .background(Soft),
        )
    }
}

/** ✕, the orb and the wave, ✓. The ✓ sits next to the edge, where the thumb already is. */
@Composable
private fun RowScope.Listening(wave: SpeechEnvelope, onRight: Boolean) {
    Spacer(Modifier.width(4.dp))
    RoundButton(if (onRight) "✕" else "✓", filled = !onRight)
    Row(
        Modifier.padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Orb(14.dp)
        Wave(wave)
    }
    RoundButton(if (onRight) "✓" else "✕", filled = onRight)
    Spacer(Modifier.width(4.dp))
}

@Composable
private fun RoundButton(mark: String, filled: Boolean) {
    Box(
        Modifier
            .size(ButtonSize)
            .clip(CircleShape)
            .background(if (filled) Ink else Color(0x26FFFFFF)),
        contentAlignment = Alignment.Center,
    ) {
        Text(mark, color = if (filled) PillBlack else Ink, fontSize = 18.sp)
    }
}

/**
 * The desktop live wave (hud.rs `live_wave`): [BARS] bars that rise and fall in place with the voice. The
 * envelope steps every 25 ms and blends between steps, so each display frame draws a new shape. Only the canvas
 * redraws; nothing recomposes while the user talks.
 */
@Composable
private fun Wave(wave: SpeechEnvelope) {
    val peaks = remember { FloatArray(BARS) }
    val frame = remember { mutableLongStateOf(0L) }
    LaunchedEffect(Unit) {
        while (true) withFrameNanos { frame.longValue = it }
    }
    Canvas(Modifier.size(WaveWidth, BarHigh)) {
        frame.longValue // Read here, so each frame redraws only this canvas.
        wave.barsAt(System.nanoTime(), peaks)
        val step = BarStep.toPx()
        val bar = BarWidth.toPx()
        val low = BarLow.toPx()
        for (i in 0 until BARS) {
            val height = low + (size.height - low) * peaks[i]
            drawRoundRect(
                Ink.copy(alpha = 0.7f + 0.3f * peaks[i]),
                topLeft = Offset(i * step + (step - bar) / 2, (size.height - height) / 2),
                size = Size(bar, height),
                cornerRadius = CornerRadius(bar / 2),
            )
        }
    }
}

private const val POP_MILLIS = 320
private const val ENTER_MILLIS = 260
private const val EXIT_MILLIS = 180

/** Ease out with a small overshoot, so the pill pops open (hud.rs `ease_pop`). */
private val EasePop = Easing { t ->
    val c1 = 1.1f
    val c3 = c1 + 1f
    val u = t.coerceIn(0f, 1f) - 1f
    1f + c3 * u * u * u + c1 * u * u
}

/** Three dots that pulse in turn while the words are worked out (desktop `finishing_dots`). */
@Composable
private fun RowScope.Working(onRight: Boolean) {
    val transition = rememberInfiniteTransition(label = "working")
    val progress by transition.animateFloat(
        0f, 1f, infiniteRepeatable(tween(900, easing = LinearEasing), RepeatMode.Restart), label = "dots",
    )
    Spacer(Modifier.width(if (onRight) 14.dp else 10.dp))
    Orb(18.dp)
    Spacer(Modifier.width(10.dp))
    Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        repeat(3) { index ->
            val phase = ((progress - index * 0.18f) % 1f + 1f) % 1f
            Box(
                Modifier
                    .size(5.dp)
                    .alpha(0.3f + 0.7f * sin(phase * PI).toFloat())
                    .clip(CircleShape)
                    .background(Ink),
            )
        }
    }
    Spacer(Modifier.width(if (onRight) 12.dp else 16.dp))
}

@Composable
private fun RowScope.Message(notice: Notice, onRight: Boolean) {
    Spacer(Modifier.width(if (onRight) 18.dp else 14.dp))
    Text(
        notice.label,
        Modifier.widthIn(max = 220.dp),
        color = if (notice.error) Danger else Ink,
        fontSize = 14.sp,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
    )
    notice.meta?.let {
        Spacer(Modifier.width(8.dp))
        Text(it, color = Soft, fontSize = 12.sp, fontFamily = FontFamily.Monospace, maxLines = 1)
    }
    Spacer(Modifier.width(if (onRight) 14.dp else 18.dp))
}
