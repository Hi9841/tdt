package io.github.hi9841.tdt.audio

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import kotlin.math.PI
import kotlin.math.exp
import kotlin.math.roundToInt
import kotlin.math.sin

/**
 * The four cues of TDT desktop (desktop/src/audio/sounds.rs), synthesized the same way: short chirps with a
 * soft envelope and a noise tick at the start. Silent when the phone is on silent or vibrate.
 */
enum class Sound { Start, Stop, Success, Error }

class Sounds(private val context: Context) {
    private val tracks = HashMap<Sound, AudioTrack>()

    /** Builds every cue once, so the first tap does not synthesize. */
    fun prewarm() = Sound.entries.forEach(::track)

    fun play(sound: Sound) {
        val audio = context.getSystemService(AudioManager::class.java)
        if (audio.ringerMode != AudioManager.RINGER_MODE_NORMAL) return
        val track = track(sound)
        // A new cue cuts off the one before, as on desktop.
        runCatching {
            track.stop()
            track.reloadStaticData()
            track.play()
        }
    }

    fun release() {
        tracks.values.forEach(AudioTrack::release)
        tracks.clear()
    }

    @Synchronized
    private fun track(sound: Sound): AudioTrack = tracks.getOrPut(sound) {
        val pcm = toPcm16(samples(sound))
        AudioTrack.Builder()
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_ASSISTANCE_SONIFICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                    .build(),
            )
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(SAMPLE_RATE)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                    .build(),
            )
            .setTransferMode(AudioTrack.MODE_STATIC)
            .setBufferSizeInBytes(pcm.size * 2)
            .build()
            .apply { write(pcm, 0, pcm.size) }
    }

    private companion object {
        const val SAMPLE_RATE = 22_050

        fun samples(sound: Sound): FloatArray = when (sound) {
            // Short rising tick. Quiet enough to sit under speech.
            Sound.Start -> chirp(520f, 880f, 0.055f, 0.11f)
            Sound.Stop -> chirp(760f, 420f, 0.048f, 0.09f)
            Sound.Success -> crossfade(
                chirp(659.25f, 659.25f, 0.042f, 0.10f),
                chirp(987.77f, 987.77f, 0.070f, 0.09f),
                (0.008f * SAMPLE_RATE).toInt(),
            )
            Sound.Error -> chirp(220f, 146f, 0.090f, 0.10f)
        }

        fun chirp(f0: Float, f1: Float, seconds: Float, amp: Float): FloatArray {
            val n = maxOf((seconds * SAMPLE_RATE).roundToInt(), 8)
            var phase = 0f
            var noise = 0.17f
            return FloatArray(n) { i ->
                val t = i / (n - 1f)
                val freq = f0 + (f1 - f0) * t
                phase += (2 * PI * freq / SAMPLE_RATE).toFloat()
                noise = (noise * 1.37f + 0.31f).let { it - it.toInt() }
                val tick = if (i < SAMPLE_RATE / 400) (noise - 0.5f) * 0.35f else 0f
                val body = sin(phase) + 0.08f * sin(2 * phase)
                (body + tick) * amp * envelope(i, n)
            }
        }

        fun envelope(i: Int, n: Int): Float {
            val t = i.toFloat() / n
            return (t / 0.08f).coerceIn(0f, 1f) * exp(-6.5f * t)
        }

        fun crossfade(a: FloatArray, b: FloatArray, length: Int): FloatArray {
            val fade = length.coerceIn(1, minOf(a.size, b.size))
            val out = FloatArray(a.size + b.size - fade)
            a.copyInto(out, 0, 0, a.size - fade)
            for (i in 0 until fade) {
                val t = i.toFloat() / fade
                out[a.size - fade + i] = a[a.size - fade + i] * (1 - t) + b[i] * t
            }
            b.copyInto(out, a.size, fade, b.size)
            return out
        }

        fun toPcm16(pcm: FloatArray) = ShortArray(pcm.size) { (pcm[it].coerceIn(-1f, 1f) * 32767f).toInt().toShort() }
    }
}
