package io.github.hi9841.tdt.audio

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import io.github.hi9841.tdt.engine.Take
import kotlin.concurrent.thread
import kotlin.math.sqrt

/**
 * Records 16 kHz mono speech and gives it to a [Take] while it comes, so the model reads the take while the user
 * talks. The microphone opens and closes on the recorder's own thread: neither blocks the thread that draws.
 *
 * [onLevel], [onEnd] and [onFail] run on the recorder thread. [onEnd]: the recording ended with no stop from the
 * user (the take is full, or the microphone went away). [onFail]: the microphone did not open.
 */
class Recorder(
    private val onLevel: (Float) -> Unit = {},
    private val onEnd: () -> Unit = {},
    private val onFail: (String) -> Unit = {},
) {
    /** The recording in work. Only the thread that calls [start], [stop] and [cancel] touches it. */
    private var session: Session? = null

    /** What the bubble's wave draws, fed from the same audio. */
    val envelope = SpeechEnvelope()

    /** Starts to record into [take]. The caller must hold RECORD_AUDIO. */
    fun start(take: Take) {
        if (session != null) return
        envelope.reset()
        session = Session(take)
    }

    /** Stops accepting audio at release. Hardware cleanup runs on the recorder thread. */
    fun stop(): Int {
        val session = session ?: return 0
        this.session = null
        session.asked = true
        session.open = false
        session.take.stop()
        return session.samples
    }

    /** Stops with no wait: the user dropped the take. */
    fun cancel() {
        session?.let {
            it.asked = true
            it.open = false
        }
        session = null
    }

    /** One recording, with its own microphone, so the next one can start while this microphone still closes. */
    private inner class Session(val take: Take) {
        /** False when the take needs no more audio. */
        @Volatile var open = true
        /** The user asked for the stop, so [onEnd] has nothing to tell. */
        @Volatile var asked = false
        @Volatile var samples = 0

        init {
            thread(name = "tdt-recorder") { run() }
        }

        private fun run() {
            val audio = try {
                microphone()
            } catch (error: Exception) {
                if (open) onFail(error.message ?: "The microphone is not available.")
                return
            }
            try {
                // 20 ms: the bubble's wave gets 50 levels a second and draws between them on every frame.
                val chunk = ShortArray(SAMPLE_RATE / 50)
                // The start cue plays while the microphone opens. The take starts after it, so the model does
                // not hear it.
                var cue = CUE_SAMPLES
                while (open) {
                    val read = audio.read(chunk, 0, chunk.size)
                    if (!open || read < 0) break
                    if (read == 0) continue
                    envelope.push(chunk, read, SAMPLE_RATE)
                    onLevel(rms(chunk, read))
                    val skip = minOf(cue, read)
                    cue -= skip
                    if (read == skip) continue
                    samples += read - skip
                    if (!take.push(chunk, skip, read - skip)) break
                }
            } finally {
                val byItself = open && !asked
                open = false
                if (byItself) onEnd()
                runCatching { audio.stop() }
                audio.release()
            }
        }
    }

    @SuppressLint("MissingPermission")
    private fun microphone(): AudioRecord {
        val minBuffer = AudioRecord.getMinBufferSize(SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        val audio = AudioRecord(
            MediaRecorder.AudioSource.VOICE_RECOGNITION,
            SAMPLE_RATE,
            AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT,
            maxOf(minBuffer, SAMPLE_RATE / 5 * 2),
        )
        if (audio.state != AudioRecord.STATE_INITIALIZED) {
            audio.release()
            error("The microphone is not available.")
        }
        audio.startRecording()
        return audio
    }

    private fun rms(chunk: ShortArray, length: Int): Float {
        var sum = 0.0
        for (i in 0 until length) {
            val value = chunk[i] / 32768.0
            sum += value * value
        }
        return sqrt(sum / length).toFloat()
    }

    companion object {
        const val SAMPLE_RATE = 16_000
        /** 80 ms: longer than the 55 ms start cue. */
        private const val CUE_SAMPLES = SAMPLE_RATE * 80 / 1000
    }
}
