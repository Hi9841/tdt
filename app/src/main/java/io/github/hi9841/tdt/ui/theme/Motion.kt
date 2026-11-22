package io.github.hi9841.tdt.ui.theme

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.Easing
import androidx.compose.animation.core.FiniteAnimationSpec
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer

/**
 * Motion for TDT. Every UI motion eases out (it starts at once, so the UI answers the touch) with a strong
 * custom curve; on-screen moves ease in and out. Nothing eases in. Exits are quicker than entrances.
 */
object Motion {
    /** Strong ease-out for UI that enters or answers a touch. */
    val EaseOut: Easing = CubicBezierEasing(0.23f, 1f, 0.32f, 1f)
    /** Strong ease-in-out for things that move across the screen. */
    val EaseInOut: Easing = CubicBezierEasing(0.77f, 0f, 0.175f, 1f)

    /** Press feedback. */
    const val PRESS_MILLIS = 160
    /** State and colour changes, small reveals. */
    const val FAST_MILLIS = 200
    /** Larger reveals: a section entering, a card growing. */
    const val ENTER_MILLIS = 300
    /** Between sections that enter together. */
    const val STAGGER_MILLIS = 50

    fun <T> fast(): FiniteAnimationSpec<T> = tween(FAST_MILLIS, easing = EaseOut)
    fun <T> enter(delay: Int = 0): FiniteAnimationSpec<T> = tween(ENTER_MILLIS, delay, EaseOut)
}

/** Shrinks to 0.97 while pressed (160 ms ease-out), so a touch feels heard. Pass the same source to `clickable`. */
@Composable
fun Modifier.pressScale(source: MutableInteractionSource, pressed: Float = 0.97f): Modifier {
    val down by source.collectIsPressedAsState()
    val scale by animateFloatAsState(
        if (down) pressed else 1f,
        tween(Motion.PRESS_MILLIS, easing = Motion.EaseOut),
        label = "press",
    )
    return graphicsLayer {
        scaleX = scale
        scaleY = scale
    }
}
