package io.github.hi9841.tdt.audio

import kotlin.math.exp
import kotlin.math.log10
import kotlin.math.sqrt

/**
 * Display-only speech energy, ported from TDT desktop (desktop/src/audio/envelope.rs). It never changes the
 * recorded samples. Every 25 ms of audio becomes one level on a fixed decibel range (-54 dBFS quiet, -12 dBFS
 * full), with a fast attack and a slower release, so pauses stay pauses. The recorder thread pushes; the UI reads
 * [barsAt] on every display frame.
 */
class SpeechEnvelope {
    private val bars = FloatArray(BARS)
    /** The bars before the last step, and when that step happened. */
    private val previous = FloatArray(BARS)
    private var steppedAt = 0L
    private var sumSquares = 0.0
    private var samples = 0
    private var level = 0f

    @Synchronized
    fun reset() {
        bars.fill(0f)
        previous.fill(0f)
        steppedAt = 0L
        sumSquares = 0.0
        samples = 0
        level = 0f
    }

    @Synchronized
    fun push(chunk: ShortArray, length: Int, sampleRate: Int) {
        val frameSamples = maxOf(Math.round(sampleRate * FRAME_SECONDS), 1)
        val before = bars.copyOf()
        var stepped = false
        for (i in 0 until length) {
            val sample = chunk[i] / 32768.0
            sumSquares += sample * sample
            samples++
            if (samples == frameSamples) {
                val rms = sqrt(sumSquares / samples).toFloat()
                // Fixed display range. Do not normalize each frame: pauses must remain pauses.
                val target = ((20f * log10(maxOf(rms, 0.000001f)) + 54f) / 42f).coerceIn(0f, 1f)
                val tau = if (target > level) 0.015f else 0.070f
                level += (target - level) * (1f - exp(-FRAME_SECONDS / tau))
                if (level < 0.005f) level = 0f
                bars.copyInto(bars, 0, 1)
                bars[BARS - 1] = level
                sumSquares = 0.0
                samples = 0
                stepped = true
            }
        }
        if (stepped) {
            before.copyInto(previous)
            steppedAt = System.nanoTime()
        }
    }

    /**
     * The bars as drawn at [nowNanos], folded into [out] (the few bars a pill shows, each the loudest of its
     * slice). A step is 25 ms, so a display that redraws at 120 Hz blends each bar from its previous value over
     * one step instead of showing the same frame three times.
     */
    @Synchronized
    fun barsAt(nowNanos: Long, out: FloatArray) {
        val t = if (steppedAt == 0L) 1f else ((nowNanos - steppedAt) / (FRAME_SECONDS * 1e9f)).coerceIn(0f, 1f)
        for (index in out.indices) {
            val start = index * BARS / out.size
            val end = maxOf((index + 1) * BARS / out.size, start + 1)
            var peak = 0f
            for (i in start until end) peak = maxOf(peak, previous[i] + (bars[i] - previous[i]) * t)
            out[index] = peak.coerceIn(0f, 1f)
        }
    }

    private companion object {
        const val BARS = 26
        const val FRAME_SECONDS = 0.025f
    }
}
